// SPDX-License-Identifier: GPL-3.0-only
//! `sys_primitives` — the shared vocabulary of the IRIX 7 object fabric.
//!
//! This crate is deliberately boring: `#![no_std]`, no allocation, no
//! `unsafe`, no hardware, no cryptography.  It defines the bytes that the
//! native shim, the WebAssembly core, the migration format and the 9P gateway
//! all agree on.
//!
//! ## Wire contract
//!
//! * The fabric header is **little-endian**.  This is a deliberate choice:
//!   the WebAssembly abstract machine stores integers little-endian, and the
//!   core is the one component that must run on every target.  See
//!   `ENDIAN.md` for the full matrix and for the gateway conversions.
//! * `FrameHeader` is exactly 128 bytes and `PayloadDescriptor` is exactly 64
//!   bytes.  Both are encoded field-by-field; the `#[repr(C)]` layout is an
//!   in-memory convenience, not the wire format.
//! * Capability tokens are a fixed 320-byte canonical layout.  There is no
//!   CBOR decoder in the TCB for security tokens; fixed offsets and strict
//!   reserved-field checks remove a parser attack surface.
//! * All identifiers are opaque byte strings.  `ObjectId` is a 32-byte
//!   BLAKE3-256 digest; it is never interpreted as an integer.

#![no_std]
#![forbid(unsafe_code)]
#![deny(missing_docs)]

use core::fmt;

// ---------------------------------------------------------------------------
// Errors and status
// ---------------------------------------------------------------------------

/// A decode or verification failure.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    /// The input ended before a complete field.
    Truncated,
    /// A magic value did not match.
    BadMagic,
    /// A version is not understood.
    BadVersion,
    /// A length field is inconsistent with the buffer.
    BadLength,
    /// The header checksum did not match.
    BadCrc,
    /// An enum discriminant is reserved or unknown.
    BadKind,
    /// A status code is not understood.
    BadStatus,
    /// A rights bitmap contains a reserved bit.
    BadRights,
    /// A reserved field is non-zero.
    NonCanonical,
    /// A protocol invariant was violated (hop count, route, epoch).
    Protocol,
    /// The signature did not verify.
    BadSignature,
    /// The token is not yet valid.
    NotYetValid,
    /// The token has expired.
    Expired,
    /// The token names a different object.
    WrongObject,
    /// The token does not carry the requested rights.
    InsufficientRights,
    /// The token has been revoked.
    Revoked,
    /// The token has already been used in this epoch.
    Replay,
    /// A capability handle is stale or was never allocated.
    StaleHandle,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            Self::Truncated => "truncated",
            Self::BadMagic => "bad magic",
            Self::BadVersion => "bad version",
            Self::BadLength => "bad length",
            Self::BadCrc => "bad checksum",
            Self::BadKind => "bad kind",
            Self::BadStatus => "bad status",
            Self::BadRights => "bad rights",
            Self::NonCanonical => "non-canonical encoding",
            Self::Protocol => "protocol violation",
            Self::BadSignature => "bad signature",
            Self::NotYetValid => "not yet valid",
            Self::Expired => "expired",
            Self::WrongObject => "wrong object",
            Self::InsufficientRights => "insufficient rights",
            Self::Revoked => "revoked",
            Self::Replay => "replay",
            Self::StaleHandle => "stale handle",
        };
        f.write_str(s)
    }
}

/// A completion status.  Negative values are errors, following the `io_uring`
/// and Linux `errno` convention; `0` is success.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(i32)]
pub enum Status {
    /// Success.
    Ok = 0,
    /// The operation was cancelled before it committed.
    Cancelled = -1,
    /// The operation exceeded its deadline.
    TimedOut = -2,
    /// The resource is temporarily unavailable; retry later.
    Again = -3,
    /// Memory or buffer quota exhausted.
    NoMemory = -4,
    /// The capability does not permit the operation.
    Permission = -5,
    /// The object or name does not exist.
    NotFound = -6,
    /// The object is busy (locked, open exclusively, migrating).
    Busy = -7,
    /// The channel or actor is gone.
    Broken = -8,
    /// The peer violated the protocol.
    Protocol = -9,
    /// The operation is not supported by this shim or object.
    NotSupported = -10,
    /// The backing device reported an I/O error.
    Io = -11,
    /// A generation or epoch is stale.
    Stale = -12,
    /// Authority was revoked.
    Revoked = -13,
    /// A quota or flow-control window is exhausted.
    OverQuota = -14,
}

impl Status {
    /// Decode a status from the wire representation.
    pub const fn from_i32(value: i32) -> Result<Self, Error> {
        match value {
            0 => Ok(Self::Ok),
            -1 => Ok(Self::Cancelled),
            -2 => Ok(Self::TimedOut),
            -3 => Ok(Self::Again),
            -4 => Ok(Self::NoMemory),
            -5 => Ok(Self::Permission),
            -6 => Ok(Self::NotFound),
            -7 => Ok(Self::Busy),
            -8 => Ok(Self::Broken),
            -9 => Ok(Self::Protocol),
            -10 => Ok(Self::NotSupported),
            -11 => Ok(Self::Io),
            -12 => Ok(Self::Stale),
            -13 => Ok(Self::Revoked),
            -14 => Ok(Self::OverQuota),
            _ => Err(Error::BadStatus),
        }
    }

    /// Whether this status is success.
    pub const fn is_ok(self) -> bool {
        matches!(self, Self::Ok)
    }
}

// ---------------------------------------------------------------------------
// Identifiers
// ---------------------------------------------------------------------------

/// A 256-bit BLAKE3 object identifier.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(transparent)]
pub struct ObjectId(pub [u8; 32]);

impl ObjectId {
    /// The all-zero identifier.  Never a valid object.
    pub const ZERO: Self = Self([0; 32]);

    /// Wrap 32 bytes.
    pub const fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    /// Borrow the bytes.
    pub const fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }

    /// Copy from a slice of exactly 32 bytes.
    pub fn from_slice(bytes: &[u8]) -> Result<Self, Error> {
        if bytes.len() != 32 {
            return Err(Error::BadLength);
        }
        let mut out = [0u8; 32];
        out.copy_from_slice(bytes);
        Ok(Self(out))
    }

    /// Whether this is the all-zero identifier.
    pub const fn is_zero(&self) -> bool {
        let mut i = 0;
        while i < 32 {
            if self.0[i] != 0 {
                return false;
            }
            i += 1;
        }
        true
    }

    /// Lower-case hex into a caller-provided 64-byte buffer.
    pub fn to_hex(&self, out: &mut [u8; 64]) {
        const HEX: &[u8; 16] = b"0123456789abcdef";
        for (i, byte) in self.0.iter().enumerate() {
            out[i * 2] = HEX[(byte >> 4) as usize];
            out[i * 2 + 1] = HEX[(byte & 0x0f) as usize];
        }
    }
}

impl fmt::Debug for ObjectId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut hex = [0u8; 64];
        self.to_hex(&mut hex);
        // The buffer is ASCII by construction.
        f.write_str(core::str::from_utf8(&hex).unwrap_or("<oid>"))
    }
}

/// Semantic domain of an object.  The kind is mixed into the BLAKE3 hash so
/// identical bytes in different domains cannot alias.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum ObjectKind {
    /// Opaque bytes.
    Blob = 1,
    /// A content-defined chunk.
    Chunk = 2,
    /// A chunk list and content metadata.
    Manifest = 3,
    /// A directory or namespace node.
    Tree = 4,
    /// A validated WebAssembly module plus manifest.
    ActorImage = 5,
    /// A serialized actor state.
    ActorState = 6,
    /// A capability token object (for audit and delegation).
    Capability = 7,
    /// A checkpoint or snapshot envelope.
    Snapshot = 8,
    /// A boot or runtime configuration document.
    Config = 9,
}

