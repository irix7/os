// SPDX-License-Identifier: GPL-3.0-only
//! Little-endian join wire bodies and the canonical membership snapshot.
//!
//! A body is carried inside one `MeshEnvelope` `MessageFrame`; the M2-1
//! record framing, 64 KiB bound, header CRC and `frame_len` agreement stay in
//! `mesh-transport` and `sys-primitives`.  This module only defines the bytes
//! of the four join messages.  Every field is encoded explicitly in
//! little-endian order; no `#[repr(C)]` struct is written to the wire.

use alloc::vec::Vec;
use core::fmt;

use mesh_core::MAX_NODES;
use sys_primitives::{MessageKind, NodeId};

/// Join body format version.
pub const VERSION: u8 = 1;
/// Join body magic: `I7J1` in little-endian byte order.
pub const MAGIC: u32 = u32::from_le_bytes(*b"I7J1");
/// Maximum join body: a 128-member confirmation plus its signature.
pub const MAX_BODY: usize = 4096;
/// Maximum members in one confirmation (the `mesh-core` hard limit).
pub const MAX_MEMBERS: usize = MAX_NODES;
/// Canonical membership snapshot version.
pub const SNAPSHOT_VERSION: u8 = 1;

/// Message-kind discriminants inside a join body.
pub const KIND_REQUEST: u8 = 1;
/// An authority epoch confirmation.
pub const KIND_CONFIRM: u8 = 2;
/// An authority refusal (informational, unsigned).
pub const KIND_REFUSAL: u8 = 3;
/// The member's terminal acknowledgement.
pub const KIND_ACK: u8 = 4;
/// A member liveness summary.
pub const KIND_SUMMARY: u8 = 5;
/// A root-signed epoch aggregate.
pub const KIND_EPOCH: u8 = 6;

/// Fixed prefix: magic, version, kind, flags.
const PREFIX: usize = 8;
/// Fixed request body length without the prefix.
const REQUEST_BODY: usize = 16 + 8 + 4 + 4 + 16 + 8;
/// Fixed acknowledgement body length without the prefix.
const ACK_BODY: usize = 16 + 4 + 4 + 16 + 8;
/// Fixed refusal body length without the prefix.
const REFUSAL_BODY: usize = 16 + 4 + 2 + 2 + 8;
/// Fixed confirmation body length without the member list and prefix.
const CONFIRM_FIXED: usize = 16 + 4 + 2 + 2 + 16 + 8 + 8 + 64;

/// A malformed join body.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WireError {
    /// The body ended before a field.
    TooShort,
    /// The body exceeds [`MAX_BODY`].
    TooLong,
    /// The magic is not `I7J1`.
    BadMagic,
    /// The format version is unknown.
    BadVersion,
    /// The message kind is unknown.
    BadKind,
    /// The flags or a reserved field is non-zero.
    NonCanonical,
    /// The declared member count is zero or above [`MAX_MEMBERS`].
    BadCount,
    /// A member list is unsorted or contains a duplicate.
    NonCanonicalMembers,
    /// A node identifier is the all-zero sentinel.
    ZeroNode,
    /// The message is shorter or longer than its kind requires.
    LengthMismatch,
    /// A member list does not match the digest that claims it.
    DigestMismatch,
    /// The refusal reason is unknown.
    BadReason,
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
            Self::BadCount => "bad-count",
            Self::NonCanonicalMembers => "non-canonical-members",
            Self::ZeroNode => "zero-node",
            Self::LengthMismatch => "length-mismatch",
            Self::DigestMismatch => "digest-mismatch",
            Self::BadReason => "bad-reason",
            Self::OutputTooSmall => "output-too-small",
        }
    }
}

impl fmt::Display for WireError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Why the authority root refused a join request.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RefusalReason {
    /// The member's last epoch is above the root's own.
    EpochRegression = 1,
    /// The request subject is not the sender or is the root itself.
    WrongSubject = 2,
    /// The sender is not the configured authority root.
    NotRoot = 3,
    /// The membership table cannot accept the member now.
    Busy = 4,
    /// A different member list claims the same epoch.
    EpochConflict = 6,
    /// The request is valid but this build cannot serve it.
    Unsupported = 5,
}

impl RefusalReason {
    /// Decode a reason discriminant.
    pub const fn from_u16(value: u16) -> Option<Self> {
        Some(match value {
            1 => Self::EpochRegression,
            2 => Self::WrongSubject,
            3 => Self::NotRoot,
            4 => Self::Busy,
            5 => Self::Unsupported,
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
            Self::EpochRegression => "epoch-regression",
            Self::WrongSubject => "wrong-subject",
            Self::NotRoot => "not-root",
            Self::Busy => "busy",
            Self::EpochConflict => "epoch-conflict",
            Self::Unsupported => "unsupported",
        }
    }
}

impl fmt::Display for RefusalReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A request from a candidate member to the authority root.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct JoinRequest {
    /// The candidate node.  It must equal the envelope source.
    pub node: NodeId,
    /// Boot/attempt identifier; it binds the confirmation to this attempt.
    pub incarnation: u64,
    /// The requester's last confirmed membership epoch.
    pub last_epoch: u32,
    /// Digest of the requester's last confirmed snapshot.
    pub last_digest: [u8; 16],
    /// Attempt nonce echoed by the confirmation.
    pub nonce: u64,
}

impl JoinRequest {
    /// Encoded body length.
    pub const LEN: usize = PREFIX + REQUEST_BODY;

    /// Encode into `out`.  Returns the number of bytes written.
    pub fn encode(&self, out: &mut [u8]) -> Result<usize, WireError> {
        if out.len() < Self::LEN {
            return Err(WireError::OutputTooSmall);
        }
        write_prefix(out, KIND_REQUEST);
        let mut cursor = PREFIX;
        put_node(out, &mut cursor, &self.node);
        put_u64(out, &mut cursor, self.incarnation);
        put_u32(out, &mut cursor, self.last_epoch);
        put_u32(out, &mut cursor, 0);
        out[cursor..cursor + 16].copy_from_slice(&self.last_digest);
        cursor += 16;
        put_u64(out, &mut cursor, self.nonce);
        debug_assert_eq!(cursor, Self::LEN);
        Ok(Self::LEN)
    }

