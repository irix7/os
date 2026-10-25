// SPDX-License-Identifier: GPL-3.0-only
//! Content-addressed objects and manifests.
//!
//! OIDs are BLAKE3-256 over `"IRIX7-OBJ-v1" || kind || length_le64 || bytes`.
//! The kind and length are part of the digest so the same bytes under a
//! different object type cannot alias. Manifests and directory objects are
//! canonical little-endian records and are themselves OIDs.

#![no_std]
#![forbid(unsafe_code)]
#![deny(missing_docs)]

extern crate alloc;

use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;
use core::fmt;
use sys_primitives::ObjectId;

/// Domain separator for every object digest.
pub const OBJECT_DOMAIN: &[u8] = b"IRIX7-OBJ-v1";

/// Manifest magic (`"I7MN"` little-endian).
pub const MANIFEST_MAGIC: u32 = 0x4e4d_3749;

/// Manifest wire version.
pub const MANIFEST_VERSION: u8 = 1;

/// Maximum chunks accepted by the decoder.
pub const MAX_CHUNKS: usize = 1_000_000;

/// Directory object magic (`"I7DR"` little-endian).
pub const DIRECTORY_MAGIC: u32 = 0x5244_3749;

/// Directory object wire version.
pub const DIRECTORY_VERSION: u8 = 1;

/// Maximum directory entries accepted by the decoder.
pub const MAX_DIRECTORY_ENTRIES: usize = 65_535;

/// Maximum directory entry name length in bytes.
pub const MAX_DIRECTORY_NAME: usize = 255;

/// Object kind included in the OID domain.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum ObjectKind {
    /// Arbitrary immutable bytes.
    Blob = 1,
    /// One CAS chunk.
    Chunk = 2,
    /// A chunk manifest.
    Manifest = 3,
    /// A directory tree node.
    Tree = 4,
    /// A Wasm actor image.
    ActorImage = 5,
    /// An actor snapshot.
    ActorState = 6,
    /// A signed capability audit record.
    Capability = 7,
    /// A retained actor snapshot.
    Snapshot = 8,
    /// A configuration root.
    Config = 9,
}

impl ObjectKind {
    /// Decode a wire kind.
    pub const fn from_u8(value: u8) -> Option<Self> {
        match value {
            1 => Some(Self::Blob),
            2 => Some(Self::Chunk),
            3 => Some(Self::Manifest),
            4 => Some(Self::Tree),
            5 => Some(Self::ActorImage),
            6 => Some(Self::ActorState),
            7 => Some(Self::Capability),
            8 => Some(Self::Snapshot),
            9 => Some(Self::Config),
            _ => None,
        }
    }

    /// Wire discriminant.
    pub const fn as_u8(self) -> u8 {
        self as u8
    }
}

/// Manifest decode/validation failure.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CasError {
    /// The magic is not `I7MN`.
    BadMagic,
    /// The manifest version is unknown.
    BadVersion,
    /// The input ended early or a length is inconsistent.
    BadLength,
    /// The chunk count exceeds [`MAX_CHUNKS`].
    TooManyChunks,
    /// A chunk length or total overflows.
    Overflow,
    /// The chunk lengths do not sum to the manifest total.
    LengthMismatch,
    /// A chunk reference does not verify against its bytes.
    ChunkMismatch,
    /// A directory name is empty, too long, or contains a reserved byte.
    BadName,
    /// Directory entries are not in canonical sorted order.
    NotSorted,
}

impl fmt::Display for CasError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::BadMagic => f.write_str("bad manifest magic"),
            Self::BadVersion => f.write_str("bad manifest version"),
            Self::BadLength => f.write_str("bad manifest length"),
            Self::TooManyChunks => f.write_str("too many manifest chunks"),
            Self::Overflow => f.write_str("manifest length overflow"),
            Self::LengthMismatch => f.write_str("manifest chunk lengths do not sum"),
            Self::ChunkMismatch => f.write_str("chunk digest mismatch"),
            Self::BadName => f.write_str("invalid directory entry name"),
            Self::NotSorted => f.write_str("directory entries are not sorted"),
        }
    }
}

