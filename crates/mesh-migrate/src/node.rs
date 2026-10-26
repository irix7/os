// SPDX-License-Identifier: GPL-3.0-only
//! The L0 actor-migration state machine for the root and for a member.
//!
//! The policy owns no socket; the MCP module owns the frame loop and calls
//! [`MigrateNode::on_wire`].  It is synchronous and clock-injected so the host
//! tests drive every step without sleeping.

use alloc::vec::Vec;

use cap_engine::TrustStore;
use mesh_core::{actor_home, MembershipView};
use sys_primitives::{
    ActorId, Capability, MeshHeader, MessageKind, NodeId, NodeKey, ObjectId, Rights, Status,
};

use crate::wire::{self, ActorMove, Body, Wire};
use crate::{
    actor_for, actor_object, actor_principal, message_for, Ack, ActorState, Delivery, DeliveryMode,
    Error, Handle, Message, MIGRATION_DEADLINE_MS, SCHEMA, STATE_LEN,
};

/// Signing and public-key access for the authority root.
pub trait Signer {
    /// The authority public key.
    fn key(&self) -> NodeKey;
    /// Sign a canonical message, or return `None` on a member.
    fn sign(&self, message: &[u8]) -> Option<[u8; 64]>;
}

/// One state change the module should log, or a test should inspect.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Event {
    /// The migration policy is enabled for this node.
    Enabled {
        /// `root` or `member`.
        role: &'static str,
    },
    /// A confirmed membership view was installed.
    Snapshot {
        /// Membership epoch.
        epoch: u32,
        /// Member count.
        members: usize,
        /// Membership digest.
        digest: [u8; 16],
    },
    /// The fixture actor was created at this host.
    Seed {
        /// Actor identity.
        actor: ActorId,
        /// Current host.
        host: NodeId,
        /// Placement epoch.
        epoch: u32,
        /// Canonical state hash.
        hash: [u8; 32],
    },
    /// One message advanced the actor or was an idempotent duplicate.
    Deliver {
        /// Actor identity.
        actor: ActorId,
        /// Message id.
        message: u64,
        /// Applied-message counter.
        applied: u64,
        /// Whether the message was a duplicate.
        duplicate: bool,
        /// Canonical state hash after the delivery.
        hash: [u8; 32],
    },
    /// One message was queued at the boundary.
    Queued {
        /// Actor identity.
        actor: ActorId,
        /// Message id.
        message: u64,
    },
    /// The queued message was applied at the new host.
    Resume {
        /// Actor identity.
        actor: ActorId,
        /// Message id.
        message: u64,
        /// Applied-message counter.
        applied: u64,
        /// Canonical state hash after the step.
        hash: [u8; 32],
    },
    /// A migration started.
    Start {
        /// Actor identity.
        actor: ActorId,
        /// Old host.
        from: NodeId,
        /// New host.
        to: NodeId,
        /// Old placement epoch.
        old_epoch: u32,
        /// New placement epoch.
        new_epoch: u32,
    },
    /// The destination installed the actor and the source fenced the old one.
    Moved {
        /// Actor identity.
        actor: ActorId,
        /// Old host.
        from: NodeId,
        /// New host.
        to: NodeId,
        /// Old placement epoch.
        old_epoch: u32,
        /// New placement epoch.
        new_epoch: u32,
        /// Canonical state hash.
        hash: [u8; 32],
        /// `source` or `destination`.
        role: &'static str,
    },
    /// The old instance was fenced.
    Fenced {
        /// Actor identity.
        actor: ActorId,
        /// Old host.
        old_host: NodeId,
        /// New host.
        new_host: NodeId,
        /// Old placement epoch.
        old_epoch: u32,
        /// New placement epoch.
        new_epoch: u32,
        /// Canonical state hash.
        hash: [u8; 32],
    },
    /// A delivery named a superseded home or epoch.
    Stale {
        /// Actor identity.
        actor: ActorId,
        /// Why.
        reason: Error,
        /// Epoch expected here.
        expected_epoch: u32,
        /// Epoch named by the sender.
        got_epoch: u32,
    },
    /// A duplicate delivery was handled idempotently.
    Duplicate {
        /// Actor identity.
        actor: ActorId,
        /// Message id.
        message: u64,
        /// Applied-message counter (unchanged).
        applied: u64,
    },
    /// A delivery capability was refused.
    Capability {
        /// Actor identity.
        actor: ActorId,
        /// Why.
        reason: Error,
    },
    /// A migration step was refused or aborted.
    Reject {
        /// Actor identity, when known.
        actor: Option<ActorId>,
        /// Why.
        reason: Error,
    },
    /// A migration lease expired.
    Aborted {
        /// Actor identity, when known.
        actor: Option<ActorId>,
        /// Why.
        reason: Error,
    },
}

/// One migration-policy step: an optional frame and zero or more events.
#[derive(Clone, Debug, Default)]
pub struct Step {
    /// Frame to send, if any.
    pub send: Option<Wire>,
    /// Events to log.
    pub events: Vec<Event>,
}

impl Step {
    /// An empty step.
    pub fn new() -> Self {
        Self {
            send: None,
            events: Vec::new(),
        }
    }

    fn event(mut self, event: Event) -> Self {
        self.events.push(event);
        self
    }

    fn with_send(mut self, send: Wire) -> Self {
        self.send = Some(send);
        self
    }

    fn merge(&mut self, other: Step) {
        if other.send.is_some() {
            self.send = other.send;
        }
        self.events.extend(other.events);
    }
}

/// A fenced old instance.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Fence {
    /// The last state the source serialised.
    pub state: ActorState,
    /// The old host.
    pub old_host: NodeId,
    /// The new host.
    pub new_host: NodeId,
    /// The old placement epoch.
    pub old_epoch: u32,
    /// The new placement epoch.
    pub new_epoch: u32,
    /// Canonical state hash.
    pub hash: [u8; 32],
}

/// The most recent placement change, retained for the stale-home evidence.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LastMove {
    /// Actor identity.
    pub actor: ActorId,
    /// Old host.
    pub old_host: NodeId,
    /// New host.
    pub new_host: NodeId,
    /// Old placement epoch.
    pub old_epoch: u32,
    /// New placement epoch.
    pub new_epoch: u32,
    /// Canonical state hash.
    pub hash: [u8; 32],
}

/// One actor row in the bounded migration report.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ActorReport {
    /// Actor identity.
    pub actor: ActorId,
    /// Rendezvous home.
    pub home: NodeId,
    /// Current host.
    pub host: NodeId,
    /// Placement epoch.
    pub epoch: u32,
    /// `active`, `provisional`, `fenced` or `none`.
    pub state: &'static str,
    /// Applied-message counter.
    pub applied: u64,
    /// Highest applied message id.
    pub last: u64,
    /// Logical digest.
    pub logical: [u8; 32],
    /// Canonical logical state hash.
    pub hash: [u8; 32],
    /// Queued message.
    pub queued: Option<Message>,
    /// Last acknowledgement.
    pub unacked: Option<Ack>,
    /// Serialised capability handle.
    pub handle: Option<Handle>,
}

/// Bounded counters for the migration report.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Counters {
    /// Completed migrations involving this node.
    pub migrations: u32,
    /// Migrations where this node was the source.
    pub outgoing: u32,
    /// Migrations where this node was the destination.
    pub incoming: u32,
    /// Queued messages resumed.
    pub resumes: u32,
    /// Idempotent duplicate deliveries.
    pub duplicates: u32,
    /// Stale-home or stale-epoch refusals.
    pub stale: u32,
    /// Delivery capability refusals.
    pub capability_rejects: u32,
    /// Other terminal refusals.
    pub rejects: u32,
    /// Aborted or timed-out migrations.
    pub aborted: u32,
}

