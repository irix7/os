// SPDX-License-Identifier: GPL-3.0-only
//! The `I7M1` migration wire bodies and the root-signed `ActorMove` directive.
//!
//! Every body is field-by-field little-endian and bounded by [`MAX_BODY`].
//! The M2-1 `MeshEnvelope` framing, CRC and 64 KiB record bound are unchanged;
//! this module lowers the per-body bound before decoding.

use alloc::vec::Vec;

use sys_primitives::{
    mesh_flags, ActorId, Capability, MeshHeader, MessageKind, NodeId, NodeKey, Status,
};

use crate::{
    capability_bytes, Error, Message, Reader, Writer, MAGIC, MAX_BODY, MAX_LEASE_MS,
    MAX_STATE_BYTES, MOVE_DOMAIN, SCHEMA, STATE_LEN, VERSION,
};

/// Fixed length of a root-signed directive.
pub const DIRECTIVE_LEN: usize = 164;
/// Body prefix: magic u32, version u8, kind u8, flags u16.
pub const PREFIX: usize = 8;

/// Authorise a source to start a move.  Root -> source.
pub const KIND_AUTHORISE: u8 = 1;
/// Source -> destination reservation request.
pub const KIND_PREPARE: u8 = 2;
/// Destination -> source reservation response.
pub const KIND_READY: u8 = 3;
/// Source -> destination state transfer.
pub const KIND_STATE: u8 = 4;
/// Destination -> source fencing order.
pub const KIND_COMMIT: u8 = 5;
/// Either side aborts before commit.
pub const KIND_ABORT: u8 = 6;
/// Source -> destination after fencing.
pub const KIND_ACK: u8 = 7;
/// Deliver one message to the current host.
pub const KIND_DELIVER: u8 = 8;
/// Host -> sender delivery result.
pub const KIND_RESULT: u8 = 9;
/// Resume the queued message at the current host.
pub const KIND_RESUME: u8 = 10;
/// A terminal refusal.
pub const KIND_ERROR: u8 = 11;

/// A root-signed actor move.
///
/// It names the confirmed membership epoch and digest, the actor's rendezvous
/// home, the old and new execution hosts, and the placement epoch.  The
/// signature is checked before any field is trusted.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ActorMove {
    /// Actor identity.
    pub actor: ActorId,
    /// Rendezvous actor home.
    pub home: NodeId,
    /// Host that currently holds the actor.
    pub old_host: NodeId,
    /// Host that will hold the actor.
    pub new_host: NodeId,
    /// Confirmed membership epoch.
    pub membership_epoch: u32,
    /// Confirmed membership digest.
    pub membership_digest: [u8; 16],
    /// Placement epoch before the move.
    pub old_epoch: u32,
    /// Placement epoch after the move (`old_epoch + 1`).
    pub new_epoch: u32,
    /// State schema expected by the source and destination.
    pub schema: u32,
    /// Lease on the move, in milliseconds.
    pub lease_ms: u32,
    /// Ed25519 signature from the authority root.
    pub signature: [u8; 64],
}