/// Digest an object with its kind and length in the domain.
pub fn object_oid(kind: ObjectKind, bytes: &[u8]) -> ObjectId {
    let mut hasher = blake3::Hasher::new();
    hasher.update(OBJECT_DOMAIN);
    hasher.update(&[kind.as_u8()]);
    hasher.update(&(bytes.len() as u64).to_le_bytes());
    hasher.update(bytes);
    let digest = hasher.finalize();
    let mut out = [0u8; 32];
    out.copy_from_slice(digest.as_bytes());
    ObjectId(out)
}

/// Digest a chunk.
pub fn chunk_oid(bytes: &[u8]) -> ObjectId {
    object_oid(ObjectKind::Chunk, bytes)
}

/// Verify one immutable chunk.
pub fn verify_chunk(oid: ObjectId, bytes: &[u8]) -> bool {
    chunk_oid(bytes) == oid
}

/// One chunk reference in a manifest.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ChunkRef {
    /// Content OID.
    pub oid: ObjectId,
    /// Chunk length in bytes.
    pub len: u64,
}

impl ChunkRef {
    /// Create a reference for `bytes`.
    pub fn new(bytes: &[u8]) -> Self {
        Self {
            oid: chunk_oid(bytes),
            len: bytes.len() as u64,
        }
    }
}

/// A canonical list of chunks for a larger immutable object.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Manifest {
    /// Target chunk size used by the splitter.
    pub chunk_size: u32,
    /// Total uncompressed length.
    pub total_len: u64,
    /// Chunk references in order.
    pub chunks: Vec<ChunkRef>,
}

impl Manifest {
    /// Build a manifest, checking the total length.
    pub fn new(chunk_size: u32, chunks: Vec<ChunkRef>) -> Result<Self, CasError> {
        let mut total_len = 0u64;
        for chunk in &chunks {
            total_len = total_len.checked_add(chunk.len).ok_or(CasError::Overflow)?;
        }
        Ok(Self {
            chunk_size,
            total_len,
            chunks,
        })
    }