/// A node's long-term Ed25519 public key.  The private key never leaves the
/// node's shim.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(transparent)]
pub struct NodeKey(pub [u8; 32]);

/// A compact 128-bit node identity, derived as `BLAKE3-128(NodeKey)`.
///
/// The mesh routes with 16-byte IDs so the routing table for 128 nodes is
/// small.  The full public key is used only for signature verification and
/// authority manifests.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
#[repr(transparent)]
pub struct NodeId(pub [u8; 16]);

impl NodeId {
    /// The all-zero ID.  Never a valid node.
    pub const ZERO: Self = Self([0; 16]);

    /// Wrap 16 bytes.
    pub const fn from_bytes(bytes: [u8; 16]) -> Self {
        Self(bytes)
    }

    /// Borrow the bytes.
    pub const fn as_bytes(&self) -> &[u8; 16] {
        &self.0
    }

    /// Whether this is the all-zero ID.
    pub const fn is_zero(&self) -> bool {
        let mut i = 0;
        while i < 16 {
            if self.0[i] != 0 {
                return false;
            }
            i += 1;
        }
        true
    }
}

/// A principal identity: a user, service or actor subject key.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(transparent)]
pub struct PrincipalId(pub [u8; 32]);

/// A globally unique actor instance identity, independent of its location.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(transparent)]
pub struct ActorId(pub [u8; 16]);

/// A multiplexing stream (an HTTP/2-style conversation).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
#[repr(transparent)]
pub struct StreamId(pub u32);

/// A request tag within a stream.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
#[repr(transparent)]
pub struct Tag(pub u32);

/// A completion-queue event identifier.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
#[repr(transparent)]
pub struct CompletionId(pub u64);

/// A shim-side event identifier bound to an interrupt or timer.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
#[repr(transparent)]
pub struct EventId(pub u64);

/// A DMA buffer token.  The core never receives a host pointer.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
#[repr(transparent)]
pub struct DmaToken(pub u32);

// ---------------------------------------------------------------------------
// Rights and handles
// ---------------------------------------------------------------------------

/// The four rights of the IRIX 7 object model.
///
/// Delegation and revocation are operations on a capability-manager object,
/// not extra bits on every capability.  This keeps the hot-path token small
/// and makes authority explicit.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
#[repr(transparent)]
pub struct Rights(pub u8);

impl Rights {
    /// Read the object's bytes or metadata.
    pub const READ: Self = Self(1 << 0);
    /// Write or replace the object.
    pub const WRITE: Self = Self(1 << 1);
    /// Execute the object as code (a Wasm module or a native image).
    pub const EXECUTE: Self = Self(1 << 2);
    /// Invoke the object as a service endpoint.
    pub const INVOKE: Self = Self(1 << 3);
    /// All four rights.
    pub const ALL: Self = Self(0x0f);

    /// The empty set.
    pub const NONE: Self = Self(0);

    /// Whether every bit in `other` is present.
    pub const fn contains(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }

    /// Set union.
    pub const fn union(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }

    /// Set intersection.
    pub const fn intersection(self, other: Self) -> Self {
        Self(self.0 & other.0)
    }

    /// Whether no rights are present.
    pub const fn is_empty(self) -> bool {
        self.0 == 0
    }

    /// Reject reserved bits.
    pub const fn from_bits(bits: u8) -> Result<Self, Error> {
        if bits & !Self::ALL.0 != 0 {
            return Err(Error::BadRights);
        }
        Ok(Self(bits))
    }
}

/// A compact capability handle used on the hot path.
///
/// The handle is an index into the receiver's capability table plus a
/// generation counter.  It is not authority by itself: the kernel checks the
/// table entry, the object identity and the rights on every use.  A stale
/// handle after revocation or slot reuse fails with [`Error::StaleHandle`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
#[repr(transparent)]
pub struct CapHandle(pub u64);

impl CapHandle {
    /// The null handle.  Never valid.
    pub const NULL: Self = Self(0);

    /// Build a handle from a slot index and generation.
    pub const fn new(index: u32, generation: u32) -> Self {
        Self(((generation as u64) << 32) | index as u64)
    }

    /// The slot index.
    pub const fn index(self) -> u32 {
        self.0 as u32
    }

    /// The generation.
    pub const fn generation(self) -> u32 {
        (self.0 >> 32) as u32
    }

    /// Whether this is the null handle.
    pub const fn is_null(self) -> bool {
        self.0 == 0
    }
}

// ---------------------------------------------------------------------------
// Capability token
// ---------------------------------------------------------------------------

/// A portable, home-signed object capability with global routing fields.
///
/// Layout (320 bytes, little-endian, always):
///
/// ```text
/// 0    magic "I7CP"
/// 4    version = 2
/// 5    rights
/// 6    flags (bit 0 = has_parent)
/// 8    object          ObjectId   (32)
/// 40   issuer          NodeKey    (32)  Ed25519 public key of the home/issuer
/// 72   subject         PrincipalId(32)
/// 104  home            NodeId     (16)  current home node of the object
/// 120  not_before      u64
/// 128  not_after       u64
/// 136  object_epoch    u32             object revocation epoch at issue
/// 140  home_epoch      u32             home assignment epoch at issue
/// 144  root_epoch      u32             authority root epoch at issue
/// 148  reserved0       u32 = 0
/// 152  nonce           [u8; 16]
/// 168  parent          ObjectId   (32; zero when flag bit 0 is clear)
/// 200  delegation      ObjectId   (32; home certificate / root manifest OID)
/// 232  reserved1       [u8; 24] = 0
/// 256  signature       [u8; 64]  (Ed25519 over bytes 0..256)
/// ```
///
/// The `home`/`home_epoch` pair is the routing hint.  A receiver can send the
/// operation to the home without a central directory.  If the home has moved,
/// the old home forwards or returns `MOVED` with the new epoch; the token is
/// not silently accepted on a stale route.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(C)]
pub struct Capability {
    /// Magic bytes.
    pub magic: [u8; 4],
    /// Token format version.
    pub version: u8,
    /// Granted rights.
    pub rights: Rights,
    /// Flags; bit 0 means a parent capability is present.
    pub flags: u16,
    /// The object this capability names.
    pub object: ObjectId,
    /// The issuing node's Ed25519 public key.
    pub issuer: NodeKey,
    /// The subject (principal or actor) this token was issued to.
    pub subject: PrincipalId,
    /// The current home node of the object.
    pub home: NodeId,
    /// Validity start, Unix nanoseconds.
    pub not_before: u64,
    /// Validity end, Unix nanoseconds.
    pub not_after: u64,
    /// Object revocation epoch at issue time.
    pub object_epoch: u32,
    /// Home assignment epoch at issue time.
    pub home_epoch: u32,
    /// Authority root epoch at issue time.
    pub root_epoch: u32,
    /// Reserved; must be zero.
    pub reserved0: u32,
    /// Anti-replay nonce.
    pub nonce: [u8; 16],
    /// Parent capability object, or zero.
    pub parent: ObjectId,
    /// Home certificate or root manifest OID.
    pub delegation: ObjectId,
    /// Reserved; must be zero.
    pub reserved1: [u8; 24],
    /// Ed25519 signature over the first 256 bytes.
    pub signature: [u8; 64],
}

/// A signature verifier supplied by the native capability engine.
///
/// Keeping this trait here means `sys_primitives` has no cryptographic
/// dependency, no allocation and no key material.  The Ed25519 implementation
/// lives in the shim (or in a separately audited `cap-engine` crate).
pub trait SignatureVerifier {
    /// Verify `signature` over `message` for `issuer`.
    fn verify(&self, issuer: &NodeKey, message: &[u8], signature: &[u8; 64]) -> bool;
}

