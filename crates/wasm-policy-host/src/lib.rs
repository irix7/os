// SPDX-License-Identifier: GPL-3.0-only
//! Native primitives behind the M1 Wasm local-policy guest.
//!
//! The guest decides the namespace layout, actor identity, capability rights
//! and the order of validation.  This crate supplies the bulk state and the
//! operations that are awkward or unsafe inside Wasm: the CAS store, the
//! actor directory and run queue, the capability table, the clock-independent
//! membership view and the rendezvous home calculation.
//!
//! Every mutation is staged. [`Policy::commit`] publishes the stage as one
//! value; [`Policy::abort`] discards it.  A failed policy therefore leaves no
//! object, actor, queued entry or capability behind.

#![cfg_attr(not(test), no_std)]
#![forbid(unsafe_code)]
#![deny(missing_docs)]

extern crate alloc;

use agnostic_core::{ActorTable, RunQueue};
use cas_core::ObjectKind;
use cas_store::Namespace;
use mesh_core::{home_for, MembershipView};
use mesh_service::MembershipTable;
use sys_primitives::{ActorId, NodeId, ObjectId};

/// Object kind byte for an immutable blob.
pub const KIND_BLOB: u8 = ObjectKind::Blob as u8;
/// Object kind byte for a directory tree.
pub const KIND_TREE: u8 = ObjectKind::Tree as u8;

/// Capability right: read the object's bytes or metadata.
pub const RIGHT_READ: u32 = 1 << 0;
/// Capability right: write or replace the object.
pub const RIGHT_WRITE: u32 = 1 << 1;
/// Capability right: execute the object as code.
pub const RIGHT_EXECUTE: u32 = 1 << 2;
/// Capability right: map the object into an address space.
pub const RIGHT_MAP: u32 = 1 << 3;
/// Capability right: use the object for DMA.
pub const RIGHT_DMA: u32 = 1 << 4;
/// Capability right: invoke the object as a service endpoint.
pub const RIGHT_INVOKE: u32 = 1 << 5;
/// Capability right: transfer the capability.
pub const RIGHT_TRANSFER: u32 = 1 << 6;
/// Capability right: duplicate the capability.
pub const RIGHT_DUP: u32 = 1 << 7;
/// Capability right: revoke derived capabilities.
pub const RIGHT_REVOKE: u32 = 1 << 8;
/// Capability right: manage the object.
pub const RIGHT_MANAGE: u32 = 1 << 9;

/// All rights the local table understands.
pub const RIGHTS_ALL: u32 = 0x3ff;

/// Maximum capabilities in the staged and committed tables.
pub const MAX_CAPS: usize = 64;

/// Policy failure.  The guest sees a negative status; the reason is local.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PolicyError {
    /// No stage is active.
    NoStage,
    /// Nothing has been committed.
    NotCommitted,
    /// The namespace operation failed.
    Namespace,
    /// The object kind byte is unknown.
    UnknownKind,
    /// The object is not present in the staged CAS.
    MissingObject,
    /// The rights bitmap is empty or contains a reserved bit.
    InvalidRights,
    /// The requested home is not this object's home.
    WrongHome,
    /// The actor operation failed.
    Actor,
    /// The run-queue operation failed.
    Queue,
    /// The capability table is full.
    CapTableFull,
    /// The committed root is not a directory object.
    BadRoot,
}

/// A four-word policy summary for the guest's evidence line.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Stats {
    /// Number of objects in the namespace store.
    pub objects: u32,
    /// Number of actors in the table.
    pub actors: u32,
    /// Number of actors in the run queue.
    pub queued: u32,
    /// Number of live capabilities.
    pub caps: u32,
}

#[derive(Clone, Copy)]
struct CapEntry {
    object: ObjectId,
    rights: u32,
    subject: [u8; 32],
    home: NodeId,
    generation: u32,
    live: bool,
}

impl CapEntry {
    const EMPTY: Self = Self {
        object: ObjectId::ZERO,
        rights: 0,
        subject: [0; 32],
        home: NodeId::ZERO,
        generation: 0,
        live: false,
    };
}

