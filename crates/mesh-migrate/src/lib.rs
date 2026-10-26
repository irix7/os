// SPDX-License-Identifier: GPL-3.0-only
//! M2 L0 stackless-actor state and migration policy.
//!
//! The actor is an explicit Rust state machine, not a thread and not a captured
//! stack.  It is polled one message at a time, so the only suspension point is a
//! message boundary by construction.  Its portable state is encoded
//! field-by-field little-endian, independently of the big-endian n32 host, and
//! identified by a BLAKE3 hash over the logical state (the placement fields
//! `host` and `epoch` are deliberately excluded because a move changes them).
//!
//! ```text
//! source                                   destination
//!   root AUTHORISE (signed ActorMove)
//!   PREPARE(state_len, hash)  ------------->
//!                             <-------------  READY
//!   STATE(canonical bytes)    ------------->
//!                             <-------------  COMMIT
//!   fence old instance, ACK   ------------->
//! ```
//!
//! ## Fail-closed rules
//!
//! * A move needs a `cap-engine` Ed25519 signature from the configured
//!   authority root, the current confirmed membership epoch and digest, and an
//!   actor that is actually hosted on the named old host at the named epoch.
//! * The destination verifies the schema, length and BLAKE3 state hash before
//!   installing the actor.  A malformed or truncated state is rejected.
//! * The old host is fenced at `COMMIT`; a delivery naming the old host or an
//!   old actor epoch is refused with `stale-epoch`/`not-home`.
//! * A delivery carries a root-issued `cap-engine` capability.  The subject is
//!   the actor principal, the object is the actor object, the home/home-epoch
//!   pair must be the current placement, and `INVOKE` is required.
//! * A duplicate message id (at or below the highest applied id) is idempotent:
//!   it returns the current state hash and does not advance the actor.
//!
//! The state format is a compatibility surface.  Any layout change bumps
//! [`SCHEMA`] and requires a migration test.

#![no_std]
#![forbid(unsafe_code)]
#![deny(missing_docs)]

extern crate alloc;

mod node;
mod wire;

pub use node::{ActorReport, Counters, Event, Fence, LastMove, MigrateNode, Report, Signer, Step};
pub use wire::{ActorMove, Body, Wire, DIRECTIVE_LEN};

use core::fmt;

use sys_primitives::{ActorId, NodeId, ObjectId, PrincipalId, Rights};

/// Portable actor-state schema.  Bump on any state-layout change.
pub const SCHEMA: u32 = 1;
/// Magic in the canonical actor-state bytes.
pub const STATE_MAGIC: u32 = u32::from_le_bytes(*b"I7AS");
/// Magic in an `I7M1` body.
pub const MAGIC: u32 = u32::from_le_bytes(*b"I7M1");
/// Wire body version.
pub const VERSION: u8 = 1;
/// Fixed canonical actor-state length.
pub const STATE_LEN: usize = 248;
/// Hard bound on an encoded actor state.
pub const MAX_STATE_BYTES: usize = 512;
/// Hard bound on one migration body.
pub const MAX_BODY: usize = 512;
/// Maximum lease a signed move may request.
pub const MAX_LEASE_MS: u32 = 30_000;
/// Migration deadline for one protocol step.
pub const MIGRATION_DEADLINE_MS: u64 = 10_000;
/// Delivery-capability validity window in milliseconds.
///
/// The migration lease, not this window, bounds the move.  Both kernels use
/// their own monotonic boot clock; a shared time service is later work.
pub const CAPABILITY_WINDOW_MS: u64 = 3_600_000;
/// Domain separator for the root-signed move directive.
pub const MOVE_DOMAIN: &[u8] = b"IRIX7-M2-ACTOR-MOVE-v1";
/// Domain separator for the logical actor-state hash.
pub const HASH_DOMAIN: &[u8] = b"IRIX7-M2-ACTOR-STATE-v1";
/// Domain separator for the deterministic actor identity.
pub const ACTOR_DOMAIN: &[u8] = b"IRIX7-M2-ACTOR-v1";
/// Domain separator for the initial logical digest.
pub const INIT_DOMAIN: &[u8] = b"IRIX7-M2-ACTOR-INIT-v1";
/// Domain separator for one applied message.
pub const STEP_DOMAIN: &[u8] = b"IRIX7-M2-ACTOR-STEP-v1";
/// Domain separator for the actor capability subject.
pub const PRINCIPAL_DOMAIN: &[u8] = b"IRIX7-M2-ACTOR-PRINCIPAL-v1";
/// Domain separator for the actor capability object.
pub const OBJECT_DOMAIN: &[u8] = b"IRIX7-M2-ACTOR-OBJECT-v1";
/// Domain separator for a deterministic message payload.
pub const MESSAGE_DOMAIN: &[u8] = b"IRIX7-M2-ACTOR-MSG-v1";