    fn decode(body: &[u8]) -> Result<Self, WireError> {
        if body.len() != REQUEST_BODY {
            return Err(WireError::LengthMismatch);
        }
        let mut reader = Reader::new(body);
        let node = reader.node()?;
        let incarnation = reader.u64()?;
        let last_epoch = reader.u32()?;
        reader.u32()?; // reserved
        let mut last_digest = [0u8; 16];
        last_digest.copy_from_slice(reader.take(16)?);
        let nonce = reader.u64()?;
        if node.is_zero() {
            return Err(WireError::ZeroNode);
        }
        Ok(Self {
            node,
            incarnation,
            last_epoch,
            last_digest,
            nonce,
        })
    }
}

/// The authority root's signed membership decision.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EpochConfirmation {
    /// The configured authority root.
    pub root: NodeId,
    /// The confirmed membership epoch.
    pub epoch: u32,
    /// The full member list, strictly increasing.
    pub members: Vec<NodeId>,
    /// The node this confirmation is for.
    pub subject: NodeId,
    /// The request nonce this confirmation answers.
    pub request_nonce: u64,
    /// The request incarnation this confirmation answers.
    pub incarnation: u64,
    /// Ed25519 signature over [`Self::signed_bytes`].
    pub signature: [u8; 64],
}

impl EpochConfirmation {
    /// The domain separator covered by the signature.
    pub const DOMAIN: &'static [u8] = b"IRIX7-M2-EPOCH-v1";

    /// Encoded body length for this member list.
    pub fn encoded_len(&self) -> usize {
        PREFIX + CONFIRM_FIXED + 16 * self.members.len()
    }

    /// Encode into `out`.  Returns the number of bytes written.
    pub fn encode(&self, out: &mut [u8]) -> Result<usize, WireError> {
        let total = self.encoded_len();
        if out.len() < total {
            return Err(WireError::OutputTooSmall);
        }
        if self.members.len() > MAX_MEMBERS || self.members.is_empty() {
            return Err(WireError::BadCount);
        }
        for pair in self.members.windows(2) {
            if pair[0] >= pair[1] {
                return Err(WireError::NonCanonicalMembers);
            }
        }
        write_prefix(out, KIND_CONFIRM);
        let mut cursor = PREFIX;
        put_node(out, &mut cursor, &self.root);
        put_u32(out, &mut cursor, self.epoch);
        put_u16(out, &mut cursor, self.members.len() as u16);
        put_u16(out, &mut cursor, 0);
        for member in &self.members {
            put_node(out, &mut cursor, member);
        }
        put_node(out, &mut cursor, &self.subject);
        put_u64(out, &mut cursor, self.request_nonce);
        put_u64(out, &mut cursor, self.incarnation);
        out[cursor..cursor + 64].copy_from_slice(&self.signature);
        cursor += 64;
        debug_assert_eq!(cursor, total);
        Ok(total)
    }

    fn decode(body: &[u8]) -> Result<Self, WireError> {
        if body.len() < CONFIRM_FIXED + 16 {
            return Err(WireError::LengthMismatch);
        }
        let mut reader = Reader::new(body);
        let root = reader.node()?;
        let epoch = reader.u32()?;
        let count = reader.u16()? as usize;
        reader.u16()?; // reserved
        if count == 0 || count > MAX_MEMBERS {
            return Err(WireError::BadCount);
        }
        if body.len() != CONFIRM_FIXED + 16 * count {
            return Err(WireError::LengthMismatch);
        }
        let mut members = Vec::with_capacity(count);
        let mut previous: Option<NodeId> = None;
        for _ in 0..count {
            let member = reader.node()?;
            if member.is_zero() {
                return Err(WireError::ZeroNode);
            }
            if let Some(previous) = previous {
                if member <= previous {
                    return Err(WireError::NonCanonicalMembers);
                }
            }
            previous = Some(member);
            members.push(member);
        }
        let subject = reader.node()?;
        let request_nonce = reader.u64()?;
        let incarnation = reader.u64()?;
        let mut signature = [0u8; 64];
        signature.copy_from_slice(reader.take(64)?);
        if root.is_zero() || subject.is_zero() {
            return Err(WireError::ZeroNode);
        }
        if epoch == 0 {
            return Err(WireError::NonCanonical);
        }
        Ok(Self {
            root,
            epoch,
            members,
            subject,
            request_nonce,
            incarnation,
            signature,
        })
    }

    /// Canonical bytes covered by the Ed25519 signature.
    ///
    /// The signature covers the version, root, epoch, full member list,
    /// subject, request nonce and incarnation.  It does not cover the framing
    /// magic or flags; those are validated separately and cannot change the
    /// meaning of the signed fields.
    pub fn signed_bytes(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(
            Self::DOMAIN.len() + 1 + 16 + 4 + 2 + 16 * self.members.len() + 16 + 8 + 8,
        );
        out.extend_from_slice(Self::DOMAIN);
        out.push(VERSION);
        out.extend_from_slice(self.root.as_bytes());
        out.extend_from_slice(&self.epoch.to_le_bytes());
        out.extend_from_slice(&(self.members.len() as u16).to_le_bytes());
        for member in &self.members {
            out.extend_from_slice(member.as_bytes());
        }
        out.extend_from_slice(self.subject.as_bytes());
        out.extend_from_slice(&self.request_nonce.to_le_bytes());
        out.extend_from_slice(&self.incarnation.to_le_bytes());
        out
    }

    /// The confirmed membership snapshot.
    pub fn snapshot(&self) -> MembershipSnapshot {
        MembershipSnapshot {
            epoch: self.epoch,
            members: self.members.clone(),
        }
    }
}

