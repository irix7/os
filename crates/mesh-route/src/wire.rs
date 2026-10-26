// SPDX-License-Identifier: GPL-3.0-only
//! Bounded little-endian bodies for the M2 remote capability route.
//!
//! A body is carried inside one `MeshEnvelope` `MessageFrame`; the M2-1
//! record framing, 64 KiB bound, header CRC and `frame_len` agreement stay in
//! `mesh-transport` and `sys-primitives`.  This module only defines the bytes
//! of the route messages.
//!
//! ```text
//! prefix: magic u32 "I7R1", version u8, kind u8, flags u16 = 0
//! INVOKE:    capability [320], operation u16, reserved u16
//! RESULT:    object [32], home [16], home_epoch u32, object_epoch u32,
//!            lease_ms u32, value [32]
//! MOVED:     object [32], old_home [16], new_home [16],
//!            old_home_epoch u32, new_home_epoch u32, object_epoch u32,
//!            lease_ms u32, grace_ms u32, reason u16, reserved u16
//! HOME_MOVE: object [32], old_home [16], new_home [16],
//!            membership_epoch u32, old_home_epoch u32, new_home_epoch u32,
//!            object_epoch u32, lease_ms u32, grace_ms u32,
//!            membership_digest [16], signature [64]
//! MOVE_ACK:  object [32], home [16], home_epoch u32, accepted u16, reason u16
//! ERROR:     object [32], reason u16, reserved u16, detail u32
//! ```
//!
//! Every field is encoded explicitly in little-endian order.  No
//! `#[repr(C)]` structure is written to the wire; the capability token is the
//! only fixed binary field and it has its own canonical codec.

use alloc::vec::Vec;
use core::fmt;

use cap_engine::verify;
use sys_primitives::{Capability, NodeId, NodeKey, ObjectId};

/// Body magic: `I7R1` in little-endian byte order.
pub const MAGIC: u32 = u32::from_le_bytes(*b"I7R1");
/// Body format version.
pub const VERSION: u8 = 1;
/// Maximum route body.  The largest body is an invoke (332 bytes).
pub const MAX_BODY: usize = 512;
/// Fixed body prefix: magic, version, kind, flags.
pub const PREFIX: usize = 8;

/// A capability-bearing object operation.
pub const KIND_INVOKE: u8 = 1;
/// A successful home result.
pub const KIND_RESULT: u8 = 2;
/// The old home redirected the sender.
pub const KIND_MOVED: u8 = 3;
/// A root-authorised home lease move.
pub const KIND_HOME_MOVE: u8 = 4;
/// The receiving home accepted or refused a move.
pub const KIND_MOVE_ACK: u8 = 5;
/// A terminal refusal.
pub const KIND_ERROR: u8 = 6;

/// The read operation: a deterministic fixture stat.
pub const OP_STAT: u16 = 1;
/// The mutable operation: a deterministic fixture commit.
///
/// A commit needs a current authoritative view and a current home lease at the
/// destination.  It is the M2 instrument for the partition rule: the side
/// without the authority fails closed instead of creating a second home.
pub const OP_COMMIT: u16 = 2;

/// Domain separator covered by the home-move signature.
pub const HOME_MOVE_DOMAIN: &[u8] = b"IRIX7-M2-HOME-MOVE-v1";