/// A refusal or codec failure.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    /// The input ended before a field.
    Truncated,
    /// Wrong body magic.
    BadMagic,
    /// Wrong body version.
    BadVersion,
    /// Unknown body kind.
    BadKind,
    /// Non-zero reserved flags.
    BadFlags,
    /// The declared length does not match the bytes.
    BadLength,
    /// Non-canonical or out-of-bound fields.
    NonCanonical,
    /// Unknown state schema.
    UnknownSchema,
    /// The actor identity does not match the directive.
    WrongActor,
    /// The actor home does not match the directive.
    WrongHome,
    /// The actor host does not match the directive.
    WrongHost,
    /// The actor epoch does not match.
    WrongEpoch,
    /// The message names an epoch older than the current placement.
    StaleEpoch,
    /// The message names a host that no longer holds the actor.
    StaleHome,
    /// The message names a stale home epoch.
    StaleLease,
    /// The actor is not resident here.
    NoActor,
    /// A different operation is in flight.
    Busy,
    /// No confirmed membership view.
    NoView,
    /// No configured peer.
    NoPeer,
    /// Only the authority root may sign a move or capability.
    NotRoot,
    /// This node is not the destination named by the directive.
    NotDestination,
    /// This node is not the source named by the directive.
    NotSource,
    /// The directive is not authorised for the current view.
    BadMove,
    /// The Ed25519 signature did not verify.
    BadSignature,
    /// The capability is untrusted or malformed.
    Capability,
    /// The capability issuer is not trusted.
    UntrustedIssuer,
    /// The capability subject is not the actor principal.
    WrongSubject,
    /// The capability does not carry `INVOKE`.
    InsufficientRights,
    /// The capability validity window has not started.
    NotYetValid,
    /// The capability validity window has expired.
    Expired,
    /// The request id was replayed.
    Replay,
    /// A message is already queued.
    QueueFull,
    /// A protocol step or migration lease expired.
    Timeout,
    /// The link was lost; nothing is retried silently.
    LinkDown,
    /// The node has no current authority.
    NoAuthority,
    /// The body kind does not match the frame kind.
    KindMismatch,
    /// The wire frame is malformed.
    Malformed,
}