impl Capability {
    /// Token length in bytes.
    pub const LEN: usize = 320;
    /// Number of bytes covered by the signature.
    pub const SIGNED_LEN: usize = 256;
    /// Format version.
    pub const VERSION: u8 = 2;
    /// Magic.
    pub const MAGIC: [u8; 4] = *b"I7CP";
    /// Flag: a parent capability is present.
    pub const FLAG_HAS_PARENT: u16 = 1;

    /// Encode the token to its canonical 320-byte form.
    pub fn encode(&self) -> [u8; Self::LEN] {
        let mut out = [0u8; Self::LEN];
        out[0..4].copy_from_slice(&self.magic);
        out[4] = self.version;
        out[5] = self.rights.0;
        put_u16(&mut out, 6, self.flags);
        out[8..40].copy_from_slice(self.object.as_bytes());
        out[40..72].copy_from_slice(&self.issuer.0);
        out[72..104].copy_from_slice(&self.subject.0);
        out[104..120].copy_from_slice(self.home.as_bytes());
        put_u64(&mut out, 120, self.not_before);
        put_u64(&mut out, 128, self.not_after);
        put_u32(&mut out, 136, self.object_epoch);
        put_u32(&mut out, 140, self.home_epoch);
        put_u32(&mut out, 144, self.root_epoch);
        put_u32(&mut out, 148, self.reserved0);
        out[152..168].copy_from_slice(&self.nonce);
        out[168..200].copy_from_slice(self.parent.as_bytes());
        out[200..232].copy_from_slice(self.delegation.as_bytes());
        out[232..256].copy_from_slice(&self.reserved1);
        out[256..320].copy_from_slice(&self.signature);
        out
    }

    /// Decode and strictly validate the canonical layout.
    pub fn decode(bytes: &[u8]) -> Result<Self, Error> {
        if bytes.len() != Self::LEN {
            return Err(Error::BadLength);
        }
        if bytes[0..4] != Self::MAGIC {
            return Err(Error::BadMagic);
        }
        if bytes[4] != Self::VERSION {
            return Err(Error::BadVersion);
        }
        let rights = Rights::from_bits(bytes[5])?;
        let flags = get_u16(bytes, 6);
        if flags & !Self::FLAG_HAS_PARENT != 0 {
            return Err(Error::NonCanonical);
        }
        let reserved0 = get_u32(bytes, 148);
        if reserved0 != 0 || bytes[232..256].iter().any(|&b| b != 0) {
            return Err(Error::NonCanonical);
        }
        let parent = ObjectId::from_slice(&bytes[168..200])?;
        if flags & Self::FLAG_HAS_PARENT == 0 && !parent.is_zero() {
            return Err(Error::NonCanonical);
        }
        if flags & Self::FLAG_HAS_PARENT != 0 && parent.is_zero() {
            return Err(Error::NonCanonical);
        }
        let mut object = [0u8; 32];
        object.copy_from_slice(&bytes[8..40]);
        let mut issuer = [0u8; 32];
        issuer.copy_from_slice(&bytes[40..72]);
        let mut subject = [0u8; 32];
        subject.copy_from_slice(&bytes[72..104]);
        let mut home = [0u8; 16];
        home.copy_from_slice(&bytes[104..120]);
        let mut nonce = [0u8; 16];
        nonce.copy_from_slice(&bytes[152..168]);
        let delegation = ObjectId::from_slice(&bytes[200..232])?;
        let mut signature = [0u8; 64];
        signature.copy_from_slice(&bytes[256..320]);
        let not_before = get_u64(bytes, 120);
        let not_after = get_u64(bytes, 128);
        if not_after < not_before {
            return Err(Error::NonCanonical);
        }
        if ObjectId(object).is_zero() {
            return Err(Error::WrongObject);
        }
        if issuer.iter().all(|&b| b == 0) {
            return Err(Error::BadSignature);
        }
        if NodeId(home).is_zero() {
            return Err(Error::NonCanonical);
        }
        Ok(Self {
            magic: Self::MAGIC,
            version: Self::VERSION,
            rights,
            flags,
            object: ObjectId(object),
            issuer: NodeKey(issuer),
            subject: PrincipalId(subject),
            home: NodeId(home),
            not_before,
            not_after,
            object_epoch: get_u32(bytes, 136),
            home_epoch: get_u32(bytes, 140),
            root_epoch: get_u32(bytes, 144),
            reserved0,
            nonce,
            parent,
            delegation,
            reserved1: [0; 24],
            signature,
        })
    }

    /// The bytes covered by the signature.
    pub fn signed_bytes(&self) -> [u8; Self::SIGNED_LEN] {
        let encoded = self.encode();
        let mut out = [0u8; Self::SIGNED_LEN];
        out.copy_from_slice(&encoded[..Self::SIGNED_LEN]);
        out
    }

    /// Check time, object, rights, home and signature.
    pub fn verify(
        &self,
        verifier: &impl SignatureVerifier,
        now_ns: u64,
        required: Rights,
        object: ObjectId,
    ) -> Result<(), Error> {
        if self.magic != Self::MAGIC {
            return Err(Error::BadMagic);
        }
        if self.version != Self::VERSION {
            return Err(Error::BadVersion);
        }
        if self.reserved0 != 0 || self.reserved1.iter().any(|&b| b != 0) {
            return Err(Error::NonCanonical);
        }
        if self.flags & !Self::FLAG_HAS_PARENT != 0 {
            return Err(Error::NonCanonical);
        }
        if self.flags & Self::FLAG_HAS_PARENT == 0 && !self.parent.is_zero() {
            return Err(Error::NonCanonical);
        }
        if self.flags & Self::FLAG_HAS_PARENT != 0 && self.parent.is_zero() {
            return Err(Error::NonCanonical);
        }
        if self.not_after < self.not_before {
            return Err(Error::NonCanonical);
        }
        if self.object.is_zero() || self.issuer.0.iter().all(|&b| b == 0) || self.home.is_zero() {
            return Err(Error::NonCanonical);
        }
        if now_ns < self.not_before {
            return Err(Error::NotYetValid);
        }
        if now_ns > self.not_after {
            return Err(Error::Expired);
        }
        if self.object != object {
            return Err(Error::WrongObject);
        }
        if !self.rights.contains(required) {
            return Err(Error::InsufficientRights);
        }
        let signed = self.signed_bytes();
        if !verifier.verify(&self.issuer, &signed, &self.signature) {
            return Err(Error::BadSignature);
        }
        Ok(())
    }

    /// Whether the token is within its validity window.
    pub const fn is_valid_at(&self, now_ns: u64) -> bool {
        now_ns >= self.not_before && now_ns <= self.not_after
    }
}

// ---------------------------------------------------------------------------
// Frame header
// ---------------------------------------------------------------------------

/// Message kind.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum MessageKind {
    /// A request expecting a response.
    Request = 0,
    /// A response to a request.
    Response = 1,
    /// A completion event.
    Completion = 2,
    /// A cancellation request.
    Cancel = 3,
    /// An asynchronous event (readiness, signal, lifecycle).
    Event = 4,
    /// A connection-level control frame.
    Control = 5,
}

impl MessageKind {
    /// Decode a discriminant.
    pub const fn from_u8(value: u8) -> Result<Self, Error> {
        match value {
            0 => Ok(Self::Request),
            1 => Ok(Self::Response),
            2 => Ok(Self::Completion),
            3 => Ok(Self::Cancel),
            4 => Ok(Self::Event),
            5 => Ok(Self::Control),
            _ => Err(Error::BadKind),
        }
    }
}