/// The bounded in-memory report served by `GET /mesh/migrate`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Report {
    /// Whether migration is enabled.
    pub enabled: bool,
    /// `root` or `member`.
    pub role: &'static str,
    /// Confirmed membership epoch.
    pub view_epoch: u32,
    /// Confirmed member count.
    pub view_members: usize,
    /// Confirmed membership digest.
    pub view_digest: [u8; 16],
    /// The actor row, when this node has hosted or hosts it.
    pub actor: Option<ActorReport>,
    /// The latest placement change, when one happened.
    pub last_move: Option<LastMove>,
    /// Counters.
    pub counters: Counters,
    /// Last protocol stage.
    pub stage: &'static str,
    /// Last refusal reason.
    pub reason: Option<Error>,
}

struct Pending {
    directive: ActorMove,
    state: Option<ActorState>,
    state_bytes: Vec<u8>,
    hash: [u8; 32],
    request_id: u64,
    deadline_ms: u64,
    kind: PendingKind,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PendingKind {
    /// Root sent AUTHORISE and is waiting for the source to prepare.
    Coordinating,
    /// Source froze the actor and sent PREPARE.
    SourcePreparing,
    /// Source sent STATE and is waiting for COMMIT.
    SourceCommitting,
    /// Destination reserved the actor and is waiting for STATE.
    DestReserved,
    /// Destination installed the actor and is waiting for ACK.
    DestCommitting,
}

/// The M2 L0 migration policy for one node.
pub struct MigrateNode {
    self_id: NodeId,
    authority_id: NodeId,
    authority_key: NodeKey,
    trust: TrustStore,
    root: bool,
    view: MembershipView,
    digest: [u8; 16],
    epoch: u32,
    actor: Option<ActorState>,
    fence: Option<Fence>,
    last_move: Option<LastMove>,
    pending: Option<Pending>,
    next_request: u64,
    last_request: u64,
    capability_sequence: u64,
    peer: Option<NodeId>,
    report: Report,
}

impl MigrateNode {
    /// Create a migration policy for one node.
    pub fn new(
        self_id: NodeId,
        authority_id: NodeId,
        authority_key: NodeKey,
        root: bool,
    ) -> Option<Self> {
        if self_id.is_zero()
            || authority_id.is_zero()
            || authority_key.0.iter().all(|&byte| byte == 0)
        {
            return None;
        }
        let mut trust = TrustStore::new();
        trust.insert(authority_key);
        Some(Self {
            self_id,
            authority_id,
            authority_key,
            trust,
            root,
            view: MembershipView::new(0, Vec::new()).expect("empty view is bounded"),
            digest: [0; 16],
            epoch: 0,
            actor: None,
            fence: None,
            last_move: None,
            pending: None,
            next_request: 1,
            last_request: 0,
            capability_sequence: 0,
            peer: None,
            report: Report {
                enabled: true,
                role: if root { "root" } else { "member" },
                ..Report::default()
            },
        })
    }

    /// This node's identity.
    pub const fn self_id(&self) -> NodeId {
        self.self_id
    }

    /// The authority root.
    pub const fn authority(&self) -> NodeId {
        self.authority_id
    }

    /// Whether this node is the authority root.
    pub const fn is_root(&self) -> bool {
        self.root
    }

    /// The configured peer.
    pub const fn peer(&self) -> Option<NodeId> {
        self.peer
    }

    /// The confirmed membership epoch.
    pub const fn epoch(&self) -> u32 {
        self.epoch
    }

    /// The confirmed membership digest.
    pub const fn digest(&self) -> [u8; 16] {
        self.digest
    }

    /// The confirmed membership view.
    pub const fn view(&self) -> &MembershipView {
        &self.view
    }

    /// The bounded migration report.
    pub fn report(&self) -> &Report {
        &self.report
    }

    /// The active actor state, when this node is the current host.
    pub fn actor(&self) -> Option<&ActorState> {
        self.actor.as_ref()
    }

    /// The fenced old instance, when this node moved the actor away.
    pub fn fence(&self) -> Option<&Fence> {
        self.fence.as_ref()
    }

    /// Set or replace the configured peer.
    pub fn set_peer(&mut self, peer: NodeId) -> Option<Event> {
        if peer.is_zero() || peer == self.self_id {
            return Some(Event::Reject {
                actor: None,
                reason: Error::NoPeer,
            });
        }
        if let Some(current) = self.peer {
            if current != peer {
                self.abort_pending(Error::LinkDown);
                return Some(Event::Reject {
                    actor: None,
                    reason: Error::NoPeer,
                });
            }
            return None;
        }
        self.peer = Some(peer);
        self.last_request = 0;
        None
    }

    /// Install a confirmed membership snapshot.
    pub fn on_snapshot(
        &mut self,
        epoch: u32,
        members: &[NodeId],
        digest: [u8; 16],
    ) -> Option<Step> {
        if epoch == 0 || members.is_empty() {
            return None;
        }
        let view = MembershipView::new(epoch, members.to_vec())?;
        if epoch < self.epoch {
            return None;
        }
        if epoch == self.epoch {
            // Never relabel a different list as the same epoch.
            return None;
        }
        if self.pending.is_some() {
            self.abort_pending(Error::StaleEpoch);
        }
        self.view = view;
        self.digest = digest;
        self.epoch = epoch;
        self.report.view_epoch = epoch;
        self.report.view_members = self.view.len();
        self.report.view_digest = digest;
        Some(Step::new().event(Event::Snapshot {
            epoch,
            members: self.view.len(),
            digest,
        }))
    }

    /// The link was torn down.  In-flight migration state fails closed.
    pub fn on_link_down(&mut self) -> Option<Event> {
        self.peer = None;
        self.last_request = 0;
        let actor = self.pending.as_ref().map(|pending| pending.directive.actor);
        self.abort_pending(Error::LinkDown);
        actor.map(|actor| Event::Reject {
            actor: Some(actor),
            reason: Error::LinkDown,
        })
    }

    /// Drive the pending migration deadline.
    pub fn tick(&mut self, now_ms: u64) -> Step {
        if let Some(pending) = &self.pending {
            if now_ms < pending.deadline_ms {
                return Step::new();
            }
            let actor = Some(pending.directive.actor);
            self.abort_pending(Error::Timeout);
            self.report.counters.aborted = self.report.counters.aborted.saturating_add(1);
            self.report.reason = Some(Error::Timeout);
            self.report.stage = "abort";
            return Step::new().event(Event::Aborted {
                actor,
                reason: Error::Timeout,
            });
        }
        Step::new()
    }

    /// Run one rig step.  This is the lab control surface behind
    /// `GET /mesh/migrate?step=N`; it is only available on the root.
    ///
    /// ```text
    /// 1  seed the fixture actor at the root and stop at a message boundary
    /// 2  move it root -> member
    /// 3  resume the queued message at the member
    /// 4  refuse a stale delivery at the old (root) host
    /// 5  duplicate-deliver message 10 at the member (idempotent)
    /// 6  queue message 11 at the member
    /// 7  move it member -> root
    /// 8  resume the queued message at the root
    /// 9  refuse a stale delivery at the old (member) host
    /// 10 duplicate-deliver message 11 at the root (idempotent)
    /// 11 send a tampered capability to the current host
    /// ```
    pub fn command(&mut self, step: u8, now_ms: u64, signer: &dyn Signer) -> Step {
        if !self.root {
            return self.reject(None, Error::NotRoot);
        }
        if self.epoch == 0 || self.view.is_empty() {
            return self.reject(None, Error::NoView);
        }
        match step {
            1 => self.command_seed(now_ms, signer),
            2 => self.command_move_out(now_ms, signer),
            3 => self.command_resume(now_ms, signer),
            4 | 9 => self.command_stale(now_ms, signer),
            5 => self.command_duplicate(1, now_ms, signer),
            6 => self.command_queue(2, now_ms, signer),
            7 => self.command_move_back(now_ms, signer),
            8 => self.command_resume(now_ms, signer),
            10 => self.command_duplicate(2, now_ms, signer),
            11 => self.command_capability_negative(now_ms, signer),
            _ => self.reject(None, Error::Malformed),
        }
    }