impl ActorMove {
    /// Build an unsigned directive.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        actor: ActorId,
        home: NodeId,
        old_host: NodeId,
        new_host: NodeId,
        membership_epoch: u32,
        membership_digest: [u8; 16],
        old_epoch: u32,
        lease_ms: u32,
    ) -> Self {
        Self {
            actor,
            home,
            old_host,
            new_host,
            membership_epoch,
            membership_digest,
            old_epoch,
            new_epoch: old_epoch.saturating_add(1),
            schema: SCHEMA,
            lease_ms,
            signature: [0; 64],
        }
    }

    /// Canonical bytes covered by the authority signature.
    pub fn signed_bytes(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(PREFIX + DIRECTIVE_LEN - 64);
        out.extend_from_slice(&MAGIC.to_le_bytes());
        out.push(VERSION);
        out.extend_from_slice(MOVE_DOMAIN);
        let mut body = [0u8; DIRECTIVE_LEN - 64];
        let mut writer = Writer::new(&mut body);
        let _ = writer.bytes(&self.actor.0);
        let _ = writer.bytes(&self.home.0);
        let _ = writer.bytes(&self.old_host.0);
        let _ = writer.bytes(&self.new_host.0);
        let _ = writer.u32(self.membership_epoch);
        let _ = writer.bytes(&self.membership_digest);
        let _ = writer.u32(self.old_epoch);
        let _ = writer.u32(self.new_epoch);
        let _ = writer.u32(self.schema);
        let _ = writer.u32(self.lease_ms);
        let len = writer.len();
        out.extend_from_slice(&body[..len]);
        out
    }

    /// Sign the directive with the authority root.
    pub fn sign(&mut self, signer: &dyn crate::Signer) -> Result<(), Error> {
        let signature = signer.sign(&self.signed_bytes()).ok_or(Error::NotRoot)?;
        self.signature = signature;
        Ok(())
    }

    /// Verify the signature and the directive shape.
    pub fn verify(&self, authority: &NodeKey) -> Result<(), Error> {
        cap_engine::verify(authority, &self.signed_bytes(), &self.signature)
            .map_err(|_| Error::BadSignature)?;
        if self.actor.0.iter().all(|&byte| byte == 0)
            || self.home.is_zero()
            || self.old_host.is_zero()
            || self.new_host.is_zero()
            || self.old_host == self.new_host
            || self.membership_epoch == 0
            || self.old_epoch == 0
            || self.new_epoch != self.old_epoch.saturating_add(1)
            || self.schema != SCHEMA
            || self.lease_ms == 0
            || self.lease_ms > MAX_LEASE_MS
        {
            return Err(Error::BadMove);
        }
        Ok(())
    }

    /// Encode the fixed directive.
    pub fn encode(&self, out: &mut [u8]) -> Result<usize, Error> {
        if out.len() < DIRECTIVE_LEN {
            return Err(Error::BadLength);
        }
        let mut writer = Writer::new(out);
        writer.bytes(&self.actor.0)?;
        writer.bytes(&self.home.0)?;
        writer.bytes(&self.old_host.0)?;
        writer.bytes(&self.new_host.0)?;
        writer.u32(self.membership_epoch)?;
        writer.bytes(&self.membership_digest)?;
        writer.u32(self.old_epoch)?;
        writer.u32(self.new_epoch)?;
        writer.u32(self.schema)?;
        writer.u32(self.lease_ms)?;
        writer.bytes(&self.signature)?;
        Ok(writer.len())
    }

    /// Decode a fixed directive without verifying its signature.
    pub fn decode(bytes: &[u8]) -> Result<Self, Error> {
        if bytes.len() != DIRECTIVE_LEN {
            return Err(Error::BadLength);
        }
        let mut reader = Reader::new(bytes);
        let actor = reader.actor()?;
        let home = reader.node()?;
        let old_host = reader.node()?;
        let new_host = reader.node()?;
        let membership_epoch = reader.u32()?;
        let mut membership_digest = [0u8; 16];
        membership_digest.copy_from_slice(reader.take(16)?);
        let old_epoch = reader.u32()?;
        let new_epoch = reader.u32()?;
        let schema = reader.u32()?;
        let lease_ms = reader.u32()?;
        let mut signature = [0u8; 64];
        signature.copy_from_slice(reader.take(64)?);
        if !reader.done() {
            return Err(Error::BadLength);
        }
        Ok(Self {
            actor,
            home,
            old_host,
            new_host,
            membership_epoch,
            membership_digest,
            old_epoch,
            new_epoch,
            schema,
            lease_ms,
            signature,
        })
    }
}