/// A root-signed epoch record for the persistent epoch store.
///
/// This is not a wire message and never crosses the mesh link.  It is the
/// local record a node writes to a scratch volume so a restart can recover the
/// authority-confirmed epoch and member list instead of falling back to the
/// NVRAM seed.  The root signs it with the same authority key that signs
/// [`EpochConfirmation`]; a member verifies it with `mesh.pub`.
///
/// The record deliberately carries the full member list, for the same reason
/// the confirmation does: an epoch number alone cannot detect two nodes
/// disagreeing about membership at that epoch.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EpochRecord {
    /// The configured authority root.
    pub root: NodeId,
    /// The confirmed membership epoch.
    pub epoch: u32,
    /// The full member list, strictly increasing.
    pub members: Vec<NodeId>,
    /// Ed25519 signature over [`Self::signed_bytes`].
    pub signature: [u8; 64],
}

impl EpochRecord {
    /// The domain separator covered by the signature.
    pub const DOMAIN: &'static [u8] = b"IRIX7-M2-EPOCH-STORE-v1";
    /// Store-record magic: `I7ER` in little-endian byte order.
    pub const MAGIC: u32 = u32::from_le_bytes(*b"I7ER");
    /// Store-record format version.
    pub const VERSION: u8 = 1;
    /// Store-record kind discriminant.
    pub const KIND: u8 = 1;
    /// Fixed prefix: magic, version, kind, flags.
    const PREFIX: usize = 8;
    /// Fixed body length without the member list and prefix.
    const FIXED: usize = 16 + 4 + 2 + 2 + 64;

    /// Encoded length for this member list.
    pub fn encoded_len(&self) -> usize {
        Self::PREFIX + Self::FIXED + 16 * self.members.len()
    }

    /// Encode into `out`.  Returns the number of bytes written.
    pub fn encode(&self, out: &mut [u8]) -> Result<usize, WireError> {
        let total = self.encoded_len();
        if out.len() < total {
            return Err(WireError::OutputTooSmall);
        }
        if self.members.len() > MAX_MEMBERS || self.members.is_empty() {
            return Err(WireError::BadCount);
        }
        if self.root.is_zero() {
            return Err(WireError::ZeroNode);
        }
        if self.epoch == 0 {
            return Err(WireError::NonCanonical);
        }
        for pair in self.members.windows(2) {
            if pair[0] >= pair[1] {
                return Err(WireError::NonCanonicalMembers);
            }
        }
        out[..4].copy_from_slice(&Self::MAGIC.to_le_bytes());
        out[4] = Self::VERSION;
        out[5] = Self::KIND;
        out[6..8].copy_from_slice(&0u16.to_le_bytes());
        let mut cursor = Self::PREFIX;
        put_node(out, &mut cursor, &self.root);
        put_u32(out, &mut cursor, self.epoch);
        put_u16(out, &mut cursor, self.members.len() as u16);
        put_u16(out, &mut cursor, 0);
        for member in &self.members {
            put_node(out, &mut cursor, member);
        }
        out[cursor..cursor + 64].copy_from_slice(&self.signature);
        cursor += 64;
        debug_assert_eq!(cursor, total);
        Ok(total)
    }

    /// Decode exactly one record.  Trailing bytes are rejected.
    pub fn decode(body: &[u8]) -> Result<Self, WireError> {
        if body.len() < Self::PREFIX + Self::FIXED + 16 {
            return Err(WireError::TooShort);
        }
        if u32::from_le_bytes([body[0], body[1], body[2], body[3]]) != Self::MAGIC {
            return Err(WireError::BadMagic);
        }
        if body[4] != Self::VERSION {
            return Err(WireError::BadVersion);
        }
        if body[5] != Self::KIND {
            return Err(WireError::BadKind);
        }
        if u16::from_le_bytes([body[6], body[7]]) != 0 {
            return Err(WireError::NonCanonical);
        }
        let mut reader = Reader::new(&body[Self::PREFIX..]);
        let root = reader.node()?;
        let epoch = reader.u32()?;
        let count = reader.u16()? as usize;
        reader.u16()?; // reserved
        if count == 0 || count > MAX_MEMBERS {
            return Err(WireError::BadCount);
        }
        let expected = Self::PREFIX + Self::FIXED + 16 * count;
        if body.len() != expected {
            return Err(WireError::LengthMismatch);
        }
        let mut members = Vec::with_capacity(count);
        let mut previous: Option<NodeId> = None;
        for _ in 0..count {
            let member = reader.node()?;
            if member.is_zero() {
                return Err(WireError::ZeroNode);
            }
            if let Some(previous) = previous {
                if member <= previous {
                    return Err(WireError::NonCanonicalMembers);
                }
            }
            previous = Some(member);
            members.push(member);
        }
        let mut signature = [0u8; 64];
        signature.copy_from_slice(reader.take(64)?);
        if root.is_zero() {
            return Err(WireError::ZeroNode);
        }
        if epoch == 0 {
            return Err(WireError::NonCanonical);
        }
        Ok(Self {
            root,
            epoch,
            members,
            signature,
        })
    }

    /// Canonical bytes covered by the Ed25519 signature.
    pub fn signed_bytes(&self) -> Vec<u8> {
        let mut out =
            Vec::with_capacity(Self::DOMAIN.len() + 1 + 16 + 4 + 2 + 16 * self.members.len());
        out.extend_from_slice(Self::DOMAIN);
        out.push(Self::VERSION);
        out.extend_from_slice(self.root.as_bytes());
        out.extend_from_slice(&self.epoch.to_le_bytes());
        out.extend_from_slice(&(self.members.len() as u16).to_le_bytes());
        for member in &self.members {
            out.extend_from_slice(member.as_bytes());
        }
        out
    }

    /// The confirmed membership snapshot.
    pub fn snapshot(&self) -> MembershipSnapshot {
        MembershipSnapshot {
            epoch: self.epoch,
            members: self.members.clone(),
        }
    }
}

