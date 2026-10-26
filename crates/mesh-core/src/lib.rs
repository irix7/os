// SPDX-License-Identifier: GPL-3.0-only
//! `mesh_core` — the testable part of the global kernel's sharding policy.
//!
//! This crate is deliberately small and `no_std` (with `alloc`):
//!
//! * rendezvous hashing from an object/actor identity to a home node;
//! * a bounded route cache with home epochs;
//! * leases with fencing epochs;
//! * a placement view built from per-node summaries.
//!
//! It does **not** implement networking, membership gossip or consensus. Those
//! live in the native shim and the mesh service; this crate is the policy that
//! must be identical on every node.
//!
//! Rendezvous scores are the first 64 bits of
//! `BLAKE3("IRIX7-HOME-v1" || oid || node || vnode)`, as specified in
//! `MESH.md`. The hash is deterministic on every node and target.

#![no_std]
#![forbid(unsafe_code)]
#![deny(missing_docs)]

extern crate alloc;

use alloc::collections::BTreeMap;
use alloc::vec::Vec;
use sys_primitives::{ActorId, NodeId, ObjectId};

/// Hard maximum number of kernel replicas in one logical computer.
pub const MAX_NODES: usize = 128;

/// Virtual nodes per physical node for rendezvous balance.
pub const VIRTUAL_NODES: u32 = 1;

/// Domain separator for the fixed M2 home-evidence object set.
///
/// The objects are a test fixture, not a production directory: their IDs are
/// derived from this domain and a little-endian index.  Every node and the
/// host rig can reproduce the same set without a shared object list.
pub const HOME_FIXTURE_DOMAIN: &[u8] = b"IRIX7-M2-HOME-OBJECT-v1";

/// Default number of objects in the M2 home-evidence set.
pub const HOME_FIXTURE_DEFAULT: u32 = 8;

/// Hard bound on the configured M2 home-evidence object count.
pub const HOME_FIXTURE_MAX: u32 = 32;

/// A membership snapshot.  It is immutable; a new epoch is a new value.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MembershipView {
    /// Membership epoch.
    pub epoch: u32,
    /// Nodes in the view, at most [`MAX_NODES`].
    pub nodes: Vec<NodeId>,
}

impl MembershipView {
    /// Create a view.  More than [`MAX_NODES`] is rejected by returning `None`.
    pub fn new(epoch: u32, nodes: Vec<NodeId>) -> Option<Self> {
        if nodes.len() > MAX_NODES {
            return None;
        }
        let mut sorted = nodes;
        sorted.sort_unstable();
        sorted.dedup();
        Some(Self {
            epoch,
            nodes: sorted,
        })
    }

    /// The single-node base case: one member, no network, no quorum.
    pub fn single(node: NodeId) -> Self {
        Self {
            epoch: 0,
            nodes: alloc::vec![node],
        }
    }

    /// Whether the view has exactly one node.
    pub fn is_single(&self) -> bool {
        self.nodes.len() == 1
    }

    /// Whether `node` is a member.
    pub fn contains(&self, node: NodeId) -> bool {
        self.nodes.binary_search(&node).is_ok()
    }

    /// Number of members.
    pub fn len(&self) -> usize {
        self.nodes.len()
    }

    /// Whether the view is empty.
    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }
}

/// Compute the home node for an object under a membership view.
///
/// Rendezvous hashing: for each physical node and virtual node, compute a
/// score from `(object, node, vnode)`; the highest score wins.  Adding or
/// removing a node only moves the objects that mapped to that node, which is
/// the property the mesh needs for bounded rebalancing.
pub fn home_for(object: &ObjectId, view: &MembershipView) -> Option<NodeId> {
    if view.nodes.is_empty() {
        return None;
    }
    let mut best_score = 0u64;
    let mut best_node = None;
    for node in &view.nodes {
        for vnode in 0..VIRTUAL_NODES {
            let score = rendezvous_score(object.as_bytes(), node.as_bytes(), vnode);
            if best_node.is_none() || score > best_score {
                best_score = score;
                best_node = Some(*node);
            }
        }
    }
    best_node
}