/// Operation code.  The low byte is stable; the high byte is a namespace.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u16)]
pub enum OpCode {
    /// No operation; used for keepalive and tests.
    Nop = 0,
    /// Protocol and identity handshake.
    Hello = 1,
    /// Capability or token authentication.
    Auth = 2,
    /// Resolve a name to an object.
    Lookup = 0x10,
    /// Open an object for I/O.
    Open = 0x11,
    /// Read a byte range.
    Read = 0x12,
    /// Write a byte range.
    Write = 0x13,
    /// Read metadata.
    Stat = 0x14,
    /// Enumerate a directory.
    ReadDir = 0x15,
    /// Create an object.
    Create = 0x16,
    /// Remove a name.
    Unlink = 0x17,
    /// Rename a name.
    Rename = 0x18,
    /// Invoke a service endpoint.
    Invoke = 0x20,
    /// Cancel an in-flight tag.
    Cancel = 0x30,
    /// Grant additional flow-control credit.
    WindowUpdate = 0x31,
    /// Liveness check.
    Ping = 0x32,
    /// Graceful connection shutdown.
    GoAway = 0x33,
    /// Prepare, transfer or commit an actor.
    Migrate = 0x40,
    /// Derive an attenuated capability.
    CapDerive = 0x50,
    /// Revoke a capability.
    CapRevoke = 0x51,
    /// Query the current home/route for an object or actor.
    RouteQuery = 0x60,
    /// Forward a frame to a home or host.
    Forward = 0x61,
    /// Ask a candidate node to host an actor.
    Place = 0x62,
    /// Accept a placement reservation.
    PlaceAccept = 0x63,
    /// Prepare an actor for migration.
    MigratePrepare = 0x64,
    /// Commit an actor migration (fencing epoch).
    MigrateCommit = 0x65,
    /// Abort an actor migration.
    MigrateAbort = 0x66,
    /// Submit an offscreen GPU job.
    GpuSubmit = 0x70,
    /// Poll or await a GPU fence.
    GpuFence = 0x71,
}

impl OpCode {
    /// Decode a discriminant.
    pub const fn from_u16(value: u16) -> Result<Self, Error> {
        match value {
            0 => Ok(Self::Nop),
            1 => Ok(Self::Hello),
            2 => Ok(Self::Auth),
            0x10 => Ok(Self::Lookup),
            0x11 => Ok(Self::Open),
            0x12 => Ok(Self::Read),
            0x13 => Ok(Self::Write),
            0x14 => Ok(Self::Stat),
            0x15 => Ok(Self::ReadDir),
            0x16 => Ok(Self::Create),
            0x17 => Ok(Self::Unlink),
            0x18 => Ok(Self::Rename),
            0x20 => Ok(Self::Invoke),
            0x30 => Ok(Self::Cancel),
            0x31 => Ok(Self::WindowUpdate),
            0x32 => Ok(Self::Ping),
            0x33 => Ok(Self::GoAway),
            0x40 => Ok(Self::Migrate),
            0x50 => Ok(Self::CapDerive),
            0x51 => Ok(Self::CapRevoke),
            0x60 => Ok(Self::RouteQuery),
            0x61 => Ok(Self::Forward),
            0x62 => Ok(Self::Place),
            0x63 => Ok(Self::PlaceAccept),
            0x64 => Ok(Self::MigratePrepare),
            0x65 => Ok(Self::MigrateCommit),
            0x66 => Ok(Self::MigrateAbort),
            0x70 => Ok(Self::GpuSubmit),
            0x71 => Ok(Self::GpuFence),
            _ => Err(Error::BadKind),
        }
    }
}

/// Frame flags.
pub mod flags {
    /// More fragments follow on this stream.
    pub const MORE: u16 = 1 << 0;
    /// End of stream.
    pub const EOS: u16 = 1 << 1;
    /// Link this operation to the next one (io_uring `IOSQE_IO_LINK`).
    pub const LINK: u16 = 1 << 2;
    /// A W3C Trace Context is present.
    pub const TRACE: u16 = 1 << 3;
    /// A capability handle is present.
    pub const CAP: u16 = 1 << 4;
    /// Do not send a response on success.
    pub const NO_REPLY: u16 = 1 << 5;
    /// The payload is CAS-backed or ring-backed.
    pub const BULK: u16 = 1 << 6;
    /// The frame is an error response.
    pub const ERROR: u16 = 1 << 7;
    /// The sender requests a completion notification.
    pub const NOTIFY: u16 = 1 << 8;
}

/// Completion flags.
pub mod completion_flags {
    /// Further completions will follow for the same submission.
    pub const MORE: u8 = 1 << 0;
    /// This is a notification, not the operation result (zero-copy send).
    pub const NOTIF: u8 = 1 << 1;
    /// The operation was cancelled.
    pub const CANCELLED: u8 = 1 << 2;
    /// This is the final completion for the submission.
    pub const FINAL: u8 = 1 << 3;
}

/// The fixed 128-byte fabric header.
///
/// ```text
/// 0   magic u32 = "7FM1" (little-endian bytes 37 46 4D 31)
/// 4   version u8 = 1
/// 5   kind u8
/// 6   flags u16
/// 8   opcode u16
/// 10  header_len u16 = 128
/// 12  stream u32
/// 16  tag u32
/// 20  parent u32
/// 24  payload_len u32 (descriptor + inline body)
/// 28  status i32
/// 32  cap_handle u64
/// 40  object [u8; 32]
/// 72  offset u64
/// 80  length u32
/// 84  priority u8
/// 85  trace_flags u8
/// 86  reserved u16 = 0
/// 88  deadline_ns u64
/// 96  trace_id [u8; 16]
/// 112 span_id [u8; 8]
/// 120 inline_len u32
/// 124 header_crc32c u32
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(C)]
pub struct FrameHeader {
    /// Magic.
    pub magic: u32,
    /// Format version.
    pub version: u8,
    /// Message kind.
    pub kind: MessageKind,
    /// Flags.
    pub flags: u16,
    /// Operation.
    pub opcode: OpCode,
    /// Header length; always 128.
    pub header_len: u16,
    /// Stream identifier.
    pub stream: StreamId,
    /// Request tag.
    pub tag: Tag,
    /// Parent tag for dependency tracking.
    pub parent: Tag,
    /// Total payload length.
    pub payload_len: u32,
    /// Response status.
    pub status: Status,
    /// Capability handle.
    pub cap: CapHandle,
    /// Primary object.
    pub object: ObjectId,
    /// Byte offset for reads and writes.
    pub offset: u64,
    /// Byte length for reads and writes.
    pub length: u32,
    /// Scheduling priority hint (0 = default, 255 = highest).
    pub priority: u8,
    /// W3C trace flags.
    pub trace_flags: u8,
    /// Reserved; must be zero.
    pub reserved: u16,
    /// Absolute deadline in Unix nanoseconds (0 = none).
    pub deadline_ns: u64,
    /// W3C trace-id.
    pub trace_id: [u8; 16],
    /// W3C parent-id / span-id.
    pub span_id: [u8; 8],
    /// Inline body length.
    pub inline_len: u32,
    /// CRC32C over bytes 0..124 with this field zeroed.
    pub header_crc32c: u32,
}

impl FrameHeader {
    /// Header length.
    pub const LEN: usize = 128;
    /// Magic value.
    pub const MAGIC: u32 = u32::from_le_bytes(*b"7FM1");
    /// Format version.
    pub const VERSION: u8 = 1;

    /// A header with zeroed identity fields.
    pub const fn new(kind: MessageKind, opcode: OpCode, stream: StreamId, tag: Tag) -> Self {
        Self {
            magic: Self::MAGIC,
            version: Self::VERSION,
            kind,
            flags: 0,
            opcode,
            header_len: Self::LEN as u16,
            stream,
            tag,
            parent: Tag(0),
            payload_len: 0,
            status: Status::Ok,
            cap: CapHandle::NULL,
            object: ObjectId::ZERO,
            offset: 0,
            length: 0,
            priority: 0,
            trace_flags: 0,
            reserved: 0,
            deadline_ns: 0,
            trace_id: [0; 16],
            span_id: [0; 8],
            inline_len: 0,
            header_crc32c: 0,
        }
    }