/// An authority refusal.  It is unsigned and only informational: a member
/// logs it and keeps its last confirmed view.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EpochRefusal {
    /// The refusing root.
    pub root: NodeId,
    /// The root's current epoch.
    pub epoch: u32,
    /// The reason for the refusal.
    pub reason: RefusalReason,
    /// The request nonce this refusal answers.
    pub request_nonce: u64,
}

impl EpochRefusal {
    /// Encoded body length.
    pub const LEN: usize = PREFIX + REFUSAL_BODY;

    /// Encode into `out`.
    pub fn encode(&self, out: &mut [u8]) -> Result<usize, WireError> {
        if out.len() < Self::LEN {
            return Err(WireError::OutputTooSmall);
        }
        write_prefix(out, KIND_REFUSAL);
        let mut cursor = PREFIX;
        put_node(out, &mut cursor, &self.root);
        put_u32(out, &mut cursor, self.epoch);
        put_u16(out, &mut cursor, self.reason.as_u16());
        put_u16(out, &mut cursor, 0);
        put_u64(out, &mut cursor, self.request_nonce);
        debug_assert_eq!(cursor, Self::LEN);
        Ok(Self::LEN)
    }

    fn decode(body: &[u8]) -> Result<Self, WireError> {
        if body.len() != REFUSAL_BODY {
            return Err(WireError::LengthMismatch);
        }
        let mut reader = Reader::new(body);
        let root = reader.node()?;
        let epoch = reader.u32()?;
        let reason = RefusalReason::from_u16(reader.u16()?).ok_or(WireError::BadReason)?;
        reader.u16()?; // reserved
        let request_nonce = reader.u64()?;
        if root.is_zero() {
            return Err(WireError::ZeroNode);
        }
        Ok(Self {
            root,
            epoch,
            reason,
            request_nonce,
        })
    }
}

/// The member's terminal acknowledgement of a confirmation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct JoinAck {
    /// The acknowledging member.
    pub node: NodeId,
    /// The installed epoch.
    pub epoch: u32,
    /// The installed snapshot digest.
    pub digest: [u8; 16],
    /// The confirmation's request nonce.
    pub request_nonce: u64,
}

impl JoinAck {
    /// Encoded body length.
    pub const LEN: usize = PREFIX + ACK_BODY;

    /// Encode into `out`.
    pub fn encode(&self, out: &mut [u8]) -> Result<usize, WireError> {
        if out.len() < Self::LEN {
            return Err(WireError::OutputTooSmall);
        }
        write_prefix(out, KIND_ACK);
        let mut cursor = PREFIX;
        put_node(out, &mut cursor, &self.node);
        put_u32(out, &mut cursor, self.epoch);
        put_u32(out, &mut cursor, 0);
        out[cursor..cursor + 16].copy_from_slice(&self.digest);
        cursor += 16;
        put_u64(out, &mut cursor, self.request_nonce);
        debug_assert_eq!(cursor, Self::LEN);
        Ok(Self::LEN)
    }

    fn decode(body: &[u8]) -> Result<Self, WireError> {
        if body.len() != ACK_BODY {
            return Err(WireError::LengthMismatch);
        }
        let mut reader = Reader::new(body);
        let node = reader.node()?;
        let epoch = reader.u32()?;
        reader.u32()?; // reserved
        let mut digest = [0u8; 16];
        digest.copy_from_slice(reader.take(16)?);
        let request_nonce = reader.u64()?;
        if node.is_zero() {
            return Err(WireError::ZeroNode);
        }
        if epoch == 0 {
            return Err(WireError::NonCanonical);
        }
        Ok(Self {
            node,
            epoch,
            digest,
            request_nonce,
        })
    }
}

/// The incarnation the authority confirmed for one member.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MemberGrant {
    /// The confirmed member.
    pub node: NodeId,
    /// The incarnation confirmed for that member.
    pub incarnation: u64,
}

/// A member's liveness summary at the epoch it currently holds.
///
/// The summary is small and unsigned on the wire: its authority comes from the
/// root-signed [`EpochSummary`] that binds `node` and `incarnation` to `epoch`
/// and `digest`.  A receiver validates every field against the signed aggregate
/// it holds, so a member cannot claim an epoch or incarnation the root did not
/// issue, and a replayed summary is rejected by the monotonic `counter`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MemberSummary {
    /// The reporting member.
    pub node: NodeId,
    /// The epoch the member holds.
    pub epoch: u32,
    /// The digest of the member's snapshot.
    pub digest: [u8; 16],
    /// The member's join incarnation.
    pub incarnation: u64,
    /// Monotonic per-incarnation summary counter.
    pub counter: u64,
}

impl MemberSummary {
    /// Encoded body length, including the prefix.
    pub const LEN: usize = PREFIX + 16 + 4 + 16 + 8 + 8;

    /// Encode into `out`.
    pub fn encode(&self, out: &mut [u8]) -> Result<usize, WireError> {
        if out.len() < Self::LEN {
            return Err(WireError::OutputTooSmall);
        }
        write_prefix(out, KIND_SUMMARY);
        let mut cursor = PREFIX;
        put_node(out, &mut cursor, &self.node);
        put_u32(out, &mut cursor, self.epoch);
        out[cursor..cursor + 16].copy_from_slice(&self.digest);
        cursor += 16;
        put_u64(out, &mut cursor, self.incarnation);
        put_u64(out, &mut cursor, self.counter);
        debug_assert_eq!(cursor, Self::LEN);
        Ok(Self::LEN)
    }

    fn decode(body: &[u8]) -> Result<Self, WireError> {
        if body.len() != Self::LEN - PREFIX {
            return Err(WireError::LengthMismatch);
        }
        let mut reader = Reader::new(body);
        let node = reader.node()?;
        let epoch = reader.u32()?;
        let mut digest = [0u8; 16];
        digest.copy_from_slice(reader.take(16)?);
        let incarnation = reader.u64()?;
        let counter = reader.u64()?;
        if node.is_zero() {
            return Err(WireError::ZeroNode);
        }
        if epoch == 0 || counter == 0 {
            return Err(WireError::NonCanonical);
        }
        Ok(Self {
            node,
            epoch,
            digest,
            incarnation,
            counter,
        })
    }
}