impl Error {
    /// A stable label for logs, reports and host tests.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Truncated => "truncated",
            Self::BadMagic => "bad-magic",
            Self::BadVersion => "bad-version",
            Self::BadKind => "bad-kind",
            Self::BadFlags => "bad-flags",
            Self::BadLength => "bad-length",
            Self::NonCanonical => "non-canonical",
            Self::UnknownSchema => "unknown-schema",
            Self::WrongActor => "wrong-actor",
            Self::WrongHome => "wrong-home",
            Self::WrongHost => "wrong-host",
            Self::WrongEpoch => "wrong-epoch",
            Self::StaleEpoch => "stale-epoch",
            Self::StaleHome => "stale-home",
            Self::StaleLease => "stale-lease",
            Self::NoActor => "no-actor",
            Self::Busy => "busy",
            Self::NoView => "no-view",
            Self::NoPeer => "no-peer",
            Self::NotRoot => "not-root",
            Self::NotDestination => "not-destination",
            Self::NotSource => "not-source",
            Self::BadMove => "bad-move",
            Self::BadSignature => "bad-signature",
            Self::Capability => "capability",
            Self::UntrustedIssuer => "untrusted-issuer",
            Self::WrongSubject => "wrong-subject",
            Self::InsufficientRights => "insufficient-rights",
            Self::NotYetValid => "not-yet-valid",
            Self::Expired => "expired",
            Self::Replay => "replay",
            Self::QueueFull => "queue-full",
            Self::Timeout => "timeout",
            Self::LinkDown => "link-down",
            Self::NoAuthority => "no-authority",
            Self::KindMismatch => "kind-mismatch",
            Self::Malformed => "malformed",
        }
    }

    /// The wire code for this refusal.
    pub const fn as_u16(self) -> u16 {
        match self {
            Self::Truncated => 1,
            Self::BadMagic => 2,
            Self::BadVersion => 3,
            Self::BadKind => 4,
            Self::BadFlags => 5,
            Self::BadLength => 6,
            Self::NonCanonical => 7,
            Self::UnknownSchema => 8,
            Self::WrongActor => 9,
            Self::WrongHome => 10,
            Self::WrongHost => 11,
            Self::WrongEpoch => 12,
            Self::StaleEpoch => 13,
            Self::StaleHome => 14,
            Self::StaleLease => 15,
            Self::NoActor => 16,
            Self::Busy => 17,
            Self::NoView => 18,
            Self::NoPeer => 19,
            Self::NotRoot => 20,
            Self::NotDestination => 21,
            Self::NotSource => 22,
            Self::BadMove => 23,
            Self::BadSignature => 24,
            Self::Capability => 25,
            Self::UntrustedIssuer => 26,
            Self::WrongSubject => 27,
            Self::InsufficientRights => 28,
            Self::NotYetValid => 29,
            Self::Expired => 30,
            Self::Replay => 31,
            Self::QueueFull => 32,
            Self::Timeout => 33,
            Self::LinkDown => 34,
            Self::NoAuthority => 35,
            Self::KindMismatch => 36,
            Self::Malformed => 37,
        }
    }

    /// Decode a wire code.
    pub const fn from_u16(value: u16) -> Option<Self> {
        Some(match value {
            1 => Self::Truncated,
            2 => Self::BadMagic,
            3 => Self::BadVersion,
            4 => Self::BadKind,
            5 => Self::BadFlags,
            6 => Self::BadLength,
            7 => Self::NonCanonical,
            8 => Self::UnknownSchema,
            9 => Self::WrongActor,
            10 => Self::WrongHome,
            11 => Self::WrongHost,
            12 => Self::WrongEpoch,
            13 => Self::StaleEpoch,
            14 => Self::StaleHome,
            15 => Self::StaleLease,
            16 => Self::NoActor,
            17 => Self::Busy,
            18 => Self::NoView,
            19 => Self::NoPeer,
            20 => Self::NotRoot,
            21 => Self::NotDestination,
            22 => Self::NotSource,
            23 => Self::BadMove,
            24 => Self::BadSignature,
            25 => Self::Capability,
            26 => Self::UntrustedIssuer,
            27 => Self::WrongSubject,
            28 => Self::InsufficientRights,
            29 => Self::NotYetValid,
            30 => Self::Expired,
            31 => Self::Replay,
            32 => Self::QueueFull,
            33 => Self::Timeout,
            34 => Self::LinkDown,
            35 => Self::NoAuthority,
            36 => Self::KindMismatch,
            37 => Self::Malformed,
            _ => return None,
        })
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Derive the deterministic M2 actor identity.
///
/// `actor[index] = BLAKE3-128("IRIX7-M2-ACTOR-v1" || index u32 LE)`.
pub fn actor_for(index: u32) -> ActorId {
    let mut hasher = blake3::Hasher::new();
    hasher.update(ACTOR_DOMAIN);
    hasher.update(&index.to_le_bytes());
    let hash = hasher.finalize();
    let mut bytes = [0u8; 16];
    bytes.copy_from_slice(&hash.as_bytes()[..16]);
    ActorId(bytes)
}

/// The capability object for an actor.
pub fn actor_object(actor: &ActorId) -> ObjectId {
    let mut hasher = blake3::Hasher::new();
    hasher.update(OBJECT_DOMAIN);
    hasher.update(&actor.0);
    ObjectId(*hasher.finalize().as_bytes())
}

/// The capability subject for an actor.
pub fn actor_principal(actor: &ActorId) -> PrincipalId {
    let mut hasher = blake3::Hasher::new();
    hasher.update(PRINCIPAL_DOMAIN);
    hasher.update(&actor.0);
    PrincipalId(*hasher.finalize().as_bytes())
}

/// The initial logical digest for an actor.
pub fn initial_logical(actor: &ActorId) -> [u8; 32] {
    let mut hasher = blake3::Hasher::new();
    hasher.update(INIT_DOMAIN);
    hasher.update(&actor.0);
    *hasher.finalize().as_bytes()
}

/// The logical digest after applying one deterministic message.
pub fn step_logical(previous: &[u8; 32], message: &Message) -> [u8; 32] {
    let mut hasher = blake3::Hasher::new();
    hasher.update(STEP_DOMAIN);
    hasher.update(previous);
    hasher.update(&message.id.to_le_bytes());
    hasher.update(&message.payload);
    *hasher.finalize().as_bytes()
}

/// The deterministic payload for fixture message `index`.
pub fn message_for(index: u32) -> Message {
    let mut hasher = blake3::Hasher::new();
    hasher.update(MESSAGE_DOMAIN);
    hasher.update(&index.to_le_bytes());
    let hash = hasher.finalize();
    let mut payload = [0u8; 32];
    payload.copy_from_slice(hash.as_bytes());
    Message {
        id: 10 + u64::from(index),
        payload,
    }
}

/// One queued or applied actor message.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Message {
    /// Monotonic message id.  The fixture starts at 10.
    pub id: u64,
    /// Opaque deterministic payload.
    pub payload: [u8; 32],
}