/// Compute the home node for an actor.  Actors use the same rendezvous space
/// as objects so the directory is sharded without a central index.
pub fn actor_home(actor: &ActorId, view: &MembershipView) -> Option<NodeId> {
    if view.nodes.is_empty() {
        return None;
    }
    let mut best_score = 0u64;
    let mut best_node = None;
    for node in &view.nodes {
        for vnode in 0..VIRTUAL_NODES {
            let score = rendezvous_score(&actor.0, node.as_bytes(), vnode);
            if best_node.is_none() || score > best_score {
                best_score = score;
                best_node = Some(*node);
            }
        }
    }
    best_node
}

/// Derive one object in the fixed M2 home-evidence fixture set.
///
/// `object = BLAKE3-256("IRIX7-M2-HOME-OBJECT-v1" || index u32 LE)`.
/// The derivation is deliberately independent of any node, epoch or object
/// directory so the same test object exists before a mesh has formed.
pub fn fixture_object(index: u32) -> ObjectId {
    let mut hasher = blake3::Hasher::new();
    hasher.update(HOME_FIXTURE_DOMAIN);
    hasher.update(&index.to_le_bytes());
    let hash = hasher.finalize();
    ObjectId(*hash.as_bytes())
}

/// Build the configured M2 home-evidence fixture set.
///
/// The count is clamped to [`HOME_FIXTURE_MAX`]; a configured count is a test
/// parameter, not an unbounded directory.
pub fn fixture_objects(count: u32) -> Vec<ObjectId> {
    (0..count.min(HOME_FIXTURE_MAX))
        .map(fixture_object)
        .collect()
}

/// One object-to-home mapping under a confirmed membership view.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HomeEntry {
    /// Object identity.
    pub object: ObjectId,
    /// Home node selected by rendezvous hashing.
    pub home: NodeId,
}

/// A deterministic home mapping for a set of objects at one membership epoch.
///
/// This is the evidence surface shared by the module and the host rig.  It is
/// a snapshot, not a route cache: it carries the membership digest so a table
/// from an older epoch cannot be mistaken for the current one.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HomeTable {
    /// Membership epoch used for every entry.
    pub epoch: u32,
    /// Canonical membership snapshot digest (first 128 bits of BLAKE3).
    pub membership: [u8; 16],
    /// Entries in the configured object order.
    pub entries: Vec<HomeEntry>,
}

impl HomeTable {
    /// Compute homes for `objects` under `view`.
    ///
    /// Returns `None` for an empty membership view: there is no legitimate
    /// home without a member.  An empty object set produces an empty table.
    pub fn new(view: &MembershipView, membership: [u8; 16], objects: &[ObjectId]) -> Option<Self> {
        if view.is_empty() {
            return None;
        }
        let mut entries = Vec::with_capacity(objects.len());
        for object in objects {
            entries.push(HomeEntry {
                object: *object,
                home: home_for(object, view)?,
            });
        }
        Some(Self {
            epoch: view.epoch,
            membership,
            entries,
        })
    }

    /// Number of object mappings.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether the table has no objects.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Look up one object's home in this table.
    pub fn home_for(&self, object: &ObjectId) -> Option<NodeId> {
        self.entries
            .iter()
            .find(|entry| entry.object == *object)
            .map(|entry| entry.home)
    }

    /// Treat this table as current only when `epoch` is its epoch.
    ///
    /// A table from an older epoch must never be presented as the current
    /// mapping.  The caller keeps the old table but answers a stale query.
    pub fn at_epoch(&self, epoch: u32) -> Result<&Self, HomeEpochError> {
        if epoch == self.epoch {
            Ok(self)
        } else {
            Err(HomeEpochError::Stale {
                requested: epoch,
                current: self.epoch,
            })
        }
    }

    /// Compare two tables and report the first disagreement.
    pub fn compare(&self, other: &Self) -> Result<(), HomeMismatch> {
        if self.epoch != other.epoch {
            return Err(HomeMismatch::Epoch {
                left: self.epoch,
                right: other.epoch,
            });
        }
        if self.membership != other.membership {
            return Err(HomeMismatch::Membership {
                left: self.membership,
                right: other.membership,
            });
        }
        if self.entries.len() != other.entries.len() {
            return Err(HomeMismatch::EntryCount {
                left: self.entries.len(),
                right: other.entries.len(),
            });
        }
        for (index, (left, right)) in self.entries.iter().zip(&other.entries).enumerate() {
            if left.object != right.object {
                return Err(HomeMismatch::Object {
                    index,
                    left: left.object,
                    right: right.object,
                });
            }
            if left.home != right.home {
                return Err(HomeMismatch::Home {
                    index,
                    object: left.object,
                    left: left.home,
                    right: right.home,
                });
            }
        }
        Ok(())
    }
}