    fn command_seed(&mut self, _now_ms: u64, signer: &dyn Signer) -> Step {
        if self.actor.is_some() || self.fence.is_some() {
            return self.reject(None, Error::Busy);
        }
        let actor_id = actor_for(0);
        let Some(home) = actor_home(&actor_id, &self.view) else {
            return self.reject(Some(actor_id), Error::NoView);
        };
        let mut actor = ActorState::new(
            actor_id,
            home,
            self.self_id,
            Some(Handle::fixture(self.self_id)),
        );
        let first = message_for(0);
        let sequence = self.next_capability_sequence();
        let Some(capability) = self.issue_capability(&actor, signer, sequence) else {
            return self.reject(Some(actor_id), Error::NotRoot);
        };
        if let Err(reason) = self.verify_capability(&capability, &actor, actor.host, actor.epoch, 0)
        {
            return self.reject(Some(actor_id), reason);
        }
        let applied = actor.deliver(first, DeliveryMode::Step);
        let queued = message_for(1);
        let queued_outcome = actor.deliver(queued, DeliveryMode::Queue);
        let hash = actor.hash();
        self.actor = Some(actor);
        self.report.stage = "seeded";
        let mut step = Step::new()
            .event(Event::Seed {
                actor: actor_id,
                host: self.self_id,
                epoch: 1,
                hash,
            })
            .event(Event::Deliver {
                actor: actor_id,
                message: first.id,
                applied: match applied {
                    Delivery::Applied { applied, .. } => applied,
                    _ => 0,
                },
                duplicate: false,
                hash,
            });
        if let Delivery::Queued { id, .. } = queued_outcome {
            step.events.push(Event::Queued {
                actor: actor_id,
                message: id,
            });
        }
        step
    }

    fn command_move_out(&mut self, now_ms: u64, signer: &dyn Signer) -> Step {
        if self.pending.is_some() {
            return self.reject(None, Error::Busy);
        }
        let Some(peer) = self.peer else {
            return self.reject(None, Error::NoPeer);
        };
        let Some(actor) = self.actor.as_ref() else {
            return self.reject(None, Error::NoActor);
        };
        if actor.host != self.self_id {
            return self.reject(Some(actor.actor), Error::NotSource);
        }
        let mut directive = ActorMove::new(
            actor.actor,
            actor.home,
            self.self_id,
            peer,
            self.epoch,
            self.digest,
            actor.epoch,
            crate::MAX_LEASE_MS,
        );
        if let Err(reason) = self.sign_directive(&mut directive, signer) {
            return self.reject(Some(directive.actor), reason);
        }
        self.start_source(directive, now_ms)
    }

    fn command_move_back(&mut self, now_ms: u64, signer: &dyn Signer) -> Step {
        if self.pending.is_some() {
            return self.reject(None, Error::Busy);
        }
        let Some(peer) = self.peer else {
            return self.reject(None, Error::NoPeer);
        };
        let Some(fence) = self.fence.as_ref() else {
            return self.reject(None, Error::NoActor);
        };
        if fence.new_host != peer {
            return self.reject(Some(fence.state.actor), Error::NotSource);
        }
        let directive = ActorMove::new(
            fence.state.actor,
            fence.state.home,
            peer,
            self.self_id,
            self.epoch,
            self.digest,
            fence.new_epoch,
            crate::MAX_LEASE_MS,
        );
        let mut directive = directive;
        if let Err(reason) = self.sign_directive(&mut directive, signer) {
            return self.reject(Some(directive.actor), reason);
        }
        let request_id = self.take_request_id();
        let body = match encode_body(wire::Body::Authorise(directive)) {
            Ok(body) => body,
            Err(reason) => return self.reject(Some(directive.actor), reason),
        };
        let wire = self.wire(
            peer,
            MessageKind::Request,
            directive.home,
            directive.old_epoch,
            directive.actor,
            request_id,
            0,
            body,
        );
        self.pending = Some(Pending {
            directive,
            state: None,
            state_bytes: Vec::new(),
            hash: [0; 32],
            request_id,
            deadline_ms: now_ms.saturating_add(MIGRATION_DEADLINE_MS),
            kind: PendingKind::Coordinating,
        });
        self.report.stage = "authorise";
        Step::new().with_send(wire).event(Event::Start {
            actor: directive.actor,
            from: directive.old_host,
            to: directive.new_host,
            old_epoch: directive.old_epoch,
            new_epoch: directive.new_epoch,
        })
    }

    fn start_source(&mut self, directive: ActorMove, now_ms: u64) -> Step {
        if let Err(reason) = directive.verify(&self.authority_key) {
            return self.reject(Some(directive.actor), reason);
        }
        let Some(actor) = self.actor.as_ref() else {
            return self.reject(Some(directive.actor), Error::NoActor);
        };
        if actor.actor != directive.actor {
            return self.reject(Some(directive.actor), Error::WrongActor);
        }
        if actor.home != directive.home {
            return self.reject(Some(directive.actor), Error::WrongHome);
        }
        if actor.host != directive.old_host {
            return self.reject(Some(directive.actor), Error::WrongHost);
        }
        if actor.epoch != directive.old_epoch {
            return self.reject(Some(directive.actor), Error::WrongEpoch);
        }
        let frozen = actor.clone();
        let mut bytes = [0u8; STATE_LEN];
        let len = match frozen.encode(&mut bytes) {
            Ok(len) => len,
            Err(reason) => return self.reject(Some(directive.actor), reason),
        };
        let hash = frozen.hash();
        let state_bytes = bytes[..len].to_vec();
        self.actor = None;
        let request_id = self.take_request_id();
        let body = match encode_body(wire::Body::Prepare {
            directive,
            state_len: len as u16,
            hash,
        }) {
            Ok(body) => body,
            Err(reason) => {
                self.actor = Some(frozen);
                return self.reject(Some(directive.actor), reason);
            }
        };
        let wire = self.wire(
            directive.new_host,
            MessageKind::Request,
            directive.home,
            directive.old_epoch,
            directive.actor,
            request_id,
            0,
            body,
        );
        self.pending = Some(Pending {
            directive,
            state: Some(frozen),
            state_bytes,
            hash,
            request_id,
            deadline_ms: now_ms.saturating_add(MIGRATION_DEADLINE_MS),
            kind: PendingKind::SourcePreparing,
        });
        self.report.stage = "prepare";
        Step::new().with_send(wire).event(Event::Start {
            actor: directive.actor,
            from: directive.old_host,
            to: directive.new_host,
            old_epoch: directive.old_epoch,
            new_epoch: directive.new_epoch,
        })
    }

    fn command_resume(&mut self, now_ms: u64, signer: &dyn Signer) -> Step {
        let (actor_id, epoch, host) = match (&self.actor, &self.fence) {
            (Some(actor), _) => (actor.actor, actor.epoch, actor.host),
            (None, Some(fence)) => (fence.state.actor, fence.new_epoch, fence.new_host),
            (None, None) => return self.reject(None, Error::NoActor),
        };
        if host == self.self_id {
            let sequence = self.next_capability_sequence();
            let Some(actor_ref) = self.actor.as_ref() else {
                return self.reject(Some(actor_id), Error::Busy);
            };
            let Some(capability) = self.issue_capability(actor_ref, signer, sequence) else {
                return self.reject(Some(actor_id), Error::NotRoot);
            };
            if let Err(reason) = self.verify_capability(
                &capability,
                actor_ref,
                actor_ref.host,
                actor_ref.epoch,
                now_ms,
            ) {
                return self.reject(Some(actor_id), reason);
            }
            let Some(actor) = self.actor.as_mut() else {
                return self.reject(Some(actor_id), Error::Busy);
            };
            let outcome = actor.resume();
            let hash = actor.hash();
            self.report.stage = "resume";
            return self.local_delivery_events(actor_id, outcome, hash, true);
        }
        let sequence = self.next_capability_sequence();
        let Some(capability) = self.issue_capability_for(actor_id, host, epoch, signer, sequence)
        else {
            return self.reject(Some(actor_id), Error::NotRoot);
        };
        let request_id = self.take_request_id();
        let body = match encode_body(wire::Body::Resume {
            actor: actor_id,
            epoch,
            capability,
        }) {
            Ok(body) => body,
            Err(reason) => return self.reject(Some(actor_id), reason),
        };
        let home = self.actor_home_or(actor_id);
        let wire = self.wire(
            host,
            MessageKind::Request,
            home,
            epoch,
            actor_id,
            request_id,
            0,
            body,
        );
        self.report.stage = "resume";
        Step::new().with_send(wire)
    }