/// The acknowledgement of the most recently applied message.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Ack {
    /// The applied message id.
    pub id: u64,
    /// The logical digest after applying it.
    pub state: [u8; 32],
}

/// A capability handle carried as serialised authority, not a pointer or fd.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Handle {
    /// Object named by the handle.
    pub object: ObjectId,
    /// Granted rights.
    pub rights: Rights,
    /// Home node at issue.
    pub home: NodeId,
    /// Home epoch at issue.
    pub home_epoch: u32,
    /// Object revocation epoch at issue.
    pub object_epoch: u32,
}

impl Handle {
    /// A handle for the first M2 fixture object.
    pub fn fixture(home: NodeId) -> Self {
        Self {
            object: mesh_core::fixture_object(0),
            rights: Rights::READ.union(Rights::INVOKE),
            home,
            home_epoch: 1,
            object_epoch: 1,
        }
    }
}

/// How a delivery should be handled at the message boundary.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DeliveryMode {
    /// Apply the message now if the actor is idle.
    Step,
    /// Queue the message without applying it.
    Queue,
}

impl DeliveryMode {
    /// Wire code.
    pub const fn as_u8(self) -> u8 {
        match self {
            Self::Step => 0,
            Self::Queue => 1,
        }
    }

    /// Decode a wire code.
    pub const fn from_u8(value: u8) -> Option<Self> {
        match value {
            0 => Some(Self::Step),
            1 => Some(Self::Queue),
            _ => None,
        }
    }
}

/// The result of one delivery.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Delivery {
    /// The message advanced the actor.
    Applied {
        /// Message id.
        id: u64,
        /// Applied-message counter after the step.
        applied: u64,
        /// Logical digest after the step.
        logical: [u8; 32],
        /// Canonical state hash after the step.
        hash: [u8; 32],
    },
    /// The message id was already applied; the actor did not advance.
    Duplicate {
        /// Message id.
        id: u64,
        /// Applied-message counter.
        applied: u64,
        /// Logical digest.
        logical: [u8; 32],
        /// Canonical state hash.
        hash: [u8; 32],
    },
    /// The message is queued at the boundary and not yet applied.
    Queued {
        /// Message id.
        id: u64,
        /// Whether the same message was already queued.
        duplicate: bool,
        /// Canonical state hash including the queue.
        hash: [u8; 32],
    },
    /// The single queue slot is occupied by a different message.
    QueueFull {
        /// Message id that could not be queued.
        id: u64,
    },
    /// There is no queued message to resume.
    Nothing,
}

