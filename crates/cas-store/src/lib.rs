// SPDX-License-Identifier: GPL-3.0-only
//! In-memory content-addressed store policy.
//!
//! The block store, chunking, superblock and crash consistency live below this
//! layer. This crate proves the object and pin state machine on the host and
//! is shared by the native and Wasm-side tests.

#![no_std]
#![forbid(unsafe_code)]
#![deny(missing_docs)]

extern crate alloc;

use alloc::collections::BTreeMap;
use alloc::vec::Vec;
use cas_core::{object_oid, ObjectKind};
use core::fmt;
use sys_primitives::ObjectId;

/// Store failure.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StoreError {
    /// The object is not present.
    Missing,
    /// The object has at least one pin and cannot be removed.
    Pinned,
}

impl fmt::Display for StoreError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Missing => f.write_str("object not found"),
            Self::Pinned => f.write_str("object is pinned"),
        }
    }
}

struct StoredObject {
    kind: ObjectKind,
    bytes: Vec<u8>,
    pins: u32,
}

/// A deterministic in-memory CAS.
#[derive(Default)]
pub struct MemoryStore {
    objects: BTreeMap<ObjectId, StoredObject>,
}

impl MemoryStore {
    /// Create an empty store.
    pub const fn new() -> Self {
        Self {
            objects: BTreeMap::new(),
        }
    }

    /// Store immutable bytes and return their OID.
    ///
    /// Re-putting identical bytes is idempotent.
    pub fn put(&mut self, kind: ObjectKind, bytes: &[u8]) -> ObjectId {
        let oid = object_oid(kind, bytes);
        if let Some(existing) = self.objects.get_mut(&oid) {
            debug_assert_eq!(existing.kind, kind);
            return oid;
        }
        self.objects.insert(
            oid,
            StoredObject {
                kind,
                bytes: bytes.to_vec(),
                pins: 0,
            },
        );
        oid
    }

    /// Fetch immutable bytes.
    pub fn get(&self, oid: ObjectId) -> Option<&[u8]> {
        self.objects.get(&oid).map(|object| object.bytes.as_slice())
    }

    /// Object kind, if present.
    pub fn kind(&self, oid: ObjectId) -> Option<ObjectKind> {
        self.objects.get(&oid).map(|object| object.kind)
    }

    /// Whether the object is present.
    pub fn contains(&self, oid: ObjectId) -> bool {
        self.objects.contains_key(&oid)
    }

    /// Number of stored objects.
    pub fn len(&self) -> usize {
        self.objects.len()
    }

    /// Whether the store is empty.
    pub fn is_empty(&self) -> bool {
        self.objects.is_empty()
    }

    /// Pin an object. Returns false if it is missing.
    pub fn pin(&mut self, oid: ObjectId) -> bool {
        if let Some(object) = self.objects.get_mut(&oid) {
            object.pins = object.pins.saturating_add(1);
            true
        } else {
            false
        }
    }

    /// Release one pin. Returns false if the object is missing or unpinned.
    pub fn unpin(&mut self, oid: ObjectId) -> bool {
        let Some(object) = self.objects.get_mut(&oid) else {
            return false;
        };
        if object.pins == 0 {
            return false;
        }
        object.pins -= 1;
        true
    }

    /// Pin count, if present.
    pub fn pins(&self, oid: ObjectId) -> Option<u32> {
        self.objects.get(&oid).map(|object| object.pins)
    }

    /// Remove an unpinned object.
    pub fn remove(&mut self, oid: ObjectId) -> Result<bool, StoreError> {
        let Some(object) = self.objects.get(&oid) else {
            return Err(StoreError::Missing);
        };
        if object.pins != 0 {
            return Err(StoreError::Pinned);
        }
        self.objects.remove(&oid);
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn put_fetch_and_idempotent_put() {
        let mut store = MemoryStore::new();
        let oid = store.put(ObjectKind::Blob, b"hello");
        assert_eq!(store.get(oid), Some(b"hello".as_slice()));
        assert_eq!(store.kind(oid), Some(ObjectKind::Blob));
        let again = store.put(ObjectKind::Blob, b"hello");
        assert_eq!(again, oid);
        assert_eq!(store.len(), 1);
    }

    #[test]
    fn kind_and_bytes_define_the_identity() {
        let mut store = MemoryStore::new();
        let blob = store.put(ObjectKind::Blob, b"hello");
        let chunk = store.put(ObjectKind::Chunk, b"hello");
        assert_ne!(blob, chunk);
        assert_eq!(store.len(), 2);
    }

    #[test]
    fn pins_prevent_removal() {
        let mut store = MemoryStore::new();
        let oid = store.put(ObjectKind::Blob, b"pinned");
        assert!(store.pin(oid));
        assert_eq!(store.pins(oid), Some(1));
        assert_eq!(store.remove(oid), Err(StoreError::Pinned));
        assert!(store.unpin(oid));
        assert_eq!(store.remove(oid), Ok(true));
        assert!(store.is_empty());
    }

    #[test]
    fn missing_objects_fail_closed() {
        let mut store = MemoryStore::new();
        let missing = ObjectId([9; 32]);
        assert!(store.get(missing).is_none());
        assert!(!store.pin(missing));
        assert!(!store.unpin(missing));
        assert_eq!(store.remove(missing), Err(StoreError::Missing));
    }
}