struct CapTable {
    entries: [CapEntry; MAX_CAPS],
    count: u32,
}

impl CapTable {
    const fn new() -> Self {
        Self {
            entries: [CapEntry::EMPTY; MAX_CAPS],
            count: 0,
        }
    }

    fn issue(
        &mut self,
        object: ObjectId,
        rights: u32,
        subject: [u8; 32],
        home: NodeId,
    ) -> Result<u64, PolicyError> {
        if rights == 0 || rights & !RIGHTS_ALL != 0 {
            return Err(PolicyError::InvalidRights);
        }
        let index = (1..MAX_CAPS)
            .find(|&slot| !self.entries[slot].live)
            .ok_or(PolicyError::CapTableFull)?;
        let entry = &mut self.entries[index];
        entry.generation = entry.generation.wrapping_add(1).max(1);
        entry.object = object;
        entry.rights = rights;
        entry.subject = subject;
        entry.home = home;
        entry.live = true;
        self.count += 1;
        Ok(((entry.generation as u64) << 32) | index as u64)
    }

    fn check(&self, handle: u64, rights: u32, object: &ObjectId) -> bool {
        if handle == 0 || rights == 0 || rights & !RIGHTS_ALL != 0 {
            return false;
        }
        let index = (handle & 0xffff_ffff) as usize;
        let generation = (handle >> 32) as u32;
        if index == 0 || index >= MAX_CAPS {
            return false;
        }
        let entry = &self.entries[index];
        entry.live
            && entry.generation == generation
            && entry.object == *object
            && entry.rights & rights == rights
    }

    fn count(&self) -> u32 {
        self.count
    }
}

struct Stage {
    namespace: Namespace,
    actors: ActorTable,
    queue: RunQueue,
    caps: CapTable,
}

struct Committed {
    namespace: Namespace,
    actors: ActorTable,
    queue: RunQueue,
    caps: CapTable,
}