impl Delivery {
    /// Whether the delivery advanced the actor.
    pub const fn applied(&self) -> bool {
        matches!(self, Self::Applied { .. } | Self::Duplicate { .. })
    }

    /// The resulting canonical state hash, when one was computed.
    pub const fn hash(&self) -> Option<[u8; 32]> {
        match self {
            Self::Applied { hash, .. }
            | Self::Duplicate { hash, .. }
            | Self::Queued { hash, .. } => Some(*hash),
            _ => None,
        }
    }
}

/// The portable state of one L0 stackless actor.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ActorState {
    /// State schema.
    pub schema: u32,
    /// Stable actor identity.
    pub actor: ActorId,
    /// Rendezvous actor home.
    pub home: NodeId,
    /// Current execution host.
    pub host: NodeId,
    /// Placement/migration epoch.
    pub epoch: u32,
    /// Number of applied messages.
    pub applied: u64,
    /// Highest applied message id.
    pub last: u64,
    /// Logical digest of the applied message chain.
    pub logical: [u8; 32],
    /// The message waiting at the next boundary, if any.
    pub queued: Option<Message>,
    /// The acknowledgement of the most recently applied message.
    pub unacked: Option<Ack>,
    /// A serialised capability handle, as authority rather than a pointer.
    pub handle: Option<Handle>,
}

impl ActorState {
    /// Create the actor at `host` with epoch 1.
    pub fn new(actor: ActorId, home: NodeId, host: NodeId, handle: Option<Handle>) -> Self {
        Self {
            schema: SCHEMA,
            actor,
            home,
            host,
            epoch: 1,
            applied: 0,
            last: 0,
            logical: initial_logical(&actor),
            queued: None,
            unacked: None,
            handle,
        }
    }

    /// The canonical state hash.
    ///
    /// Placement (`host`/`epoch`) is excluded: a move changes it, while the
    /// logical actor state must be identical at source and destination.
    pub fn hash(&self) -> [u8; 32] {
        hash_state(
            self.schema,
            &self.actor,
            &self.home,
            self.applied,
            self.last,
            &self.logical,
            self.queued.as_ref(),
            self.unacked.as_ref(),
            self.handle.as_ref(),
        )
    }

    /// Apply or queue one delivery.
    pub fn deliver(&mut self, message: Message, mode: DeliveryMode) -> Delivery {
        if message.id <= self.last {
            return Delivery::Duplicate {
                id: message.id,
                applied: self.applied,
                logical: self.logical,
                hash: self.hash(),
            };
        }
        match mode {
            DeliveryMode::Step => {
                if self.queued.is_some() {
                    return if self.queued.map(|queued| queued.id) == Some(message.id) {
                        Delivery::Queued {
                            id: message.id,
                            duplicate: true,
                            hash: self.hash(),
                        }
                    } else {
                        Delivery::QueueFull { id: message.id }
                    };
                }
                self.apply(message)
            }
            DeliveryMode::Queue => {
                if let Some(queued) = self.queued {
                    return if queued.id == message.id {
                        Delivery::Queued {
                            id: message.id,
                            duplicate: true,
                            hash: self.hash(),
                        }
                    } else {
                        Delivery::QueueFull { id: message.id }
                    };
                }
                self.queued = Some(message);
                Delivery::Queued {
                    id: message.id,
                    duplicate: false,
                    hash: self.hash(),
                }
            }
        }
    }

    /// Apply the queued message, if any.
    pub fn resume(&mut self) -> Delivery {
        let Some(message) = self.queued.take() else {
            return Delivery::Nothing;
        };
        self.apply(message)
    }