/// Why a route frame was refused.  The discriminant is stable on the wire.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u16)]
pub enum Reason {
    /// No reason; used in a normal `MOVED`.
    None = 0,
    /// The destination is not the home named by the route.
    WrongHome = 1,
    /// The destination holds no lease and is not a draining old home.
    NotHome = 2,
    /// The request is older than the destination's home or membership epoch.
    StaleEpoch = 3,
    /// The object generation does not match the lease.
    StaleObjectEpoch = 4,
    /// The request names an epoch the destination has not issued.
    UnknownEpoch = 5,
    /// The capability or body names a different object.
    WrongObject = 6,
    /// The capability subject is not the sending principal.
    WrongSubject = 7,
    /// The capability issuer is not the configured authority.
    UntrustedIssuer = 8,
    /// The capability signature did not verify.
    BadSignature = 9,
    /// The capability validity window has passed.
    Expired = 10,
    /// The capability is not valid yet.
    NotYetValid = 11,
    /// The capability lacks the required right.
    InsufficientRights = 12,
    /// The capability was issued for another home assignment.
    StaleCapability = 13,
    /// A second `MOVED` for one operation.
    RepeatedMoved = 14,
    /// The body did not decode or violated the bounds.
    Malformed = 15,
    /// No peer link is configured or available.
    NoPeer = 16,
    /// The operation or move deadline passed.
    Timeout = 17,
    /// A replayed or non-monotonic request id.
    Replay = 18,
    /// The home move is not authorised or inconsistent with the view.
    BadMove = 19,
    /// The configured fixture set cannot provide the scenario objects.
    Fixture = 20,
    /// One operation is already outstanding.
    Busy = 21,
    /// The link went down with work outstanding.
    LinkDown = 22,
    /// A response does not answer the outstanding request.
    UnknownRequest = 23,
    /// The frame source is not the configured peer.
    PeerMismatch = 24,
    /// The operation code is not supported.
    WrongOperation = 25,
    /// The node cannot confirm a current authoritative view, so a mutable
    /// commit is unavailable.  It is not a redirect and not a second home.
    NoAuthority = 26,
    /// The node is the rendezvous home but holds no current lease, so the
    /// object is unavailable here.
    NotCurrent = 27,
    /// A response did not carry the trace id of its traced request.
    TraceMismatch = 28,
}

impl Reason {
    /// Decode a wire discriminant.
    pub const fn from_u16(value: u16) -> Option<Self> {
        Some(match value {
            0 => Self::None,
            1 => Self::WrongHome,
            2 => Self::NotHome,
            3 => Self::StaleEpoch,
            4 => Self::StaleObjectEpoch,
            5 => Self::UnknownEpoch,
            6 => Self::WrongObject,
            7 => Self::WrongSubject,
            8 => Self::UntrustedIssuer,
            9 => Self::BadSignature,
            10 => Self::Expired,
            11 => Self::NotYetValid,
            12 => Self::InsufficientRights,
            13 => Self::StaleCapability,
            14 => Self::RepeatedMoved,
            15 => Self::Malformed,
            16 => Self::NoPeer,
            17 => Self::Timeout,
            18 => Self::Replay,
            19 => Self::BadMove,
            20 => Self::Fixture,
            21 => Self::Busy,
            22 => Self::LinkDown,
            23 => Self::UnknownRequest,
            24 => Self::PeerMismatch,
            25 => Self::WrongOperation,
            26 => Self::NoAuthority,
            27 => Self::NotCurrent,
            28 => Self::TraceMismatch,
            _ => return None,
        })
    }

    /// The wire discriminant.
    pub const fn as_u16(self) -> u16 {
        self as u16
    }

    /// A short stable label for serial logs and evidence.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::WrongHome => "wrong-home",
            Self::NotHome => "not-home",
            Self::StaleEpoch => "stale-epoch",
            Self::StaleObjectEpoch => "stale-object-epoch",
            Self::UnknownEpoch => "unknown-epoch",
            Self::WrongObject => "wrong-object",
            Self::WrongSubject => "wrong-subject",
            Self::UntrustedIssuer => "untrusted-issuer",
            Self::BadSignature => "bad-signature",
            Self::Expired => "expired",
            Self::NotYetValid => "not-yet-valid",
            Self::InsufficientRights => "insufficient-rights",
            Self::StaleCapability => "stale-capability",
            Self::RepeatedMoved => "repeated-moved",
            Self::Malformed => "malformed",
            Self::NoPeer => "no-peer",
            Self::Timeout => "timeout",
            Self::Replay => "replay",
            Self::BadMove => "bad-move",
            Self::Fixture => "fixture",
            Self::Busy => "busy",
            Self::LinkDown => "link-down",
            Self::UnknownRequest => "unknown-request",
            Self::PeerMismatch => "peer-mismatch",
            Self::WrongOperation => "wrong-operation",
            Self::NoAuthority => "no-authority",
            Self::NotCurrent => "not-current",
            Self::TraceMismatch => "trace-mismatch",
        }
    }
}