enum Current<'a> {
    Stage(&'a Stage),
    Committed(&'a Committed),
}

impl Stage {
    fn new(view: &MembershipView) -> Self {
        Self {
            namespace: Namespace::new(),
            actors: ActorTable::new(view.clone()),
            queue: RunQueue::new(),
            caps: CapTable::new(),
        }
    }

    fn namespace(&self) -> &Namespace {
        &self.namespace
    }

    fn caps(&self) -> &CapTable {
        &self.caps
    }
}

impl Committed {
    fn from_stage(stage: Stage) -> Self {
        Self {
            namespace: stage.namespace,
            actors: stage.actors,
            queue: stage.queue,
            caps: stage.caps,
        }
    }

    fn namespace(&self) -> &Namespace {
        &self.namespace
    }

    fn actor_table(&self) -> &ActorTable {
        &self.actors
    }

    fn caps(&self) -> &CapTable {
        &self.caps
    }
}

/// The host side of the Wasm local policy.
pub struct Policy {
    node: NodeId,
    view: MembershipView,
    stage: Option<Stage>,
    committed: Option<Committed>,
}

impl Policy {
    /// Build the one-node policy view from the machine identity.
    pub fn new(node: NodeId, now_ms: u64) -> Self {
        let table = MembershipTable::single(node, now_ms);
        Self {
            node,
            view: table.snapshot(),
            stage: None,
            committed: None,
        }
    }

    /// This machine's node identity.
    pub const fn node(&self) -> NodeId {
        self.node
    }

    /// Current membership view.
    pub const fn view(&self) -> &MembershipView {
        &self.view
    }

    /// Start a fresh staged policy, discarding any earlier stage.
    pub fn begin(&mut self) -> Result<(), PolicyError> {
        self.stage = Some(Stage::new(&self.view));
        Ok(())
    }

    /// Discard the stage and any previous commit.
    ///
    /// The guest calls this on a failed policy.  The module also calls it when
    /// the guest traps or returns a non-success result, so a partial policy
    /// cannot remain published.
    pub fn abort(&mut self) {
        self.stage = None;
        self.committed = None;
    }

    /// Publish the staged policy if it is internally consistent.
    ///
    /// Publication is a single move, so a caller never observes half a
    /// namespace, an actor without its queue entry or a capability without its
    /// object.
    pub fn commit(&mut self) -> Result<Stats, PolicyError> {
        let stage = self.stage.take().ok_or(PolicyError::NoStage)?;
        if stage.namespace.store().kind(stage.namespace.root()) != Some(ObjectKind::Tree) {
            self.stage = Some(stage);
            return Err(PolicyError::BadRoot);
        }
        for entry in &stage.caps.entries[1..] {
            if entry.live && !stage.namespace.store().contains(entry.object) {
                self.stage = Some(stage);
                return Err(PolicyError::MissingObject);
            }
        }
        let stats = stats_of(&stage);
        self.committed = Some(Committed::from_stage(stage));
        Ok(stats)
    }

    /// Whether a policy is committed.
    pub const fn is_committed(&self) -> bool {
        self.committed.is_some()
    }

    fn current(&self) -> Option<Current<'_>> {
        match (&self.stage, &self.committed) {
            (Some(stage), _) => Some(Current::Stage(stage)),
            (None, Some(committed)) => Some(Current::Committed(committed)),
            (None, None) => None,
        }
    }

    fn current_stage(&mut self) -> Result<&mut Stage, PolicyError> {
        self.stage.as_mut().ok_or(PolicyError::NoStage)
    }

    /// Create a directory path in the staged namespace.
    pub fn mkdir(&mut self, path: &str) -> Result<ObjectId, PolicyError> {
        self.current_stage()?
            .namespace
            .create_dir_all(path)
            .map_err(|_| PolicyError::Namespace)
    }

    /// Store bytes and publish them at `path` in the staged namespace.
    pub fn put(&mut self, path: &str, kind: u8, bytes: &[u8]) -> Result<ObjectId, PolicyError> {
        let kind = ObjectKind::from_u8(kind).ok_or(PolicyError::UnknownKind)?;
        self.current_stage()?
            .namespace
            .put(path, kind, bytes)
            .map_err(|_| PolicyError::Namespace)
    }

    /// Resolve a path in the current namespace.
    pub fn lookup(&self, path: &str) -> Result<ObjectId, PolicyError> {
        match self.current() {
            Some(Current::Stage(stage)) => stage
                .namespace()
                .lookup(path)
                .map_err(|_| PolicyError::Namespace),
            Some(Current::Committed(committed)) => committed
                .namespace()
                .lookup(path)
                .map_err(|_| PolicyError::Namespace),
            None => Err(PolicyError::NoStage),
        }
    }

    /// Fetch an object by OID from the current CAS.
    pub fn get(&self, oid: &ObjectId) -> Result<(ObjectKind, &[u8]), PolicyError> {
        let store = match self.current() {
            Some(Current::Stage(stage)) => stage.namespace().store(),
            Some(Current::Committed(committed)) => committed.namespace().store(),
            None => return Err(PolicyError::NoStage),
        };
        let kind = store.kind(*oid).ok_or(PolicyError::MissingObject)?;
        let bytes = store.get(*oid).ok_or(PolicyError::MissingObject)?;
        Ok((kind, bytes))
    }

    /// Current root directory OID.
    pub fn root(&self) -> Result<ObjectId, PolicyError> {
        match self.current() {
            Some(Current::Stage(stage)) => Ok(stage.namespace().root()),
            Some(Current::Committed(committed)) => Ok(committed.namespace().root()),
            None => Err(PolicyError::NoStage),
        }
    }

    /// Resolve an object's home in the current membership view.
    pub fn home(&self, oid: &ObjectId) -> Option<NodeId> {
        home_for(oid, &self.view)
    }

    /// Spawn an actor at its rendezvous home.
    pub fn actor_spawn(&mut self, id: ActorId) -> Result<(), PolicyError> {
        self.current_stage()?
            .actors
            .spawn(id)
            .map(|_| ())
            .map_err(|_| PolicyError::Actor)
    }

    /// Place an actor on a host.  The single-node policy requires its home.
    pub fn actor_place(&mut self, id: ActorId, host: NodeId) -> Result<(), PolicyError> {
        let stage = self.current_stage()?;
        let actor = stage.actors.get(id).ok_or(PolicyError::Actor)?;
        if host != actor.home {
            return Err(PolicyError::WrongHome);
        }
        stage
            .actors
            .place(id, host)
            .map(|_| ())
            .map_err(|_| PolicyError::Actor)
    }

    /// Put an actor on the fair run queue.
    pub fn actor_queue(&mut self, id: ActorId) -> Result<(), PolicyError> {
        self.current_stage()?
            .queue
            .push_fair(id, 0)
            .map_err(|_| PolicyError::Queue)
    }

    /// Issue a capability over a staged object.
    pub fn cap_issue(
        &mut self,
        object: ObjectId,
        rights: u32,
        subject: [u8; 32],
        home: NodeId,
    ) -> Result<u64, PolicyError> {
        if rights == 0 || rights & !RIGHTS_ALL != 0 {
            return Err(PolicyError::InvalidRights);
        }
        if home_for(&object, &self.view) != Some(home) {
            return Err(PolicyError::WrongHome);
        }
        let stage = self.current_stage()?;
        if !stage.namespace.store().contains(object) {
            return Err(PolicyError::MissingObject);
        }
        stage.caps.issue(object, rights, subject, home)
    }

    /// Check a live capability for an object and rights set.
    pub fn cap_check(&self, handle: u64, rights: u32, object: &ObjectId) -> bool {
        match self.current() {
            Some(Current::Stage(stage)) => stage.caps().check(handle, rights, object),
            Some(Current::Committed(committed)) => committed.caps().check(handle, rights, object),
            None => false,
        }
    }

    /// Counters for the guest's evidence line.
    pub fn stats(&self) -> Result<Stats, PolicyError> {
        match self.current() {
            Some(Current::Stage(stage)) => Ok(stats_of(stage)),
            Some(Current::Committed(committed)) => Ok(Stats {
                objects: committed.namespace.store().len() as u32,
                actors: committed.actor_table().len() as u32,
                queued: committed.queue.len() as u32,
                caps: committed.caps().count(),
            }),
            None => Err(PolicyError::NoStage),
        }
    }
}