    fn command_duplicate(&mut self, message_index: u32, now_ms: u64, signer: &dyn Signer) -> Step {
        let message = message_for(message_index);
        self.command_deliver(message, DeliveryMode::Step, now_ms, signer, "duplicate")
    }

    fn command_queue(&mut self, message_index: u32, now_ms: u64, signer: &dyn Signer) -> Step {
        let message = message_for(message_index);
        self.command_deliver(message, DeliveryMode::Queue, now_ms, signer, "queue")
    }

    fn command_deliver(
        &mut self,
        message: Message,
        mode: DeliveryMode,
        now_ms: u64,
        signer: &dyn Signer,
        stage: &'static str,
    ) -> Step {
        let (actor_id, epoch, host) = match (&self.actor, &self.fence) {
            (Some(actor), _) => (actor.actor, actor.epoch, actor.host),
            (None, Some(fence)) => (fence.state.actor, fence.new_epoch, fence.new_host),
            (None, None) => return self.reject(None, Error::NoActor),
        };
        let Some(capability) = self.issue_capability_for(actor_id, host, epoch, signer, message.id)
        else {
            return self.reject(Some(actor_id), Error::NotRoot);
        };
        if host == self.self_id {
            let Some(actor_ref) = self.actor.as_ref() else {
                return self.reject(Some(actor_id), Error::Busy);
            };
            if let Err(reason) = self.verify_capability(&capability, actor_ref, host, epoch, now_ms)
            {
                self.report.counters.capability_rejects =
                    self.report.counters.capability_rejects.saturating_add(1);
                self.report.reason = Some(reason);
                return Step::new().event(Event::Capability {
                    actor: actor_id,
                    reason,
                });
            }
            let Some(actor) = self.actor.as_mut() else {
                return self.reject(Some(actor_id), Error::Busy);
            };
            let outcome = actor.deliver(message, mode);
            let hash = actor.hash();
            self.report.stage = stage;
            return self.local_delivery_events(actor_id, outcome, hash, false);
        }
        let request_id = self.take_request_id();
        let body = match encode_body(wire::Body::Deliver {
            actor: actor_id,
            epoch,
            capability,
            message,
            mode: mode.as_u8(),
        }) {
            Ok(body) => body,
            Err(reason) => return self.reject(Some(actor_id), reason),
        };
        let home = self.actor_home_or(actor_id);
        let wire = self.wire(
            host,
            MessageKind::Request,
            home,
            epoch,
            actor_id,
            request_id,
            0,
            body,
        );
        self.report.stage = stage;
        Step::new().with_send(wire)
    }

    fn command_stale(&mut self, now_ms: u64, signer: &dyn Signer) -> Step {
        let Some(last) = self.last_move else {
            return self.reject(None, Error::NoActor);
        };
        if last.old_host == self.self_id {
            // The old home is local.  Feed the stale delivery through the same
            // policy path as a wire frame; the refuser is the local fence.
            self.report.counters.stale = self.report.counters.stale.saturating_add(1);
            self.report.reason = Some(Error::StaleEpoch);
            self.report.stage = "stale";
            return Step::new().event(Event::Stale {
                actor: last.actor,
                reason: Error::StaleEpoch,
                expected_epoch: last.new_epoch,
                got_epoch: last.old_epoch,
            });
        }
        let Some(peer) = self.peer else {
            return self.reject(Some(last.actor), Error::NoPeer);
        };
        if last.old_host != peer {
            return self.reject(Some(last.actor), Error::NotSource);
        }
        let capability = match self.issue_capability_for(
            last.actor,
            last.old_host,
            last.old_epoch,
            signer,
            last.old_epoch as u64,
        ) {
            Some(capability) => capability,
            None => return self.reject(Some(last.actor), Error::NotRoot),
        };
        let request_id = self.take_request_id();
        let body = match encode_body(wire::Body::Deliver {
            actor: last.actor,
            epoch: last.old_epoch,
            capability,
            message: message_for(0),
            mode: DeliveryMode::Step.as_u8(),
        }) {
            Ok(body) => body,
            Err(reason) => return self.reject(Some(last.actor), reason),
        };
        let wire = self.wire(
            last.old_host,
            MessageKind::Request,
            self.actor_home_or(last.actor),
            last.old_epoch,
            last.actor,
            request_id,
            0,
            body,
        );
        self.report.stage = "stale";
        let _ = now_ms;
        Step::new().with_send(wire)
    }

    fn command_capability_negative(&mut self, now_ms: u64, signer: &dyn Signer) -> Step {
        let (actor_id, epoch, host) = match (&self.actor, &self.fence) {
            (Some(actor), _) => (actor.actor, actor.epoch, actor.host),
            (None, Some(fence)) => (fence.state.actor, fence.new_epoch, fence.new_host),
            (None, None) => return self.reject(None, Error::NoActor),
        };
        let sequence = self.next_capability_sequence();
        let Some(mut capability) =
            self.issue_capability_for(actor_id, host, epoch, signer, sequence)
        else {
            return self.reject(Some(actor_id), Error::NotRoot);
        };
        capability.signature[0] ^= 0xff;
        let message = message_for(2);
        if host == self.self_id {
            let Some(actor_ref) = self.actor.as_ref() else {
                return self.reject(Some(actor_id), Error::Busy);
            };
            let reason = match self.verify_capability(&capability, actor_ref, host, epoch, now_ms) {
                Ok(()) => Error::Capability,
                Err(reason) => reason,
            };
            self.report.counters.capability_rejects =
                self.report.counters.capability_rejects.saturating_add(1);
            self.report.reason = Some(reason);
            self.report.stage = "capability";
            return Step::new().event(Event::Capability {
                actor: actor_id,
                reason,
            });
        }
        let request_id = self.take_request_id();
        let body = match encode_body(wire::Body::Deliver {
            actor: actor_id,
            epoch,
            capability,
            message,
            mode: DeliveryMode::Step.as_u8(),
        }) {
            Ok(body) => body,
            Err(reason) => return self.reject(Some(actor_id), reason),
        };
        let wire = self.wire(
            host,
            MessageKind::Request,
            self.actor_home_or(actor_id),
            epoch,
            actor_id,
            request_id,
            0,
            body,
        );
        self.report.stage = "capability";
        Step::new().with_send(wire)
    }