impl fmt::Display for Reason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A malformed route body.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WireError {
    /// The body ended before a field.
    TooShort,
    /// The body exceeds [`MAX_BODY`].
    TooLong,
    /// The magic is not `I7R1`.
    BadMagic,
    /// The format version is unknown.
    BadVersion,
    /// The message kind is unknown.
    BadKind,
    /// A reserved field is non-zero.
    NonCanonical,
    /// The body is shorter or longer than its kind requires.
    LengthMismatch,
    /// A node identifier is the all-zero sentinel.
    ZeroNode,
    /// The capability token did not decode.
    BadCapability,
    /// The move signature did not verify.
    BadSignature,
    /// The caller's output buffer is too small.
    OutputTooSmall,
}

impl WireError {
    /// A short stable label for serial logs and evidence.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::TooShort => "too-short",
            Self::TooLong => "too-long",
            Self::BadMagic => "bad-magic",
            Self::BadVersion => "bad-version",
            Self::BadKind => "bad-kind",
            Self::NonCanonical => "non-canonical",
            Self::LengthMismatch => "length-mismatch",
            Self::ZeroNode => "zero-node",
            Self::BadCapability => "bad-capability",
            Self::BadSignature => "bad-signature",
            Self::OutputTooSmall => "output-too-small",
        }
    }
}

impl fmt::Display for WireError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A capability-bearing stat request.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Invoke {
    /// The capability that authorises the operation.
    pub capability: Capability,
    /// The operation code.
    pub operation: u16,
}

impl Invoke {
    /// Encoded body length.
    pub const LEN: usize = PREFIX + Capability::LEN + 2 + 2;

    /// Encode into `out`.  Returns the number of bytes written.
    #[inline(never)]
    pub fn encode(&self, out: &mut [u8]) -> Result<usize, WireError> {
        if out.len() < Self::LEN {
            return Err(WireError::OutputTooSmall);
        }
        write_prefix(out, KIND_INVOKE);
        out[PREFIX..PREFIX + Capability::LEN].copy_from_slice(&self.capability.encode());
        let mut cursor = PREFIX + Capability::LEN;
        put_u16(out, &mut cursor, self.operation);
        put_u16(out, &mut cursor, 0);
        debug_assert_eq!(cursor, Self::LEN);
        Ok(Self::LEN)
    }

    /// Decode exactly one body.
    #[inline(never)]
    pub fn decode(body: &[u8]) -> Result<Self, WireError> {
        if body.len() != Self::LEN {
            return Err(WireError::LengthMismatch);
        }
        if prefix_kind(body)? != KIND_INVOKE {
            return Err(WireError::BadKind);
        }
        let mut reader = Reader::new(&body[PREFIX..]);
        let capability = reader.capability()?;
        let operation = reader.u16()?;
        let reserved = reader.u16()?;
        if reserved != 0 {
            return Err(WireError::NonCanonical);
        }
        Ok(Self {
            capability,
            operation,
        })
    }
}

/// A successful fixture stat at the current home.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct InvokeResult {
    /// Object identity.
    pub object: ObjectId,
    /// The home that served the operation.
    pub home: NodeId,
    /// The home assignment epoch used by the result.
    pub home_epoch: u32,
    /// The object generation used by the result.
    pub object_epoch: u32,
    /// Remaining lease in milliseconds as observed by the home.
    pub lease_ms: u32,
    /// Deterministic fixture value.
    pub value: [u8; 32],
}

impl InvokeResult {
    /// Encoded body length.
    pub const LEN: usize = PREFIX + 32 + 16 + 4 + 4 + 4 + 32;