/// Why a table cannot answer a query at the requested epoch.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HomeEpochError {
    /// The requested epoch is not the table's confirmed epoch.
    Stale {
        /// The epoch the caller asked for.
        requested: u32,
        /// The table's confirmed epoch.
        current: u32,
    },
}

impl HomeEpochError {
    /// A short stable label for logs and evidence.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Stale { .. } => "stale-epoch",
        }
    }
}

/// The first disagreement between two home tables.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HomeMismatch {
    /// The tables were computed at different membership epochs.
    Epoch {
        /// Left-hand epoch.
        left: u32,
        /// Right-hand epoch.
        right: u32,
    },
    /// The tables carry different membership digests.
    Membership {
        /// Left-hand digest.
        left: [u8; 16],
        /// Right-hand digest.
        right: [u8; 16],
    },
    /// The tables contain different numbers of objects.
    EntryCount {
        /// Left-hand count.
        left: usize,
        /// Right-hand count.
        right: usize,
    },
    /// The tables list different objects at the same position.
    Object {
        /// Entry position.
        index: usize,
        /// Left-hand object.
        left: ObjectId,
        /// Right-hand object.
        right: ObjectId,
    },
    /// The tables disagree on one object's home.
    Home {
        /// Entry position.
        index: usize,
        /// The disputed object.
        object: ObjectId,
        /// Left-hand home.
        left: NodeId,
        /// Right-hand home.
        right: NodeId,
    },
}

impl HomeMismatch {
    /// A short stable label for logs and evidence.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Epoch { .. } => "epoch",
            Self::Membership { .. } => "membership",
            Self::EntryCount { .. } => "entry-count",
            Self::Object { .. } => "object",
            Self::Home { .. } => "home",
        }
    }
}

/// A route to an object's current home.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ObjectRoute {
    /// Object identity.
    pub object: ObjectId,
    /// Current home node.
    pub home: NodeId,
    /// Home assignment epoch.
    pub home_epoch: u32,
    /// Object revocation epoch.
    pub object_epoch: u32,
    /// Lease expiry in Unix nanoseconds (0 = no lease).
    pub lease_until: u64,
}

impl ObjectRoute {
    /// Whether the cached route has the same home and epochs as `current`.
    pub fn matches(&self, current: &ObjectRoute) -> bool {
        self.object == current.object
            && self.home == current.home
            && self.home_epoch == current.home_epoch
            && self.object_epoch >= current.object_epoch
    }

    /// Whether the route's lease is valid at `now_ns`.
    pub fn lease_valid(&self, now_ns: u64) -> bool {
        self.lease_until == 0 || now_ns <= self.lease_until
    }
}

/// A bounded route cache.  It is not authoritative; the home node is.
#[derive(Debug)]
pub struct RouteCache {
    map: BTreeMap<[u8; 32], ObjectRoute>,
    capacity: usize,
}

impl RouteCache {
    /// Create a cache with a hard capacity.
    pub fn new(capacity: usize) -> Self {
        Self {
            map: BTreeMap::new(),
            capacity,
        }
    }

    /// Look up a route.
    pub fn get(&self, object: &ObjectId) -> Option<&ObjectRoute> {
        self.map.get(object.as_bytes())
    }

    /// Insert or replace a route.  Evicts the lowest key when full.
    pub fn insert(&mut self, route: ObjectRoute) {
        if self.capacity == 0 {
            return;
        }
        if self.map.len() >= self.capacity && !self.map.contains_key(route.object.as_bytes()) {
            if let Some(first) = self.map.keys().next().copied() {
                self.map.remove(&first);
            }
        }
        self.map.insert(*route.object.as_bytes(), route);
    }

    /// Every cached route, in key order.
    pub fn iter(&self) -> impl Iterator<Item = &ObjectRoute> {
        self.map.values()
    }

    /// Remove a route (for example, after `MOVED` or `STALE`).
    pub fn invalidate(&mut self, object: &ObjectId) {
        self.map.remove(object.as_bytes());
    }

    /// Number of cached routes.
    pub fn len(&self) -> usize {
        self.map.len()
    }