/// A root-signed aggregate of the whole membership at one epoch.
///
/// This is the aggregated refresh: one signature and one record per change set,
/// carrying every member's confirmed incarnation.  A member installs the
/// snapshot once, and its periodic [`MemberSummary`] is then verifiable against
/// this record without a per-member authority signature.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EpochSummary {
    /// The authority root that signed the aggregate.
    pub root: NodeId,
    /// The aggregate epoch.
    pub epoch: u32,
    /// Confirmed members and incarnations, in canonical node order.
    pub grants: Vec<MemberGrant>,
    /// Digest of the member list at this epoch.
    pub digest: [u8; 16],
    /// Ed25519 signature over [`EpochSummary::signed_bytes`].
    pub signature: [u8; 64],
}

impl EpochSummary {
    /// The domain separator for the signed aggregate.
    pub const DOMAIN: &'static [u8] = b"IRIX7-M3-EPOCH-SUMMARY-v1";
    /// Fixed body length without the grants, including the prefix.
    pub const FIXED: usize = PREFIX + 16 + 4 + 2 + 2 + 16 + 64;
    /// Encoded length for `members` grants.
    pub const fn len(members: usize) -> usize {
        Self::FIXED + 24 * members
    }
    /// The largest encoded aggregate (a full 128-member view).
    pub const MAX_LEN: usize = Self::len(MAX_MEMBERS);

    /// Build an aggregate from an ordered member list and incarnations.
    ///
    /// The list is canonicalised and cross-checked against `grants`: a grant
    /// for an unknown member, a duplicate, or a missing grant is rejected.
    pub fn new(root: NodeId, epoch: u32, grants: &[MemberGrant]) -> Result<Self, WireError> {
        let nodes: Vec<NodeId> = grants.iter().map(|grant| grant.node).collect();
        let snapshot = MembershipSnapshot::new(epoch, &nodes)?;
        let mut sorted = grants.to_vec();
        sorted.sort_unstable_by_key(|grant| grant.node);
        for pair in sorted.windows(2) {
            if pair[0].node == pair[1].node {
                return Err(WireError::NonCanonicalMembers);
            }
        }
        Ok(Self {
            root,
            epoch,
            digest: snapshot.digest(),
            grants: sorted,
            signature: [0u8; 64],
        })
    }

    /// The snapshot this aggregate authorises.
    pub fn snapshot(&self) -> Result<MembershipSnapshot, WireError> {
        let nodes: Vec<NodeId> = self.grants.iter().map(|grant| grant.node).collect();
        let snapshot = MembershipSnapshot::new(self.epoch, &nodes)?;
        if snapshot.digest() != self.digest {
            return Err(WireError::DigestMismatch);
        }
        Ok(snapshot)
    }

    /// The confirmed incarnation for `node`, if it is a member.
    pub fn grant(&self, node: NodeId) -> Option<u64> {
        self.grants
            .binary_search_by(|grant| grant.node.cmp(&node))
            .ok()
            .map(|index| self.grants[index].incarnation)
    }

    /// The canonical signed bytes.
    pub fn signed_bytes(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(
            Self::DOMAIN.len() + 1 + 16 + 4 + 2 + 2 + 24 * self.grants.len() + 16,
        );
        out.extend_from_slice(Self::DOMAIN);
        out.push(VERSION);
        out.extend_from_slice(self.root.as_bytes());
        out.extend_from_slice(&self.epoch.to_le_bytes());
        out.extend_from_slice(&(self.grants.len() as u16).to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        for grant in &self.grants {
            out.extend_from_slice(grant.node.as_bytes());
            out.extend_from_slice(&grant.incarnation.to_le_bytes());
        }
        out.extend_from_slice(&self.digest);
        out
    }

    /// Encode into `out`.
    pub fn encode(&self, out: &mut [u8]) -> Result<usize, WireError> {
        let len = Self::len(self.grants.len());
        if out.len() < len {
            return Err(WireError::OutputTooSmall);
        }
        if len > MAX_BODY {
            return Err(WireError::TooLong);
        }
        write_prefix(out, KIND_EPOCH);
        let mut cursor = PREFIX;
        put_node(out, &mut cursor, &self.root);
        put_u32(out, &mut cursor, self.epoch);
        put_u16(out, &mut cursor, self.grants.len() as u16);
        put_u16(out, &mut cursor, 0);
        for grant in &self.grants {
            put_node(out, &mut cursor, &grant.node);
            put_u64(out, &mut cursor, grant.incarnation);
        }
        out[cursor..cursor + 16].copy_from_slice(&self.digest);
        cursor += 16;
        out[cursor..cursor + 64].copy_from_slice(&self.signature);
        cursor += 64;
        debug_assert_eq!(cursor, len);
        Ok(len)
    }

    fn decode(body: &[u8]) -> Result<Self, WireError> {
        if body.len() < Self::FIXED - PREFIX {
            return Err(WireError::TooShort);
        }
        let mut reader = Reader::new(body);
        let root = reader.node()?;
        let epoch = reader.u32()?;
        let count = reader.u16()? as usize;
        reader.u16()?; // reserved
        if count == 0 || count > MAX_MEMBERS {
            return Err(WireError::BadCount);
        }
        let expected = Self::len(count) - PREFIX;
        if body.len() != expected {
            return Err(WireError::LengthMismatch);
        }
        let mut grants = Vec::with_capacity(count);
        for _ in 0..count {
            let node = reader.node()?;
            let incarnation = reader.u64()?;
            grants.push(MemberGrant { node, incarnation });
        }
        let mut digest = [0u8; 16];
        digest.copy_from_slice(reader.take(16)?);
        let mut signature = [0u8; 64];
        signature.copy_from_slice(reader.take(64)?);
        if root.is_zero() || epoch == 0 {
            return Err(WireError::NonCanonical);
        }
        for pair in grants.windows(2) {
            if pair[0].node >= pair[1].node {
                return Err(WireError::NonCanonicalMembers);
            }
        }
        let summary = Self {
            root,
            epoch,
            grants,
            digest,
            signature,
        };
        // Recompute the digest so a decoded aggregate cannot claim a member
        // list it does not carry.
        summary.snapshot()?;
        Ok(summary)
    }
}

/// One decoded join body.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum JoinMessage {
    /// A member asks to join.
    Request(JoinRequest),
    /// The root confirms an epoch and member list.
    Confirm(EpochConfirmation),
    /// The root refuses a request.
    Refused(EpochRefusal),
    /// The member acknowledges an installed confirmation.
    Ack(JoinAck),
    /// A member's liveness summary at the epoch it holds.
    Summary(MemberSummary),
    /// The root's signed aggregate for one change set.
    Epoch(EpochSummary),
}