fn stats_of(stage: &Stage) -> Stats {
    Stats {
        objects: stage.namespace.store().len() as u32,
        actors: stage.actors.len() as u32,
        queued: stage.queue.len() as u32,
        caps: stage.caps.count(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cas_core::Directory;

    const MOTD: &[u8] = b"Welcome to IRIX 7.\n";

    fn node() -> NodeId {
        NodeId([
            0x02, 0x11, 0x22, 0x33, 0x44, 0x55, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
        ])
    }

    fn actor() -> ActorId {
        let mut bytes = node().0;
        bytes[15] = 1;
        ActorId(bytes)
    }

    fn expected_root() -> ObjectId {
        let mut namespace = Namespace::new();
        namespace.create_dir_all("/etc").expect("etc");
        namespace
            .put("/etc/motd", ObjectKind::Blob, MOTD)
            .expect("motd");
        namespace.root()
    }

    #[test]
    fn staged_policy_matches_the_native_namespace() {
        let mut policy = Policy::new(node(), 0);
        policy.begin().expect("begin");
        policy.mkdir("/etc").expect("mkdir");
        let motd = policy.put("/etc/motd", KIND_BLOB, MOTD).expect("put");
        assert_eq!(policy.lookup("/etc/motd"), Ok(motd));
        assert_eq!(policy.root(), Ok(expected_root()));
        let (kind, bytes) = policy.get(&motd).expect("get");
        assert_eq!(kind, ObjectKind::Blob);
        assert_eq!(bytes, MOTD);
        assert_eq!(policy.home(&motd), Some(node()));

        let stats = policy.stats().expect("stats");
        assert_eq!(stats.objects, 5);
        assert_eq!(stats.actors, 0);
        assert_eq!(stats.queued, 0);
        assert_eq!(stats.caps, 0);

        let stats = policy.commit().expect("commit");
        assert_eq!(stats.objects, 5);
        assert!(policy.is_committed());
        assert_eq!(policy.root(), Ok(expected_root()));
    }

    #[test]
    fn abort_leaves_no_partial_policy() {
        let mut policy = Policy::new(node(), 0);
        policy.begin().expect("begin");
        policy.mkdir("/etc").expect("mkdir");
        policy.put("/etc/motd", KIND_BLOB, MOTD).expect("put");
        assert!(!policy.is_committed());
        assert!(policy.stats().is_ok());
        policy.abort();
        assert!(!policy.is_committed());
        assert!(policy.stats().is_err());
        assert_eq!(policy.root(), Err(PolicyError::NoStage));
        assert_eq!(policy.lookup("/etc/motd"), Err(PolicyError::NoStage));
    }

    #[test]
    fn actor_is_spawned_placed_and_queued() {
        let mut policy = Policy::new(node(), 0);
        policy.begin().expect("begin");
        policy.actor_spawn(actor()).expect("spawn");
        policy.actor_place(actor(), node()).expect("place");
        policy.actor_queue(actor()).expect("queue");
        let stats = policy.stats().expect("stats");
        assert_eq!(stats.actors, 1);
        assert_eq!(stats.queued, 1);
    }

    #[test]
    fn capability_rights_and_object_are_enforced() {
        let mut policy = Policy::new(node(), 0);
        policy.begin().expect("begin");
        policy.mkdir("/etc").expect("mkdir");
        let motd = policy.put("/etc/motd", KIND_BLOB, MOTD).expect("put");
        let root = policy.root().expect("root");
        let handle = policy
            .cap_issue(
                root,
                RIGHT_READ | RIGHT_WRITE | RIGHT_INVOKE,
                [0x33; 32],
                node(),
            )
            .expect("cap");
        assert!(policy.cap_check(handle, RIGHT_READ, &root));
        assert!(!policy.cap_check(handle, RIGHT_MANAGE, &root));
        assert!(!policy.cap_check(handle, RIGHT_READ, &motd));
        assert!(!policy.cap_check(handle ^ (1 << 32), RIGHT_READ, &root));
        assert_eq!(policy.stats().expect("stats").caps, 1);
        policy.commit().expect("commit");
        assert!(policy.cap_check(handle, RIGHT_READ, &root));
        assert!(!policy.cap_check(handle, RIGHT_MANAGE, &root));
    }

    #[test]
    fn a_capability_over_another_home_is_rejected() {
        let mut policy = Policy::new(node(), 0);
        policy.begin().expect("begin");
        policy.mkdir("/etc").expect("mkdir");
        let motd = policy.put("/etc/motd", KIND_BLOB, MOTD).expect("put");
        assert_eq!(
            policy.cap_issue(motd, RIGHT_READ, [0; 32], NodeId([9; 16])),
            Err(PolicyError::WrongHome)
        );
    }

    #[test]
    fn bad_kind_and_empty_rights_fail_closed() {
        let mut policy = Policy::new(node(), 0);
        policy.begin().expect("begin");
        assert_eq!(
            policy.put("/blob", 0xff, b"x"),
            Err(PolicyError::UnknownKind)
        );
        policy.mkdir("/etc").expect("mkdir");
        let root = policy.root().expect("root");
        assert_eq!(
            policy.cap_issue(root, 0, [0; 32], node()),
            Err(PolicyError::InvalidRights)
        );
    }

    #[test]
    fn committed_roots_are_directory_objects() {
        let mut policy = Policy::new(node(), 0);
        policy.begin().expect("begin");
        policy.mkdir("/etc").expect("mkdir");
        let root = policy.root().expect("root");
        let (kind, bytes) = policy.get(&root).expect("get");
        assert_eq!(kind, ObjectKind::Tree);
        assert_eq!(Directory::decode(bytes).expect("decode").len(), 1);
    }
}