    /// Canonical byte encoding.
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(24 + self.chunks.len() * 40);
        out.extend_from_slice(&MANIFEST_MAGIC.to_le_bytes());
        out.push(MANIFEST_VERSION);
        out.push(ObjectKind::Manifest.as_u8());
        out.extend_from_slice(&[0u8; 2]);
        out.extend_from_slice(&self.chunk_size.to_le_bytes());
        out.extend_from_slice(&self.total_len.to_le_bytes());
        out.extend_from_slice(&(self.chunks.len() as u32).to_le_bytes());
        for chunk in &self.chunks {
            out.extend_from_slice(chunk.oid.as_bytes());
            out.extend_from_slice(&chunk.len.to_le_bytes());
        }
        out
    }

    /// Decode and validate a canonical manifest.
    pub fn decode(bytes: &[u8]) -> Result<Self, CasError> {
        if bytes.len() < 24 {
            return Err(CasError::BadLength);
        }
        if u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]) != MANIFEST_MAGIC {
            return Err(CasError::BadMagic);
        }
        if bytes[4] != MANIFEST_VERSION {
            return Err(CasError::BadVersion);
        }
        if bytes[5] != ObjectKind::Manifest.as_u8() || bytes[6] != 0 || bytes[7] != 0 {
            return Err(CasError::BadLength);
        }
        let chunk_size = u32::from_le_bytes([bytes[8], bytes[9], bytes[10], bytes[11]]);
        let total_len = u64::from_le_bytes([
            bytes[12], bytes[13], bytes[14], bytes[15], bytes[16], bytes[17], bytes[18], bytes[19],
        ]);
        let count = u32::from_le_bytes([bytes[20], bytes[21], bytes[22], bytes[23]]) as usize;
        if count > MAX_CHUNKS {
            return Err(CasError::TooManyChunks);
        }
        let expected = 24usize
            .checked_add(count.checked_mul(40).ok_or(CasError::Overflow)?)
            .ok_or(CasError::Overflow)?;
        if bytes.len() != expected {
            return Err(CasError::BadLength);
        }
        let mut chunks = Vec::with_capacity(count);
        let mut offset = 24;
        let mut sum = 0u64;
        for _ in 0..count {
            let oid = ObjectId::from_slice(&bytes[offset..offset + 32])
                .map_err(|_| CasError::BadLength)?;
            let len = u64::from_le_bytes([
                bytes[offset + 32],
                bytes[offset + 33],
                bytes[offset + 34],
                bytes[offset + 35],
                bytes[offset + 36],
                bytes[offset + 37],
                bytes[offset + 38],
                bytes[offset + 39],
            ]);
            sum = sum.checked_add(len).ok_or(CasError::Overflow)?;
            chunks.push(ChunkRef { oid, len });
            offset += 40;
        }
        if sum != total_len {
            return Err(CasError::LengthMismatch);
        }
        Ok(Self {
            chunk_size,
            total_len,
            chunks,
        })
    }

    /// OID of the canonical manifest encoding.
    pub fn oid(&self) -> ObjectId {
        object_oid(ObjectKind::Manifest, &self.encode())
    }

    /// Verify every chunk of `bytes` in order.
    pub fn verify(&self, bytes: &[u8]) -> bool {
        if bytes.len() as u64 != self.total_len {
            return false;
        }
        let mut offset = 0usize;
        for chunk in &self.chunks {
            let end = match offset.checked_add(chunk.len as usize) {
                Some(end) => end,
                None => return false,
            };
            if end > bytes.len() || !verify_chunk(chunk.oid, &bytes[offset..end]) {
                return false;
            }
            offset = end;
        }
        offset == bytes.len()
    }
}

/// One immutable directory object.
///
/// Entries are sorted by name and encoded in that order.  The encoding is
/// little-endian and deliberately small:
///
/// ```text
///   0   magic "I7DR"            4
///   4   version = 1             1
///   5   kind = Tree             1
///   6   reserved = 0            2
///   8   entry count             4
///  12   entries, sorted:
///         name length           2
///         flags = 0             2
///         object OID           32
///         UTF-8 name        1..255
/// ```
///
/// Names must not be empty, `.`, `..`, contain `/` or NUL, or exceed 255
/// bytes.  The encoder emits canonical bytes; the decoder rejects unsorted or
/// non-canonical input.  A directory does not know the kinds of its targets;
/// the store answers that from the OID.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Directory {
    entries: BTreeMap<String, ObjectId>,
}

impl Directory {
    /// Create an empty directory.
    pub const fn new() -> Self {
        Self {
            entries: BTreeMap::new(),
        }
    }

    /// Number of entries.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether the directory is empty.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Look up one direct child.
    pub fn get(&self, name: &str) -> Option<ObjectId> {
        self.entries.get(name).copied()
    }

    /// Insert or replace one child.
    ///
    /// Returns the previous OID, if the name existed.
    pub fn insert(&mut self, name: &str, oid: ObjectId) -> Result<Option<ObjectId>, CasError> {
        validate_name(name)?;
        Ok(self.entries.insert(String::from(name), oid))
    }

    /// Remove one child.
    pub fn remove(&mut self, name: &str) -> Option<ObjectId> {
        self.entries.remove(name)
    }

    /// Iterate over entries in canonical name order.
    pub fn iter(&self) -> impl Iterator<Item = (&str, ObjectId)> {
        self.entries.iter().map(|(name, oid)| (name.as_str(), *oid))
    }

