// SPDX-License-Identifier: GPL-3.0-only
//! M2 remote capability routing, route-cache leases and the one-`MOVED` retry.
//!
//! The route is the existing `mesh-core` rendezvous home over the
//! authority-confirmed membership snapshot.  There is no second directory:
//! a sender computes (or recalls from its bounded route cache) the home for
//! an object, carries a `cap-engine`-signed `Capability` token to that home,
//! and the home verifies the token and its own lease before serving.
//!
//! ```text
//! sender A                         old home B                    new home A
//!   Request/Invoke  ------------->
//!                    <-------------  Response/Invoke MOVED
//!   route cache := new home
//!   Request/Invoke (retry once) ------------------------------->
//!                                     (old home is DRAINING)     result
//! ```
//!
//! ## Fail-closed rules
//!
//! * A node serves an object only when it holds a current `HomeLease`.  A
//!   lease names the home assignment epoch and the object generation.
//! * A request whose epochs are older than the lease is rejected with
//!   `stale-epoch`; a request whose epochs are newer than the lease is
//!   rejected with `unknown-epoch`.  Only an *equal* old epoch on a
//!   `DRAINING` lease produces `MOVED`.
//! * A node that is not the current home and holds no draining lease rejects
//!   with `not-home`; it never answers with a computed redirect.
//! * A `MOVED` answer must come from the cached old home, must move exactly
//!   one home epoch, must name a confirmed member, and must stay within the
//!   lease and grace bounds.  The sender retries once.  A second `MOVED`
//!   fails closed with `repeated-moved`; there is no unbounded chase.
//! * The capability must be issued by the configured authority, name the
//!   sending principal as its subject, carry `READ`, be inside its validity
//!   window and match the object generation and the home-lease fencing.  A
//!   token issued at the old home is accepted at the new home only for the
//!   documented one-epoch move; any other home/epoch pair is
//!   `stale-capability`.
//! * Bodies are bounded to [`wire::MAX_BODY`], decoded field by field, and
//!   must consume exactly their declared length.  Request ids must increase;
//!   a response must answer the outstanding request.  Every operation has a
//!   deadline and a link loss aborts the outstanding operation.
//!
//! ## The controlled move
//!
//! M2 has no leave/removal, so this slice does not manufacture a membership
//! epoch change.  The stale-home path is driven by a **root-signed home-lease
//! move** of one fixture object from its rendezvous home to the other
//! confirmed member.  It is a lab fixture: bounded, signed, time-limited, and
//! scoped to the confirmed membership epoch and digest.  It is not a
//! placement policy and not a persistent directory.  The old home enters
//! `DRAINING` for a bounded grace period and answers `MOVED`; the new home
//! holds the next `home_epoch` lease.

#![no_std]
#![forbid(unsafe_code)]
#![deny(missing_docs)]

extern crate alloc;

mod wire;

use alloc::boxed::Box;
use alloc::vec::Vec;
use core::fmt;

use cap_engine::{KeyError, SigningAuthority, TrustStore};
use mesh_core::{
    fixture_objects, home_for, MembershipView, ObjectRoute, RouteCache, HOME_FIXTURE_MAX,
};
use sys_primitives::{
    mesh_flags, Capability, FrameHeader, MeshHeader, MessageKind, NodeId, NodeKey, ObjectId,
    PrincipalId, Rights, Status, TraceContext,
};

pub use wire::{
    HomeMove, Invoke, InvokeResult, MoveAck, Moved, Reason, RouteBody, RouteErrorBody, WireError,
    HOME_MOVE_DOMAIN, KIND_ERROR, KIND_HOME_MOVE, KIND_INVOKE, KIND_MOVED, KIND_MOVE_ACK,
    KIND_RESULT, MAGIC, MAX_BODY, OP_COMMIT, OP_STAT, PREFIX, VERSION,
};

/// Route-cache entry lifetime.
pub const ROUTE_LEASE_MS: u64 = 60_000;
/// Active home-lease lifetime installed from the confirmed view.
///
/// This bounds read availability at a home that cannot renew.  It is long
/// because a read does not change the object.
pub const HOME_LEASE_MS: u64 = 120_000;
/// Lifetime of the authority's write lease on a member's home.
///
/// A mutable commit needs a *fresh* write lease, renewed by an accepted frame
/// from the authority (or, at the root, by the root itself).  The value is
/// deliberately shorter than the failure-detector window
/// ([`mesh_service::FAIL_AFTER_MS`] = 12 s) and longer than one heartbeat
/// (5 s): after the link is cut a member can accept a commit for at most 6 s,
/// while the root only re-homes the object after 12 s of silence.  The old
/// home therefore stops committing before the new home can exist.
pub const WRITE_LEASE_MS: u64 = 6_000;
/// Forwarding grace granted to a draining old home.
pub const GRACE_MS: u64 = 30_000;
/// Maximum lease a signed move may request.
pub const MAX_LEASE_MS: u32 = 120_000;
/// Maximum forwarding grace a signed move may request.
pub const MAX_GRACE_MS: u32 = 60_000;
/// Deadline for one invoke round trip.
pub const OPERATION_DEADLINE_MS: u64 = 10_000;
/// Deadline for one home-move acknowledgement.
pub const MOVE_DEADLINE_MS: u64 = 10_000;
/// Route-cache capacity: the bounded M2 fixture set.
pub const ROUTE_CACHE_CAPACITY: usize = HOME_FIXTURE_MAX as usize;
/// One initial attempt plus exactly one retry after `MOVED`.
pub const MAX_ATTEMPTS: u8 = 2;
/// The fixed M2 authority root epoch carried by a capability.
pub const ROOT_EPOCH: u32 = 1;
/// M2 capability validity window in milliseconds.
///
/// Both kernels use their own monotonic boot clock and there is no wall-clock
/// service in the module ABI.  The route lease, not this window, is the real
/// time bound on the data path; the window only rejects a capability that has
/// outlived a long boot.  A shared time service is a later slice.
pub const CAPABILITY_WINDOW_MS: u64 = 3_600_000;
/// Fixture object generation.
pub const OBJECT_EPOCH: u32 = 1;

/// Signing and public-key access for the authority root.
pub trait RouteSigner {
    /// The authority public key.
    fn key(&self) -> NodeKey;
    /// Sign a canonical message, or return `None` on a member.
    fn sign(&self, message: &[u8]) -> Option<[u8; 64]>;
}

impl RouteSigner for SigningAuthority {
    fn key(&self) -> NodeKey {
        self.public()
    }

    fn sign(&self, message: &[u8]) -> Option<[u8; 64]> {
        Some(SigningAuthority::sign(self, message))
    }
}

/// The M2 route principal for a node.  It is a deterministic subject key, not
/// a user identity and not a secret.
pub fn principal_for(node: &NodeId) -> PrincipalId {
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"IRIX7-M2-ROUTE-PRINCIPAL-v1");
    hasher.update(node.as_bytes());
    PrincipalId(*hasher.finalize().as_bytes())
}

/// The deterministic value a home returns for the M2 stat fixture.
pub fn route_value(object: &ObjectId, home_epoch: u32, object_epoch: u32) -> [u8; 32] {
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"IRIX7-M2-ROUTE-VALUE-v1");
    hasher.update(object.as_bytes());
    hasher.update(&home_epoch.to_le_bytes());
    hasher.update(&object_epoch.to_le_bytes());
    *hasher.finalize().as_bytes()
}

/// The deterministic value a home returns for the M2 mutable commit fixture.
///
/// The commit is a fixture effect, not stored state: the gate is authority and
/// lease fencing, not write semantics.  The value is domain-separated so a
/// commit result can never be mistaken for a read result.
pub fn commit_value(object: &ObjectId, home_epoch: u32, object_epoch: u32) -> [u8; 32] {
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"IRIX7-M2-ROUTE-COMMIT-v1");
    hasher.update(object.as_bytes());
    hasher.update(&home_epoch.to_le_bytes());
    hasher.update(&object_epoch.to_le_bytes());
    *hasher.finalize().as_bytes()
}

/// The value a home returns for one operation.
pub fn operation_value(
    operation: u16,
    object: &ObjectId,
    home_epoch: u32,
    object_epoch: u32,
) -> [u8; 32] {
    match operation {
        OP_COMMIT => commit_value(object, home_epoch, object_epoch),
        _ => route_value(object, home_epoch, object_epoch),
    }
}

/// A stable label for an operation code.
pub const fn operation_label(operation: u16) -> &'static str {
    match operation {
        OP_COMMIT => "commit",
        _ => "stat",
    }
}

/// One outbound mesh record, before the module wraps it in a `MeshEnvelope`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Wire {
    /// Final destination.
    pub dst: NodeId,
    /// Message kind.
    pub kind: MessageKind,
    /// Operation code.
    pub opcode: OpCode,
    /// Frame status.
    pub status: Status,
    /// `MeshHeader` flags.
    pub flags: u8,
    /// Source membership epoch.
    pub src_epoch: u32,
    /// Destination membership epoch.
    pub dst_epoch: u32,
    /// Home named by the route.
    pub home: NodeId,
    /// Home assignment epoch.
    pub home_epoch: u32,
    /// Object generation.
    pub object_epoch: u32,
    /// Object identity.
    pub object: ObjectId,
    /// Request id at the sender.
    pub request_id: u64,
    /// Request id this frame answers.
    pub reply_to: u64,
    /// Encoded route body.
    pub body: Vec<u8>,
    /// W3C trace context carried by the enclosing `MessageFrame`.
    ///
    /// The mesh header has no trace fields; the fabric frame does. A caller
    /// that wraps this record in a `MessageFrame` applies the context with
    /// [`Wire::apply_trace`]. Responses inherit the request trace id and use a
    /// fresh child span.
    pub trace: Option<TraceContext>,
}

/// Operation code re-exported so callers do not depend on `sys-primitives`.
pub use sys_primitives::OpCode;

impl Wire {
    /// Build the routing header for this record.
    pub fn header(&self, src: NodeId) -> MeshHeader {
        let mut header = MeshHeader::new(src, self.dst, self.home);
        header.flags = self.flags;
        if self.kind == MessageKind::Response {
            header.flags |= mesh_flags::REPLY;
        }
        header.src_epoch = self.src_epoch;
        header.dst_epoch = self.dst_epoch;
        header.home_epoch = self.home_epoch;
        header.object_epoch = self.object_epoch;
        header.request_id = self.request_id;
        header.reply_to = self.reply_to;
        header.ttl = MeshHeader::MAX_HOPS;
        header
    }

    /// Copy this record's trace context into a fabric frame header.
    ///
    /// A record without a trace leaves the frame header untouched, so the
    /// untraced M2 path keeps its exact previous bytes.
    pub fn apply_trace(&self, header: &mut FrameHeader) {
        let Some(trace) = self.trace else {
            return;
        };
        header.flags |= sys_primitives::flags::TRACE;
        header.trace_flags = trace.flags;
        header.trace_id = trace.trace_id;
        header.span_id = trace.span_id;
    }
}

/// Deterministic child span for a response to `request_id`.
///
/// A response keeps the request trace id but never reuses the request span
/// id. The value is derived rather than random so a host test and a serial
/// transcript can recompute it.
fn response_span(request_id: u64) -> [u8; 8] {
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"IRIX7-M2-TRACE-RESPONSE-v1");
    hasher.update(&request_id.to_le_bytes());
    let digest = hasher.finalize();
    let mut span = [0u8; 8];
    span.copy_from_slice(&digest.as_bytes()[..8]);
    span
}