    /// Encode into `out`.  Returns the number of bytes written.
    #[inline(never)]
    pub fn encode(&self, out: &mut [u8]) -> Result<usize, WireError> {
        if out.len() < Self::LEN {
            return Err(WireError::OutputTooSmall);
        }
        write_prefix(out, KIND_RESULT);
        let mut cursor = PREFIX;
        put_object(out, &mut cursor, &self.object);
        put_node(out, &mut cursor, &self.home);
        put_u32(out, &mut cursor, self.home_epoch);
        put_u32(out, &mut cursor, self.object_epoch);
        put_u32(out, &mut cursor, self.lease_ms);
        out[cursor..cursor + 32].copy_from_slice(&self.value);
        cursor += 32;
        debug_assert_eq!(cursor, Self::LEN);
        Ok(Self::LEN)
    }

    /// Decode exactly one body.
    #[inline(never)]
    pub fn decode(body: &[u8]) -> Result<Self, WireError> {
        if body.len() != Self::LEN {
            return Err(WireError::LengthMismatch);
        }
        if prefix_kind(body)? != KIND_RESULT {
            return Err(WireError::BadKind);
        }
        let mut reader = Reader::new(&body[PREFIX..]);
        let object = reader.object()?;
        let home = reader.node()?;
        let home_epoch = reader.u32()?;
        let object_epoch = reader.u32()?;
        let lease_ms = reader.u32()?;
        let mut value = [0u8; 32];
        value.copy_from_slice(reader.take(32)?);
        if home.is_zero() || home_epoch == 0 || object_epoch == 0 {
            return Err(WireError::NonCanonical);
        }
        Ok(Self {
            object,
            home,
            home_epoch,
            object_epoch,
            lease_ms,
            value,
        })
    }
}

/// A bounded redirect from the old home.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Moved {
    /// Object identity.
    pub object: ObjectId,
    /// The old home that answered.
    pub old_home: NodeId,
    /// The new home to retry at.
    pub new_home: NodeId,
    /// The old home assignment epoch.
    pub old_home_epoch: u32,
    /// The new home assignment epoch.
    pub new_home_epoch: u32,
    /// The object generation.
    pub object_epoch: u32,
    /// Remaining lease advertised by the old home.
    pub lease_ms: u32,
    /// Remaining forwarding grace advertised by the old home.
    pub grace_ms: u32,
    /// Why the old home redirected.
    pub reason: Reason,
}

impl Moved {
    /// Encoded body length.
    pub const LEN: usize = PREFIX + 32 + 16 + 16 + 4 + 4 + 4 + 4 + 4 + 2 + 2;

    /// Encode into `out`.  Returns the number of bytes written.
    #[inline(never)]
    pub fn encode(&self, out: &mut [u8]) -> Result<usize, WireError> {
        if out.len() < Self::LEN {
            return Err(WireError::OutputTooSmall);
        }
        if self.new_home == self.old_home
            || self.new_home_epoch != self.old_home_epoch.saturating_add(1)
            || self.lease_ms == 0
            || self.grace_ms == 0
        {
            return Err(WireError::NonCanonical);
        }
        write_prefix(out, KIND_MOVED);
        let mut cursor = PREFIX;
        put_object(out, &mut cursor, &self.object);
        put_node(out, &mut cursor, &self.old_home);
        put_node(out, &mut cursor, &self.new_home);
        put_u32(out, &mut cursor, self.old_home_epoch);
        put_u32(out, &mut cursor, self.new_home_epoch);
        put_u32(out, &mut cursor, self.object_epoch);
        put_u32(out, &mut cursor, self.lease_ms);
        put_u32(out, &mut cursor, self.grace_ms);
        put_u16(out, &mut cursor, self.reason.as_u16());
        put_u16(out, &mut cursor, 0);
        debug_assert_eq!(cursor, Self::LEN);
        Ok(Self::LEN)
    }