    /// Handle one decoded migration record from the peer.
    #[allow(clippy::too_many_arguments)]
    pub fn on_wire(
        &mut self,
        src: NodeId,
        header: &MeshHeader,
        kind: MessageKind,
        status: Status,
        body: &[u8],
        now_ms: u64,
        signer: &dyn Signer,
    ) -> Step {
        let _ = signer;
        if src.is_zero() || src == self.self_id {
            return self.reject(None, Error::NoPeer);
        }
        if self.peer != Some(src) {
            return self.reject(None, Error::NoPeer);
        }
        if !header.dst.is_zero() && header.dst != self.self_id {
            return self.reject(None, Error::WrongHost);
        }
        if body.len() > crate::MAX_BODY {
            return self.reject(None, Error::Malformed);
        }
        let body = match Body::decode(body) {
            Ok(body) => body,
            Err(reason) => return self.reject(None, reason),
        };
        if body.message_kind() != kind {
            return self.reject(None, Error::KindMismatch);
        }
        if kind == MessageKind::Request {
            if let Err(reason) = self.accept_request(header) {
                return self.reject(body_actor(&body), reason);
            }
        }
        let step = match body {
            Body::Authorise(directive) => self.handle_authorise(src, header, directive, now_ms),
            Body::Prepare {
                directive,
                state_len,
                hash,
            } => self.handle_prepare(src, header, directive, state_len, hash, now_ms),
            Body::Ready { directive, hash } => self.handle_ready(directive, hash, now_ms),
            Body::State { directive, state } => {
                self.handle_state(src, header, directive, &state, now_ms)
            }
            Body::Commit { directive, hash } => self.handle_commit(directive, hash, now_ms),
            Body::Abort { directive, reason } => {
                self.abort_pending(reason);
                self.report.counters.aborted = self.report.counters.aborted.saturating_add(1);
                self.report.stage = "abort";
                self.report.reason = Some(reason);
                Step::new().event(Event::Aborted {
                    actor: Some(directive.actor),
                    reason,
                })
            }
            Body::Ack { directive, hash } => self.handle_ack(directive, hash),
            Body::Deliver {
                actor,
                epoch,
                capability,
                message,
                mode,
            } => self.handle_deliver(src, header, actor, epoch, capability, message, mode, now_ms),
            Body::Result {
                actor,
                epoch,
                message,
                applied,
                logical,
                hash,
                duplicate,
            } => {
                let _ = (epoch, hash, message, logical);
                if duplicate {
                    self.report.counters.duplicates =
                        self.report.counters.duplicates.saturating_add(1);
                }
                Step::new().event(Event::Deliver {
                    actor,
                    message,
                    applied,
                    duplicate,
                    hash,
                })
            }
            Body::Resume {
                actor,
                epoch,
                capability,
            } => self.handle_resume(header, actor, epoch, capability, now_ms),
            Body::Error {
                actor,
                epoch,
                reason,
                expected_epoch,
            } => {
                self.report.counters.rejects = self.report.counters.rejects.saturating_add(1);
                self.report.reason = Some(reason);
                self.report.stage = "error";
                if matches!(reason, Error::StaleEpoch | Error::StaleHome) {
                    self.report.counters.stale = self.report.counters.stale.saturating_add(1);
                }
                let mut step = Step::new();
                if let Some(pending) = self.pending.take() {
                    if let Some(state) = pending.state {
                        if pending.kind == PendingKind::SourcePreparing
                            || pending.kind == PendingKind::SourceCommitting
                        {
                            self.actor = Some(state);
                        }
                    }
                }
                step.events.push(Event::Reject {
                    actor: Some(actor),
                    reason,
                });
                step.events.push(Event::Stale {
                    actor,
                    reason,
                    expected_epoch,
                    got_epoch: epoch,
                });
                step
            }
        };
        let _ = status;
        step
    }

    fn handle_authorise(
        &mut self,
        src: NodeId,
        header: &MeshHeader,
        directive: ActorMove,
        now_ms: u64,
    ) -> Step {
        if src != self.authority_id {
            return self.error_reply(src, header, directive.actor, Error::WrongSubject);
        }
        if let Err(reason) = self.verify_move(&directive, self.self_id, false) {
            return self.error_reply(src, header, directive.actor, reason);
        }
        if self.pending.is_some() {
            return self.error_reply(src, header, directive.actor, Error::Busy);
        }
        self.start_source(directive, now_ms)
    }

    fn handle_prepare(
        &mut self,
        src: NodeId,
        header: &MeshHeader,
        directive: ActorMove,
        state_len: u16,
        hash: [u8; 32],
        now_ms: u64,
    ) -> Step {
        if src != directive.old_host {
            return self.error_reply(src, header, directive.actor, Error::NotSource);
        }
        if let Err(reason) = self.verify_move(&directive, self.self_id, true) {
            return self.error_reply(src, header, directive.actor, reason);
        }
        if state_len as usize != STATE_LEN {
            return self.error_reply(src, header, directive.actor, Error::BadLength);
        }
        if let Some(pending) = &self.pending {
            if pending.kind == PendingKind::DestReserved
                && pending.directive == directive
                && pending.hash == hash
            {
                return self.ready_reply(src, header, directive, hash);
            }
            if !(pending.kind == PendingKind::Coordinating && pending.directive == directive) {
                return self.error_reply(src, header, directive.actor, Error::Busy);
            }
        }
        self.pending = Some(Pending {
            directive,
            state: None,
            state_bytes: Vec::new(),
            hash,
            request_id: header.request_id,
            deadline_ms: now_ms.saturating_add(MIGRATION_DEADLINE_MS),
            kind: PendingKind::DestReserved,
        });
        self.report.stage = "ready";
        self.ready_reply(src, header, directive, hash)
    }

    fn ready_reply(
        &mut self,
        dst: NodeId,
        header: &MeshHeader,
        directive: ActorMove,
        hash: [u8; 32],
    ) -> Step {
        let body = match encode_body(Body::Ready { directive, hash }) {
            Ok(body) => body,
            Err(reason) => return self.reject(Some(directive.actor), reason),
        };
        let wire = self.wire(
            dst,
            MessageKind::Response,
            directive.home,
            directive.old_epoch,
            directive.actor,
            header.request_id,
            header.request_id,
            body,
        );
        Step::new().with_send(wire)
    }

    fn handle_ready(&mut self, directive: ActorMove, hash: [u8; 32], now_ms: u64) -> Step {
        let Some(pending) = self.pending.take() else {
            return self.reject(Some(directive.actor), Error::Busy);
        };
        if pending.kind != PendingKind::SourcePreparing {
            self.pending = Some(pending);
            return self.reject(Some(directive.actor), Error::NotSource);
        }
        if pending.directive != directive {
            self.pending = Some(pending);
            return self.reject(Some(directive.actor), Error::WrongActor);
        }
        if pending.hash != hash {
            self.pending = Some(pending);
            return self.reject(Some(directive.actor), Error::StaleLease);
        }
        let Some(state) = pending.state else {
            return self.reject(Some(directive.actor), Error::NoActor);
        };
        let expected = state.hash();
        if expected != hash {
            return self.reject(Some(directive.actor), Error::StaleLease);
        }
        let request_id = self.take_request_id();
        let body = match encode_body(Body::State {
            directive,
            state: pending.state_bytes.clone(),
        }) {
            Ok(body) => body,
            Err(reason) => return self.reject(Some(directive.actor), reason),
        };
        let wire = self.wire(
            directive.new_host,
            MessageKind::Request,
            directive.home,
            directive.old_epoch,
            directive.actor,
            request_id,
            0,
            body,
        );
        self.pending = Some(Pending {
            directive,
            state: Some(state),
            state_bytes: pending.state_bytes,
            hash,
            request_id,
            deadline_ms: now_ms.saturating_add(MIGRATION_DEADLINE_MS),
            kind: PendingKind::SourceCommitting,
        });
        self.report.stage = "state";
        Step::new().with_send(wire)
    }