/// One state change the module should log, or a test should inspect.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Event {
    /// The confirmed membership view was installed.
    View {
        /// Membership epoch.
        epoch: u32,
        /// Member count.
        members: usize,
        /// Membership digest.
        digest: [u8; 16],
    },
    /// A signed home move was applied.
    Move {
        /// Object identity.
        object: ObjectId,
        /// Old home.
        from: NodeId,
        /// New home.
        to: NodeId,
        /// Old home assignment epoch.
        old_home_epoch: u32,
        /// New home assignment epoch.
        new_home_epoch: u32,
        /// New lease in milliseconds.
        lease_ms: u32,
        /// Forwarding grace in milliseconds.
        grace_ms: u32,
        /// `old` or `new`.
        role: &'static str,
    },
    /// A `MOVED` redirect was produced or accepted.
    Moved {
        /// Object identity.
        object: ObjectId,
        /// Old home.
        from: NodeId,
        /// New home.
        to: NodeId,
        /// Old home assignment epoch.
        old_home_epoch: u32,
        /// New home assignment epoch.
        new_home_epoch: u32,
        /// Remaining lease in milliseconds.
        lease_ms: u32,
        /// Remaining grace in milliseconds.
        grace_ms: u32,
        /// `remote` for a wire answer, `local` for the local old home.
        source: &'static str,
    },
    /// A cached route or home lease named a home that left the confirmed
    /// view.  The stale entry is dropped and the object is re-routed locally;
    /// no message is sent to the removed node.
    Fence {
        /// Object identity.
        object: ObjectId,
        /// The stale home that is no longer a confirmed member.
        stale_home: NodeId,
        /// The stale home assignment epoch.
        stale_home_epoch: u32,
        /// The recomputed home.
        home: NodeId,
        /// The current membership epoch used to recompute it.
        home_epoch: u32,
        /// `left-view` for a route cache entry, `lease-left-view` for a lease.
        reason: &'static str,
    },
    /// The node's authoritative-view state changed.
    Authority {
        /// `current` or `lost`.
        state: &'static str,
        /// The confirmed epoch held while in this state.
        epoch: u32,
    },
    /// A stale wire frame was replayed through the receive path and refused.
    Replay {
        /// The refusal reason.
        reason: Reason,
        /// The source membership epoch named by the frame.
        src_epoch: u32,
        /// The local membership epoch.
        epoch: u32,
    },
    /// The sender retried exactly once after `MOVED`.
    Retry {
        /// Object identity.
        object: ObjectId,
        /// New home.
        home: NodeId,
        /// New home assignment epoch.
        home_epoch: u32,
        /// Attempt number (always 2 in this slice).
        attempt: u8,
    },
    /// The home served a fixture operation.
    Complete {
        /// Object identity.
        object: ObjectId,
        /// Home that served it.
        home: NodeId,
        /// Home assignment epoch.
        home_epoch: u32,
        /// Object generation.
        object_epoch: u32,
        /// `home` when served locally, `sender` when a result was verified.
        role: &'static str,
        /// The operation code.
        operation: u16,
        /// Deterministic value.
        value: [u8; 32],
    },
    /// The sender verified the final result.
    Result {
        /// Object identity.
        object: ObjectId,
        /// Home that served it.
        home: NodeId,
        /// Home assignment epoch.
        home_epoch: u32,
        /// The operation code.
        operation: u16,
        /// Deterministic value.
        value: [u8; 32],
        /// Attempts used.
        attempts: u8,
        /// Whether one `MOVED` was consumed.
        moved: bool,
    },
    /// A terminal refusal.
    Reject {
        /// Object identity, when known.
        object: Option<ObjectId>,
        /// Why.
        reason: Reason,
    },
    /// A pending operation or move exceeded its deadline.
    Timeout {
        /// Object identity, when known.
        object: Option<ObjectId>,
        /// `operation` or `move`.
        phase: &'static str,
    },
}

impl fmt::Display for Event {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::View {
                epoch,
                members,
                digest,
            } => write!(
                f,
                "mesh-route: view epoch={epoch} members={members} digest={}",
                Hex(&digest[..])
            ),
            Self::Move {
                object,
                from,
                to,
                old_home_epoch,
                new_home_epoch,
                lease_ms,
                grace_ms,
                role,
            } => write!(
                f,
                "mesh-route: move object={} from={} to={} old_home_epoch={} new_home_epoch={} lease_ms={} grace_ms={} role={role}",
                Hex(object.as_bytes()),
                Hex(from.as_bytes()),
                Hex(to.as_bytes()),
                old_home_epoch,
                new_home_epoch,
                lease_ms,
                grace_ms
            ),
            Self::Moved {
                object,
                from,
                to,
                old_home_epoch,
                new_home_epoch,
                lease_ms,
                grace_ms,
                source,
            } => write!(
                f,
                "mesh-route: moved object={} from={} to={} old_home_epoch={} new_home_epoch={} lease_ms={} grace_ms={} source={source}",
                Hex(object.as_bytes()),
                Hex(from.as_bytes()),
                Hex(to.as_bytes()),
                old_home_epoch,
                new_home_epoch,
                lease_ms,
                grace_ms
            ),
            Self::Fence {
                object,
                stale_home,
                stale_home_epoch,
                home,
                home_epoch,
                reason,
            } => write!(
                f,
                "mesh-route: fence object={} stale_home={} stale_home_epoch={} home={} home_epoch={} reason={reason}",
                Hex(object.as_bytes()),
                Hex(stale_home.as_bytes()),
                stale_home_epoch,
                Hex(home.as_bytes()),
                home_epoch
            ),
            Self::Authority { state, epoch } => {
                write!(f, "mesh-route: authority state={state} epoch={epoch}")
            }
            Self::Replay {
                reason,
                src_epoch,
                epoch,
            } => write!(
                f,
                "mesh-route: replay refused reason={reason} src_epoch={src_epoch} epoch={epoch}"
            ),
            Self::Retry {
                object,
                home,
                home_epoch,
                attempt,
            } => write!(
                f,
                "mesh-route: retry object={} home={} home_epoch={} attempt={attempt}",
                Hex(object.as_bytes()),
                Hex(home.as_bytes()),
                home_epoch
            ),
            Self::Complete {
                object,
                home,
                home_epoch,
                object_epoch,
                role,
                operation,
                value,
            } => write!(
                f,
                "mesh-route: complete object={} home={} home_epoch={} object_epoch={} op={} role={role} value={}",
                Hex(object.as_bytes()),
                Hex(home.as_bytes()),
                home_epoch,
                object_epoch,
                operation_label(*operation),
                Hex(&value[..])
            ),
            Self::Result {
                object,
                home,
                home_epoch,
                operation,
                value,
                attempts,
                moved,
            } => write!(
                f,
                "mesh-route: result object={} home={} home_epoch={} op={} value={} attempts={attempts} moved={}",
                Hex(object.as_bytes()),
                Hex(home.as_bytes()),
                home_epoch,
                operation_label(*operation),
                Hex(&value[..]),
                if *moved { 1 } else { 0 }
            ),
            Self::Reject { object, reason } => {
                write!(f, "mesh-route: reject reason={reason}")?;
                if let Some(object) = object {
                    write!(f, " object={}", Hex(object.as_bytes()))?;
                }
                Ok(())
            }
            Self::Timeout { object, phase } => {
                write!(f, "mesh-route: timeout phase={phase}")?;
                if let Some(object) = object {
                    write!(f, " object={}", Hex(object.as_bytes()))?;
                }
                Ok(())
            }
        }
    }
}

struct Hex<'a>(&'a [u8]);

impl fmt::Display for Hex<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for byte in self.0 {
            write!(f, "{byte:02X}")?;
        }
        Ok(())
    }
}

/// A home lease held by this node for one object.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HomeLease {
    /// Object identity.
    pub object: ObjectId,
    /// The home node named by this lease (always this node while it holds it).
    pub home: NodeId,
    /// Home assignment epoch.
    pub home_epoch: u32,
    /// Object generation.
    pub object_epoch: u32,
    /// Active lease expiry in milliseconds.
    pub lease_until_ms: u64,
    /// Draining grace expiry in milliseconds.
    pub grace_until_ms: u64,
    /// Whether the lease is active or draining.
    pub state: LeaseState,
    /// New home while draining.
    pub new_home: NodeId,
    /// New home assignment epoch while draining.
    pub new_home_epoch: u32,
    /// Old home for a lease installed by a move at the new home.
    pub previous_home: NodeId,
    /// Old home assignment epoch for a lease installed by a move.
    pub previous_home_epoch: u32,
}

/// Lease state.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LeaseState {
    /// The node is the current home.
    Active,
    /// The node was the home and is forwarding to the new home.
    Draining,
}

impl LeaseState {
    /// A stable label.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Draining => "draining",
        }
    }
}

impl HomeLease {
    /// Whether this lease can serve or redirect at `now_ms`.
    pub fn valid_at(&self, now_ms: u64) -> bool {
        match self.state {
            LeaseState::Active => now_ms <= self.lease_until_ms,
            LeaseState::Draining => now_ms <= self.grace_until_ms,
        }
    }

    /// Remaining milliseconds in the current lease or grace.
    pub fn remaining_ms(&self, now_ms: u64) -> u32 {
        let until = match self.state {
            LeaseState::Active => self.lease_until_ms,
            LeaseState::Draining => self.grace_until_ms,
        };
        until.saturating_sub(now_ms).min(u64::from(u32::MAX)) as u32
    }

    /// Remaining milliseconds of the new home's lease as known by the old
    /// home.  Only meaningful while draining.
    pub fn moved_lease_remaining_ms(&self, now_ms: u64) -> u32 {
        self.lease_until_ms
            .saturating_sub(now_ms)
            .min(u64::from(u32::MAX)) as u32
    }
}

/// One step of route policy: an optional frame to send and zero or more
/// events to log.
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

/// Maximum result rows kept for `GET /mesh/route`.
pub const MAX_REPORT_RESULTS: usize = 8;

/// One completed operation in the bounded route report.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ResultReport {
    /// Object identity.
    pub object: ObjectId,
    /// Home that served it.
    pub home: NodeId,
    /// Home assignment epoch.
    pub home_epoch: u32,
    /// Operation code (`OP_STAT` or `OP_COMMIT`).
    pub operation: u16,
    /// Deterministic value.
    pub value: [u8; 32],
    /// Attempts used.
    pub attempts: u8,
    /// Whether one `MOVED` was consumed.
    pub moved: bool,
}

/// One stale route or lease fenced by a membership change.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FenceReport {
    /// Object identity.
    pub object: ObjectId,
    /// The home that left the confirmed view.
    pub stale_home: NodeId,
    /// The stale home assignment epoch.
    pub stale_home_epoch: u32,
    /// The recomputed home.
    pub home: NodeId,
    /// The current membership epoch.
    pub home_epoch: u32,
    /// `left-view` or `lease-left-view`.
    pub reason: &'static str,
}

/// A bounded, in-memory evidence report for `GET /mesh/route`.
///
/// The module serves this over HTTP; it is not a directory and it is not
/// persisted.  The rig recomputes the values and epochs independently.
#[derive(Clone, Debug, Default)]
pub struct RouteReport {
    /// Confirmed view epoch.
    pub view_epoch: u32,
    /// Confirmed member count.
    pub view_members: usize,
    /// Confirmed membership digest.
    pub view_digest: [u8; 16],
    /// The object this node moved away from.
    pub move_old: Option<ObjectId>,
    /// The object this node accepted as the new home.
    pub move_new: Option<ObjectId>,
    /// `MOVED` answers sent for a remote old home.
    pub moved_remote: u32,
    /// `MOVED` answers produced by a local old home.
    pub moved_local: u32,
    /// Retries after `MOVED`.
    pub retries: u32,
    /// Home completions.
    pub completes: u32,
    /// Terminal refusals.
    pub rejects: u32,
    /// Last refusal reason.
    pub last_reject: Option<Reason>,
    /// Deadlines that expired.
    pub timeouts: u32,
    /// Stale routes or leases fenced by a membership change.
    pub fences: u32,
    /// The bounded fence rows.
    pub fence_rows: Vec<FenceReport>,
    /// Whether the node holds a current authoritative view.
    pub authority_current: bool,
    /// The epoch the authority line names.
    pub authority_epoch: u32,
    /// Whether a mutable commit is available right now.
    pub mutable: bool,
    /// Stale wire frames replayed through the receive path.
    pub replays: u32,
    /// The last replay refusal.
    pub last_replay: Option<Reason>,
    /// Commits served or completed locally.
    pub commits: u32,
    /// Completed operation rows.
    pub results: Vec<ResultReport>,
}