    /// Encode the header, computing the checksum.
    pub fn encode(&self) -> [u8; Self::LEN] {
        let mut out = [0u8; Self::LEN];
        put_u32(&mut out, 0, Self::MAGIC);
        out[4] = self.version;
        out[5] = self.kind as u8;
        put_u16(&mut out, 6, self.flags);
        put_u16(&mut out, 8, self.opcode as u16);
        put_u16(&mut out, 10, self.header_len);
        put_u32(&mut out, 12, self.stream.0);
        put_u32(&mut out, 16, self.tag.0);
        put_u32(&mut out, 20, self.parent.0);
        put_u32(&mut out, 24, self.payload_len);
        put_i32(&mut out, 28, self.status as i32);
        put_u64(&mut out, 32, self.cap.0);
        out[40..72].copy_from_slice(self.object.as_bytes());
        put_u64(&mut out, 72, self.offset);
        put_u32(&mut out, 80, self.length);
        out[84] = self.priority;
        out[85] = self.trace_flags;
        put_u16(&mut out, 86, self.reserved);
        put_u64(&mut out, 88, self.deadline_ns);
        out[96..112].copy_from_slice(&self.trace_id);
        out[112..120].copy_from_slice(&self.span_id);
        put_u32(&mut out, 120, self.inline_len);
        put_u32(&mut out, 124, 0);
        let crc = crc32c(&out);
        put_u32(&mut out, 124, crc);
        out
    }

    /// Decode and validate the header.
    pub fn decode(bytes: &[u8]) -> Result<Self, Error> {
        if bytes.len() < Self::LEN {
            return Err(Error::Truncated);
        }
        if get_u32(bytes, 0) != Self::MAGIC {
            return Err(Error::BadMagic);
        }
        if bytes[4] != Self::VERSION {
            return Err(Error::BadVersion);
        }
        let kind = MessageKind::from_u8(bytes[5])?;
        let flags = get_u16(bytes, 6);
        let opcode = OpCode::from_u16(get_u16(bytes, 8))?;
        let header_len = get_u16(bytes, 10);
        if header_len as usize != Self::LEN {
            return Err(Error::BadLength);
        }
        let status = Status::from_i32(get_i32(bytes, 28))?;
        let reserved = get_u16(bytes, 86);
        if reserved != 0 {
            return Err(Error::NonCanonical);
        }
        let mut object = [0u8; 32];
        object.copy_from_slice(&bytes[40..72]);
        let mut trace_id = [0u8; 16];
        trace_id.copy_from_slice(&bytes[96..112]);
        let mut span_id = [0u8; 8];
        span_id.copy_from_slice(&bytes[112..120]);
        let header = Self {
            magic: Self::MAGIC,
            version: Self::VERSION,
            kind,
            flags,
            opcode,
            header_len,
            stream: StreamId(get_u32(bytes, 12)),
            tag: Tag(get_u32(bytes, 16)),
            parent: Tag(get_u32(bytes, 20)),
            payload_len: get_u32(bytes, 24),
            status,
            cap: CapHandle(get_u64(bytes, 32)),
            object: ObjectId(object),
            offset: get_u64(bytes, 72),
            length: get_u32(bytes, 80),
            priority: bytes[84],
            trace_flags: bytes[85],
            reserved,
            deadline_ns: get_u64(bytes, 88),
            trace_id,
            span_id,
            inline_len: get_u32(bytes, 120),
            header_crc32c: get_u32(bytes, 124),
        };
        if header.payload_len < PayloadDescriptor::LEN as u32 {
            return Err(Error::BadLength);
        }
        if header.inline_len as usize > header.payload_len as usize - PayloadDescriptor::LEN {
            return Err(Error::BadLength);
        }
        let mut check = [0u8; Self::LEN];
        check.copy_from_slice(&bytes[..Self::LEN]);
        put_u32(&mut check, 124, 0);
        if crc32c(&check) != header.header_crc32c {
            return Err(Error::BadCrc);
        }
        Ok(header)
    }
}

// ---------------------------------------------------------------------------
// Payload descriptor
// ---------------------------------------------------------------------------

/// How the body is carried.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum PayloadKind {
    /// Bytes follow the descriptor.
    Inline = 0,
    /// A range of a content-addressed object.
    Cas = 1,
    /// A range of a shared-memory ring.
    Ring = 2,
    /// A device DMA buffer token.
    Dma = 3,
    /// No body.
    Empty = 4,
}

impl PayloadKind {
    /// Decode a discriminant.
    pub const fn from_u8(value: u8) -> Result<Self, Error> {
        match value {
            0 => Ok(Self::Inline),
            1 => Ok(Self::Cas),
            2 => Ok(Self::Ring),
            3 => Ok(Self::Dma),
            4 => Ok(Self::Empty),
            _ => Err(Error::BadKind),
        }
    }
}

/// The fixed 64-byte payload descriptor.
///
/// ```text
/// 0   kind u8
/// 1   flags u8
/// 2   reserved u16 = 0
/// 4   inline_len u32
/// 8   length u64
/// 16  offset u64
/// 24  object [u8; 32]
/// 56  ring_id u32
/// 60  dma_token u32
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(C)]
pub struct PayloadDescriptor {
    /// Payload kind.
    pub kind: PayloadKind,
    /// Payload flags (kind-specific).
    pub flags: u8,
    /// Reserved; must be zero.
    pub reserved: u16,
    /// Inline bytes present after this descriptor.
    pub inline_len: u32,
    /// Logical payload length.
    pub length: u64,
    /// Offset within the object or ring.
    pub offset: u64,
    /// Object identity for CAS payloads.
    pub object: ObjectId,
    /// Ring identifier for shared-memory payloads.
    pub ring_id: u32,
    /// DMA token for device payloads.
    pub dma_token: u32,
}

impl PayloadDescriptor {
    /// Descriptor length.
    pub const LEN: usize = 64;

    /// An empty descriptor.
    pub const fn empty() -> Self {
        Self {
            kind: PayloadKind::Empty,
            flags: 0,
            reserved: 0,
            inline_len: 0,
            length: 0,
            offset: 0,
            object: ObjectId::ZERO,
            ring_id: 0,
            dma_token: 0,
        }
    }

    /// Encode to the canonical 64 bytes.
    pub fn encode(&self) -> [u8; Self::LEN] {
        let mut out = [0u8; Self::LEN];
        out[0] = self.kind as u8;
        out[1] = self.flags;
        put_u16(&mut out, 2, self.reserved);
        put_u32(&mut out, 4, self.inline_len);
        put_u64(&mut out, 8, self.length);
        put_u64(&mut out, 16, self.offset);
        out[24..56].copy_from_slice(self.object.as_bytes());
        put_u32(&mut out, 56, self.ring_id);
        put_u32(&mut out, 60, self.dma_token);
        out
    }

    /// Decode and validate.
    pub fn decode(bytes: &[u8]) -> Result<Self, Error> {
        if bytes.len() < Self::LEN {
            return Err(Error::Truncated);
        }
        let kind = PayloadKind::from_u8(bytes[0])?;
        let reserved = get_u16(bytes, 2);
        if reserved != 0 {
            return Err(Error::NonCanonical);
        }
        let object = ObjectId::from_slice(&bytes[24..56])?;
        let inline_len = get_u32(bytes, 4);
        let length = get_u64(bytes, 8);
        if inline_len as u64 > length {
            return Err(Error::BadLength);
        }
        if kind != PayloadKind::Inline && inline_len != 0 {
            return Err(Error::NonCanonical);
        }
        Ok(Self {
            kind,
            flags: bytes[1],
            reserved,
            inline_len,
            length,
            offset: get_u64(bytes, 16),
            object,
            ring_id: get_u32(bytes, 56),
            dma_token: get_u32(bytes, 60),
        })
    }
}