    /// Decode exactly one body.
    #[inline(never)]
    pub fn decode(body: &[u8]) -> Result<Self, WireError> {
        if body.len() != Self::LEN {
            return Err(WireError::LengthMismatch);
        }
        if prefix_kind(body)? != KIND_MOVED {
            return Err(WireError::BadKind);
        }
        let mut reader = Reader::new(&body[PREFIX..]);
        let object = reader.object()?;
        let old_home = reader.node()?;
        let new_home = reader.node()?;
        let old_home_epoch = reader.u32()?;
        let new_home_epoch = reader.u32()?;
        let object_epoch = reader.u32()?;
        let lease_ms = reader.u32()?;
        let grace_ms = reader.u32()?;
        let reason = Reason::from_u16(reader.u16()?).ok_or(WireError::NonCanonical)?;
        let reserved = reader.u16()?;
        if reserved != 0
            || old_home.is_zero()
            || new_home.is_zero()
            || old_home == new_home
            || object_epoch == 0
            || new_home_epoch != old_home_epoch.saturating_add(1)
            || lease_ms == 0
            || grace_ms == 0
        {
            return Err(WireError::NonCanonical);
        }
        Ok(Self {
            object,
            old_home,
            new_home,
            old_home_epoch,
            new_home_epoch,
            object_epoch,
            lease_ms,
            grace_ms,
            reason,
        })
    }
}

/// A root-authorised move of one object's home lease.
///
/// This is the M2 lab control message for the stale-home path, not a second
/// placement model.  The old home must be the rendezvous winner under the
/// confirmed view; the new home is the other confirmed member.  The move is
/// bounded by a lease and a forwarding grace period and is signed by the
/// configured authority root.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HomeMove {
    /// Object whose lease moves.
    pub object: ObjectId,
    /// The rendezvous home under the confirmed view.
    pub old_home: NodeId,
    /// The member that becomes the new home.
    pub new_home: NodeId,
    /// The confirmed membership epoch this move is scoped to.
    pub membership_epoch: u32,
    /// The old home assignment epoch.
    pub old_home_epoch: u32,
    /// The new home assignment epoch; exactly one greater.
    pub new_home_epoch: u32,
    /// The object generation.
    pub object_epoch: u32,
    /// The new lease length in milliseconds.
    pub lease_ms: u32,
    /// The old home's forwarding grace in milliseconds.
    pub grace_ms: u32,
    /// The confirmed membership digest this move is scoped to.
    pub membership_digest: [u8; 16],
    /// Ed25519 signature over [`Self::signed_bytes`].
    pub signature: [u8; 64],
}

impl HomeMove {
    /// Encoded body length.
    pub const LEN: usize = PREFIX + 32 + 16 + 16 + 4 + 4 + 4 + 4 + 4 + 4 + 16 + 64;

    /// Encode into `out`.  Returns the number of bytes written.
    #[inline(never)]
    pub fn encode(&self, out: &mut [u8]) -> Result<usize, WireError> {
        if out.len() < Self::LEN {
            return Err(WireError::OutputTooSmall);
        }
        if self.old_home.is_zero()
            || self.new_home.is_zero()
            || self.old_home == self.new_home
            || self.membership_epoch == 0
            || self.old_home_epoch == 0
            || self.new_home_epoch != self.old_home_epoch.saturating_add(1)
            || self.object_epoch == 0
            || self.lease_ms == 0
            || self.grace_ms == 0
        {
            return Err(WireError::NonCanonical);
        }
        write_prefix(out, KIND_HOME_MOVE);
        let mut cursor = PREFIX;
        put_object(out, &mut cursor, &self.object);
        put_node(out, &mut cursor, &self.old_home);
        put_node(out, &mut cursor, &self.new_home);
        put_u32(out, &mut cursor, self.membership_epoch);
        put_u32(out, &mut cursor, self.old_home_epoch);
        put_u32(out, &mut cursor, self.new_home_epoch);
        put_u32(out, &mut cursor, self.object_epoch);
        put_u32(out, &mut cursor, self.lease_ms);
        put_u32(out, &mut cursor, self.grace_ms);
        out[cursor..cursor + 16].copy_from_slice(&self.membership_digest);
        cursor += 16;
        out[cursor..cursor + 64].copy_from_slice(&self.signature);
        cursor += 64;
        debug_assert_eq!(cursor, Self::LEN);
        Ok(Self::LEN)
    }