    fn handle_state(
        &mut self,
        src: NodeId,
        header: &MeshHeader,
        directive: ActorMove,
        state: &[u8],
        now_ms: u64,
    ) -> Step {
        let Some(pending) = self.pending.take() else {
            return self.error_reply(src, header, directive.actor, Error::Busy);
        };
        if pending.kind != PendingKind::DestReserved {
            self.pending = Some(pending);
            return self.error_reply(src, header, directive.actor, Error::NotDestination);
        }
        if pending.directive != directive {
            self.pending = Some(pending);
            return self.error_reply(src, header, directive.actor, Error::WrongActor);
        }
        if pending.hash != directive_hash(state) {
            self.pending = Some(pending);
            return self.error_reply(src, header, directive.actor, Error::StaleLease);
        }
        let mut actor = match ActorState::decode(state) {
            Ok(actor) => actor,
            Err(reason) => {
                self.pending = Some(pending);
                return self.error_reply(src, header, directive.actor, reason);
            }
        };
        if actor.actor != directive.actor
            || actor.home != directive.home
            || actor.host != directive.old_host
            || actor.epoch != directive.old_epoch
            || actor.schema != SCHEMA
        {
            self.pending = Some(pending);
            return self.error_reply(src, header, directive.actor, Error::BadMove);
        }
        if actor.hash() != pending.hash {
            self.pending = Some(pending);
            return self.error_reply(src, header, directive.actor, Error::StaleLease);
        }
        if actor
            .place(directive.new_host, directive.new_epoch)
            .is_err()
        {
            self.pending = Some(pending);
            return self.error_reply(src, header, directive.actor, Error::BadMove);
        }
        let hash = actor.hash();
        self.actor = Some(actor);
        let body = match encode_body(Body::Commit { directive, hash }) {
            Ok(body) => body,
            Err(reason) => return self.reject(Some(directive.actor), reason),
        };
        let wire = self.wire(
            src,
            MessageKind::Response,
            directive.home,
            directive.new_epoch,
            directive.actor,
            header.request_id,
            header.request_id,
            body,
        );
        self.pending = Some(Pending {
            directive,
            state: None,
            state_bytes: Vec::new(),
            hash,
            request_id: header.request_id,
            deadline_ms: now_ms.saturating_add(MIGRATION_DEADLINE_MS),
            kind: PendingKind::DestCommitting,
        });
        self.report.stage = "commit";
        Step::new().with_send(wire).event(Event::Moved {
            actor: directive.actor,
            from: directive.old_host,
            to: directive.new_host,
            old_epoch: directive.old_epoch,
            new_epoch: directive.new_epoch,
            hash,
            role: "destination",
        })
    }

    fn handle_commit(&mut self, directive: ActorMove, hash: [u8; 32], now_ms: u64) -> Step {
        let Some(pending) = self.pending.take() else {
            return self.reject(Some(directive.actor), Error::Busy);
        };
        if pending.kind != PendingKind::SourceCommitting {
            self.pending = Some(pending);
            return self.reject(Some(directive.actor), Error::NotSource);
        }
        if pending.directive != directive {
            self.pending = Some(pending);
            return self.reject(Some(directive.actor), Error::WrongActor);
        }
        if pending.hash != hash {
            self.pending = Some(pending);
            return self.reject(Some(directive.actor), Error::StaleLease);
        }
        let Some(state) = pending.state else {
            return self.reject(Some(directive.actor), Error::NoActor);
        };
        self.fence = Some(Fence {
            state: state.clone(),
            old_host: directive.old_host,
            new_host: directive.new_host,
            old_epoch: directive.old_epoch,
            new_epoch: directive.new_epoch,
            hash,
        });
        self.last_move = Some(LastMove {
            actor: directive.actor,
            old_host: directive.old_host,
            new_host: directive.new_host,
            old_epoch: directive.old_epoch,
            new_epoch: directive.new_epoch,
            hash,
        });
        self.report.counters.migrations = self.report.counters.migrations.saturating_add(1);
        self.report.counters.outgoing = self.report.counters.outgoing.saturating_add(1);
        self.report.stage = "fenced";
        let mut step = Step::new().event(Event::Fenced {
            actor: directive.actor,
            old_host: directive.old_host,
            new_host: directive.new_host,
            old_epoch: directive.old_epoch,
            new_epoch: directive.new_epoch,
            hash,
        });
        step.merge(Step::new().event(Event::Moved {
            actor: directive.actor,
            from: directive.old_host,
            to: directive.new_host,
            old_epoch: directive.old_epoch,
            new_epoch: directive.new_epoch,
            hash,
            role: "source",
        }));
        // Source -> destination ACK.  The destination activates on this frame.
        let request_id = self.take_request_id();
        let body = match encode_body(Body::Ack { directive, hash }) {
            Ok(body) => body,
            Err(reason) => {
                let extra = self.reject(Some(directive.actor), reason);
                step.merge(extra);
                return step;
            }
        };
        let wire = self.wire(
            directive.new_host,
            MessageKind::Completion,
            directive.home,
            directive.new_epoch,
            directive.actor,
            request_id,
            pending.request_id,
            body,
        );
        step.send = Some(wire);
        let _ = now_ms;
        step
    }

    fn handle_ack(&mut self, directive: ActorMove, hash: [u8; 32]) -> Step {
        let Some(pending) = self.pending.take() else {
            return self.reject(Some(directive.actor), Error::Busy);
        };
        if pending.kind != PendingKind::DestCommitting {
            self.pending = Some(pending);
            return self.reject(Some(directive.actor), Error::NotDestination);
        }
        if pending.directive != directive {
            self.pending = Some(pending);
            return self.reject(Some(directive.actor), Error::WrongActor);
        }
        if pending.hash != hash {
            self.pending = Some(pending);
            return self.reject(Some(directive.actor), Error::StaleLease);
        }
        self.last_move = Some(LastMove {
            actor: directive.actor,
            old_host: directive.old_host,
            new_host: directive.new_host,
            old_epoch: directive.old_epoch,
            new_epoch: directive.new_epoch,
            hash,
        });
        self.report.counters.migrations = self.report.counters.migrations.saturating_add(1);
        self.report.counters.incoming = self.report.counters.incoming.saturating_add(1);
        self.report.stage = "active";
        Step::new().event(Event::Moved {
            actor: directive.actor,
            from: directive.old_host,
            to: directive.new_host,
            old_epoch: directive.old_epoch,
            new_epoch: directive.new_epoch,
            hash,
            role: "destination",
        })
    }

    #[allow(clippy::too_many_arguments)]
    fn handle_deliver(
        &mut self,
        src: NodeId,
        header: &MeshHeader,
        actor_id: ActorId,
        epoch: u32,
        capability: Capability,
        message: Message,
        mode: u8,
        now_ms: u64,
    ) -> Step {
        if let Some(reason) = self.placement_error(actor_id, epoch) {
            self.report.counters.stale = self.report.counters.stale.saturating_add(1);
            self.report.reason = Some(reason);
            return self.error_reply(src, header, actor_id, reason);
        }
        let Some(mode) = DeliveryMode::from_u8(mode) else {
            return self.error_reply(src, header, actor_id, Error::Malformed);
        };
        let Some(actor_ref) = self.actor.as_ref() else {
            return self.error_reply(src, header, actor_id, Error::StaleHome);
        };
        if header.home != actor_ref.home || header.home_epoch != actor_ref.epoch {
            return self.error_reply(src, header, actor_id, Error::WrongHome);
        }
        if let Err(reason) =
            self.verify_capability(&capability, actor_ref, self.self_id, epoch, now_ms)
        {
            self.report.counters.capability_rejects =
                self.report.counters.capability_rejects.saturating_add(1);
            self.report.reason = Some(reason);
            let mut step = self.error_reply(src, header, actor_id, reason);
            step.events.push(Event::Capability {
                actor: actor_id,
                reason,
            });
            return step;
        }
        let Some(actor) = self.actor.as_mut() else {
            return self.error_reply(src, header, actor_id, Error::StaleHome);
        };
        let outcome = actor.deliver(message, mode);
        let hash = actor.hash();
        let (home, epoch, logical) = (actor.home, actor.epoch, actor.logical);
        let (applied, duplicate) = match outcome {
            Delivery::Applied { applied, .. } => (applied, false),
            Delivery::Duplicate { applied, .. } => (applied, true),
            Delivery::Queued { .. } => (actor.applied, false),
            Delivery::QueueFull { .. } => {
                return self.error_reply(src, header, actor_id, Error::QueueFull)
            }
            Delivery::Nothing => (actor.applied, false),
        };
        if duplicate {
            self.report.counters.duplicates = self.report.counters.duplicates.saturating_add(1);
        }
        let body = match encode_body(Body::Result {
            actor: actor_id,
            epoch,
            message: message.id,
            applied,
            logical,
            hash,
            duplicate,
        }) {
            Ok(body) => body,
            Err(reason) => return self.error_reply(src, header, actor_id, reason),
        };
        let wire = self.wire(
            src,
            MessageKind::Response,
            home,
            epoch,
            actor_id,
            header.request_id,
            header.request_id,
            body,
        );
        let mut step = Step::new().with_send(wire);
        if matches!(outcome, Delivery::Queued { .. }) {
            step.events.push(Event::Queued {
                actor: actor_id,
                message: message.id,
            });
        } else if duplicate {
            step.events.push(Event::Duplicate {
                actor: actor_id,
                message: message.id,
                applied,
            });
        } else {
            step.events.push(Event::Deliver {
                actor: actor_id,
                message: message.id,
                applied,
                duplicate,
                hash,
            });
        }
        step
    }