// ---------------------------------------------------------------------------
// Message frame
// ---------------------------------------------------------------------------

/// One fabric frame.  The same type is used for local IPC and for the network;
/// only the transport changes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MessageFrame<'a> {
    /// Header.
    pub header: FrameHeader,
    /// Payload descriptor.
    pub payload: PayloadDescriptor,
    /// Inline bytes, if any.
    pub body: &'a [u8],
}

impl<'a> MessageFrame<'a> {
    /// A frame with an inline body.
    pub fn inline(
        kind: MessageKind,
        opcode: OpCode,
        stream: StreamId,
        tag: Tag,
        body: &'a [u8],
    ) -> Result<Self, Error> {
        let body_len = u32::try_from(body.len()).map_err(|_| Error::BadLength)?;
        let mut header = FrameHeader::new(kind, opcode, stream, tag);
        header.inline_len = body_len;
        header.payload_len = PayloadDescriptor::LEN as u32 + body_len;
        header.length = body_len;
        let payload = PayloadDescriptor {
            kind: PayloadKind::Inline,
            flags: 0,
            reserved: 0,
            inline_len: body_len,
            length: body_len as u64,
            offset: 0,
            object: ObjectId::ZERO,
            ring_id: 0,
            dma_token: 0,
        };
        Ok(Self {
            header,
            payload,
            body,
        })
    }

    /// A CAS-backed frame.  The receiver resolves `object[offset..offset+len]`.
    pub fn cas(
        kind: MessageKind,
        opcode: OpCode,
        stream: StreamId,
        tag: Tag,
        object: ObjectId,
        offset: u64,
        length: u64,
    ) -> Self {
        let mut header = FrameHeader::new(kind, opcode, stream, tag);
        header.flags |= flags::BULK;
        header.payload_len = PayloadDescriptor::LEN as u32;
        header.object = object;
        header.offset = offset;
        header.length = u32::try_from(length).unwrap_or(u32::MAX);
        let payload = PayloadDescriptor {
            kind: PayloadKind::Cas,
            flags: 0,
            reserved: 0,
            inline_len: 0,
            length,
            offset,
            object,
            ring_id: 0,
            dma_token: 0,
        };
        Self {
            header,
            payload,
            body: &[],
        }
    }

    /// Total encoded length.
    pub fn encoded_len(&self) -> usize {
        FrameHeader::LEN + PayloadDescriptor::LEN + self.body.len()
    }

    /// Encode into `out`.  Returns the number of bytes written.
    pub fn encode(&self, out: &mut [u8]) -> Result<usize, Error> {
        let total = self.encoded_len();
        if out.len() < total {
            return Err(Error::Truncated);
        }
        if self.header.magic != FrameHeader::MAGIC {
            return Err(Error::BadMagic);
        }
        if self.header.version != FrameHeader::VERSION {
            return Err(Error::BadVersion);
        }
        if self.header.header_len as usize != FrameHeader::LEN {
            return Err(Error::BadLength);
        }
        if self.header.inline_len as usize != self.body.len() {
            return Err(Error::BadLength);
        }
        if self.header.payload_len as usize != PayloadDescriptor::LEN + self.body.len() {
            return Err(Error::BadLength);
        }
        if self.payload.kind != PayloadKind::Inline && !self.body.is_empty() {
            return Err(Error::NonCanonical);
        }
        if self.payload.kind == PayloadKind::Inline && self.payload.length != self.body.len() as u64
        {
            return Err(Error::BadLength);
        }
        out[..FrameHeader::LEN].copy_from_slice(&self.header.encode());
        out[FrameHeader::LEN..FrameHeader::LEN + PayloadDescriptor::LEN]
            .copy_from_slice(&self.payload.encode());
        out[FrameHeader::LEN + PayloadDescriptor::LEN..total].copy_from_slice(self.body);
        Ok(total)
    }

    /// Decode one frame from `buf`.  Returns the frame and bytes consumed.
    pub fn decode(buf: &'a [u8]) -> Result<(Self, usize), Error> {
        let header = FrameHeader::decode(buf)?;
        let total = FrameHeader::LEN
            .checked_add(header.payload_len as usize)
            .ok_or(Error::BadLength)?;
        if buf.len() < total {
            return Err(Error::Truncated);
        }
        let payload = PayloadDescriptor::decode(&buf[FrameHeader::LEN..])?;
        if payload.inline_len != header.inline_len {
            return Err(Error::BadLength);
        }
        if payload.kind == PayloadKind::Inline && payload.length != header.inline_len as u64 {
            return Err(Error::BadLength);
        }
        let body_start = FrameHeader::LEN + PayloadDescriptor::LEN;
        let body_end = body_start
            .checked_add(header.inline_len as usize)
            .ok_or(Error::BadLength)?;
        if body_end > total {
            return Err(Error::BadLength);
        }
        Ok((
            Self {
                header,
                payload,
                body: &buf[body_start..body_end],
            },
            total,
        ))
    }
}

// ---------------------------------------------------------------------------
// Mesh envelope
// ---------------------------------------------------------------------------

/// Mesh envelope flags.
pub mod mesh_flags {
    /// The frame is being forwarded; `hop_count` is incremented.
    pub const FORWARD: u8 = 1 << 0;
    /// The frame is a reply to `reply_to`.
    pub const REPLY: u8 = 1 << 1;
    /// The object home moved; the payload carries the new home/epoch.
    pub const MOVED: u8 = 1 << 2;
    /// Membership/route probe.
    pub const PROBE: u8 = 1 << 3;
    /// Actor migration control.
    pub const MIGRATE: u8 = 1 << 4;
    /// The envelope was dropped at a hop; `ttl` or route error.
    pub const DROP: u8 = 1 << 5;
}

/// The fixed 128-byte mesh header.  It is used only between kernel replicas;
/// the single-node path uses [`MessageFrame`] directly.
///
/// ```text
/// 0   magic u32 = "I7MH"
/// 4   version u8 = 1
/// 5   flags u8
/// 6   header_len u16 = 128
/// 8   src NodeId (16)
/// 24  dst NodeId (16)
/// 40  home NodeId (16)
/// 56  route_hint NodeId (16)
/// 72  src_epoch u32
/// 76  dst_epoch u32
/// 80  home_epoch u32
/// 84  object_epoch u32
/// 88  request_id u64
/// 96  reply_to u64
/// 104 ttl u8
/// 105 hop_count u8
/// 106 priority u8
/// 107 reserved u8 = 0
/// 108 frame_len u32
/// 112 reserved [u8; 12] = 0
/// 124 header_crc32c u32
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(C)]
pub struct MeshHeader {
    /// Magic.
    pub magic: u32,
    /// Format version.
    pub version: u8,
    /// Flags.
    pub flags: u8,
    /// Header length; always 128.
    pub header_len: u16,
    /// Source node.
    pub src: NodeId,
    /// Final destination node.
    pub dst: NodeId,
    /// Object/actor home node.
    pub home: NodeId,
    /// Next-hop hint; zero means route by `dst`.
    pub route_hint: NodeId,
    /// Source membership epoch.
    pub src_epoch: u32,
    /// Destination membership epoch (0 = unknown).
    pub dst_epoch: u32,
    /// Home assignment epoch.
    pub home_epoch: u32,
    /// Object/actor epoch.
    pub object_epoch: u32,
    /// Unique request id at the source.
    pub request_id: u64,
    /// Request id this frame replies to (0 = unsolicited).
    pub reply_to: u64,
    /// Remaining hops; max 8.
    pub ttl: u8,
    /// Hops already taken.
    pub hop_count: u8,
    /// Scheduling priority hint.
    pub priority: u8,
    /// Reserved; must be zero.
    pub reserved: u8,
    /// Encoded `MessageFrame` length.
    pub frame_len: u32,
    /// Reserved; must be zero.
    pub reserved2: [u8; 12],
    /// CRC32C over bytes 0..124 with this field zeroed.
    pub header_crc32c: u32,
}