    /// Decode exactly one body.  The signature is not verified here.
    #[inline(never)]
    pub fn decode(body: &[u8]) -> Result<Self, WireError> {
        if body.len() != Self::LEN {
            return Err(WireError::LengthMismatch);
        }
        if prefix_kind(body)? != KIND_HOME_MOVE {
            return Err(WireError::BadKind);
        }
        let mut reader = Reader::new(&body[PREFIX..]);
        let object = reader.object()?;
        let old_home = reader.node()?;
        let new_home = reader.node()?;
        let membership_epoch = reader.u32()?;
        let old_home_epoch = reader.u32()?;
        let new_home_epoch = reader.u32()?;
        let object_epoch = reader.u32()?;
        let lease_ms = reader.u32()?;
        let grace_ms = reader.u32()?;
        let mut membership_digest = [0u8; 16];
        membership_digest.copy_from_slice(reader.take(16)?);
        let mut signature = [0u8; 64];
        signature.copy_from_slice(reader.take(64)?);
        if old_home.is_zero()
            || new_home.is_zero()
            || old_home == new_home
            || membership_epoch == 0
            || old_home_epoch == 0
            || new_home_epoch != old_home_epoch.saturating_add(1)
            || object_epoch == 0
            || lease_ms == 0
            || grace_ms == 0
        {
            return Err(WireError::NonCanonical);
        }
        Ok(Self {
            object,
            old_home,
            new_home,
            membership_epoch,
            old_home_epoch,
            new_home_epoch,
            object_epoch,
            lease_ms,
            grace_ms,
            membership_digest,
            signature,
        })
    }

    /// Canonical bytes covered by the authority signature.
    pub fn signed_bytes(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(HOME_MOVE_DOMAIN.len() + 1 + 32 + 16 + 16 + 4 * 7 + 16);
        out.extend_from_slice(HOME_MOVE_DOMAIN);
        out.push(VERSION);
        out.extend_from_slice(self.object.as_bytes());
        out.extend_from_slice(self.old_home.as_bytes());
        out.extend_from_slice(self.new_home.as_bytes());
        out.extend_from_slice(&self.membership_epoch.to_le_bytes());
        out.extend_from_slice(&self.old_home_epoch.to_le_bytes());
        out.extend_from_slice(&self.new_home_epoch.to_le_bytes());
        out.extend_from_slice(&self.object_epoch.to_le_bytes());
        out.extend_from_slice(&self.lease_ms.to_le_bytes());
        out.extend_from_slice(&self.grace_ms.to_le_bytes());
        out.extend_from_slice(&self.membership_digest);
        out
    }

    /// Verify the authority signature over this move.
    pub fn verify(&self, authority: &NodeKey) -> Result<(), WireError> {
        verify(authority, &self.signed_bytes(), &self.signature)
            .map_err(|_| WireError::BadSignature)
    }
}

/// The receiving home's answer to a [`HomeMove`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MoveAck {
    /// Object whose lease was moved.
    pub object: ObjectId,
    /// The acknowledging home.
    pub home: NodeId,
    /// The home assignment epoch the acknowledgement is for.
    pub home_epoch: u32,
    /// Whether the move was applied.
    pub accepted: bool,
    /// Why the move was refused, when `accepted` is false.
    pub reason: Reason,
}

impl MoveAck {
    /// Encoded body length.
    pub const LEN: usize = PREFIX + 32 + 16 + 4 + 2 + 2;