impl JoinMessage {
    /// Decode exactly one body.  Trailing bytes are rejected.
    pub fn decode(body: &[u8]) -> Result<Self, WireError> {
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
        let payload = &body[PREFIX..];
        match body[5] {
            KIND_REQUEST => Ok(Self::Request(JoinRequest::decode(payload)?)),
            KIND_CONFIRM => Ok(Self::Confirm(EpochConfirmation::decode(payload)?)),
            KIND_REFUSAL => Ok(Self::Refused(EpochRefusal::decode(payload)?)),
            KIND_ACK => Ok(Self::Ack(JoinAck::decode(payload)?)),
            KIND_SUMMARY => Ok(Self::Summary(MemberSummary::decode(payload)?)),
            KIND_EPOCH => Ok(Self::Epoch(EpochSummary::decode(payload)?)),
            _ => Err(WireError::BadKind),
        }
    }

    /// Encode into `out`.
    pub fn encode(&self, out: &mut [u8]) -> Result<usize, WireError> {
        match self {
            Self::Request(request) => request.encode(out),
            Self::Confirm(confirmation) => confirmation.encode(out),
            Self::Refused(refusal) => refusal.encode(out),
            Self::Ack(ack) => ack.encode(out),
            Self::Summary(summary) => summary.encode(out),
            Self::Epoch(summary) => summary.encode(out),
        }
    }

    /// A short kind label for logs.
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Request(_) => "request",
            Self::Confirm(_) => "confirm",
            Self::Refused(_) => "refused",
            Self::Ack(_) => "ack",
            Self::Summary(_) => "summary",
            Self::Epoch(_) => "epoch",
        }
    }

    /// The envelope `MessageKind` this body must travel in.
    ///
    /// The join body kind and the frame kind are independent fields; binding
    /// them stops a Response from being treated as a request and vice versa.
    pub fn expected_kind(&self) -> MessageKind {
        match self {
            Self::Request(_) => MessageKind::Request,
            Self::Confirm(_) | Self::Refused(_) => MessageKind::Response,
            Self::Ack(_) => MessageKind::Completion,
            // Gossip is a push stream with no request/response pairing: a
            // summary and an epoch aggregate are both control records.
            Self::Summary(_) | Self::Epoch(_) => MessageKind::Control,
        }
    }
}

/// A canonical, epoch-tagged membership snapshot.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MembershipSnapshot {
    /// Membership epoch.
    pub epoch: u32,
    /// Sorted, unique member list.
    pub members: Vec<NodeId>,
}

impl MembershipSnapshot {
    /// Canonical snapshot magic: `I7MS`.
    pub const MAGIC: &'static [u8; 4] = b"I7MS";
    /// The domain separator for the snapshot digest.
    pub const DOMAIN: &'static [u8] = b"IRIX7-M2-MEMBERSHIP-v1";

    /// Build a canonical snapshot, sorting and rejecting duplicates.
    pub fn new(epoch: u32, members: &[NodeId]) -> Result<Self, WireError> {
        let members = canonical_members(members)?;
        Ok(Self { epoch, members })
    }

    /// Build from a routing view.
    pub fn from_view(view: &mesh_core::MembershipView) -> Result<Self, WireError> {
        Self::new(view.epoch, &view.nodes)
    }

    /// The routing view for this snapshot.
    pub fn view(&self) -> mesh_core::MembershipView {
        mesh_core::MembershipView {
            epoch: self.epoch,
            nodes: self.members.clone(),
        }
    }