    /// Whether the cache is empty.
    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }
}

/// A lease granted by a home node.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Lease {
    /// Node holding the lease.
    pub owner: NodeId,
    /// Epoch of the lease (fencing token).
    pub epoch: u32,
    /// Expiry in Unix nanoseconds.
    pub expires_ns: u64,
}

impl Lease {
    /// Whether the lease is valid at `now_ns`.
    pub fn is_valid(&self, now_ns: u64) -> bool {
        now_ns <= self.expires_ns
    }

    /// Whether `other` fences this lease.
    pub fn fenced_by(&self, other: &Lease) -> bool {
        other.epoch > self.epoch
    }
}

/// A per-node load summary.  It is an aggregate, not a global run queue.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NodeSummary {
    /// Node identity.
    pub node: NodeId,
    /// CPU utilisation in per-mille (0–1000).
    pub cpu_permille: u16,
    /// Free memory in MiB.
    pub mem_free_mib: u32,
    /// Runnable actors on the node.
    pub runnable: u16,
    /// Whether the node has a usable GPU.
    pub gpu: bool,
    /// Summary epoch.
    pub epoch: u32,
}

/// A placement view: the bounded, eventually consistent aggregate of node
/// summaries.  It is not a scheduler and it owns no run queue.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlacementView {
    /// Summaries, at most [`MAX_NODES`].
    pub summaries: Vec<NodeSummary>,
    /// View epoch.
    pub epoch: u32,
}

impl PlacementView {
    /// Create a view, rejecting more than [`MAX_NODES`] summaries.
    pub fn new(epoch: u32, summaries: Vec<NodeSummary>) -> Option<Self> {
        if summaries.len() > MAX_NODES {
            return None;
        }
        Some(Self { summaries, epoch })
    }

    /// Choose a host for `actor`.
    ///
    /// The score prefers low CPU, low runnable count, enough free memory and
    /// (when required) a GPU.  Ties are broken by a rendezvous score so the
    /// decision is deterministic and spread across nodes.
    pub fn choose_host(
        &self,
        actor: &ActorId,
        require_gpu: bool,
        min_mem_mib: u32,
    ) -> Option<NodeId> {
        let mut best: Option<(u32, u64, NodeId)> = None;
        for summary in &self.summaries {
            if require_gpu && !summary.gpu {
                continue;
            }
            if summary.mem_free_mib < min_mem_mib {
                continue;
            }
            let mem_pressure = 4096u32.saturating_sub(summary.mem_free_mib) / 64;
            let load = summary.cpu_permille as u32 + summary.runnable as u32 * 8 + mem_pressure;
            let tie = rendezvous_score(&actor.0, summary.node.as_bytes(), 0);
            let replace = match best {
                None => true,
                Some((best_load, best_tie, _)) => {
                    load < best_load || (load == best_load && tie > best_tie)
                }
            };
            if replace {
                best = Some((load, tie, summary.node));
            }
        }
        best.map(|(_, _, node)| node)
    }
}