    /// Encode into `out`.  Returns the number of bytes written.
    #[inline(never)]
    pub fn encode(&self, out: &mut [u8]) -> Result<usize, WireError> {
        if out.len() < Self::LEN {
            return Err(WireError::OutputTooSmall);
        }
        write_prefix(out, KIND_MOVE_ACK);
        let mut cursor = PREFIX;
        put_object(out, &mut cursor, &self.object);
        put_node(out, &mut cursor, &self.home);
        put_u32(out, &mut cursor, self.home_epoch);
        put_u16(out, &mut cursor, if self.accepted { 0 } else { 1 });
        put_u16(out, &mut cursor, self.reason.as_u16());
        debug_assert_eq!(cursor, Self::LEN);
        Ok(Self::LEN)
    }

    /// Decode exactly one body.
    #[inline(never)]
    pub fn decode(body: &[u8]) -> Result<Self, WireError> {
        if body.len() != Self::LEN {
            return Err(WireError::LengthMismatch);
        }
        if prefix_kind(body)? != KIND_MOVE_ACK {
            return Err(WireError::BadKind);
        }
        let mut reader = Reader::new(&body[PREFIX..]);
        let object = reader.object()?;
        let home = reader.node()?;
        let home_epoch = reader.u32()?;
        let code = reader.u16()?;
        let reason = Reason::from_u16(reader.u16()?).ok_or(WireError::NonCanonical)?;
        if home.is_zero() || home_epoch == 0 || code > 1 {
            return Err(WireError::NonCanonical);
        }
        let accepted = code == 0;
        if accepted && reason != Reason::None {
            return Err(WireError::NonCanonical);
        }
        Ok(Self {
            object,
            home,
            home_epoch,
            accepted,
            reason,
        })
    }
}

/// A terminal refusal body.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RouteErrorBody {
    /// Object identity, or zero when the body was unreadable.
    pub object: ObjectId,
    /// Why the operation was refused.
    pub reason: Reason,
    /// Optional detail; zero in this slice.
    pub detail: u32,
}

impl RouteErrorBody {
    /// Encoded body length.
    pub const LEN: usize = PREFIX + 32 + 2 + 2 + 4;

    /// Encode into `out`.  Returns the number of bytes written.
    #[inline(never)]
    pub fn encode(&self, out: &mut [u8]) -> Result<usize, WireError> {
        if out.len() < Self::LEN {
            return Err(WireError::OutputTooSmall);
        }
        write_prefix(out, KIND_ERROR);
        let mut cursor = PREFIX;
        put_object(out, &mut cursor, &self.object);
        put_u16(out, &mut cursor, self.reason.as_u16());
        put_u16(out, &mut cursor, 0);
        put_u32(out, &mut cursor, self.detail);
        debug_assert_eq!(cursor, Self::LEN);
        Ok(Self::LEN)
    }

    /// Decode exactly one body.
    #[inline(never)]
    pub fn decode(body: &[u8]) -> Result<Self, WireError> {
        if body.len() != Self::LEN {
            return Err(WireError::LengthMismatch);
        }
        if prefix_kind(body)? != KIND_ERROR {
            return Err(WireError::BadKind);
        }
        let mut reader = Reader::new(&body[PREFIX..]);
        let object = reader.object()?;
        let reason = Reason::from_u16(reader.u16()?).ok_or(WireError::NonCanonical)?;
        let reserved = reader.u16()?;
        let detail = reader.u32()?;
        if reserved != 0 {
            return Err(WireError::NonCanonical);
        }
        Ok(Self {
            object,
            reason,
            detail,
        })
    }
}

/// One decoded route body.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RouteBody {
    /// A capability-bearing stat request.
    Invoke(Invoke),
    /// A successful stat result.
    Result(InvokeResult),
    /// A redirect.
    Moved(Moved),
    /// A root-authorised move.
    HomeMove(HomeMove),
    /// A move acknowledgement.
    MoveAck(MoveAck),
    /// A terminal refusal.
    Error(RouteErrorBody),
}