struct PendingOp {
    object: ObjectId,
    route: ObjectRoute,
    capability: Capability,
    operation: u16,
    request_id: u64,
    deadline_ms: u64,
    attempt: u8,
    moved: bool,
    trace: Option<TraceContext>,
}

struct PendingMove {
    directive: HomeMove,
    request_id: u64,
    deadline_ms: u64,
}

enum Pending {
    Op(Box<PendingOp>),
    Move(Box<PendingMove>),
}

impl Pending {
    fn request_id(&self) -> u64 {
        match self {
            Self::Op(op) => op.request_id,
            Self::Move(mv) => mv.request_id,
        }
    }

    fn deadline_ms(&self) -> u64 {
        match self {
            Self::Op(op) => op.deadline_ms,
            Self::Move(mv) => mv.deadline_ms,
        }
    }

    fn object(&self) -> ObjectId {
        match self {
            Self::Op(op) => op.object,
            Self::Move(mv) => mv.directive.object,
        }
    }

    fn phase(&self) -> &'static str {
        match self {
            Self::Op(_) => "operation",
            Self::Move(_) => "move",
        }
    }
}

/// The M2 route policy for one node.
pub struct RouteNode {
    self_id: NodeId,
    authority_id: NodeId,
    authority_key: NodeKey,
    trust: TrustStore,
    root: bool,
    fixture_count: u32,
    view: MembershipView,
    digest: [u8; 16],
    epoch: u32,
    peer: Option<NodeId>,
    homes: Vec<HomeLease>,
    cache: RouteCache,
    next_request: u64,
    last_request: u64,
    pending: Option<Pending>,
    snapshot_seen: Option<(u32, [u8; 16])>,
    tracked_remote: Option<ObjectId>,
    tracked_local: Option<ObjectId>,
    /// Expiry of the authority's write lease at this home, in milliseconds.
    write_until_ms: u64,
    /// Next time the root refreshes its read leases.  Refreshing every tick
    /// would recompute eight rendezvous scores continuously on the emulated
    /// kernel and starve the control plane.
    lease_renew_at_ms: u64,
    /// The last invoke this node put on the wire, kept for the failure fence
    /// evidence.  A delayed duplicate of it is replayed through the receive
    /// path after a view change; it must be refused by the epoch fence.
    stale: Option<(ObjectRoute, Wire)>,
    /// Whether the confirmed view is still current.  A member clears this
    /// when it loses the authority; only a fresh confirmation restores it.
    authority_current: bool,
    report: RouteReport,
}