/// Deterministic 64-bit rendezvous score: the first 64 bits of BLAKE3 over a
/// domain separator, identity, node and virtual-node index.
fn rendezvous_score(identity: &[u8], node: &[u8; 16], vnode: u32) -> u64 {
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"IRIX7-HOME-v1");
    hasher.update(identity);
    hasher.update(node);
    hasher.update(&vnode.to_le_bytes());
    let hash = hasher.finalize();
    let bytes = hash.as_bytes();
    u64::from_le_bytes([
        bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5], bytes[6], bytes[7],
    ])
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    fn node(n: u8) -> NodeId {
        NodeId([n; 16])
    }

    #[test]
    fn single_node_is_home_for_everything() {
        let view = MembershipView::single(node(1));
        assert!(view.is_single());
        for i in 0..1000u32 {
            let oid = ObjectId([(i & 0xff) as u8; 32]);
            assert_eq!(home_for(&oid, &view), Some(node(1)));
        }
    }

    #[test]
    fn rendezvous_spreads_across_128_nodes() {
        let nodes: Vec<NodeId> = (0..128u8).map(node).collect();
        let view = MembershipView::new(1, nodes).expect("view");
        let mut counts = [0u32; 128];
        for i in 0..1_024u32 {
            let mut oid = [0u8; 32];
            oid[0..4].copy_from_slice(&i.to_le_bytes());
            oid[4] = 0xa5;
            let home = home_for(&ObjectId(oid), &view).expect("home");
            counts[home.0[0] as usize] += 1;
        }
        let average = 1_024 / 128;
        let mut min = u32::MAX;
        let mut max = 0;
        for count in counts {
            min = min.min(count);
            max = max.max(count);
        }
        assert!(min > 0, "every node should own something");
        assert!(max < average * 3, "distribution should be bounded");
    }

    fn digest(byte: u8) -> [u8; 16] {
        [byte; 16]
    }

    fn fixture_table(view: &MembershipView, count: u32) -> HomeTable {
        let objects = fixture_objects(count);
        HomeTable::new(view, digest(0xa5), &objects).expect("table")
    }

    #[test]
    fn fixture_objects_are_deterministic_and_distinct() {
        let first = fixture_objects(HOME_FIXTURE_DEFAULT);
        let second = fixture_objects(HOME_FIXTURE_DEFAULT);
        assert_eq!(first, second);
        assert_eq!(first.len(), HOME_FIXTURE_DEFAULT as usize);
        let mut sorted = first.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), first.len());
        // The count is bounded even if a caller asks for more.
        assert_eq!(
            fixture_objects(HOME_FIXTURE_MAX + 100).len(),
            HOME_FIXTURE_MAX as usize
        );
        // Pin the domain derivation so a refactor cannot silently move every
        // object into a different rendezvous space.
        assert_eq!(&fixture_object(0).0[..4], &[0xBE, 0xB6, 0xD1, 0xC7]);
    }

    #[test]
    fn home_table_rejects_an_empty_membership_view() {
        let view = MembershipView::new(2, vec![]).expect("view");
        let objects = fixture_objects(4);
        assert_eq!(HomeTable::new(&view, digest(1), &objects), None);
    }

    #[test]
    fn home_table_accepts_an_empty_object_set() {
        let view = MembershipView::new(2, vec![node(1), node(2)]).expect("view");
        let table = HomeTable::new(&view, digest(1), &[]).expect("table");
        assert!(table.is_empty());
        assert_eq!(table.len(), 0);
        assert_eq!(table.epoch, 2);
    }

    #[test]
    fn single_node_is_home_for_every_fixture_object() {
        let view = MembershipView::new(2, vec![node(7)]).expect("view");
        let objects = fixture_objects(16);
        let table = HomeTable::new(&view, digest(3), &objects).expect("table");
        assert_eq!(table.len(), objects.len());
        for (entry, object) in table.entries.iter().zip(&objects) {
            assert_eq!(entry.object, *object);
            assert_eq!(entry.home, node(7));
        }
    }

    #[test]
    fn two_node_homes_are_deterministic_and_bounded() {
        let view = MembershipView::new(2, vec![node(2), node(1)]).expect("view");
        let objects = fixture_objects(HOME_FIXTURE_DEFAULT);
        let first = HomeTable::new(&view, digest(4), &objects).expect("table");
        let second = HomeTable::new(&view, digest(4), &objects).expect("table");
        assert_eq!(first, second);
        assert_eq!(first.len(), objects.len());
        assert!(first
            .entries
            .iter()
            .all(|entry| entry.home == node(1) || entry.home == node(2)));
        // A larger set must exercise both winners and stay close to an even
        // split; a single node owning everything is a placement regression.
        let many = fixture_table(&view, 256);
        let mut counts = [0usize; 3];
        for entry in &many.entries {
            counts[entry.home.0[0] as usize] += 1;
        }
        assert!(counts[1] > 0);
        assert!(counts[2] > 0);
        let average = many.len() / 2;
        assert!(counts[1] <= average * 3 / 2);
        assert!(counts[2] <= average * 3 / 2);
    }

    #[test]
    fn epoch_change_is_visible_and_a_stale_query_is_rejected() {
        let view2 = MembershipView::new(2, vec![node(1), node(2)]).expect("view");
        let view3 = MembershipView::new(3, vec![node(1), node(2)]).expect("view");
        let old = fixture_table(&view2, HOME_FIXTURE_DEFAULT);
        let new = fixture_table(&view3, HOME_FIXTURE_DEFAULT);
        // Membership did not change, so only the epoch and digest differ.
        assert_eq!(old.entries, new.entries);
        assert_eq!(old.at_epoch(2), Ok(&old));
        assert_eq!(
            old.at_epoch(3),
            Err(HomeEpochError::Stale {
                requested: 3,
                current: 2
            })
        );
        assert_eq!(
            old.compare(&new),
            Err(HomeMismatch::Epoch { left: 2, right: 3 })
        );

        let mut other_digest = new.clone();
        other_digest.membership = digest(0xee);
        assert_eq!(
            new.compare(&other_digest),
            Err(HomeMismatch::Membership {
                left: digest(0xa5),
                right: digest(0xee)
            })
        );
    }

    #[test]
    fn home_mismatch_reports_object_and_home_disagreement() {
        let view = MembershipView::new(2, vec![node(1), node(2)]).expect("view");
        let table = fixture_table(&view, HOME_FIXTURE_DEFAULT);
        assert_eq!(table.compare(&table), Ok(()));

        let mut other = table.clone();
        other.entries[0].home = if other.entries[0].home == node(1) {
            node(2)
        } else {
            node(1)
        };
        assert_eq!(
            table.compare(&other),
            Err(HomeMismatch::Home {
                index: 0,
                object: table.entries[0].object,
                left: table.entries[0].home,
                right: other.entries[0].home,
            })
        );

        let mut other = table.clone();
        other.entries[0].object = ObjectId([0xee; 32]);
        assert_eq!(
            table.compare(&other),
            Err(HomeMismatch::Object {
                index: 0,
                left: table.entries[0].object,
                right: ObjectId([0xee; 32]),
            })
        );

        let mut other = table.clone();
        other.entries.pop();
        assert_eq!(
            table.compare(&other),
            Err(HomeMismatch::EntryCount {
                left: HOME_FIXTURE_DEFAULT as usize,
                right: HOME_FIXTURE_DEFAULT as usize - 1,
            })
        );
    }

    #[test]
    fn removing_a_node_moves_only_its_objects() {
        let three = MembershipView::new(4, vec![node(1), node(2), node(3)]).expect("view");
        let two = MembershipView::new(5, vec![node(1), node(2)]).expect("view");
        let objects = fixture_objects(256);
        let before = HomeTable::new(&three, digest(1), &objects).expect("before");
        let after = HomeTable::new(&two, digest(2), &objects).expect("after");
        let mut moved = 0;
        let mut stayed = 0;
        for (old, new) in before.entries.iter().zip(&after.entries) {
            assert_eq!(old.object, new.object);
            if old.home == node(3) {
                moved += 1;
                assert_ne!(new.home, node(3));
            } else {
                stayed += 1;
                assert_eq!(old.home, new.home);
            }
        }
        assert!(moved > 0, "the removed node must have owned some objects");
        assert!(stayed > 0, "other homes must not churn");
    }

    #[test]
    fn route_cache_is_bounded() {
        let mut cache = RouteCache::new(2);
        for i in 0..4u8 {
            cache.insert(ObjectRoute {
                object: ObjectId([i; 32]),
                home: node(i),
                home_epoch: 1,
                object_epoch: 1,
                lease_until: 0,
            });
        }
        assert_eq!(cache.len(), 2);
    }

    #[test]
    fn lease_expiry_and_fencing() {
        let lease = Lease {
            owner: node(1),
            epoch: 4,
            expires_ns: 100,
        };
        assert!(lease.is_valid(100));
        assert!(!lease.is_valid(101));
        let newer = Lease {
            owner: node(2),
            epoch: 5,
            expires_ns: 200,
        };
        assert!(lease.fenced_by(&newer));
    }

    #[test]
    fn placement_prefers_gpu_when_required() {
        let view = PlacementView::new(
            1,
            vec![
                NodeSummary {
                    node: node(1),
                    cpu_permille: 100,
                    mem_free_mib: 4096,
                    runnable: 0,
                    gpu: false,
                    epoch: 1,
                },
                NodeSummary {
                    node: node(2),
                    cpu_permille: 900,
                    mem_free_mib: 4096,
                    runnable: 8,
                    gpu: true,
                    epoch: 1,
                },
            ],
        )
        .expect("view");
        assert_eq!(view.choose_host(&ActorId([7; 16]), true, 64), Some(node(2)));
        assert_eq!(
            view.choose_host(&ActorId([7; 16]), false, 64),
            Some(node(1))
        );
    }
}