    fn apply(&mut self, message: Message) -> Delivery {
        self.logical = step_logical(&self.logical, &message);
        self.applied = self.applied.saturating_add(1);
        self.last = message.id;
        self.unacked = Some(Ack {
            id: message.id,
            state: self.logical,
        });
        Delivery::Applied {
            id: message.id,
            applied: self.applied,
            logical: self.logical,
            hash: self.hash(),
        }
    }

    /// Move the placement to `host`/`epoch` without changing the logical state.
    pub fn place(&mut self, host: NodeId, epoch: u32) -> Result<(), Error> {
        if host.is_zero() || epoch == 0 {
            return Err(Error::NonCanonical);
        }
        self.host = host;
        self.epoch = epoch;
        Ok(())
    }

    /// Encode the canonical state bytes.
    pub fn encode(&self, out: &mut [u8]) -> Result<usize, Error> {
        if out.len() < STATE_LEN {
            return Err(Error::Truncated);
        }
        out[..STATE_LEN].fill(0);
        out[0..4].copy_from_slice(&STATE_MAGIC.to_le_bytes());
        out[4..8].copy_from_slice(&self.schema.to_le_bytes());
        out[8..24].copy_from_slice(&self.actor.0);
        out[24..40].copy_from_slice(&self.home.0);
        out[40..56].copy_from_slice(&self.host.0);
        out[56..60].copy_from_slice(&self.epoch.to_le_bytes());
        out[60..68].copy_from_slice(&self.applied.to_le_bytes());
        out[68..76].copy_from_slice(&self.last.to_le_bytes());
        out[76..108].copy_from_slice(&self.logical);
        // 108 queued flag, 109..117 id, 117..149 payload
        if let Some(queued) = self.queued {
            out[108] = 1;
            out[109..117].copy_from_slice(&queued.id.to_le_bytes());
            out[117..149].copy_from_slice(&queued.payload);
        }
        // 149 unacked flag, 150..158 id, 158..190 state
        if let Some(unacked) = self.unacked {
            out[149] = 1;
            out[150..158].copy_from_slice(&unacked.id.to_le_bytes());
            out[158..190].copy_from_slice(&unacked.state);
        }
        // 190 handle flag, 191..223 object, 223 rights, 224..240 home,
        // 240..244 home_epoch, 244..248 object_epoch
        if let Some(handle) = self.handle {
            out[190] = 1;
            out[191..223].copy_from_slice(handle.object.as_bytes());
            out[223] = handle.rights.0;
            out[224..240].copy_from_slice(handle.home.as_bytes());
            out[240..244].copy_from_slice(&handle.home_epoch.to_le_bytes());
            out[244..248].copy_from_slice(&handle.object_epoch.to_le_bytes());
        }
        Ok(STATE_LEN)
    }