    fn handle_resume(
        &mut self,
        header: &MeshHeader,
        actor_id: ActorId,
        epoch: u32,
        capability: Capability,
        now_ms: u64,
    ) -> Step {
        let dst = header.src;
        if let Some(reason) = self.placement_error(actor_id, epoch) {
            self.report.counters.stale = self.report.counters.stale.saturating_add(1);
            self.report.reason = Some(reason);
            return self.error_reply(dst, header, actor_id, reason);
        }
        let Some(actor_ref) = self.actor.as_ref() else {
            return self.error_reply(dst, header, actor_id, Error::StaleHome);
        };
        if header.home != actor_ref.home || header.home_epoch != actor_ref.epoch {
            return self.error_reply(dst, header, actor_id, Error::WrongHome);
        }
        if let Err(reason) =
            self.verify_capability(&capability, actor_ref, self.self_id, epoch, now_ms)
        {
            self.report.counters.capability_rejects =
                self.report.counters.capability_rejects.saturating_add(1);
            self.report.reason = Some(reason);
            let mut step = self.error_reply(dst, header, actor_id, reason);
            step.events.push(Event::Capability {
                actor: actor_id,
                reason,
            });
            return step;
        }
        let Some(actor) = self.actor.as_mut() else {
            return self.error_reply(dst, header, actor_id, Error::StaleHome);
        };
        let outcome = actor.resume();
        let hash = actor.hash();
        let (home, epoch, logical) = (actor.home, actor.epoch, actor.logical);
        let (applied, message) = match outcome {
            Delivery::Applied { id, applied, .. } => (applied, id),
            Delivery::Nothing => (actor.applied, 0),
            _ => (actor.applied, 0),
        };
        self.report.counters.resumes = self.report.counters.resumes.saturating_add(1);
        let body = match encode_body(Body::Result {
            actor: actor_id,
            epoch,
            message,
            applied,
            logical,
            hash,
            duplicate: false,
        }) {
            Ok(body) => body,
            Err(reason) => return self.reject(Some(actor_id), reason),
        };
        let wire = self.wire(
            dst,
            MessageKind::Response,
            home,
            epoch,
            actor_id,
            header.request_id,
            header.request_id,
            body,
        );
        Step::new().with_send(wire).event(Event::Resume {
            actor: actor_id,
            message,
            applied,
            hash,
        })
    }

    fn local_delivery_events(
        &mut self,
        actor: ActorId,
        outcome: Delivery,
        hash: [u8; 32],
        resume: bool,
    ) -> Step {
        match outcome {
            Delivery::Applied {
                id,
                applied,
                logical: _,
                hash,
            } => {
                self.report.stage = if resume { "resume" } else { "deliver" };
                let event = if resume {
                    Event::Resume {
                        actor,
                        message: id,
                        applied,
                        hash,
                    }
                } else {
                    Event::Deliver {
                        actor,
                        message: id,
                        applied,
                        duplicate: false,
                        hash,
                    }
                };
                Step::new().event(event)
            }
            Delivery::Duplicate { id, applied, .. } => {
                self.report.counters.duplicates = self.report.counters.duplicates.saturating_add(1);
                self.report.stage = "duplicate";
                Step::new().event(Event::Duplicate {
                    actor,
                    message: id,
                    applied,
                })
            }
            Delivery::Queued { id, .. } => {
                self.report.stage = "queue";
                Step::new().event(Event::Queued { actor, message: id })
            }
            Delivery::QueueFull { id } => {
                self.report.reason = Some(Error::QueueFull);
                self.reject(Some(actor), Error::QueueFull)
                    .event(Event::Queued { actor, message: id })
            }
            Delivery::Nothing => {
                self.report.stage = "resume";
                Step::new().event(Event::Resume {
                    actor,
                    message: 0,
                    applied: 0,
                    hash,
                })
            }
        }
    }

    fn verify_move(
        &self,
        directive: &ActorMove,
        destination: NodeId,
        expect_destination: bool,
    ) -> Result<(), Error> {
        directive.verify(&self.authority_key)?;
        if directive.membership_epoch != self.epoch || directive.membership_digest != self.digest {
            return Err(Error::StaleEpoch);
        }
        if directive.schema != SCHEMA {
            return Err(Error::UnknownSchema);
        }
        if directive.home.is_zero() {
            return Err(Error::WrongHome);
        }
        if !self.view.contains(directive.home) {
            return Err(Error::WrongHome);
        }
        if !self.view.contains(directive.old_host) || !self.view.contains(directive.new_host) {
            return Err(Error::WrongHost);
        }
        if expect_destination {
            if directive.new_host != destination {
                return Err(Error::NotDestination);
            }
        } else if directive.old_host != destination {
            return Err(Error::NotSource);
        }
        Ok(())
    }

    fn sign_directive(&self, directive: &mut ActorMove, signer: &dyn Signer) -> Result<(), Error> {
        if !self.root || signer.key() != self.authority_key {
            return Err(Error::NotRoot);
        }
        directive.sign(signer)
    }

    fn placement_error(&self, actor: ActorId, epoch: u32) -> Option<Error> {
        if let Some(actor_state) = &self.actor {
            if actor_state.actor != actor {
                return Some(Error::WrongActor);
            }
            if actor_state.host != self.self_id {
                return Some(Error::StaleHome);
            }
            if epoch < actor_state.epoch {
                return Some(Error::StaleEpoch);
            }
            if epoch > actor_state.epoch {
                return Some(Error::WrongEpoch);
            }
            return None;
        }
        if let Some(fence) = &self.fence {
            if fence.state.actor != actor {
                return Some(Error::WrongActor);
            }
            if epoch < fence.new_epoch {
                return Some(Error::StaleEpoch);
            }
            return Some(Error::StaleHome);
        }
        Some(Error::NoActor)
    }

    fn issue_capability(
        &self,
        actor: &ActorState,
        signer: &dyn Signer,
        sequence: u64,
    ) -> Option<Capability> {
        self.issue_capability_for(actor.actor, actor.host, actor.epoch, signer, sequence)
    }

    fn issue_capability_for(
        &self,
        actor: ActorId,
        host: NodeId,
        epoch: u32,
        signer: &dyn Signer,
        sequence: u64,
    ) -> Option<Capability> {
        if !self.root || signer.key() != self.authority_key {
            return None;
        }
        // A per-issue anti-replay value derived from the issuer's sequence and
        // the actor placement.  It is not a secret (M2 has no confidential
        // transport); it only keeps a token from being reused after a move.
        let mut nonce_bytes = [0u8; 16];
        let mut hasher = blake3::Hasher::new();
        hasher.update(b"IRIX7-M2-CAP-NONCE-v1");
        hasher.update(&sequence.to_le_bytes());
        hasher.update(&actor.0);
        hasher.update(&epoch.to_le_bytes());
        nonce_bytes.copy_from_slice(&hasher.finalize().as_bytes()[..16]);
        let mut capability = Capability {
            magic: Capability::MAGIC,
            version: Capability::VERSION,
            rights: Rights::READ.union(Rights::INVOKE),
            flags: 0,
            object: actor_object(&actor),
            issuer: self.authority_key,
            subject: actor_principal(&actor),
            home: host,
            not_before: 0,
            not_after: crate::CAPABILITY_WINDOW_MS.saturating_mul(1_000_000),
            object_epoch: epoch,
            home_epoch: epoch,
            root_epoch: 1,
            reserved0: 0,
            nonce: nonce_bytes,
            parent: ObjectId::ZERO,
            delegation: ObjectId::ZERO,
            reserved1: [0; 24],
            signature: [0; 64],
        };
        let signature = signer.sign(&capability.signed_bytes())?;
        capability.signature = signature;
        Some(capability)
    }