    /// Canonical encoding.
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(12 + self.entries.len() * 36);
        out.extend_from_slice(&DIRECTORY_MAGIC.to_le_bytes());
        out.push(DIRECTORY_VERSION);
        out.push(ObjectKind::Tree.as_u8());
        out.extend_from_slice(&[0u8; 2]);
        out.extend_from_slice(&(self.entries.len() as u32).to_le_bytes());
        for (name, oid) in &self.entries {
            out.extend_from_slice(&(name.len() as u16).to_le_bytes());
            out.extend_from_slice(&[0u8; 2]);
            out.extend_from_slice(oid.as_bytes());
            out.extend_from_slice(name.as_bytes());
        }
        out
    }

    /// OID of the canonical encoding.
    pub fn oid(&self) -> ObjectId {
        object_oid(ObjectKind::Tree, &self.encode())
    }

    /// Decode and strictly validate a directory object.
    pub fn decode(bytes: &[u8]) -> Result<Self, CasError> {
        if bytes.len() < 12 {
            return Err(CasError::BadLength);
        }
        if u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]) != DIRECTORY_MAGIC {
            return Err(CasError::BadMagic);
        }
        if bytes[4] != DIRECTORY_VERSION {
            return Err(CasError::BadVersion);
        }
        if bytes[5] != ObjectKind::Tree.as_u8() || bytes[6] != 0 || bytes[7] != 0 {
            return Err(CasError::BadLength);
        }
        let count = u32::from_le_bytes([bytes[8], bytes[9], bytes[10], bytes[11]]) as usize;
        if count > MAX_DIRECTORY_ENTRIES || count > (bytes.len() - 12) / 36 {
            return Err(CasError::BadLength);
        }
        let mut entries = BTreeMap::new();
        let mut offset = 12usize;
        let mut previous: Option<String> = None;
        for _ in 0..count {
            if bytes.len() - offset < 36 {
                return Err(CasError::BadLength);
            }
            let name_len = u16::from_le_bytes([bytes[offset], bytes[offset + 1]]) as usize;
            if name_len == 0 || name_len > MAX_DIRECTORY_NAME {
                return Err(CasError::BadName);
            }
            if bytes[offset + 2] != 0 || bytes[offset + 3] != 0 {
                return Err(CasError::BadLength);
            }
            let oid = ObjectId::from_slice(&bytes[offset + 4..offset + 36])
                .map_err(|_| CasError::BadLength)?;
            offset += 36;
            if bytes.len() - offset < name_len {
                return Err(CasError::BadLength);
            }
            let name = core::str::from_utf8(&bytes[offset..offset + name_len])
                .map_err(|_| CasError::BadName)?;
            validate_name(name)?;
            if let Some(last) = &previous {
                if last.as_str() >= name {
                    return Err(CasError::NotSorted);
                }
            }
            entries.insert(String::from(name), oid);
            previous = Some(String::from(name));
            offset += name_len;
        }
        if offset != bytes.len() {
            return Err(CasError::BadLength);
        }
        Ok(Self { entries })
    }
}