    /// Canonical bytes: version, epoch, count and sorted member IDs.
    pub fn canonical_bytes(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(
            Self::DOMAIN.len() + 4 + 1 + 1 + 2 + 4 + 2 + 2 + 16 * self.members.len(),
        );
        out.extend_from_slice(Self::DOMAIN);
        out.extend_from_slice(Self::MAGIC);
        out.push(SNAPSHOT_VERSION);
        out.push(0);
        out.extend_from_slice(&self.epoch.to_le_bytes());
        out.extend_from_slice(&(self.members.len() as u16).to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        for member in &self.members {
            out.extend_from_slice(member.as_bytes());
        }
        out
    }

    /// The first 128 bits of BLAKE3 over the canonical bytes.
    pub fn digest(&self) -> [u8; 16] {
        let hash = blake3::hash(&self.canonical_bytes());
        let mut digest = [0u8; 16];
        digest.copy_from_slice(&hash.as_bytes()[..16]);
        digest
    }

    /// Number of members.
    pub fn len(&self) -> usize {
        self.members.len()
    }

    /// Whether the snapshot is empty (it never should be).
    pub fn is_empty(&self) -> bool {
        self.members.is_empty()
    }
}

/// Sort a member list and reject empty, zero and duplicate entries.
pub fn canonical_members(members: &[NodeId]) -> Result<Vec<NodeId>, WireError> {
    if members.is_empty() {
        return Err(WireError::BadCount);
    }
    if members.len() > MAX_MEMBERS {
        return Err(WireError::BadCount);
    }
    if members.iter().any(|node| node.is_zero()) {
        return Err(WireError::ZeroNode);
    }
    let mut sorted = members.to_vec();
    sorted.sort_unstable();
    for pair in sorted.windows(2) {
        if pair[0] == pair[1] {
            return Err(WireError::NonCanonicalMembers);
        }
    }
    Ok(sorted)
}

fn write_prefix(out: &mut [u8], kind: u8) {
    out[..4].copy_from_slice(&MAGIC.to_le_bytes());
    out[4] = VERSION;
    out[5] = kind;
    out[6..8].copy_from_slice(&0u16.to_le_bytes());
}

fn put_node(out: &mut [u8], cursor: &mut usize, node: &NodeId) {
    out[*cursor..*cursor + 16].copy_from_slice(node.as_bytes());
    *cursor += 16;
}

fn put_u16(out: &mut [u8], cursor: &mut usize, value: u16) {
    out[*cursor..*cursor + 2].copy_from_slice(&value.to_le_bytes());
    *cursor += 2;
}

fn put_u32(out: &mut [u8], cursor: &mut usize, value: u32) {
    out[*cursor..*cursor + 4].copy_from_slice(&value.to_le_bytes());
    *cursor += 4;
}

fn put_u64(out: &mut [u8], cursor: &mut usize, value: u64) {
    out[*cursor..*cursor + 8].copy_from_slice(&value.to_le_bytes());
    *cursor += 8;
}

struct Reader<'a> {
    bytes: &'a [u8],
    offset: usize,
}