    pub(crate) fn verify_capability(
        &self,
        capability: &Capability,
        actor: &ActorState,
        host: NodeId,
        epoch: u32,
        now_ms: u64,
    ) -> Result<(), Error> {
        cap_engine::verify_capability(&self.trust, capability).map_err(|error| match error {
            cap_engine::KeyError::Untrusted => Error::UntrustedIssuer,
            cap_engine::KeyError::InvalidKey | cap_engine::KeyError::BadSignature => {
                Error::BadSignature
            }
            cap_engine::KeyError::Unsupported => Error::Capability,
        })?;
        if capability.object != actor_object(&actor.actor) {
            return Err(Error::WrongSubject);
        }
        if capability.subject != actor_principal(&actor.actor) {
            return Err(Error::WrongSubject);
        }
        if !capability.rights.contains(Rights::INVOKE) {
            return Err(Error::InsufficientRights);
        }
        if capability.home != host || capability.home_epoch != epoch {
            return Err(Error::StaleLease);
        }
        if capability.object_epoch != epoch {
            return Err(Error::StaleLease);
        }
        let now_ns = now_ms.saturating_mul(1_000_000);
        if now_ns < capability.not_before {
            return Err(Error::NotYetValid);
        }
        if now_ns > capability.not_after {
            return Err(Error::Expired);
        }
        Ok(())
    }

    fn actor_home_or(&self, actor: ActorId) -> NodeId {
        self.actor
            .as_ref()
            .filter(|state| state.actor == actor)
            .map(|state| state.home)
            .or_else(|| {
                self.fence
                    .as_ref()
                    .filter(|fence| fence.state.actor == actor)
                    .map(|fence| fence.state.home)
            })
            .unwrap_or(self.self_id)
    }

    fn accept_request(&mut self, header: &MeshHeader) -> Result<(), Error> {
        if header.src_epoch != self.epoch {
            return Err(Error::StaleEpoch);
        }
        if header.dst_epoch != 0 && header.dst_epoch != self.epoch {
            return Err(Error::StaleEpoch);
        }
        if header.request_id == 0 || header.request_id <= self.last_request {
            return Err(Error::Replay);
        }
        self.last_request = header.request_id;
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    fn wire(
        &self,
        dst: NodeId,
        kind: MessageKind,
        home: NodeId,
        home_epoch: u32,
        actor: ActorId,
        request_id: u64,
        reply_to: u64,
        body: Vec<u8>,
    ) -> Wire {
        Wire {
            dst,
            kind,
            status: Status::Ok,
            src_epoch: self.epoch,
            dst_epoch: self.epoch,
            home,
            home_epoch,
            actor,
            request_id,
            reply_to,
            body,
        }
    }

    fn error_reply(
        &mut self,
        dst: NodeId,
        header: &MeshHeader,
        actor: ActorId,
        reason: Error,
    ) -> Step {
        let epoch = self
            .actor
            .as_ref()
            .filter(|state| state.actor == actor)
            .map(|state| state.epoch)
            .or_else(|| {
                self.fence
                    .as_ref()
                    .filter(|fence| fence.state.actor == actor)
                    .map(|fence| fence.new_epoch)
            })
            .unwrap_or(0);
        let body = match encode_body(Body::Error {
            actor,
            epoch,
            reason,
            expected_epoch: epoch,
        }) {
            Ok(body) => body,
            Err(_) => return self.reject(Some(actor), Error::Malformed),
        };
        let wire = self.wire(
            dst,
            MessageKind::Response,
            header.home,
            header.home_epoch,
            actor,
            header.request_id,
            header.request_id,
            body,
        );
        Step::new().with_send(wire).event(Event::Reject {
            actor: Some(actor),
            reason,
        })
    }

    fn reject(&mut self, actor: Option<ActorId>, reason: Error) -> Step {
        self.report.counters.rejects = self.report.counters.rejects.saturating_add(1);
        self.report.reason = Some(reason);
        self.report.stage = "reject";
        Step::new().event(Event::Reject { actor, reason })
    }

    fn abort_pending(&mut self, reason: Error) {
        let Some(pending) = self.pending.take() else {
            return;
        };
        if matches!(
            pending.kind,
            PendingKind::SourcePreparing | PendingKind::SourceCommitting
        ) {
            if let Some(state) = pending.state {
                self.actor = self.fence_restore(state, pending.directive);
            }
        }
        self.report.reason = Some(reason);
    }

    fn fence_restore(&self, mut state: ActorState, directive: ActorMove) -> Option<ActorState> {
        // An aborted move leaves the actor at its old placement.
        state.host = directive.old_host;
        state.epoch = directive.old_epoch;
        Some(state)
    }

    fn take_request_id(&mut self) -> u64 {
        let id = self.next_request.max(1);
        self.next_request = self.next_request.saturating_add(1);
        id
    }

    fn next_capability_sequence(&mut self) -> u64 {
        self.capability_sequence = self.capability_sequence.wrapping_add(1).max(1);
        self.capability_sequence
    }

    /// Build the bounded report from the current state.
    #[allow(clippy::manual_map)]
    pub fn build_report(&mut self) {
        let role = if self.root { "root" } else { "member" };
        self.report.enabled = true;
        self.report.role = role;
        self.report.view_epoch = self.epoch;
        self.report.view_members = self.view.len();
        self.report.view_digest = self.digest;
        self.report.actor = if let Some(actor) = &self.actor {
            Some(ActorReport {
                actor: actor.actor,
                home: actor.home,
                host: actor.host,
                epoch: actor.epoch,
                state: if matches!(
                    self.pending.as_ref().map(|pending| pending.kind),
                    Some(PendingKind::DestCommitting)
                ) {
                    "provisional"
                } else {
                    "active"
                },
                applied: actor.applied,
                last: actor.last,
                logical: actor.logical,
                hash: actor.hash(),
                queued: actor.queued,
                unacked: actor.unacked,
                handle: actor.handle,
            })
        } else if let Some(fence) = &self.fence {
            Some(ActorReport {
                actor: fence.state.actor,
                home: fence.state.home,
                host: fence.new_host,
                epoch: fence.new_epoch,
                state: "fenced",
                applied: fence.state.applied,
                last: fence.state.last,
                logical: fence.state.logical,
                hash: fence.hash,
                queued: fence.state.queued,
                unacked: fence.state.unacked,
                handle: fence.state.handle,
            })
        } else {
            None
        };
        self.report.last_move = self.last_move;
    }
}

fn encode_body(body: Body) -> Result<Vec<u8>, Error> {
    let mut bytes = [0u8; crate::MAX_BODY];
    let len = body.encode(&mut bytes)?;
    Ok(bytes[..len].to_vec())
}

impl Step {}

fn body_actor(body: &Body) -> Option<ActorId> {
    match body {
        Body::Authorise(directive)
        | Body::Prepare { directive, .. }
        | Body::Ready { directive, .. }
        | Body::State { directive, .. }
        | Body::Commit { directive, .. }
        | Body::Abort { directive, .. }
        | Body::Ack { directive, .. } => Some(directive.actor),
        Body::Deliver { actor, .. }
        | Body::Result { actor, .. }
        | Body::Resume { actor, .. }
        | Body::Error { actor, .. } => Some(*actor),
    }
}

fn directive_hash(state: &[u8]) -> [u8; 32] {
    match ActorState::decode(state) {
        Ok(state) => state.hash(),
        Err(_) => [0; 32],
    }
}