fn validate_name(name: &str) -> Result<(), CasError> {
    if name.is_empty()
        || name.len() > MAX_DIRECTORY_NAME
        || name == "."
        || name == ".."
        || name.as_bytes().contains(&b'/')
        || name.as_bytes().contains(&0)
    {
        return Err(CasError::BadName);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    #[test]
    fn kind_and_length_are_in_the_digest() {
        let a = object_oid(ObjectKind::Blob, b"hello");
        let b = object_oid(ObjectKind::Chunk, b"hello");
        let c = object_oid(ObjectKind::Blob, b"hello!");
        assert_ne!(a, b);
        assert_ne!(a, c);
        assert_eq!(a, object_oid(ObjectKind::Blob, b"hello"));
    }

    #[test]
    fn chunk_verification() {
        let oid = chunk_oid(b"chunk");
        assert!(verify_chunk(oid, b"chunk"));
        assert!(!verify_chunk(oid, b"chunk!"));
    }

    #[test]
    fn manifest_round_trip_and_oid() {
        let chunks = vec![ChunkRef::new(b"alpha"), ChunkRef::new(b"beta")];
        let manifest = Manifest::new(64, chunks).expect("manifest");
        assert_eq!(manifest.total_len, 9);
        let encoded = manifest.encode();
        let decoded = Manifest::decode(&encoded).expect("decode");
        assert_eq!(decoded, manifest);
        assert_eq!(decoded.oid(), manifest.oid());
        assert!(decoded.verify(b"alphabeta"));
        assert!(!decoded.verify(b"alpha"));
    }

    #[test]
    fn malformed_manifests_fail_closed() {
        let manifest = Manifest::new(1, vec![ChunkRef::new(b"x")]).expect("manifest");
        let mut encoded = manifest.encode();
        encoded[0] ^= 1;
        assert_eq!(Manifest::decode(&encoded), Err(CasError::BadMagic));
        let mut encoded = manifest.encode();
        encoded[4] = 2;
        assert_eq!(Manifest::decode(&encoded), Err(CasError::BadVersion));
        let mut encoded = manifest.encode();
        encoded.pop();
        assert_eq!(Manifest::decode(&encoded), Err(CasError::BadLength));
    }
    #[test]
    fn directory_round_trip_is_canonical() {
        let mut directory = Directory::new();
        assert!(directory.is_empty());
        let hello = object_oid(ObjectKind::Blob, b"hello");
        let motd = object_oid(ObjectKind::Blob, b"motd");
        directory.insert("hello.txt", hello).expect("name");
        directory
            .insert("etc", object_oid(ObjectKind::Tree, b"etc"))
            .expect("name");
        directory.insert("motd", motd).expect("name");
        let encoded = directory.encode();
        let decoded = Directory::decode(&encoded).expect("decode");
        assert_eq!(decoded, directory);
        assert_eq!(decoded.get("hello.txt"), Some(hello));
        assert_eq!(decoded.len(), 3);
        assert_eq!(decoded.oid(), directory.oid());
        let names: Vec<&str> = decoded.iter().map(|(name, _)| name).collect();
        assert_eq!(names, vec!["etc", "hello.txt", "motd"]);
    }

    #[test]
    fn directory_rejects_noncanonical_input() {
        // Build an unsorted two-entry object directly.
        let mut unsorted = Vec::new();
        unsorted.extend_from_slice(&DIRECTORY_MAGIC.to_le_bytes());
        unsorted.push(DIRECTORY_VERSION);
        unsorted.push(ObjectKind::Tree.as_u8());
        unsorted.extend_from_slice(&[0; 2]);
        unsorted.extend_from_slice(&2u32.to_le_bytes());
        for name in ["b", "a"] {
            unsorted.extend_from_slice(&(name.len() as u16).to_le_bytes());
            unsorted.extend_from_slice(&[0; 2]);
            unsorted.extend_from_slice(object_oid(ObjectKind::Blob, name.as_bytes()).as_bytes());
            unsorted.extend_from_slice(name.as_bytes());
        }
        assert_eq!(Directory::decode(&unsorted), Err(CasError::NotSorted));
    }

    #[test]
    fn directory_names_are_bounded() {
        let mut directory = Directory::new();
        let oid = object_oid(ObjectKind::Blob, b"x");
        assert_eq!(directory.insert("", oid), Err(CasError::BadName));
        assert_eq!(directory.insert(".", oid), Err(CasError::BadName));
        assert_eq!(directory.insert("..", oid), Err(CasError::BadName));
        assert_eq!(directory.insert("a/b", oid), Err(CasError::BadName));
        assert_eq!(directory.insert("a\0b", oid), Err(CasError::BadName));
        assert_eq!(
            directory.insert(&"x".repeat(MAX_DIRECTORY_NAME + 1), oid),
            Err(CasError::BadName)
        );
    }
}