impl MeshHeader {
    /// Header length.
    pub const LEN: usize = 128;
    /// Magic.
    pub const MAGIC: u32 = u32::from_le_bytes(*b"I7MH");
    /// Format version.
    pub const VERSION: u8 = 1;
    /// Maximum hops.
    pub const MAX_HOPS: u8 = 8;

    /// A header with zeroed fields.
    pub const fn new(src: NodeId, dst: NodeId, home: NodeId) -> Self {
        Self {
            magic: Self::MAGIC,
            version: Self::VERSION,
            flags: 0,
            header_len: Self::LEN as u16,
            src,
            dst,
            home,
            route_hint: NodeId::ZERO,
            src_epoch: 0,
            dst_epoch: 0,
            home_epoch: 0,
            object_epoch: 0,
            request_id: 0,
            reply_to: 0,
            ttl: Self::MAX_HOPS,
            hop_count: 0,
            priority: 0,
            reserved: 0,
            frame_len: 0,
            reserved2: [0; 12],
            header_crc32c: 0,
        }
    }

    /// Encode with checksum.
    pub fn encode(&self) -> [u8; Self::LEN] {
        let mut out = [0u8; Self::LEN];
        put_u32(&mut out, 0, Self::MAGIC);
        out[4] = self.version;
        out[5] = self.flags;
        put_u16(&mut out, 6, self.header_len);
        out[8..24].copy_from_slice(self.src.as_bytes());
        out[24..40].copy_from_slice(self.dst.as_bytes());
        out[40..56].copy_from_slice(self.home.as_bytes());
        out[56..72].copy_from_slice(self.route_hint.as_bytes());
        put_u32(&mut out, 72, self.src_epoch);
        put_u32(&mut out, 76, self.dst_epoch);
        put_u32(&mut out, 80, self.home_epoch);
        put_u32(&mut out, 84, self.object_epoch);
        put_u64(&mut out, 88, self.request_id);
        put_u64(&mut out, 96, self.reply_to);
        out[104] = self.ttl;
        out[105] = self.hop_count;
        out[106] = self.priority;
        out[107] = self.reserved;
        put_u32(&mut out, 108, self.frame_len);
        out[112..124].copy_from_slice(&self.reserved2);
        put_u32(&mut out, 124, 0);
        let crc = crc32c(&out);
        put_u32(&mut out, 124, crc);
        out
    }

    /// Decode and validate.
    pub fn decode(bytes: &[u8]) -> Result<Self, Error> {
        if bytes.len() < Self::LEN {
            return Err(Error::Truncated);
        }
        if get_u32(bytes, 0) != Self::MAGIC {
            return Err(Error::BadMagic);
        }
        if bytes[4] != Self::VERSION {
            return Err(Error::BadVersion);
        }
        let header_len = get_u16(bytes, 6);
        if header_len as usize != Self::LEN {
            return Err(Error::BadLength);
        }
        if bytes[107] != 0 || bytes[112..124].iter().any(|&b| b != 0) {
            return Err(Error::NonCanonical);
        }
        let mut src = [0u8; 16];
        src.copy_from_slice(&bytes[8..24]);
        let mut dst = [0u8; 16];
        dst.copy_from_slice(&bytes[24..40]);
        let mut home = [0u8; 16];
        home.copy_from_slice(&bytes[40..56]);
        let mut route_hint = [0u8; 16];
        route_hint.copy_from_slice(&bytes[56..72]);
        let mut reserved2 = [0u8; 12];
        reserved2.copy_from_slice(&bytes[112..124]);
        let header = Self {
            magic: Self::MAGIC,
            version: Self::VERSION,
            flags: bytes[5],
            header_len,
            src: NodeId(src),
            dst: NodeId(dst),
            home: NodeId(home),
            route_hint: NodeId(route_hint),
            src_epoch: get_u32(bytes, 72),
            dst_epoch: get_u32(bytes, 76),
            home_epoch: get_u32(bytes, 80),
            object_epoch: get_u32(bytes, 84),
            request_id: get_u64(bytes, 88),
            reply_to: get_u64(bytes, 96),
            ttl: bytes[104],
            hop_count: bytes[105],
            priority: bytes[106],
            reserved: bytes[107],
            frame_len: get_u32(bytes, 108),
            reserved2,
            header_crc32c: get_u32(bytes, 124),
        };
        if header.ttl > Self::MAX_HOPS || header.hop_count > header.ttl {
            return Err(Error::Protocol);
        }
        if header.frame_len < (FrameHeader::LEN + PayloadDescriptor::LEN) as u32 {
            return Err(Error::BadLength);
        }
        let mut check = [0u8; Self::LEN];
        check.copy_from_slice(&bytes[..Self::LEN]);
        put_u32(&mut check, 124, 0);
        if crc32c(&check) != header.header_crc32c {
            return Err(Error::BadCrc);
        }
        Ok(header)
    }
}

/// A mesh envelope: routing header plus one fabric frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MeshEnvelope<'a> {
    /// Routing header.
    pub header: MeshHeader,
    /// Fabric frame.
    pub frame: MessageFrame<'a>,
}

impl<'a> MeshEnvelope<'a> {
    /// Total encoded length.
    pub fn encoded_len(&self) -> usize {
        MeshHeader::LEN + self.frame.encoded_len()
    }

    /// Encode the envelope.
    pub fn encode(&self, out: &mut [u8]) -> Result<usize, Error> {
        let frame_len = self.frame.encoded_len();
        let total = MeshHeader::LEN + frame_len;
        if out.len() < total {
            return Err(Error::Truncated);
        }
        let mut header = self.header;
        header.header_len = MeshHeader::LEN as u16;
        header.frame_len = u32::try_from(frame_len).map_err(|_| Error::BadLength)?;
        out[..MeshHeader::LEN].copy_from_slice(&header.encode());
        self.frame.encode(&mut out[MeshHeader::LEN..total])?;
        Ok(total)
    }

    /// Decode one envelope.  Returns the envelope and bytes consumed.
    pub fn decode(buf: &'a [u8]) -> Result<(Self, usize), Error> {
        let header = MeshHeader::decode(buf)?;
        let total = MeshHeader::LEN
            .checked_add(header.frame_len as usize)
            .ok_or(Error::BadLength)?;
        if buf.len() < total {
            return Err(Error::Truncated);
        }
        let (frame, consumed) = MessageFrame::decode(&buf[MeshHeader::LEN..total])?;
        if consumed != header.frame_len as usize {
            return Err(Error::BadLength);
        }
        Ok((Self { header, frame }, total))
    }
}

// ---------------------------------------------------------------------------
// Completions
// ---------------------------------------------------------------------------

/// What a completion carries.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum CompletionKind {
    /// Nothing beyond the status.
    None = 0,
    /// A byte count.
    Bytes = 1,
    /// A readiness mask.
    Readiness = 2,
    /// An object identity.
    Object = 3,
    /// A capability handle.
    Handle = 4,
    /// A frame was received.
    Frame = 5,
    /// An interrupt fired.
    Irq = 6,
    /// A timer expired.
    Timer = 7,
}

impl CompletionKind {
    /// Decode a discriminant.
    pub const fn from_u8(value: u8) -> Result<Self, Error> {
        match value {
            0 => Ok(Self::None),
            1 => Ok(Self::Bytes),
            2 => Ok(Self::Readiness),
            3 => Ok(Self::Object),
            4 => Ok(Self::Handle),
            5 => Ok(Self::Frame),
            6 => Ok(Self::Irq),
            7 => Ok(Self::Timer),
            _ => Err(Error::BadKind),
        }
    }
}