/// One decoded `I7M1` body.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Body {
    /// Root authorises a source to start a move.
    Authorise(ActorMove),
    /// Source requests a destination reservation.
    Prepare {
        /// Signed move.
        directive: ActorMove,
        /// Length of the state that will follow.
        state_len: u16,
        /// Expected canonical state hash.
        hash: [u8; 32],
    },
    /// Destination reserves the actor.
    Ready {
        /// Signed move.
        directive: ActorMove,
        /// Expected canonical state hash.
        hash: [u8; 32],
    },
    /// Source transfers the canonical state.
    State {
        /// Signed move.
        directive: ActorMove,
        /// Canonical actor-state bytes.
        state: Vec<u8>,
    },
    /// Destination confirms installation and orders the source to fence.
    Commit {
        /// Signed move.
        directive: ActorMove,
        /// Installed canonical state hash.
        hash: [u8; 32],
    },
    /// Either side aborts before commit.
    Abort {
        /// Signed move.
        directive: ActorMove,
        /// Why the move was abandoned.
        reason: Error,
    },
    /// Source fenced the old instance.
    Ack {
        /// Signed move.
        directive: ActorMove,
        /// Fenced canonical state hash.
        hash: [u8; 32],
    },
    /// Deliver one message to the actor's current host.
    Deliver {
        /// Actor identity.
        actor: ActorId,
        /// Current placement epoch named by the sender.
        epoch: u32,
        /// Root-issued `INVOKE` capability.
        capability: Capability,
        /// Message to deliver.
        message: Message,
        /// `0` step now, `1` queue only.
        mode: u8,
    },
    /// The host's answer to a delivery or resume.
    Result {
        /// Actor identity.
        actor: ActorId,
        /// Placement epoch at the host.
        epoch: u32,
        /// Message id.
        message: u64,
        /// Applied-message counter.
        applied: u64,
        /// Logical digest.
        logical: [u8; 32],
        /// Canonical state hash.
        hash: [u8; 32],
        /// Whether the message was an idempotent duplicate.
        duplicate: bool,
    },
    /// Resume the queued message at the actor's current host.
    Resume {
        /// Actor identity.
        actor: ActorId,
        /// Current placement epoch named by the sender.
        epoch: u32,
        /// Root-issued `INVOKE` capability.
        capability: Capability,
    },
    /// A terminal refusal.
    Error {
        /// Actor identity.
        actor: ActorId,
        /// Placement epoch at the refusing node.
        epoch: u32,
        /// Why.
        reason: Error,
        /// The epoch the refusal expected, when relevant.
        expected_epoch: u32,
    },
}

impl Body {
    /// The body kind byte.
    pub const fn kind(&self) -> u8 {
        match self {
            Self::Authorise(_) => KIND_AUTHORISE,
            Self::Prepare { .. } => KIND_PREPARE,
            Self::Ready { .. } => KIND_READY,
            Self::State { .. } => KIND_STATE,
            Self::Commit { .. } => KIND_COMMIT,
            Self::Abort { .. } => KIND_ABORT,
            Self::Ack { .. } => KIND_ACK,
            Self::Deliver { .. } => KIND_DELIVER,
            Self::Result { .. } => KIND_RESULT,
            Self::Resume { .. } => KIND_RESUME,
            Self::Error { .. } => KIND_ERROR,
        }
    }

    /// The frame kind this body must travel in.
    pub const fn message_kind(&self) -> MessageKind {
        match self {
            Self::Authorise(_)
            | Self::Prepare { .. }
            | Self::State { .. }
            | Self::Deliver { .. } => MessageKind::Request,
            Self::Ready { .. }
            | Self::Commit { .. }
            | Self::Abort { .. }
            | Self::Result { .. }
            | Self::Error { .. } => MessageKind::Response,
            Self::Ack { .. } => MessageKind::Completion,
            Self::Resume { .. } => MessageKind::Request,
        }
    }