    /// Decode canonical state bytes, rejecting truncation, unknown schemas and
    /// non-canonical absent fields.
    pub fn decode(bytes: &[u8]) -> Result<Self, Error> {
        if bytes.len() != STATE_LEN {
            return Err(if bytes.len() < STATE_LEN {
                Error::Truncated
            } else {
                Error::BadLength
            });
        }
        if u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]) != STATE_MAGIC {
            return Err(Error::BadMagic);
        }
        let schema = u32::from_le_bytes([bytes[4], bytes[5], bytes[6], bytes[7]]);
        if schema != SCHEMA {
            return Err(Error::UnknownSchema);
        }
        let mut actor = [0u8; 16];
        actor.copy_from_slice(&bytes[8..24]);
        let mut home = [0u8; 16];
        home.copy_from_slice(&bytes[24..40]);
        let mut host = [0u8; 16];
        host.copy_from_slice(&bytes[40..56]);
        let epoch = u32::from_le_bytes([bytes[56], bytes[57], bytes[58], bytes[59]]);
        let applied = read_u64(&bytes[60..68]);
        let last = read_u64(&bytes[68..76]);
        let mut logical = [0u8; 32];
        logical.copy_from_slice(&bytes[76..108]);
        let queued = match bytes[108] {
            0 => {
                if bytes[109..149].iter().any(|&byte| byte != 0) {
                    return Err(Error::NonCanonical);
                }
                None
            }
            1 => {
                let id = read_u64(&bytes[109..117]);
                if id == 0 {
                    return Err(Error::NonCanonical);
                }
                let mut payload = [0u8; 32];
                payload.copy_from_slice(&bytes[117..149]);
                Some(Message { id, payload })
            }
            _ => return Err(Error::NonCanonical),
        };
        let unacked = match bytes[149] {
            0 => {
                if bytes[150..190].iter().any(|&byte| byte != 0) {
                    return Err(Error::NonCanonical);
                }
                None
            }
            1 => {
                let id = read_u64(&bytes[150..158]);
                if id == 0 {
                    return Err(Error::NonCanonical);
                }
                let mut state = [0u8; 32];
                state.copy_from_slice(&bytes[158..190]);
                Some(Ack { id, state })
            }
            _ => return Err(Error::NonCanonical),
        };
        let handle = match bytes[190] {
            0 => {
                if bytes[191..248].iter().any(|&byte| byte != 0) {
                    return Err(Error::NonCanonical);
                }
                None
            }
            1 => {
                let object =
                    ObjectId::from_slice(&bytes[191..223]).map_err(|_| Error::BadLength)?;
                let rights = Rights::from_bits(bytes[223]).map_err(|_| Error::NonCanonical)?;
                let mut home = [0u8; 16];
                home.copy_from_slice(&bytes[224..240]);
                if object.is_zero() || NodeId(home).is_zero() {
                    return Err(Error::NonCanonical);
                }
                Some(Handle {
                    object,
                    rights,
                    home: NodeId(home),
                    home_epoch: u32::from_le_bytes([
                        bytes[240], bytes[241], bytes[242], bytes[243],
                    ]),
                    object_epoch: u32::from_le_bytes([
                        bytes[244], bytes[245], bytes[246], bytes[247],
                    ]),
                })
            }
            _ => return Err(Error::NonCanonical),
        };
        if ActorId(actor).0.iter().all(|&byte| byte == 0)
            || NodeId(home).is_zero()
            || NodeId(host).is_zero()
            || epoch == 0
            || (last == 0) != (applied == 0)
            || applied == 0 && !logical.iter().any(|&byte| byte != 0)
        {
            return Err(Error::NonCanonical);
        }
        Ok(Self {
            schema,
            actor: ActorId(actor),
            home: NodeId(home),
            host: NodeId(host),
            epoch,
            applied,
            last,
            logical,
            queued,
            unacked,
            handle,
        })
    }
}

/// The canonical logical state hash.
#[allow(clippy::too_many_arguments)]
pub fn hash_state(
    schema: u32,
    actor: &ActorId,
    home: &NodeId,
    applied: u64,
    last: u64,
    logical: &[u8; 32],
    queued: Option<&Message>,
    unacked: Option<&Ack>,
    handle: Option<&Handle>,
) -> [u8; 32] {
    let mut hasher = blake3::Hasher::new();
    hasher.update(HASH_DOMAIN);
    hasher.update(&schema.to_le_bytes());
    hasher.update(&actor.0);
    hasher.update(&home.0);
    hasher.update(&applied.to_le_bytes());
    hasher.update(&last.to_le_bytes());
    hasher.update(logical);
    match queued {
        Some(queued) => {
            hasher.update(&[1]);
            hasher.update(&queued.id.to_le_bytes());
            hasher.update(&queued.payload);
        }
        None => {
            hasher.update(&[0]);
        }
    }
    match unacked {
        Some(unacked) => {
            hasher.update(&[1]);
            hasher.update(&unacked.id.to_le_bytes());
            hasher.update(&unacked.state);
        }
        None => {
            hasher.update(&[0]);
        }
    }
    match handle {
        Some(handle) => {
            hasher.update(&[1]);
            hasher.update(handle.object.as_bytes());
            hasher.update(&[handle.rights.0]);
            hasher.update(handle.home.as_bytes());
            hasher.update(&handle.home_epoch.to_le_bytes());
            hasher.update(&handle.object_epoch.to_le_bytes());
        }
        None => {
            hasher.update(&[0]);
        }
    }
    *hasher.finalize().as_bytes()
}