/// A 32-byte completion-queue entry.
///
/// ```text
/// 0   event u64
/// 8   tag u32
/// 12  status i32
/// 16  kind u8
/// 17  flags u8
/// 18  reserved u16 = 0
/// 20  value0 u32
/// 24  value1 u64
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(C)]
pub struct Completion {
    /// The submission token returned by `host_submit`.
    pub event: CompletionId,
    /// The request tag.
    pub tag: Tag,
    /// Status.
    pub status: Status,
    /// Result kind.
    pub kind: CompletionKind,
    /// Completion flags.
    pub flags: u8,
    /// Reserved.
    pub reserved: u16,
    /// Kind-specific value 0.
    pub value0: u32,
    /// Kind-specific value 1.
    pub value1: u64,
}

impl Completion {
    /// Encoded length.
    pub const LEN: usize = 32;

    /// Encode to 32 bytes.
    pub fn encode(&self) -> [u8; Self::LEN] {
        let mut out = [0u8; Self::LEN];
        put_u64(&mut out, 0, self.event.0);
        put_u32(&mut out, 8, self.tag.0);
        put_i32(&mut out, 12, self.status as i32);
        out[16] = self.kind as u8;
        out[17] = self.flags;
        put_u16(&mut out, 18, self.reserved);
        put_u32(&mut out, 20, self.value0);
        put_u64(&mut out, 24, self.value1);
        out
    }

    /// Decode from 32 bytes.
    pub fn decode(bytes: &[u8]) -> Result<Self, Error> {
        if bytes.len() < Self::LEN {
            return Err(Error::Truncated);
        }
        let reserved = get_u16(bytes, 18);
        if reserved != 0 {
            return Err(Error::NonCanonical);
        }
        Ok(Self {
            event: CompletionId(get_u64(bytes, 0)),
            tag: Tag(get_u32(bytes, 8)),
            status: Status::from_i32(get_i32(bytes, 12))?,
            kind: CompletionKind::from_u8(bytes[16])?,
            flags: bytes[17],
            reserved,
            value0: get_u32(bytes, 20),
            value1: get_u64(bytes, 24),
        })
    }
}

// ---------------------------------------------------------------------------
// Trace context
// ---------------------------------------------------------------------------

/// W3C Trace Context fields carried inside the fabric header.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TraceContext {
    /// 16-byte trace-id.
    pub trace_id: [u8; 16],
    /// 8-byte parent-id / span-id.
    pub span_id: [u8; 8],
    /// Trace flags; bit 0 is `sampled`.
    pub flags: u8,
}

impl TraceContext {
    /// A child span on the same trace.
    pub fn child(&self, new_span: [u8; 8]) -> Self {
        Self {
            trace_id: self.trace_id,
            span_id: new_span,
            flags: self.flags,
        }
    }

    /// Format a W3C `traceparent` value (`00-<32hex>-<16hex>-<2hex>`).
    pub fn traceparent(&self, out: &mut [u8; 55]) -> Result<(), Error> {
        const HEX: &[u8; 16] = b"0123456789abcdef";
        out[0] = b'0';
        out[1] = b'0';
        out[2] = b'-';
        for (i, byte) in self.trace_id.iter().enumerate() {
            out[3 + i * 2] = HEX[(byte >> 4) as usize];
            out[4 + i * 2] = HEX[(byte & 0x0f) as usize];
        }
        out[35] = b'-';
        for (i, byte) in self.span_id.iter().enumerate() {
            out[36 + i * 2] = HEX[(byte >> 4) as usize];
            out[37 + i * 2] = HEX[(byte & 0x0f) as usize];
        }
        out[52] = b'-';
        out[53] = HEX[(self.flags >> 4) as usize];
        out[54] = HEX[(self.flags & 0x0f) as usize];
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Small encoding helpers
// ---------------------------------------------------------------------------

fn put_u16(out: &mut [u8], off: usize, value: u16) {
    out[off..off + 2].copy_from_slice(&value.to_le_bytes());
}

fn put_u32(out: &mut [u8], off: usize, value: u32) {
    out[off..off + 4].copy_from_slice(&value.to_le_bytes());
}

fn put_i32(out: &mut [u8], off: usize, value: i32) {
    out[off..off + 4].copy_from_slice(&value.to_le_bytes());
}

fn put_u64(out: &mut [u8], off: usize, value: u64) {
    out[off..off + 8].copy_from_slice(&value.to_le_bytes());
}

fn get_u16(input: &[u8], off: usize) -> u16 {
    u16::from_le_bytes([input[off], input[off + 1]])
}

fn get_u32(input: &[u8], off: usize) -> u32 {
    u32::from_le_bytes([input[off], input[off + 1], input[off + 2], input[off + 3]])
}

fn get_i32(input: &[u8], off: usize) -> i32 {
    i32::from_le_bytes([input[off], input[off + 1], input[off + 2], input[off + 3]])
}

fn get_u64(input: &[u8], off: usize) -> u64 {
    u64::from_le_bytes([
        input[off],
        input[off + 1],
        input[off + 2],
        input[off + 3],
        input[off + 4],
        input[off + 5],
        input[off + 6],
        input[off + 7],
    ])
}

/// CRC-32C (Castagnoli), the same polynomial used by iSCSI, ext4 and NVMe.
/// The implementation is table-free so it stays small on the Indy.
pub fn crc32c(data: &[u8]) -> u32 {
    let mut crc = 0xffff_ffffu32;
    for &byte in data {
        crc ^= byte as u32;
        for _ in 0..8 {
            let mask = (crc & 1).wrapping_neg();
            crc = (crc >> 1) ^ (0x82f6_3b78 & mask);
        }
    }
    !crc
}

// ---------------------------------------------------------------------------
// Compile-time layout checks
// ---------------------------------------------------------------------------

const _: () = assert!(core::mem::size_of::<ObjectId>() == 32);
const _: () = assert!(core::mem::size_of::<NodeKey>() == 32);
const _: () = assert!(core::mem::size_of::<NodeId>() == 16);
const _: () = assert!(core::mem::size_of::<CapHandle>() == 8);
const _: () = assert!(core::mem::size_of::<Capability>() == Capability::LEN);
const _: () = assert!(core::mem::offset_of!(Capability, object) == 8);
const _: () = assert!(core::mem::offset_of!(Capability, home) == 104);
const _: () = assert!(core::mem::offset_of!(Capability, delegation) == 200);
const _: () = assert!(core::mem::offset_of!(Capability, signature) == 256);
const _: () = assert!(core::mem::size_of::<FrameHeader>() == FrameHeader::LEN);
const _: () = assert!(core::mem::offset_of!(FrameHeader, cap) == 32);
const _: () = assert!(core::mem::offset_of!(FrameHeader, object) == 40);
const _: () = assert!(core::mem::offset_of!(FrameHeader, header_crc32c) == 124);
const _: () = assert!(core::mem::size_of::<PayloadDescriptor>() == PayloadDescriptor::LEN);
const _: () = assert!(core::mem::size_of::<MeshHeader>() == MeshHeader::LEN);
const _: () = assert!(core::mem::offset_of!(MeshHeader, src) == 8);
const _: () = assert!(core::mem::offset_of!(MeshHeader, home) == 40);
const _: () = assert!(core::mem::offset_of!(MeshHeader, frame_len) == 108);
const _: () = assert!(core::mem::offset_of!(MeshHeader, header_crc32c) == 124);
const _: () = assert!(core::mem::size_of::<Completion>() == Completion::LEN);
const _: () = assert!(core::mem::size_of::<TraceContext>() == 25);