impl<'a> Reader<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, offset: 0 }
    }

    fn take(&mut self, count: usize) -> Result<&'a [u8], WireError> {
        let end = self.offset.checked_add(count).ok_or(WireError::TooShort)?;
        let slice = self
            .bytes
            .get(self.offset..end)
            .ok_or(WireError::TooShort)?;
        self.offset = end;
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

    fn u64(&mut self) -> Result<u64, WireError> {
        let bytes = self.take(8)?;
        Ok(u64::from_le_bytes([
            bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5], bytes[6], bytes[7],
        ]))
    }

    fn node(&mut self) -> Result<NodeId, WireError> {
        let bytes = self.take(16)?;
        let mut node = [0u8; 16];
        node.copy_from_slice(bytes);
        Ok(NodeId(node))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    fn node(byte: u8) -> NodeId {
        NodeId([byte; 16])
    }

    fn confirmation() -> EpochConfirmation {
        EpochConfirmation {
            root: node(1),
            epoch: 2,
            members: vec![node(1), node(2)],
            subject: node(2),
            request_nonce: 0x1122_3344_5566_7788,
            incarnation: 0x99aa_bbcc_ddee_ff00,
            signature: [7u8; 64],
        }
    }

    fn sample_record() -> EpochRecord {
        EpochRecord {
            root: node(1),
            epoch: 3,
            members: vec![node(1), node(2)],
            signature: [9u8; 64],
        }
    }

    #[test]
    fn epoch_record_round_trips_exactly() {
        let record = sample_record();
        let mut body = [0u8; MAX_BODY];
        let len = record.encode(&mut body).unwrap();
        assert_eq!(len, 96 + 16 * 2);
        assert_eq!(EpochRecord::decode(&body[..len]), Ok(record));
        // A truncated or extended record is corrupt, not a prefix match.
        assert_eq!(
            EpochRecord::decode(&body[..len - 1]),
            Err(WireError::LengthMismatch)
        );
        assert_eq!(
            EpochRecord::decode(&body[..len + 1]),
            Err(WireError::LengthMismatch)
        );
    }

    #[test]
    fn epoch_record_rejects_noncanonical_fields() {
        let mut record = sample_record();
        record.members = vec![node(2), node(1)];
        let mut body = [0u8; MAX_BODY];
        assert_eq!(
            record.encode(&mut body),
            Err(WireError::NonCanonicalMembers)
        );

        let mut record = sample_record();
        record.epoch = 0;
        let mut body = [0u8; MAX_BODY];
        assert_eq!(record.encode(&mut body), Err(WireError::NonCanonical));

        let mut record = sample_record();
        record.root = NodeId::ZERO;
        let mut body = [0u8; MAX_BODY];
        assert_eq!(record.encode(&mut body), Err(WireError::ZeroNode));
    }

    #[test]
    fn epoch_record_signed_bytes_bind_epoch_and_members() {
        let record = sample_record();
        let mut other = record.clone();
        other.epoch = 4;
        assert_ne!(record.signed_bytes(), other.signed_bytes());
        let mut other = record.clone();
        other.members = vec![node(1), node(2), node(3)];
        assert_ne!(record.signed_bytes(), other.signed_bytes());
        let mut other = record;
        other.signature = [0u8; 64];
        // The signature is not part of the bytes it covers.
        assert_eq!(sample_record().signed_bytes(), other.signed_bytes());
    }

    #[test]
    fn request_round_trips_exactly() {
        let request = JoinRequest {
            node: node(2),
            incarnation: 9,
            last_epoch: 1,
            last_digest: [3u8; 16],
            nonce: 42,
        };
        let mut body = [0u8; MAX_BODY];
        assert_eq!(request.encode(&mut body), Ok(JoinRequest::LEN));
        assert_eq!(JoinRequest::LEN, 64);
        assert_eq!(
            JoinMessage::decode(&body[..JoinRequest::LEN]),
            Ok(JoinMessage::Request(request))
        );
        // Trailing bytes are not a second message.
        assert_eq!(
            JoinMessage::decode(&body[..JoinRequest::LEN + 1]),
            Err(WireError::LengthMismatch)
        );
    }

    #[test]
    fn confirmation_round_trips_and_length_is_exact() {
        let confirmation = confirmation();
        let mut body = [0u8; MAX_BODY];
        let len = confirmation.encode(&mut body).expect("encode");
        assert_eq!(len, 128 + 2 * 16);
        assert_eq!(
            JoinMessage::decode(&body[..len]),
            Ok(JoinMessage::Confirm(confirmation.clone()))
        );
        assert_eq!(
            confirmation.snapshot().digest(),
            confirmation.snapshot().digest()
        );
    }

    #[test]
    fn ack_and_refusal_round_trip() {
        let ack = JoinAck {
            node: node(2),
            epoch: 2,
            digest: [4u8; 16],
            request_nonce: 5,
        };
        let mut body = [0u8; MAX_BODY];
        let len = ack.encode(&mut body).expect("ack");
        assert_eq!(len, JoinAck::LEN);
        assert_eq!(JoinMessage::decode(&body[..len]), Ok(JoinMessage::Ack(ack)));

        let refusal = EpochRefusal {
            root: node(1),
            epoch: 1,
            reason: RefusalReason::EpochRegression,
            request_nonce: 5,
        };
        let len = refusal.encode(&mut body).expect("refusal");
        assert_eq!(len, EpochRefusal::LEN);
        assert_eq!(
            JoinMessage::decode(&body[..len]),
            Ok(JoinMessage::Refused(refusal))
        );
    }

    #[test]
    fn body_kinds_bind_to_envelope_kinds() {
        let request = JoinMessage::Request(JoinRequest {
            node: node(2),
            incarnation: 1,
            last_epoch: 1,
            last_digest: [0u8; 16],
            nonce: 2,
        });
        assert_eq!(request.expected_kind(), MessageKind::Request);
        assert_eq!(
            JoinMessage::Confirm(confirmation()).expected_kind(),
            MessageKind::Response
        );
        assert_eq!(
            JoinMessage::Refused(EpochRefusal {
                root: node(1),
                epoch: 1,
                reason: RefusalReason::Busy,
                request_nonce: 1,
            })
            .expected_kind(),
            MessageKind::Response
        );
        assert_eq!(
            JoinMessage::Ack(JoinAck {
                node: node(2),
                epoch: 2,
                digest: [1u8; 16],
                request_nonce: 2,
            })
            .expected_kind(),
            MessageKind::Completion
        );
    }

    #[test]
    fn bad_header_fields_are_rejected() {
        let confirmation = confirmation();
        let mut body = [0u8; MAX_BODY];
        let len = confirmation.encode(&mut body).expect("encode");
        let mut magic = body;
        magic[0] ^= 0xff;
        assert_eq!(JoinMessage::decode(&magic[..len]), Err(WireError::BadMagic));
        let mut version = body;
        version[4] = 9;
        assert_eq!(
            JoinMessage::decode(&version[..len]),
            Err(WireError::BadVersion)
        );
        let mut kind = body;
        kind[5] = 9;
        assert_eq!(JoinMessage::decode(&kind[..len]), Err(WireError::BadKind));
        let mut flags = body;
        flags[6] = 1;
        assert_eq!(
            JoinMessage::decode(&flags[..len]),
            Err(WireError::NonCanonical)
        );
    }

    #[test]
    fn member_lists_must_be_canonical() {
        let mut confirmation = confirmation();
        let mut body = [0u8; MAX_BODY];
        confirmation.members = vec![node(2), node(1)];
        assert_eq!(
            confirmation.encode(&mut body),
            Err(WireError::NonCanonicalMembers)
        );

        let mut duplicate = confirmation.clone();
        duplicate.members = vec![node(1), node(1)];
        assert_eq!(
            duplicate.encode(&mut body),
            Err(WireError::NonCanonicalMembers)
        );

        // A zero member is caught by the decoder, which also covers a wire
        // body produced by a peer rather than this encoder.
        let mut zero = confirmation.clone();
        zero.members = vec![NodeId::ZERO, node(1)];
        let len = zero.encode(&mut body).expect("encode");
        assert_eq!(JoinMessage::decode(&body[..len]), Err(WireError::ZeroNode));
    }

    #[test]
    fn truncated_and_oversize_bodies_are_rejected() {
        assert_eq!(JoinMessage::decode(&[0u8; 4]), Err(WireError::TooShort));
        assert_eq!(
            JoinMessage::decode(&vec![0u8; MAX_BODY + 1]),
            Err(WireError::TooLong)
        );
        let confirmation = confirmation();
        let mut body = [0u8; MAX_BODY];
        let len = confirmation.encode(&mut body).expect("encode");
        assert_eq!(
            JoinMessage::decode(&body[..len - 1]),
            Err(WireError::LengthMismatch)
        );
    }

    #[test]
    fn snapshot_digest_is_deterministic_and_order_independent() {
        let first = MembershipSnapshot::new(2, &[node(2), node(1)]).expect("snapshot");
        let second = MembershipSnapshot::new(2, &[node(1), node(2)]).expect("snapshot");
        assert_eq!(first.members, vec![node(1), node(2)]);
        assert_eq!(first.digest(), second.digest());
        let higher = MembershipSnapshot::new(3, &[node(1), node(2)]).expect("snapshot");
        assert_ne!(first.digest(), higher.digest());
        assert_eq!(
            MembershipSnapshot::new(1, &[node(1), node(1)]),
            Err(WireError::NonCanonicalMembers)
        );
        assert_eq!(MembershipSnapshot::new(1, &[]), Err(WireError::BadCount));
    }

    #[test]
    fn snapshot_digest_has_a_fixed_vector() {
        // A fixed vector pins the canonical bytes: any layout change is a
        // protocol change and must bump the snapshot version and the ADR.
        let snapshot = MembershipSnapshot::new(2, &[node(1), node(2)]).expect("snapshot");
        let digest = snapshot.digest();
        assert_eq!(
            digest,
            [
                0x50, 0xd2, 0xf9, 0x69, 0x6e, 0x00, 0x25, 0x94, 0x37, 0xdf, 0x3a, 0x5a, 0xcc, 0xeb,
                0x0b, 0xef,
            ],
            "canonical snapshot digest changed: {:02x?}",
            digest
        );
    }
}