    /// Encode the body.
    pub fn encode(&self, out: &mut [u8]) -> Result<usize, Error> {
        if out.len() < MAX_BODY {
            return Err(Error::BadLength);
        }
        out[..MAX_BODY].fill(0);
        let mut writer = Writer::new(out);
        writer.u32(MAGIC)?;
        writer.u8(VERSION)?;
        writer.u8(self.kind())?;
        writer.u16(0)?;
        match self {
            Self::Authorise(directive) => {
                let mut buffer = [0u8; DIRECTIVE_LEN];
                let len = directive.encode(&mut buffer)?;
                writer.bytes(&buffer[..len])?;
            }
            Self::Prepare {
                directive,
                state_len,
                hash,
            } => {
                let mut buffer = [0u8; DIRECTIVE_LEN];
                let len = directive.encode(&mut buffer)?;
                writer.bytes(&buffer[..len])?;
                writer.u16(*state_len)?;
                writer.bytes(hash)?;
            }
            Self::Ready { directive, hash }
            | Self::Commit { directive, hash }
            | Self::Ack { directive, hash } => {
                let mut buffer = [0u8; DIRECTIVE_LEN];
                let len = directive.encode(&mut buffer)?;
                writer.bytes(&buffer[..len])?;
                writer.bytes(hash)?;
            }
            Self::State { directive, state } => {
                if state.len() != STATE_LEN || state.len() > MAX_STATE_BYTES {
                    return Err(Error::BadLength);
                }
                let mut buffer = [0u8; DIRECTIVE_LEN];
                let len = directive.encode(&mut buffer)?;
                writer.bytes(&buffer[..len])?;
                writer.bytes(state)?;
            }
            Self::Abort { directive, reason } => {
                let mut buffer = [0u8; DIRECTIVE_LEN];
                let len = directive.encode(&mut buffer)?;
                writer.bytes(&buffer[..len])?;
                writer.u16(reason.as_u16())?;
            }
            Self::Deliver {
                actor,
                epoch,
                capability,
                message,
                mode,
            } => {
                writer.bytes(&actor.0)?;
                writer.u32(*epoch)?;
                let encoded = capability_bytes(capability);
                writer.bytes(&encoded)?;
                writer.u64(message.id)?;
                writer.bytes(&message.payload)?;
                writer.u8(*mode)?;
            }
            Self::Result {
                actor,
                epoch,
                message,
                applied,
                logical,
                hash,
                duplicate,
            } => {
                writer.bytes(&actor.0)?;
                writer.u32(*epoch)?;
                writer.u64(*message)?;
                writer.u64(*applied)?;
                writer.bytes(logical)?;
                writer.bytes(hash)?;
                writer.u8(if *duplicate { 1 } else { 0 })?;
            }
            Self::Resume {
                actor,
                epoch,
                capability,
            } => {
                writer.bytes(&actor.0)?;
                writer.u32(*epoch)?;
                let encoded = capability_bytes(capability);
                writer.bytes(&encoded)?;
            }
            Self::Error {
                actor,
                epoch,
                reason,
                expected_epoch,
            } => {
                writer.bytes(&actor.0)?;
                writer.u32(*epoch)?;
                writer.u16(reason.as_u16())?;
                writer.u32(*expected_epoch)?;
            }
        }
        Ok(writer.len())
    }