impl RouteBody {
    /// Decode exactly one body.  Trailing bytes are rejected.
    #[inline(never)]
    pub fn decode(body: &[u8]) -> Result<Self, WireError> {
        match prefix_kind(body)? {
            KIND_INVOKE => Ok(Self::Invoke(Invoke::decode(body)?)),
            KIND_RESULT => Ok(Self::Result(InvokeResult::decode(body)?)),
            KIND_MOVED => Ok(Self::Moved(Moved::decode(body)?)),
            KIND_HOME_MOVE => Ok(Self::HomeMove(HomeMove::decode(body)?)),
            KIND_MOVE_ACK => Ok(Self::MoveAck(MoveAck::decode(body)?)),
            KIND_ERROR => Ok(Self::Error(RouteErrorBody::decode(body)?)),
            _ => Err(WireError::BadKind),
        }
    }
}

fn prefix_kind(body: &[u8]) -> Result<u8, WireError> {
    if body.len() < PREFIX {
        return Err(WireError::TooShort);
    }
    if body.len() > MAX_BODY {
        return Err(WireError::TooLong);
    }
    if u32::from_le_bytes([body[0], body[1], body[2], body[3]]) != MAGIC {
        return Err(WireError::BadMagic);
    }
    if body[4] != VERSION {
        return Err(WireError::BadVersion);
    }
    if u16::from_le_bytes([body[6], body[7]]) != 0 {
        return Err(WireError::NonCanonical);
    }
    Ok(body[5])
}

fn write_prefix(out: &mut [u8], kind: u8) {
    out[0..4].copy_from_slice(&MAGIC.to_le_bytes());
    out[4] = VERSION;
    out[5] = kind;
    out[6] = 0;
    out[7] = 0;
}

fn put_u16(out: &mut [u8], cursor: &mut usize, value: u16) {
    out[*cursor..*cursor + 2].copy_from_slice(&value.to_le_bytes());
    *cursor += 2;
}

fn put_u32(out: &mut [u8], cursor: &mut usize, value: u32) {
    out[*cursor..*cursor + 4].copy_from_slice(&value.to_le_bytes());
    *cursor += 4;
}

fn put_node(out: &mut [u8], cursor: &mut usize, node: &NodeId) {
    out[*cursor..*cursor + 16].copy_from_slice(node.as_bytes());
    *cursor += 16;
}

fn put_object(out: &mut [u8], cursor: &mut usize, object: &ObjectId) {
    out[*cursor..*cursor + 32].copy_from_slice(object.as_bytes());
    *cursor += 32;
}

struct Reader<'a> {
    bytes: &'a [u8],
    position: usize,
}

impl<'a> Reader<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, position: 0 }
    }

    fn take(&mut self, count: usize) -> Result<&'a [u8], WireError> {
        let end = self
            .position
            .checked_add(count)
            .ok_or(WireError::LengthMismatch)?;
        if end > self.bytes.len() {
            return Err(WireError::TooShort);
        }
        let slice = &self.bytes[self.position..end];
        self.position = end;
        Ok(slice)
    }

    fn u16(&mut self) -> Result<u16, WireError> {
        let bytes = self.take(2)?;
        Ok(u16::from_le_bytes([bytes[0], bytes[1]]))
    }

    fn u32(&mut self) -> Result<u32, WireError> {
        let bytes = self.take(4)?;
        Ok(u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
    }

    fn node(&mut self) -> Result<NodeId, WireError> {
        let bytes = self.take(16)?;
        let mut node = [0u8; 16];
        node.copy_from_slice(bytes);
        if node.iter().all(|&byte| byte == 0) {
            return Err(WireError::ZeroNode);
        }
        Ok(NodeId(node))
    }

    fn object(&mut self) -> Result<ObjectId, WireError> {
        let bytes = self.take(32)?;
        let mut object = [0u8; 32];
        object.copy_from_slice(bytes);
        if ObjectId(object).is_zero() {
            return Err(WireError::NonCanonical);
        }
        Ok(ObjectId(object))
    }

    fn capability(&mut self) -> Result<Capability, WireError> {
        let bytes = self.take(Capability::LEN)?;
        Capability::decode(bytes).map_err(|_| WireError::BadCapability)
    }
}