impl RouteNode {
    /// Create a route policy for one node.
    ///
    /// Returns `None` for a zero identity, a zero authority or a zero key: a
    /// node without a confirmed authority must not claim a route.
    pub fn new(
        self_id: NodeId,
        authority_id: NodeId,
        authority_key: NodeKey,
        root: bool,
        fixture_count: u32,
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
            fixture_count: fixture_count.min(HOME_FIXTURE_MAX),
            view: MembershipView::new(0, Vec::new()).expect("empty view is bounded"),
            digest: [0; 16],
            epoch: 0,
            peer: None,
            homes: Vec::new(),
            cache: RouteCache::new(ROUTE_CACHE_CAPACITY),
            next_request: 1,
            last_request: 0,
            pending: None,
            snapshot_seen: None,
            tracked_remote: None,
            tracked_local: None,
            write_until_ms: 0,
            lease_renew_at_ms: 0,
            stale: None,
            authority_current: true,
            report: RouteReport::default(),
        })
    }

    /// This node's identity.
    pub const fn self_id(&self) -> NodeId {
        self.self_id
    }

    /// The configured authority root.
    pub const fn authority(&self) -> NodeId {
        self.authority_id
    }

    /// Whether this node is the authority root.
    pub const fn is_root(&self) -> bool {
        self.root
    }

    /// The configured peer, once the link is up.
    pub const fn peer(&self) -> Option<NodeId> {
        self.peer
    }

    /// The current confirmed view epoch.
    pub const fn epoch(&self) -> u32 {
        self.epoch
    }

    /// The current confirmed membership digest.
    pub const fn digest(&self) -> [u8; 16] {
        self.digest
    }

    /// The current confirmed membership view.
    pub const fn view(&self) -> &MembershipView {
        &self.view
    }

    /// The bounded route evidence report.
    pub fn report(&self) -> &RouteReport {
        &self.report
    }

    /// Whether the confirmed view is still current at this node.
    pub const fn authority_current(&self) -> bool {
        self.authority_current
    }

    /// Whether a mutable commit is available here at `now_ms`.
    ///
    /// This is the route half of the M2 availability rule: a current
    /// authoritative view *and* a fresh authority write lease.  The home lease
    /// is checked separately when the operation is served.  The write lease is
    /// what closes the partition handover window: it expires before the root's
    /// failure window can re-home the object.
    pub fn mutable_available_at(&self, now_ms: u64) -> bool {
        self.authority_current
            && self.epoch != 0
            && !self.view.is_empty()
            && (self.root || now_ms <= self.write_until_ms)
    }

    /// Remaining authority write lease in milliseconds.
    pub fn write_lease_remaining(&self, now_ms: u64) -> u32 {
        if self.root {
            return u32::MAX;
        }
        self.write_until_ms
            .saturating_sub(now_ms)
            .min(u64::from(u32::MAX)) as u32
    }

    /// Note one accepted frame from the authority, renewing the write lease.
    ///
    /// Any frame from the confirmed peer proves the authority is reachable, in
    /// the same way the membership heartbeat does.  A member that hears nothing
    /// stops accepting commits after [`WRITE_LEASE_MS`] even before the failure
    /// detector declares the authority lost.
    #[inline(never)]
    pub fn on_peer_frame(&mut self, now_ms: u64) {
        if self.root || !self.authority_current {
            // A late frame after authority was declared lost proves contact,
            // but it does not restore authority.  It must not keep a write
            // lease alive either.
            return;
        }
        self.write_until_ms = now_ms.saturating_add(WRITE_LEASE_MS);
        // Read leases are renewed by contact with the authority as well.  The
        // member still cannot create a lease or move a home; it can only keep
        // the leases the confirmed view gave it alive while that view is
        // current.  After the authority is lost no frame arrives, so the
        // leases expire and the objects become unavailable.
        let epoch = self.epoch;
        let until = now_ms.saturating_add(HOME_LEASE_MS);
        for lease in &mut self.homes {
            if lease.state == LeaseState::Active && lease.home_epoch <= epoch {
                lease.lease_until_ms = lease.lease_until_ms.max(until);
            }
        }
    }

    /// The node lost contact with the authority.
    ///
    /// The confirmed view and every unexpired lease are kept, so a read may
    /// still be served from a current lease, but no new home is created and a
    /// mutable commit fails closed.  Only [`RouteNode::on_snapshot`] restores
    /// the current state.
    #[inline(never)]
    pub fn on_authority_lost(&mut self) -> Option<Event> {
        self.write_until_ms = 0;
        if !self.authority_current {
            return None;
        }
        self.authority_current = false;
        Some(Event::Authority {
            state: "lost",
            epoch: self.epoch,
        })
    }

    /// A fresh confirmation restored an authoritative view.
    #[inline(never)]
    pub fn on_authority_restored(&mut self) -> Option<Event> {
        if self.authority_current {
            return None;
        }
        self.authority_current = true;
        Some(Event::Authority {
            state: "current",
            epoch: self.epoch,
        })
    }

    /// Fold one event into the bounded report.  Fast path; no formatting.
    #[inline(never)]
    pub fn note_event(&mut self, event: &Event) {
        match event {
            Event::View {
                epoch,
                members,
                digest,
            } => {
                self.report.view_epoch = *epoch;
                self.report.view_members = *members;
                self.report.view_digest = *digest;
                // A confirmed view is current by construction; only a
                // subsequent `Authority { state: "lost" }` clears it.
                self.report.authority_current = true;
                self.report.authority_epoch = *epoch;
                self.report.mutable = *epoch != 0;
            }
            Event::Move { object, role, .. } => {
                if *role == "old" {
                    self.report.move_old = Some(*object);
                } else {
                    self.report.move_new = Some(*object);
                }
            }
            Event::Moved { source, .. } => {
                if *source == "remote" {
                    self.report.moved_remote = self.report.moved_remote.saturating_add(1);
                } else {
                    self.report.moved_local = self.report.moved_local.saturating_add(1);
                }
            }
            Event::Retry { .. } => {
                self.report.retries = self.report.retries.saturating_add(1);
            }
            Event::Fence {
                object,
                stale_home,
                stale_home_epoch,
                home,
                home_epoch,
                reason,
            } => {
                self.report.fences = self.report.fences.saturating_add(1);
                if self.report.fence_rows.len() < MAX_REPORT_RESULTS {
                    self.report.fence_rows.push(FenceReport {
                        object: *object,
                        stale_home: *stale_home,
                        stale_home_epoch: *stale_home_epoch,
                        home: *home,
                        home_epoch: *home_epoch,
                        reason,
                    });
                }
            }
            Event::Authority { state, epoch } => {
                self.report.authority_current = *state == "current";
                self.report.authority_epoch = *epoch;
                self.report.mutable = *state == "current" && self.epoch != 0;
            }
            Event::Replay { reason, .. } => {
                self.report.replays = self.report.replays.saturating_add(1);
                self.report.last_replay = Some(*reason);
            }
            Event::Complete { role, .. } if *role == "home" => {
                self.report.completes = self.report.completes.saturating_add(1);
            }
            Event::Result {
                object,
                home,
                home_epoch,
                operation,
                value,
                attempts,
                moved,
            } => {
                if *operation == OP_COMMIT {
                    self.report.commits = self.report.commits.saturating_add(1);
                }
                if self.report.results.len() < MAX_REPORT_RESULTS {
                    self.report.results.push(ResultReport {
                        object: *object,
                        home: *home,
                        home_epoch: *home_epoch,
                        operation: *operation,
                        value: *value,
                        attempts: *attempts,
                        moved: *moved,
                    });
                }
            }
            Event::Reject { reason, .. } => {
                self.report.rejects = self.report.rejects.saturating_add(1);
                self.report.last_reject = Some(*reason);
            }
            Event::Timeout { .. } => {
                self.report.timeouts = self.report.timeouts.saturating_add(1);
            }
            _ => {}
        }
    }

    /// The current home lease for an object.
    pub fn home_lease(&self, object: &ObjectId) -> Option<&HomeLease> {
        self.homes.iter().find(|lease| lease.object == *object)
    }

    fn home_lease_mut(&mut self, object: &ObjectId) -> Option<&mut HomeLease> {
        self.homes.iter_mut().find(|lease| lease.object == *object)
    }

    fn put_home_lease(&mut self, lease: HomeLease) {
        if let Some(slot) = self
            .homes
            .iter_mut()
            .find(|existing| existing.object == lease.object)
        {
            *slot = lease;
        } else {
            debug_assert!(self.homes.len() < ROUTE_CACHE_CAPACITY);
            self.homes.push(lease);
        }
    }

    fn drop_home_lease(&mut self, object: &ObjectId) {
        if let Some(index) = self.homes.iter().position(|lease| lease.object == *object) {
            self.homes.remove(index);
        }
    }

    /// Every home lease, in object order.
    pub fn leases(&self) -> impl Iterator<Item = &HomeLease> {
        self.homes.iter()
    }

    /// The cached route for an object, if it is still within its lease.
    pub fn cached_route(&self, object: &ObjectId, now_ms: u64) -> Option<ObjectRoute> {
        self.cache
            .get(object)
            .copied()
            .filter(|route| route.lease_valid(now_ms.saturating_mul(1_000_000)))
    }

    /// Install a confirmed membership snapshot and fence stale routes.
    ///
    /// A lower epoch is ignored; the same epoch and digest is idempotent; a
    /// same-epoch different digest is caller error and is ignored.  A new
    /// epoch rebuilds the route cache and home leases from the snapshot and
    /// emits one [`Event::Fence`] per cached route or lease that named a home
    /// which is no longer a confirmed member.  The stale entry is dropped and
    /// the object is re-routed locally; nothing is sent to the removed node.
    ///
    /// A new confirmed view also supersedes a lab lease move: an object that
    /// is homed here under the new view is re-installed as an `Active` lease
    /// at the current epoch even if the old lease was `DRAINING`.
    #[inline(never)]
    pub fn on_snapshot(
        &mut self,
        epoch: u32,
        members: &[NodeId],
        digest: [u8; 16],
        now_ms: u64,
    ) -> Option<Step> {
        if epoch == 0 || members.is_empty() {
            return None;
        }
        let view = MembershipView::new(epoch, members.to_vec())?;
        if let Some((seen_epoch, seen_digest)) = self.snapshot_seen {
            if epoch < seen_epoch {
                return None;
            }
            if epoch == seen_epoch {
                // Never relabel a different member list as the same epoch; the
                // authority has already rejected equivocation.
                let _ = seen_digest;
                return None;
            }
        }
        self.snapshot_seen = Some((epoch, digest));
        self.view = view;
        self.digest = digest;
        self.epoch = epoch;
        if self.root {
            self.write_until_ms = now_ms.saturating_add(WRITE_LEASE_MS);
        } else {
            // The confirmation itself proves the authority is reachable.
            self.write_until_ms = now_ms.saturating_add(WRITE_LEASE_MS);
        }
        let mut step = self.sync_view(now_ms);
        step.events.insert(
            0,
            Event::View {
                epoch,
                members: self.view.len(),
                digest,
            },
        );
        if !self.authority_current {
            self.authority_current = true;
            step.events.insert(
                1,
                Event::Authority {
                    state: "current",
                    epoch,
                },
            );
        }
        Some(step)
    }

    /// Set or replace the configured peer.
    ///
    /// A different peer on an established link is a protocol failure; the
    /// caller tears the link down.  A reconnect re-arms nothing.
    #[inline(never)]
    pub fn set_peer(&mut self, peer: NodeId, _now_ms: u64) -> Option<Event> {
        if peer.is_zero() || peer == self.self_id {
            return Some(Event::Reject {
                object: None,
                reason: Reason::PeerMismatch,
            });
        }
        if let Some(current) = self.peer {
            if current != peer {
                self.abort_pending(Reason::PeerMismatch);
                return Some(Event::Reject {
                    object: None,
                    reason: Reason::PeerMismatch,
                });
            }
            return None;
        }
        self.peer = Some(peer);
        self.last_request = 0;
        None
    }

    /// Handle a link teardown.  The confirmed view is kept; in-flight work
    /// fails closed and is not silently retried.
    #[inline(never)]
    pub fn on_link_down(&mut self) -> Option<Event> {
        self.peer = None;
        self.last_request = 0;
        let object = self.abort_pending(Reason::LinkDown);
        object.map(|object| Event::Reject {
            object: Some(object),
            reason: Reason::LinkDown,
        })
    }

    /// Begin one operation against the current route for `object`.
    ///
    /// This is the entry point used by the rig and by host tests.  It refuses
    /// a second outstanding operation.
    #[inline(never)]
    pub fn begin_operation(
        &mut self,
        object: ObjectId,
        now_ms: u64,
        signer: &dyn RouteSigner,
    ) -> Step {
        self.begin_operation_with(object, OP_STAT, now_ms, signer)
    }

    /// Begin one read (`OP_STAT`) or mutable (`OP_COMMIT`) operation.
    ///
    /// A commit is the M2 mutable path.  It needs a current authoritative view
    /// at this node and a current home lease at the destination, so the side of
    /// a partition without the authority fails closed instead of creating a
    /// second home.
    #[inline(never)]
    pub fn begin_operation_with(
        &mut self,
        object: ObjectId,
        operation: u16,
        now_ms: u64,
        signer: &dyn RouteSigner,
    ) -> Step {
        self.begin_operation_with_trace(object, operation, None, now_ms, signer)
    }

    /// Begin one operation carrying a W3C trace context.
    ///
    /// The context is placed on the outbound `MessageFrame` and inherited by
    /// the home's response. A `MOVED` retry keeps the same trace id.
    #[inline(never)]
    pub fn begin_operation_traced(
        &mut self,
        object: ObjectId,
        operation: u16,
        trace: TraceContext,
        now_ms: u64,
        signer: &dyn RouteSigner,
    ) -> Step {
        self.begin_operation_with_trace(object, operation, Some(trace), now_ms, signer)
    }

    #[inline(never)]
    fn begin_operation_with_trace(
        &mut self,
        object: ObjectId,
        operation: u16,
        trace: Option<TraceContext>,
        now_ms: u64,
        signer: &dyn RouteSigner,
    ) -> Step {
        if self.pending.is_some() {
            return reject(Some(object), Reason::Busy);
        }
        if operation != OP_STAT && operation != OP_COMMIT {
            return reject(Some(object), Reason::WrongOperation);
        }
        if self.epoch == 0 || self.view.is_empty() {
            return reject(Some(object), Reason::NoPeer);
        }
        if operation == OP_COMMIT && !self.mutable_available_at(now_ms) {
            return reject(Some(object), Reason::NoAuthority);
        }
        if !self.is_fixture_object(&object) {
            return reject(Some(object), Reason::Fixture);
        }
        let Some(route) = self.route_for(&object, now_ms) else {
            return reject(Some(object), Reason::Fixture);
        };
        self.start_attempt(object, route, operation, 1, false, trace, now_ms, signer)
    }

    /// The first fixture object whose confirmed rendezvous home is `home`.
    pub fn first_home(&self, home: NodeId) -> Option<ObjectId> {
        fixture_objects(self.fixture_count)
            .into_iter()
            .find(|object| home_for(object, &self.view) == Some(home))
    }

    /// Run one rig step.  This is the lab control surface behind
    /// `GET /mesh/route?step=N`; the rig chooses the order and the policy
    /// chooses the object from the confirmed view.
    ///
    /// ```text
    /// 1  remote stat on the first peer-homed object
    /// 2  move that object from the peer to this node
    /// 3  repeat the stat, consuming the wire MOVED
    /// 4  move the first this-node-homed object to the peer
    /// 5  repeat the stat, consuming the local MOVED and retrying remotely
    /// ```
    pub fn command(&mut self, step: u8, now_ms: u64, signer: &dyn RouteSigner) -> Step {
        if self.pending.is_some() {
            return reject(None, Reason::Busy);
        }
        match step {
            1..=5 => {
                let Some(peer) = self.peer else {
                    return reject(None, Reason::NoPeer);
                };
                match step {
                    1 => {
                        let Some(object) = self.first_home(peer) else {
                            return reject(None, Reason::Fixture);
                        };
                        if self.tracked_local.is_none() {
                            self.tracked_local = self.first_home(self.self_id);
                        }
                        self.tracked_remote = Some(object);
                        self.begin_operation(object, now_ms, signer)
                    }
                    2 => {
                        let Some(object) = self.tracked_remote else {
                            return reject(None, Reason::Fixture);
                        };
                        self.start_move(object, peer, self.self_id, self.epoch, now_ms, signer)
                    }
                    3 => {
                        let Some(object) = self.tracked_remote else {
                            return reject(None, Reason::Fixture);
                        };
                        self.begin_operation(object, now_ms, signer)
                    }
                    4 => {
                        let Some(object) = self.tracked_local else {
                            return reject(None, Reason::Fixture);
                        };
                        self.start_move(object, self.self_id, peer, self.epoch, now_ms, signer)
                    }
                    _ => {
                        let Some(object) = self.tracked_local else {
                            return reject(None, Reason::Fixture);
                        };
                        self.begin_operation(object, now_ms, signer)
                    }
                }
            }
            6 => self.command_fence(now_ms, signer),
            7 => self.command_replay(now_ms),
            8 => {
                let Some(object) = self.first_home(self.self_id) else {
                    return reject(None, Reason::NotCurrent);
                };
                self.begin_operation_with(object, OP_COMMIT, now_ms, signer)
            }
            9 => {
                let Some(object) = self.first_home(self.self_id) else {
                    return reject(None, Reason::NotCurrent);
                };
                self.begin_operation(object, now_ms, signer)
            }
            10 => {
                // Commit an object homed on the peer.  Used after a rejoin to
                // prove the recovered member serves a mutable operation from
                // the authority without a quorum.
                let Some(peer) = self.peer else {
                    return reject(None, Reason::NoPeer);
                };
                let Some(object) = self.first_home(peer) else {
                    return reject(None, Reason::NotCurrent);
                };
                self.begin_operation_with(object, OP_COMMIT, now_ms, signer)
            }
            _ => reject(None, Reason::Malformed),
        }
    }

    /// Step 6: fence a stale route after a membership change.
    ///
    /// If this node still remembers an invoke it sent to a home that has left
    /// the confirmed view, the cached entry is dropped and the object is
    /// re-routed from the new view.  The operation completes at the recomputed
    /// home without contacting the removed node and without waiting for any
    /// quorum.  A node with no stale entry (the isolated member) instead probes
    /// an object homed on the other side; that must fail with `no-peer`, never
    /// by inventing a local home.
    #[inline(never)]
    fn command_fence(&mut self, now_ms: u64, signer: &dyn RouteSigner) -> Step {
        if let Some((route, _)) = self.stale {
            if !self.view.contains(route.home) {
                self.cache.invalidate(&route.object);
                return self.begin_operation(route.object, now_ms, signer);
            }
        }
        let Some(object) = fixture_objects(self.fixture_count)
            .into_iter()
            .find(|object| home_for(object, &self.view) != Some(self.self_id))
        else {
            return reject(None, Reason::NotCurrent);
        };
        self.begin_operation(object, now_ms, signer)
    }

    /// Step 7: replay the last stale wire frame through the receive path.
    ///
    /// A delayed duplicate can arrive after a membership change.  It is fed
    /// through the same header, epoch and lease checks as a live frame.  The
    /// peer check is deliberately bypassed because the sender may already have
    /// been removed; nothing else is relaxed.
    #[inline(never)]
    fn command_replay(&mut self, now_ms: u64) -> Step {
        let Some((_, wire)) = self.stale.clone() else {
            return reject(None, Reason::NotCurrent);
        };
        let src = wire.dst;
        let header = wire.header(self.self_id);
        if let Err(reason) = self.accept_request_header(&header) {
            return Step::new()
                .event(Event::Replay {
                    reason,
                    src_epoch: header.src_epoch,
                    epoch: self.epoch,
                })
                .event(Event::Reject {
                    object: Some(wire.object),
                    reason,
                });
        }
        let Ok(invoke) = Invoke::decode(&wire.body) else {
            return reject(Some(wire.object), Reason::Malformed);
        };
        let mut step = Step::new().event(Event::Replay {
            reason: Reason::None,
            src_epoch: header.src_epoch,
            epoch: self.epoch,
        });
        let mut served = self.serve_invoke(src, &header, wire.trace, wire.object, invoke, now_ms);
        step.events.append(&mut served.events);
        if served.send.is_some() {
            step.send = served.send;
        }
        step
    }

    /// Drive deadlines and home-lease expiry.
    #[inline(never)]
    pub fn tick(&mut self, now_ms: u64, signer: &dyn RouteSigner) -> Step {
        let mut step = Step::new();
        if self.root && self.authority_current {
            self.write_until_ms = now_ms.saturating_add(WRITE_LEASE_MS);
            if now_ms >= self.lease_renew_at_ms {
                self.renew_root_leases(now_ms);
                self.lease_renew_at_ms = now_ms.saturating_add(HOME_LEASE_MS / 2);
            }
        }
        self.expire_homes(now_ms);
        if let Some(pending) = &self.pending {
            if now_ms >= pending.deadline_ms() {
                let object = pending.object();
                let phase = pending.phase();
                self.pending = None;
                step.events.push(Event::Timeout {
                    object: Some(object),
                    phase,
                });
                step.events.push(Event::Reject {
                    object: Some(object),
                    reason: Reason::Timeout,
                });
                return step;
            }
        }
        let _ = signer;
        step
    }

    /// Handle one decoded route record from the peer.
    ///
    /// The caller has already validated the `MeshEnvelope` header, CRC,
    /// `frame_len` and the M2-1 record bound, and has identified `src` as the
    /// configured peer.  `trace` is read from the enclosing `MessageFrame`
    /// header; a response must carry the same trace id when the request did.
    /// This function owns the route body bounds and the replay, epoch and
    /// authority checks.
    #[allow(clippy::too_many_arguments)]
    #[inline(never)]
    pub fn on_wire(
        &mut self,
        src: NodeId,
        header: &MeshHeader,
        trace: Option<TraceContext>,
        object: ObjectId,
        kind: MessageKind,
        opcode: OpCode,
        status: Status,
        body: &[u8],
        now_ms: u64,
        signer: &dyn RouteSigner,
    ) -> Step {
        if src.is_zero() || src == self.self_id {
            return reject(None, Reason::PeerMismatch);
        }
        if self.peer != Some(src) {
            return reject(None, Reason::PeerMismatch);
        }
        if !header.dst.is_zero() && header.dst != self.self_id {
            return reject(None, Reason::WrongHome);
        }
        if body.len() > MAX_BODY {
            self.abort_if_pending();
            return reject(None, Reason::Malformed);
        }
        match (kind, opcode) {
            (MessageKind::Request, OpCode::Invoke) => {
                if let Err(reason) = self.accept_request_header(header) {
                    return reject(Some(object), reason);
                }
                let invoke = match Invoke::decode(body) {
                    Ok(invoke) => invoke,
                    Err(_) => return reject(Some(object), Reason::Malformed),
                };
                self.last_request = header.request_id;
                self.serve_invoke(src, header, trace, object, invoke, now_ms)
            }
            (MessageKind::Response, OpCode::Invoke) => {
                if header.reply_to == 0 || header.reply_to != self.pending_request() {
                    return reject(Some(object), Reason::UnknownRequest);
                }
                self.handle_invoke_response(
                    src, header, trace, object, status, body, now_ms, signer,
                )
            }
            (MessageKind::Request, OpCode::RouteQuery) => {
                if let Err(reason) = self.accept_request_header(header) {
                    return reject(Some(object), reason);
                }
                let directive = match HomeMove::decode(body) {
                    Ok(directive) => directive,
                    Err(_) => return reject(Some(object), Reason::Malformed),
                };
                self.last_request = header.request_id;
                self.handle_move_request(src, header, trace, object, directive, now_ms)
            }
            (MessageKind::Response, OpCode::RouteQuery) => {
                if header.reply_to == 0 || header.reply_to != self.pending_request() {
                    return reject(Some(object), Reason::UnknownRequest);
                }
                self.handle_move_ack(header, object, body)
            }
            _ => reject(Some(object), Reason::Malformed),
        }
    }

    #[inline(never)]
    fn accept_request_header(&self, header: &MeshHeader) -> Result<(), Reason> {
        // Epoch fences are checked before the request id: a frame from a
        // superseded membership epoch is refused because it is stale, not
        // because it is a duplicate.  A duplicate from the *current* epoch is
        // still refused as a replay.
        if header.src_epoch != self.epoch {
            return Err(Reason::StaleEpoch);
        }
        if header.dst_epoch != 0 && header.dst_epoch != self.epoch {
            return Err(Reason::StaleEpoch);
        }
        if header.request_id == 0 || header.request_id <= self.last_request {
            return Err(Reason::Replay);
        }
        Ok(())
    }

    #[inline(never)]
    fn pending_request(&self) -> u64 {
        self.pending.as_ref().map(Pending::request_id).unwrap_or(0)
    }

    #[inline(never)]
    #[allow(clippy::too_many_arguments)]
    fn start_attempt(
        &mut self,
        object: ObjectId,
        route: ObjectRoute,
        operation: u16,
        attempt: u8,
        moved: bool,
        trace: Option<TraceContext>,
        now_ms: u64,
        signer: &dyn RouteSigner,
    ) -> Step {
        if attempt > MAX_ATTEMPTS {
            return reject(Some(object), Reason::RepeatedMoved);
        }
        if operation != OP_STAT && operation != OP_COMMIT {
            return reject(Some(object), Reason::WrongOperation);
        }
        if operation == OP_COMMIT && !self.mutable_available_at(now_ms) {
            return reject(Some(object), Reason::NoAuthority);
        }
        if !self.authority_current && route.home != self.self_id {
            // A member without a current authoritative view cannot reach a
            // remote home.  It cannot issue the capability the home would
            // check either, so fail closed before the capability path.
            return reject(Some(object), Reason::NoAuthority);
        }
        if !self.view.contains(route.home) {
            // A route to a node that is not in the confirmed view is fenced;
            // the cache entry is stale and the object is re-resolved locally.
            self.cache.invalidate(&object);
            return match self.route_for(&object, now_ms) {
                Some(fresh) => self.start_attempt(
                    object, fresh, operation, attempt, moved, trace, now_ms, signer,
                ),
                None => reject(Some(object), Reason::NotCurrent),
            };
        }
        if route.home == self.self_id {
            return self.local_serve(
                object,
                route.home_epoch,
                operation,
                attempt,
                moved,
                trace,
                now_ms,
                signer,
                None,
            );
        }
        let Some(peer) = self.peer else {
            return reject(Some(object), Reason::NoPeer);
        };
        if route.home != peer {
            // M2 has exactly two nodes.  A route to a third member would need
            // a hop the transport does not provide in this slice.
            return reject(Some(object), Reason::PeerMismatch);
        }
        match self.send_invoke(
            object, route, operation, attempt, moved, trace, now_ms, signer, None,
        ) {
            Ok(wire) => Step::new().with_send(wire),
            Err(reason) => reject(Some(object), reason),
        }
    }

    #[allow(clippy::too_many_arguments)]
    #[inline(never)]
    fn send_invoke(
        &mut self,
        object: ObjectId,
        route: ObjectRoute,
        operation: u16,
        attempt: u8,
        moved: bool,
        trace: Option<TraceContext>,
        now_ms: u64,
        signer: &dyn RouteSigner,
        capability: Option<Capability>,
    ) -> Result<Wire, Reason> {
        let request_id = self.take_request_id();
        let capability = match capability {
            Some(capability) => capability,
            None => self.issue_capability(object, &route, operation, request_id, signer)?,
        };
        let mut buffer = [0u8; Invoke::LEN];
        let len = Invoke {
            capability,
            operation,
        }
        .encode(&mut buffer)
        .map_err(|_| Reason::Malformed)?;
        let wire = Wire {
            dst: route.home,
            kind: MessageKind::Request,
            opcode: OpCode::Invoke,
            status: Status::Ok,
            flags: 0,
            src_epoch: self.epoch,
            dst_epoch: self.epoch,
            home: route.home,
            home_epoch: route.home_epoch,
            object_epoch: route.object_epoch,
            object,
            request_id,
            reply_to: 0,
            body: buffer[..len].to_vec(),
            trace,
        };
        // Keep the last frame this node put on the wire.  A delayed duplicate
        // of it is the stale-lease evidence after a membership change.  This
        // is a bounded, single-entry copy (one `Wire`, at most a 10 KiB body)
        // and it is not a directory.
        self.stale = Some((route, wire.clone()));
        self.pending = Some(Pending::Op(Box::new(PendingOp {
            object,
            route,
            capability,
            operation,
            request_id,
            deadline_ms: now_ms.saturating_add(OPERATION_DEADLINE_MS),
            attempt,
            moved,
            trace,
        })));
        Ok(wire)
    }

    #[inline(never)]
    #[allow(clippy::too_many_arguments)]
    fn local_serve(
        &mut self,
        object: ObjectId,
        home_epoch: u32,
        operation: u16,
        attempt: u8,
        moved: bool,
        trace: Option<TraceContext>,
        now_ms: u64,
        signer: &dyn RouteSigner,
        capability: Option<Capability>,
    ) -> Step {
        if operation == OP_COMMIT && !self.mutable_available_at(now_ms) {
            return reject(Some(object), Reason::NoAuthority);
        }
        let Some(lease) = self.home_lease(&object).copied() else {
            // The object is homed here but there is no current lease.  That is
            // `not-current`, not `not-home`: a redirect would be a fabricated
            // second home.
            let reason = if home_for(&object, &self.view) == Some(self.self_id) {
                Reason::NotCurrent
            } else {
                Reason::NotHome
            };
            return reject(Some(object), reason);
        };
        if !lease.valid_at(now_ms) {
            self.drop_home_lease(&object);
            let reason = if home_for(&object, &self.view) == Some(self.self_id) {
                Reason::NotCurrent
            } else {
                Reason::NotHome
            };
            return reject(Some(object), reason);
        }
        if home_epoch < lease.home_epoch {
            return reject(Some(object), Reason::StaleEpoch);
        }
        if home_epoch > lease.home_epoch {
            return reject(Some(object), Reason::UnknownEpoch);
        }
        let route = ObjectRoute {
            object,
            home: self.self_id,
            home_epoch,
            object_epoch: lease.object_epoch,
            lease_until: lease.lease_until_ms.saturating_mul(1_000_000),
        };
        let capability = match capability {
            Some(capability) => capability,
            None => match self.issue_capability(object, &route, operation, 0, signer) {
                Ok(capability) => capability,
                Err(reason) => return reject(Some(object), reason),
            },
        };
        if let Err(reason) =
            self.verify_capability(&capability, self.self_id, operation, &lease, now_ms)
        {
            return reject(Some(object), reason);
        }
        match lease.state {
            LeaseState::Active => {
                let value =
                    operation_value(operation, &object, lease.home_epoch, lease.object_epoch);
                Step::new()
                    .event(Event::Complete {
                        object,
                        home: self.self_id,
                        home_epoch: lease.home_epoch,
                        object_epoch: lease.object_epoch,
                        role: "home",
                        operation,
                        value,
                    })
                    .event(Event::Result {
                        object,
                        home: self.self_id,
                        home_epoch: lease.home_epoch,
                        operation,
                        value,
                        attempts: attempt,
                        moved,
                    })
            }
            LeaseState::Draining => {
                if attempt >= MAX_ATTEMPTS {
                    return reject(Some(object), Reason::RepeatedMoved);
                }
                let grace_ms = lease.remaining_ms(now_ms);
                let lease_ms = lease.moved_lease_remaining_ms(now_ms);
                if grace_ms == 0 || lease_ms == 0 {
                    self.drop_home_lease(&object);
                    return reject(Some(object), Reason::NotHome);
                }
                let next = ObjectRoute {
                    object,
                    home: lease.new_home,
                    home_epoch: lease.new_home_epoch,
                    object_epoch: lease.object_epoch,
                    lease_until: now_ms
                        .saturating_add(u64::from(lease_ms))
                        .saturating_mul(1_000_000),
                };
                self.cache.insert(next);
                let Some(peer) = self.peer else {
                    return reject(Some(object), Reason::NoPeer);
                };
                if lease.new_home != peer && lease.new_home != self.self_id {
                    return reject(Some(object), Reason::UnknownEpoch);
                }
                let retry = if lease.new_home == self.self_id {
                    self.local_serve(
                        object,
                        lease.new_home_epoch,
                        operation,
                        attempt + 1,
                        true,
                        trace,
                        now_ms,
                        signer,
                        Some(capability),
                    )
                } else {
                    match self.send_invoke(
                        object,
                        next,
                        operation,
                        attempt + 1,
                        true,
                        trace,
                        now_ms,
                        signer,
                        Some(capability),
                    ) {
                        Ok(wire) => Step::new().with_send(wire).event(Event::Retry {
                            object,
                            home: next.home,
                            home_epoch: next.home_epoch,
                            attempt: attempt + 1,
                        }),
                        Err(reason) => reject(Some(object), reason),
                    }
                };
                let mut step = Step::new().event(Event::Moved {
                    object,
                    from: self.self_id,
                    to: lease.new_home,
                    old_home_epoch: lease.home_epoch,
                    new_home_epoch: lease.new_home_epoch,
                    lease_ms,
                    grace_ms,
                    source: "local",
                });
                step.merge(retry);
                step
            }
        }
    }

    #[inline(never)]
    fn issue_capability(
        &self,
        object: ObjectId,
        route: &ObjectRoute,
        operation: u16,
        request_id: u64,
        signer: &dyn RouteSigner,
    ) -> Result<Capability, Reason> {
        if !self.root || signer.key() != self.authority_key {
            return Err(Reason::BadSignature);
        }
        let mut nonce = [0u8; 16];
        let bytes = request_id.to_le_bytes();
        for (index, slot) in nonce.iter_mut().enumerate() {
            *slot = bytes[index % 8];
        }
        let mut capability = Capability {
            magic: Capability::MAGIC,
            version: Capability::VERSION,
            rights: match operation {
                OP_COMMIT => Rights::READ.union(Rights::WRITE),
                _ => Rights::READ,
            },
            flags: 0,
            object,
            issuer: self.authority_key,
            subject: principal_for(&self.self_id),
            home: route.home,
            not_before: 0,
            not_after: CAPABILITY_WINDOW_MS.saturating_mul(1_000_000),
            object_epoch: route.object_epoch,
            home_epoch: route.home_epoch,
            root_epoch: ROOT_EPOCH,
            reserved0: 0,
            nonce,
            parent: ObjectId::ZERO,
            delegation: ObjectId::ZERO,
            reserved1: [0; 24],
            signature: [0; 64],
        };
        let signature = signer
            .sign(&capability.signed_bytes())
            .ok_or(Reason::BadSignature)?;
        capability.signature = signature;
        Ok(capability)
    }

    #[inline(never)]
    fn verify_capability(
        &self,
        capability: &Capability,
        src: NodeId,
        operation: u16,
        lease: &HomeLease,
        now_ms: u64,
    ) -> Result<(), Reason> {
        if capability.issuer != self.authority_key {
            return Err(Reason::UntrustedIssuer);
        }
        // Verify the signature before trusting any capability field.  The
        // semantic checks below are for diagnostics and lease fencing; none
        // of them may run on bytes the authority did not sign.
        cap_engine::verify_capability(&self.trust, capability).map_err(|error| match error {
            KeyError::Untrusted => Reason::UntrustedIssuer,
            KeyError::InvalidKey | KeyError::BadSignature => Reason::BadSignature,
            KeyError::Unsupported => Reason::BadSignature,
        })?;
        if capability.object != lease.object {
            return Err(Reason::WrongObject);
        }
        if capability.subject != principal_for(&src) {
            return Err(Reason::WrongSubject);
        }
        if capability.root_epoch != ROOT_EPOCH {
            return Err(Reason::StaleCapability);
        }
        let required = match operation {
            OP_COMMIT => Rights::READ.union(Rights::WRITE),
            _ => Rights::READ,
        };
        if !capability.rights.contains(required) {
            return Err(Reason::InsufficientRights);
        }
        let now_ns = now_ms.saturating_mul(1_000_000);
        if now_ns < capability.not_before {
            return Err(Reason::NotYetValid);
        }
        if now_ns > capability.not_after {
            return Err(Reason::Expired);
        }
        if capability.object_epoch != lease.object_epoch {
            return Err(Reason::StaleObjectEpoch);
        }
        let fenced = match lease.state {
            LeaseState::Draining => {
                capability.home == self.self_id && capability.home_epoch == lease.home_epoch
            }
            LeaseState::Active => {
                // A fresh operation at the current home is always valid.  A
                // token issued at the old home is accepted only for the one
                // epoch recorded by the move, which is the retry path.
                let current =
                    capability.home == self.self_id && capability.home_epoch == lease.home_epoch;
                let predecessor = !lease.previous_home.is_zero()
                    && capability.home == lease.previous_home
                    && capability.home_epoch == lease.previous_home_epoch
                    && lease.home_epoch == lease.previous_home_epoch.saturating_add(1);
                current || predecessor
            }
        };
        if !fenced {
            return Err(Reason::StaleCapability);
        }
        Ok(())
    }

    #[inline(never)]
    fn serve_invoke(
        &mut self,
        src: NodeId,
        header: &MeshHeader,
        trace: Option<TraceContext>,
        object: ObjectId,
        invoke: Invoke,
        now_ms: u64,
    ) -> Step {
        if invoke.operation != OP_STAT && invoke.operation != OP_COMMIT {
            return self.error_reply(src, object, header, trace, Reason::WrongOperation);
        }
        // A mutable commit needs a current authoritative view *here*.  A home
        // that cannot confirm the current view is unavailable, not a second
        // authority.
        if invoke.operation == OP_COMMIT && !self.mutable_available_at(now_ms) {
            return self.error_reply(src, object, header, trace, Reason::NoAuthority);
        }
        let capability = invoke.capability;
        if capability.object != object || capability.object_epoch != header.object_epoch {
            return self.error_reply(src, object, header, trace, Reason::WrongObject);
        }
        if header.home != self.self_id {
            return self.error_reply(src, object, header, trace, Reason::WrongHome);
        }
        let Some(lease) = self.home_lease(&object).copied() else {
            let reason = if home_for(&object, &self.view) == Some(self.self_id) {
                Reason::NotCurrent
            } else {
                Reason::NotHome
            };
            return self.error_reply(src, object, header, trace, reason);
        };
        if !lease.valid_at(now_ms) {
            self.drop_home_lease(&object);
            let reason = if home_for(&object, &self.view) == Some(self.self_id) {
                Reason::NotCurrent
            } else {
                Reason::NotHome
            };
            return self.error_reply(src, object, header, trace, reason);
        }
        if header.object_epoch < lease.object_epoch {
            return self.error_reply(src, object, header, trace, Reason::StaleObjectEpoch);
        }
        if header.object_epoch > lease.object_epoch {
            return self.error_reply(src, object, header, trace, Reason::UnknownEpoch);
        }
        if header.home_epoch < lease.home_epoch {
            return self.error_reply(src, object, header, trace, Reason::StaleEpoch);
        }
        if header.home_epoch > lease.home_epoch {
            return self.error_reply(src, object, header, trace, Reason::UnknownEpoch);
        }
        if let Err(reason) =
            self.verify_capability(&capability, src, invoke.operation, &lease, now_ms)
        {
            return self.error_reply(src, object, header, trace, reason);
        }
        match lease.state {
            LeaseState::Active => {
                let value = operation_value(
                    invoke.operation,
                    &object,
                    lease.home_epoch,
                    lease.object_epoch,
                );
                let result = InvokeResult {
                    object,
                    home: self.self_id,
                    home_epoch: lease.home_epoch,
                    object_epoch: lease.object_epoch,
                    lease_ms: lease.remaining_ms(now_ms),
                    value,
                };
                let mut body = [0u8; InvokeResult::LEN];
                let Ok(len) = result.encode(&mut body) else {
                    return self.error_reply(src, object, header, trace, Reason::Malformed);
                };
                let wire = self.reply_wire(
                    src,
                    object,
                    header,
                    trace,
                    MessageKind::Response,
                    OpCode::Invoke,
                    Status::Ok,
                    0,
                    lease.home_epoch,
                    lease.object_epoch,
                    body[..len].to_vec(),
                );
                Step::new().with_send(wire).event(Event::Complete {
                    object,
                    home: self.self_id,
                    home_epoch: lease.home_epoch,
                    object_epoch: lease.object_epoch,
                    role: "home",
                    operation: invoke.operation,
                    value,
                })
            }
            LeaseState::Draining => {
                let lease_ms = lease.moved_lease_remaining_ms(now_ms);
                let grace_ms = lease.remaining_ms(now_ms);
                if lease_ms == 0 || grace_ms == 0 {
                    self.drop_home_lease(&object);
                    return self.error_reply(src, object, header, trace, Reason::NotHome);
                }
                let moved = Moved {
                    object,
                    old_home: self.self_id,
                    new_home: lease.new_home,
                    old_home_epoch: lease.home_epoch,
                    new_home_epoch: lease.new_home_epoch,
                    object_epoch: lease.object_epoch,
                    lease_ms,
                    grace_ms,
                    reason: Reason::None,
                };
                let mut body = [0u8; Moved::LEN];
                let Ok(len) = moved.encode(&mut body) else {
                    return self.error_reply(src, object, header, trace, Reason::Malformed);
                };
                let wire = self.reply_wire(
                    src,
                    object,
                    header,
                    trace,
                    MessageKind::Response,
                    OpCode::Invoke,
                    Status::Stale,
                    mesh_flags::MOVED,
                    lease.home_epoch,
                    lease.object_epoch,
                    body[..len].to_vec(),
                );
                Step::new().with_send(wire).event(Event::Moved {
                    object,
                    from: self.self_id,
                    to: lease.new_home,
                    old_home_epoch: lease.home_epoch,
                    new_home_epoch: lease.new_home_epoch,
                    lease_ms,
                    grace_ms,
                    source: "remote",
                })
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    #[inline(never)]
    fn reply_wire(
        &self,
        dst: NodeId,
        object: ObjectId,
        request: &MeshHeader,
        trace: Option<TraceContext>,
        kind: MessageKind,
        opcode: OpCode,
        status: Status,
        flags: u8,
        home_epoch: u32,
        object_epoch: u32,
        body: Vec<u8>,
    ) -> Wire {
        Wire {
            dst,
            kind,
            opcode,
            status,
            flags,
            src_epoch: self.epoch,
            dst_epoch: self.epoch,
            home: self.self_id,
            home_epoch,
            object_epoch,
            object,
            request_id: request.request_id,
            reply_to: request.request_id,
            body,
            trace: trace.map(|context| context.child(response_span(request.request_id))),
        }
    }

    #[inline(never)]
    fn error_reply(
        &mut self,
        dst: NodeId,
        object: ObjectId,
        header: &MeshHeader,
        trace: Option<TraceContext>,
        reason: Reason,
    ) -> Step {
        let body = RouteErrorBody {
            object,
            reason,
            detail: 0,
        };
        let mut buffer = [0u8; RouteErrorBody::LEN];
        let Ok(len) = body.encode(&mut buffer) else {
            return reject(Some(object), Reason::Malformed);
        };
        let wire = self.reply_wire(
            dst,
            object,
            header,
            trace,
            MessageKind::Response,
            OpCode::Invoke,
            status_for(reason),
            0,
            header.home_epoch,
            header.object_epoch,
            buffer[..len].to_vec(),
        );
        Step::new().with_send(wire).event(Event::Reject {
            object: Some(object),
            reason,
        })
    }

    #[inline(never)]
    #[allow(clippy::too_many_arguments)]
    fn handle_invoke_response(
        &mut self,
        _src: NodeId,
        header: &MeshHeader,
        trace: Option<TraceContext>,
        object: ObjectId,
        status: Status,
        body: &[u8],
        now_ms: u64,
        signer: &dyn RouteSigner,
    ) -> Step {
        let Some(Pending::Op(pending)) = self.pending.take() else {
            return reject(Some(object), Reason::UnknownRequest);
        };
        if let Some(expected) = pending.trace {
            if trace.map(|context| context.trace_id) != Some(expected.trace_id) {
                return self.fail_op(pending.object, Reason::TraceMismatch);
            }
        }
        if header.flags & mesh_flags::MOVED != 0 || status == Status::Stale {
            let moved = match Moved::decode(body) {
                Ok(moved) => moved,
                Err(_) => return self.fail_op(pending.object, Reason::Malformed),
            };
            if moved.object != pending.object
                || moved.old_home != pending.route.home
                || moved.old_home_epoch != pending.route.home_epoch
                || moved.object_epoch != pending.route.object_epoch
                || moved.new_home == moved.old_home
                || !self.view.contains(moved.new_home)
                || moved.lease_ms == 0
                || moved.lease_ms > MAX_LEASE_MS
                || moved.grace_ms == 0
                || moved.grace_ms > MAX_GRACE_MS
            {
                return self.fail_op(pending.object, Reason::BadMove);
            }
            if pending.moved || pending.attempt >= MAX_ATTEMPTS {
                return self.fail_op(pending.object, Reason::RepeatedMoved);
            }
            if moved.reason != Reason::None {
                return self.fail_op(pending.object, Reason::BadMove);
            }
            let next = ObjectRoute {
                object: pending.object,
                home: moved.new_home,
                home_epoch: moved.new_home_epoch,
                object_epoch: moved.object_epoch,
                lease_until: now_ms
                    .saturating_add(u64::from(moved.lease_ms))
                    .saturating_mul(1_000_000),
            };
            self.cache.insert(next);
            // A remote `MOVED` names the next home.  In the two-node M2 rig
            // that is this sender; a third member would need the multi-hop
            // forward path that this slice does not implement.
            let retry = if moved.new_home == self.self_id {
                let mut step = Step::new().event(Event::Retry {
                    object: pending.object,
                    home: self.self_id,
                    home_epoch: next.home_epoch,
                    attempt: pending.attempt + 1,
                });
                let local = self.local_serve(
                    pending.object,
                    next.home_epoch,
                    pending.operation,
                    pending.attempt + 1,
                    true,
                    pending.trace,
                    now_ms,
                    signer,
                    Some(pending.capability),
                );
                step.merge(local);
                step
            } else {
                self.fail_op(pending.object, Reason::UnknownEpoch)
            };
            let mut step = Step::new().event(Event::Moved {
                object: pending.object,
                from: pending.route.home,
                to: moved.new_home,
                old_home_epoch: pending.route.home_epoch,
                new_home_epoch: moved.new_home_epoch,
                lease_ms: moved.lease_ms,
                grace_ms: moved.grace_ms,
                source: "remote",
            });
            step.merge(retry);
            step
        } else if status.is_ok() {
            let result = match InvokeResult::decode(body) {
                Ok(result) => result,
                Err(_) => return self.fail_op(pending.object, Reason::Malformed),
            };
            if result.object != pending.object
                || result.home != pending.route.home
                || result.home_epoch != pending.route.home_epoch
                || result.object_epoch != pending.route.object_epoch
                || result.value
                    != operation_value(
                        pending.operation,
                        &pending.object,
                        pending.route.home_epoch,
                        pending.route.object_epoch,
                    )
            {
                return self.fail_op(pending.object, Reason::BadMove);
            }
            Step::new()
                .event(Event::Complete {
                    object: result.object,
                    home: result.home,
                    home_epoch: result.home_epoch,
                    object_epoch: result.object_epoch,
                    role: "sender",
                    operation: pending.operation,
                    value: result.value,
                })
                .event(Event::Result {
                    object: result.object,
                    home: result.home,
                    home_epoch: result.home_epoch,
                    operation: pending.operation,
                    value: result.value,
                    attempts: pending.attempt,
                    moved: pending.moved,
                })
        } else {
            let error = match RouteErrorBody::decode(body) {
                Ok(error) => error,
                Err(_) => return self.fail_op(pending.object, Reason::Malformed),
            };
            if error.object != pending.object && !error.object.is_zero() {
                return self.fail_op(pending.object, Reason::BadMove);
            }
            self.fail_op(pending.object, error.reason)
        }
    }

    #[inline(never)]
    fn handle_move_request(
        &mut self,
        src: NodeId,
        header: &MeshHeader,
        trace: Option<TraceContext>,
        object: ObjectId,
        directive: HomeMove,
        now_ms: u64,
    ) -> Step {
        if src != self.authority_id {
            return self.move_ack(
                src,
                header,
                trace,
                directive,
                false,
                Reason::WrongSubject,
                now_ms,
            );
        }
        if object != directive.object
            || header.home != directive.old_home
            || header.home_epoch != directive.old_home_epoch
            || header.object_epoch != directive.object_epoch
            || header.src_epoch != self.epoch
        {
            return self.move_ack(
                src,
                header,
                trace,
                directive,
                false,
                Reason::BadMove,
                now_ms,
            );
        }
        match self.apply_move(&directive, now_ms) {
            Ok(event) => {
                let mut step =
                    self.move_ack(src, header, trace, directive, true, Reason::None, now_ms);
                step.events.insert(0, event);
                step
            }
            Err(reason) => self.move_ack(src, header, trace, directive, false, reason, now_ms),
        }
    }

    #[inline(never)]
    #[allow(clippy::too_many_arguments)]
    fn move_ack(
        &mut self,
        dst: NodeId,
        header: &MeshHeader,
        trace: Option<TraceContext>,
        directive: HomeMove,
        accepted: bool,
        reason: Reason,
        _now_ms: u64,
    ) -> Step {
        // The acknowledgement names the *new* home, not necessarily the node
        // sending it: the old home acknowledges on behalf of the new home.
        let ack = MoveAck {
            object: directive.object,
            home: directive.new_home,
            home_epoch: directive.new_home_epoch,
            accepted,
            reason,
        };
        let mut buffer = [0u8; MoveAck::LEN];
        let Ok(len) = ack.encode(&mut buffer) else {
            return reject(Some(directive.object), Reason::Malformed);
        };
        let wire = self.reply_wire(
            dst,
            directive.object,
            header,
            trace,
            MessageKind::Response,
            OpCode::RouteQuery,
            if accepted {
                Status::Ok
            } else {
                status_for(reason)
            },
            0,
            directive.old_home_epoch,
            directive.object_epoch,
            buffer[..len].to_vec(),
        );
        let mut step = Step::new().with_send(wire);
        if !accepted {
            step.events.push(Event::Reject {
                object: Some(directive.object),
                reason,
            });
        }
        step
    }

    #[inline(never)]
    fn handle_move_ack(&mut self, _header: &MeshHeader, object: ObjectId, body: &[u8]) -> Step {
        let Some(Pending::Move(pending)) = self.pending.take() else {
            return reject(Some(object), Reason::UnknownRequest);
        };
        let ack = match MoveAck::decode(body) {
            Ok(ack) => ack,
            Err(_) => return self.fail_move(&pending, Reason::Malformed),
        };
        if ack.object != pending.directive.object
            || ack.home != pending.directive.new_home
            || ack.home_epoch != pending.directive.new_home_epoch
        {
            return self.fail_move(&pending, Reason::BadMove);
        }
        if !ack.accepted {
            return Step::new().event(Event::Reject {
                object: Some(ack.object),
                reason: ack.reason,
            });
        }
        Step::new()
    }

    #[inline(never)]
    fn fail_move(&mut self, pending: &PendingMove, reason: Reason) -> Step {
        reject(Some(pending.directive.object), reason)
    }

    #[inline(never)]
    fn fail_op(&mut self, object: ObjectId, reason: Reason) -> Step {
        reject(Some(object), reason)
    }

    #[inline(never)]
    fn apply_move(&mut self, directive: &HomeMove, now_ms: u64) -> Result<Event, Reason> {
        if directive.membership_epoch != self.epoch || directive.membership_digest != self.digest {
            return Err(Reason::StaleEpoch);
        }
        if directive.lease_ms == 0
            || directive.lease_ms > MAX_LEASE_MS
            || directive.grace_ms == 0
            || directive.grace_ms > MAX_GRACE_MS
            || directive.grace_ms > directive.lease_ms
            || directive.object_epoch == 0
            || directive.new_home_epoch != directive.old_home_epoch.saturating_add(1)
        {
            return Err(Reason::BadMove);
        }
        // The authority signature is checked before any membership field is
        // used to decide whether the move is plausible.
        directive
            .verify(&self.authority_key)
            .map_err(|_| Reason::BadSignature)?;
        if !self.is_fixture_object(&directive.object) {
            return Err(Reason::Fixture);
        }
        let expected = home_for(&directive.object, &self.view).ok_or(Reason::Fixture)?;
        if expected != directive.old_home {
            return Err(Reason::BadMove);
        }
        if !self.view.contains(directive.new_home) {
            return Err(Reason::BadMove);
        }
        if self.self_id == directive.new_home {
            if let Some(existing) = self.home_lease(&directive.object) {
                if existing.state == LeaseState::Active
                    && existing.home_epoch == directive.new_home_epoch
                    && existing.previous_home == directive.old_home
                {
                    return Ok(Event::Move {
                        object: directive.object,
                        from: directive.old_home,
                        to: directive.new_home,
                        old_home_epoch: directive.old_home_epoch,
                        new_home_epoch: directive.new_home_epoch,
                        lease_ms: directive.lease_ms,
                        grace_ms: directive.grace_ms,
                        role: "new",
                    });
                }
                if existing.home_epoch > directive.new_home_epoch {
                    return Err(Reason::StaleEpoch);
                }
            }
            let lease = HomeLease {
                object: directive.object,
                home: self.self_id,
                home_epoch: directive.new_home_epoch,
                object_epoch: directive.object_epoch,
                lease_until_ms: now_ms.saturating_add(u64::from(directive.lease_ms)),
                grace_until_ms: 0,
                state: LeaseState::Active,
                new_home: NodeId::ZERO,
                new_home_epoch: 0,
                previous_home: directive.old_home,
                previous_home_epoch: directive.old_home_epoch,
            };
            self.put_home_lease(lease);
            // Deliberately do not refresh the route cache here.  A route
            // cache entry is a rendezvous-membership hint; it is corrected by
            // `MOVED` or by lease expiry, not by a lease move this node
            // happens to know about.  That is what makes the stale-home path
            // observable at the wire.
            Ok(Event::Move {
                object: directive.object,
                from: directive.old_home,
                to: directive.new_home,
                old_home_epoch: directive.old_home_epoch,
                new_home_epoch: directive.new_home_epoch,
                lease_ms: directive.lease_ms,
                grace_ms: directive.grace_ms,
                role: "new",
            })
        } else if self.self_id == directive.old_home {
            let Some(lease) = self.home_lease_mut(&directive.object) else {
                return Err(Reason::NotHome);
            };
            if lease.state != LeaseState::Active
                || lease.home_epoch != directive.old_home_epoch
                || lease.object_epoch != directive.object_epoch
            {
                return Err(Reason::BadMove);
            }
            lease.state = LeaseState::Draining;
            lease.new_home = directive.new_home;
            lease.new_home_epoch = directive.new_home_epoch;
            lease.grace_until_ms = now_ms.saturating_add(u64::from(directive.grace_ms));
            lease.lease_until_ms = now_ms.saturating_add(u64::from(directive.lease_ms));
            Ok(Event::Move {
                object: directive.object,
                from: directive.old_home,
                to: directive.new_home,
                old_home_epoch: directive.old_home_epoch,
                new_home_epoch: directive.new_home_epoch,
                lease_ms: directive.lease_ms,
                grace_ms: directive.grace_ms,
                role: "old",
            })
        } else {
            Err(Reason::WrongHome)
        }
    }

    #[inline(never)]
    fn route_for(&mut self, object: &ObjectId, now_ms: u64) -> Option<ObjectRoute> {
        let now_ns = now_ms.saturating_mul(1_000_000);
        if let Some(route) = self.cache.get(object).copied() {
            if self.view.contains(route.home) && route.lease_valid(now_ns) {
                return Some(route);
            }
            // Never use a cached route to a node that is no longer a
            // confirmed member.  The entry is fenced and re-resolved from the
            // current view below.
            self.cache.invalidate(object);
        }
        let home = home_for(object, &self.view)?;
        let route = ObjectRoute {
            object: *object,
            home,
            home_epoch: self.epoch,
            object_epoch: OBJECT_EPOCH,
            lease_until: now_ms
                .saturating_add(ROUTE_LEASE_MS)
                .saturating_mul(1_000_000),
        };
        self.cache.insert(route);
        Some(route)
    }

    #[inline(never)]
    fn sync_view(&mut self, now_ms: u64) -> Step {
        let mut step = Step::new();
        let objects = fixture_objects(self.fixture_count);
        // A confirmed membership change fences every cached route that named a
        // node which is no longer a member.  The route cache is then rebuilt
        // from the new view.  The fence is local: nothing is sent to the
        // removed node and no quorum is consulted.
        let cached: Vec<ObjectRoute> = self.cache.iter().copied().collect();
        for route in &cached {
            if self.view.contains(route.home) {
                continue;
            }
            if let Some(new_home) = home_for(&route.object, &self.view) {
                step.events.push(Event::Fence {
                    object: route.object,
                    stale_home: route.home,
                    stale_home_epoch: route.home_epoch,
                    home: new_home,
                    home_epoch: self.epoch,
                    reason: "left-view",
                });
            }
        }
        self.cache = RouteCache::new(ROUTE_CACHE_CAPACITY);
        for object in &objects {
            if let Some(home) = home_for(object, &self.view) {
                self.cache.insert(ObjectRoute {
                    object: *object,
                    home,
                    home_epoch: self.epoch,
                    object_epoch: OBJECT_EPOCH,
                    lease_until: now_ms
                        .saturating_add(ROUTE_LEASE_MS)
                        .saturating_mul(1_000_000),
                });
            }
        }
        // A lease held by a node that is no longer the home is fenced.  A
        // lease whose object is homed here again is re-installed below, which
        // is how a new confirmed view supersedes a lab lease move.
        let mut stale: Vec<(ObjectId, NodeId, u32, NodeId)> = Vec::new();
        for lease in &self.homes {
            let home = home_for(&lease.object, &self.view).unwrap_or(self.self_id);
            if home == self.self_id && lease.state == LeaseState::Active {
                continue;
            }
            stale.push((lease.object, lease.home, lease.home_epoch, home));
        }
        for (object, stale_home, stale_home_epoch, home) in stale {
            if stale_home != home {
                step.events.push(Event::Fence {
                    object,
                    stale_home,
                    stale_home_epoch,
                    home,
                    home_epoch: self.epoch,
                    reason: "lease-left-view",
                });
            }
            self.drop_home_lease(&object);
        }
        let epoch = self.epoch;
        self.lease_renew_at_ms = now_ms.saturating_add(HOME_LEASE_MS / 2);
        for object in &objects {
            if home_for(object, &self.view) != Some(self.self_id) {
                continue;
            }
            match self.home_lease_mut(object) {
                Some(lease) if lease.state == LeaseState::Active => {
                    if lease.home_epoch < epoch {
                        lease.home_epoch = epoch;
                        lease.object_epoch = OBJECT_EPOCH;
                        lease.lease_until_ms = now_ms.saturating_add(HOME_LEASE_MS);
                    }
                }
                Some(_) | None => {
                    self.put_home_lease(HomeLease {
                        object: *object,
                        home: self.self_id,
                        home_epoch: epoch,
                        object_epoch: OBJECT_EPOCH,
                        lease_until_ms: now_ms.saturating_add(HOME_LEASE_MS),
                        grace_until_ms: 0,
                        state: LeaseState::Active,
                        new_home: NodeId::ZERO,
                        new_home_epoch: 0,
                        previous_home: NodeId::ZERO,
                        previous_home_epoch: 0,
                    });
                }
            }
        }
        step
    }

    /// Renew the authority root's own home leases from its current view.
    ///
    /// Only the 1-of-1 authority may do this: its confirmed view *is* the
    /// current authority.  A member cannot renew a lease without a fresh
    /// confirmation, so on the side of a partition without the root its leases
    /// expire and the objects become unavailable.  This does not create a new
    /// home or advance an epoch.
    #[inline(never)]
    fn renew_root_leases(&mut self, now_ms: u64) {
        if !self.root || !self.authority_current || self.epoch == 0 {
            return;
        }
        let objects = fixture_objects(self.fixture_count);
        let epoch = self.epoch;
        for object in &objects {
            if home_for(object, &self.view) != Some(self.self_id) {
                continue;
            }
            match self.home_lease_mut(object) {
                Some(lease) if lease.state == LeaseState::Active => {
                    if lease.home_epoch <= epoch {
                        lease.object_epoch = OBJECT_EPOCH;
                        lease.lease_until_ms = now_ms.saturating_add(HOME_LEASE_MS);
                    }
                }
                Some(_) => {}
                None => {
                    self.put_home_lease(HomeLease {
                        object: *object,
                        home: self.self_id,
                        home_epoch: self.epoch,
                        object_epoch: OBJECT_EPOCH,
                        lease_until_ms: now_ms.saturating_add(HOME_LEASE_MS),
                        grace_until_ms: 0,
                        state: LeaseState::Active,
                        new_home: NodeId::ZERO,
                        new_home_epoch: 0,
                        previous_home: NodeId::ZERO,
                        previous_home_epoch: 0,
                    });
                }
            }
        }
    }

    #[inline(never)]
    fn expire_homes(&mut self, now_ms: u64) {
        self.homes.retain(|lease| lease.valid_at(now_ms));
    }

    #[inline(never)]
    fn is_fixture_object(&self, object: &ObjectId) -> bool {
        fixture_objects(self.fixture_count)
            .iter()
            .any(|entry| entry == object)
    }

    #[inline(never)]
    fn take_request_id(&mut self) -> u64 {
        let id = self.next_request.max(1);
        self.next_request = self.next_request.saturating_add(1);
        id
    }

    #[inline(never)]
    fn abort_pending(&mut self, _reason: Reason) -> Option<ObjectId> {
        self.pending.take().map(|pending| pending.object())
    }

    #[inline(never)]
    fn abort_if_pending(&mut self) {
        self.pending = None;
    }

    #[inline(never)]
    fn start_move(
        &mut self,
        object: ObjectId,
        old_home: NodeId,
        new_home: NodeId,
        old_home_epoch: u32,
        now_ms: u64,
        signer: &dyn RouteSigner,
    ) -> Step {
        if self.pending.is_some() {
            return reject(Some(object), Reason::Busy);
        }
        if !self.root || signer.key() != self.authority_key {
            return reject(Some(object), Reason::WrongSubject);
        }
        let Some(peer) = self.peer else {
            return reject(Some(object), Reason::NoPeer);
        };
        if new_home != self.self_id && new_home != peer {
            return reject(Some(object), Reason::PeerMismatch);
        }
        let mut directive = HomeMove {
            object,
            old_home,
            new_home,
            membership_epoch: self.epoch,
            old_home_epoch,
            new_home_epoch: old_home_epoch.saturating_add(1),
            object_epoch: OBJECT_EPOCH,
            lease_ms: ROUTE_LEASE_MS.min(u64::from(MAX_LEASE_MS)) as u32,
            grace_ms: GRACE_MS.min(u64::from(MAX_GRACE_MS)) as u32,
            membership_digest: self.digest,
            signature: [0; 64],
        };
        let Some(signature) = signer.sign(&directive.signed_bytes()) else {
            return reject(Some(object), Reason::BadSignature);
        };
        directive.signature = signature;
        let event = match self.apply_move(&directive, now_ms) {
            Ok(event) => event,
            Err(reason) => return reject(Some(object), reason),
        };
        let request_id = self.take_request_id();
        let mut body = [0u8; HomeMove::LEN];
        let Ok(len) = directive.encode(&mut body) else {
            return reject(Some(object), Reason::Malformed);
        };
        let wire = Wire {
            dst: peer,
            kind: MessageKind::Request,
            opcode: OpCode::RouteQuery,
            status: Status::Ok,
            flags: 0,
            src_epoch: self.epoch,
            dst_epoch: self.epoch,
            home: directive.old_home,
            home_epoch: directive.old_home_epoch,
            object_epoch: directive.object_epoch,
            object,
            request_id,
            reply_to: 0,
            body: body[..len].to_vec(),
            trace: None,
        };
        self.pending = Some(Pending::Move(Box::new(PendingMove {
            directive,
            request_id,
            deadline_ms: now_ms.saturating_add(MOVE_DEADLINE_MS),
        })));
        Step::new().with_send(wire).event(event)
    }
}

fn reject(object: Option<ObjectId>, reason: Reason) -> Step {
    Step::new().event(Event::Reject { object, reason })
}

fn status_for(reason: Reason) -> Status {
    match reason {
        Reason::None => Status::Ok,
        Reason::StaleEpoch
        | Reason::StaleObjectEpoch
        | Reason::UnknownEpoch
        | Reason::StaleCapability
        | Reason::RepeatedMoved => Status::Stale,
        Reason::WrongSubject
        | Reason::UntrustedIssuer
        | Reason::BadSignature
        | Reason::InsufficientRights
        | Reason::Expired
        | Reason::NotYetValid => Status::Permission,
        Reason::NotHome | Reason::NotCurrent | Reason::Fixture | Reason::WrongObject => {
            Status::NotFound
        }
        Reason::NoAuthority => Status::Permission,
        Reason::WrongHome
        | Reason::Malformed
        | Reason::Replay
        | Reason::BadMove
        | Reason::UnknownRequest
        | Reason::PeerMismatch
        | Reason::TraceMismatch
        | Reason::WrongOperation => Status::Protocol,
        Reason::NoPeer | Reason::LinkDown | Reason::Busy => Status::Again,
        Reason::Timeout => Status::TimedOut,
    }
}

#[cfg(test)]
mod tests;