    /// Decode one body and enforce its exact length.
    pub fn decode(bytes: &[u8]) -> Result<Self, Error> {
        if bytes.len() > MAX_BODY {
            return Err(Error::BadLength);
        }
        let mut reader = Reader::new(bytes);
        if reader.u32()? != MAGIC {
            return Err(Error::BadMagic);
        }
        if reader.u8()? != VERSION {
            return Err(Error::BadVersion);
        }
        let kind = reader.u8()?;
        if reader.u16()? != 0 {
            return Err(Error::BadFlags);
        }
        let body = match kind {
            KIND_AUTHORISE => Self::Authorise(read_directive(&mut reader)?),
            KIND_PREPARE => {
                let directive = read_directive(&mut reader)?;
                let state_len = reader.u16()?;
                if state_len as usize != STATE_LEN {
                    return Err(Error::BadLength);
                }
                let hash = reader.digest()?;
                Self::Prepare {
                    directive,
                    state_len,
                    hash,
                }
            }
            KIND_READY => Self::Ready {
                directive: read_directive(&mut reader)?,
                hash: reader.digest()?,
            },
            KIND_STATE => {
                let directive = read_directive(&mut reader)?;
                let state = reader.rest();
                if state.len() != STATE_LEN {
                    return Err(Error::BadLength);
                }
                Self::State {
                    directive,
                    state: state.to_vec(),
                }
            }
            KIND_COMMIT => Self::Commit {
                directive: read_directive(&mut reader)?,
                hash: reader.digest()?,
            },
            KIND_ABORT => Self::Abort {
                directive: read_directive(&mut reader)?,
                reason: Error::from_u16(reader.u16()?).ok_or(Error::BadKind)?,
            },
            KIND_ACK => Self::Ack {
                directive: read_directive(&mut reader)?,
                hash: reader.digest()?,
            },
            KIND_DELIVER => Self::Deliver {
                actor: reader.actor()?,
                epoch: reader.u32()?,
                capability: read_capability(&mut reader)?,
                message: Message {
                    id: reader.u64()?,
                    payload: {
                        let mut payload = [0u8; 32];
                        payload.copy_from_slice(reader.take(32)?);
                        payload
                    },
                },
                mode: reader.u8()?,
            },
            KIND_RESULT => Self::Result {
                actor: reader.actor()?,
                epoch: reader.u32()?,
                message: reader.u64()?,
                applied: reader.u64()?,
                logical: reader.digest()?,
                hash: reader.digest()?,
                duplicate: match reader.u8()? {
                    0 => false,
                    1 => true,
                    _ => return Err(Error::NonCanonical),
                },
            },
            KIND_RESUME => Self::Resume {
                actor: reader.actor()?,
                epoch: reader.u32()?,
                capability: read_capability(&mut reader)?,
            },
            KIND_ERROR => Self::Error {
                actor: reader.actor()?,
                epoch: reader.u32()?,
                reason: Error::from_u16(reader.u16()?).ok_or(Error::BadKind)?,
                expected_epoch: reader.u32()?,
            },
            _ => return Err(Error::BadKind),
        };
        if !reader.done() {
            return Err(Error::BadLength);
        }
        Ok(body)
    }
}

fn read_directive(reader: &mut Reader<'_>) -> Result<ActorMove, Error> {
    let bytes = reader.take(DIRECTIVE_LEN)?;
    ActorMove::decode(bytes)
}

fn read_capability(reader: &mut Reader<'_>) -> Result<Capability, Error> {
    let bytes = reader.take(320)?;
    Capability::decode(bytes).map_err(|_| Error::Capability)
}

/// One outbound migration record, before the module wraps it in an envelope.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Wire {
    /// Destination host.
    pub dst: NodeId,
    /// Frame kind.
    pub kind: MessageKind,
    /// Frame status.
    pub status: Status,
    /// Source membership epoch.
    pub src_epoch: u32,
    /// Destination membership epoch.
    pub dst_epoch: u32,
    /// Actor rendezvous home.
    pub home: NodeId,
    /// Placement epoch named by the frame.
    pub home_epoch: u32,
    /// Actor identity.
    pub actor: ActorId,
    /// Request id at the sender.
    pub request_id: u64,
    /// Request id this frame answers.
    pub reply_to: u64,
    /// Encoded `I7M1` body.
    pub body: Vec<u8>,
}

impl Wire {
    /// Build the routing header for this record.
    pub fn header(&self, src: NodeId) -> MeshHeader {
        let mut header = MeshHeader::new(src, self.dst, self.home);
        header.flags = mesh_flags::MIGRATE;
        if self.kind != MessageKind::Request {
            header.flags |= mesh_flags::REPLY;
        }
        header.src_epoch = self.src_epoch;
        header.dst_epoch = self.dst_epoch;
        header.home_epoch = self.home_epoch;
        header.object_epoch = self.home_epoch;
        header.request_id = self.request_id;
        header.reply_to = self.reply_to;
        header.ttl = MeshHeader::MAX_HOPS;
        header
    }
}