pub(crate) fn read_u64(bytes: &[u8]) -> u64 {
    u64::from_le_bytes([
        bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5], bytes[6], bytes[7],
    ])
}

/// A bounded canonical writer used by the wire codec.
pub(crate) struct Writer<'a> {
    out: &'a mut [u8],
    len: usize,
}

impl<'a> Writer<'a> {
    pub(crate) fn new(out: &'a mut [u8]) -> Self {
        Self { out, len: 0 }
    }

    pub(crate) fn bytes(&mut self, data: &[u8]) -> Result<(), Error> {
        let end = self.len.checked_add(data.len()).ok_or(Error::BadLength)?;
        if end > self.out.len() {
            return Err(Error::BadLength);
        }
        self.out[self.len..end].copy_from_slice(data);
        self.len = end;
        Ok(())
    }

    pub(crate) fn u8(&mut self, value: u8) -> Result<(), Error> {
        self.bytes(&[value])
    }

    pub(crate) fn u16(&mut self, value: u16) -> Result<(), Error> {
        self.bytes(&value.to_le_bytes())
    }

    pub(crate) fn u32(&mut self, value: u32) -> Result<(), Error> {
        self.bytes(&value.to_le_bytes())
    }

    pub(crate) fn u64(&mut self, value: u64) -> Result<(), Error> {
        self.bytes(&value.to_le_bytes())
    }

    pub(crate) fn len(&self) -> usize {
        self.len
    }
}

/// A bounded canonical reader.
pub(crate) struct Reader<'a> {
    bytes: &'a [u8],
    position: usize,
}

impl<'a> Reader<'a> {
    pub(crate) fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, position: 0 }
    }

    pub(crate) fn take(&mut self, count: usize) -> Result<&'a [u8], Error> {
        let end = self.position.checked_add(count).ok_or(Error::Truncated)?;
        if end > self.bytes.len() {
            return Err(Error::Truncated);
        }
        let slice = &self.bytes[self.position..end];
        self.position = end;
        Ok(slice)
    }

    pub(crate) fn u8(&mut self) -> Result<u8, Error> {
        Ok(self.take(1)?[0])
    }

    pub(crate) fn u16(&mut self) -> Result<u16, Error> {
        let bytes = self.take(2)?;
        Ok(u16::from_le_bytes([bytes[0], bytes[1]]))
    }

    pub(crate) fn u32(&mut self) -> Result<u32, Error> {
        let bytes = self.take(4)?;
        Ok(u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
    }

    pub(crate) fn u64(&mut self) -> Result<u64, Error> {
        let bytes = self.take(8)?;
        Ok(read_u64(bytes))
    }

    pub(crate) fn node(&mut self) -> Result<NodeId, Error> {
        let bytes = self.take(16)?;
        let mut node = [0u8; 16];
        node.copy_from_slice(bytes);
        Ok(NodeId(node))
    }

    pub(crate) fn actor(&mut self) -> Result<ActorId, Error> {
        let bytes = self.take(16)?;
        let mut actor = [0u8; 16];
        actor.copy_from_slice(bytes);
        Ok(ActorId(actor))
    }

    pub(crate) fn digest(&mut self) -> Result<[u8; 32], Error> {
        let bytes = self.take(32)?;
        let mut digest = [0u8; 32];
        digest.copy_from_slice(bytes);
        Ok(digest)
    }

    pub(crate) fn done(&self) -> bool {
        self.position == self.bytes.len()
    }

    pub(crate) fn rest(&mut self) -> &'a [u8] {
        let slice = &self.bytes[self.position..];
        self.position = self.bytes.len();
        slice
    }
}

/// Encode a capability into exactly 320 bytes.
pub(crate) fn capability_bytes(capability: &sys_primitives::Capability) -> [u8; 320] {
    capability.encode()
}

#[cfg(test)]
mod tests;
