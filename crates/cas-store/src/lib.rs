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
use alloc::string::String;
use alloc::vec::Vec;
use cas_core::{object_oid, CasError, Directory, ObjectKind, MAX_DIRECTORY_NAME};
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

/// Namespace failure.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NamespaceError {
    /// A path component does not exist.
    Missing,
    /// The path is empty, contains `..`, or names the root when an entry is required.
    InvalidPath,
    /// A path component exists but is not a directory.
    NotDirectory,
    /// A stored directory object is malformed.
    Cas(CasError),
}

impl From<CasError> for NamespaceError {
    fn from(value: CasError) -> Self {
        Self::Cas(value)
    }
}

impl fmt::Display for NamespaceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Missing => f.write_str("namespace entry not found"),
            Self::InvalidPath => f.write_str("invalid namespace path"),
            Self::NotDirectory => f.write_str("namespace entry is not a directory"),
            Self::Cas(error) => write!(f, "bad directory object: {error}"),
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

/// A local namespace over an in-memory CAS.
///
/// The root and every child directory are ordinary [`Directory`] objects in
/// the store.  Lookup follows direct children, so a path read is one tree
/// decode per component, not a global snapshot.  Updates copy the affected
/// directory objects upward and publish a new root OID.  This is the
/// single-node namespace policy; remote homes and leases are layered above it.
pub struct Namespace {
    root: ObjectId,
    store: MemoryStore,
}

impl Namespace {
    /// Create a namespace with one empty root directory.
    pub fn new() -> Self {
        let mut store = MemoryStore::new();
        let root = store.put(ObjectKind::Tree, &Directory::new().encode());
        Self { root, store }
    }

    /// Current root directory OID.
    pub const fn root(&self) -> ObjectId {
        self.root
    }

    /// Read-only view of the backing store.
    pub const fn store(&self) -> &MemoryStore {
        &self.store
    }

    /// Resolve a path to its object OID.  `/` resolves to the root directory.
    pub fn lookup(&self, path: &str) -> Result<ObjectId, NamespaceError> {
        let components = components(path)?;
        let mut current = self.root;
        for (index, name) in components.iter().enumerate() {
            let directory = self.load_directory(current)?;
            let next = directory.get(name).ok_or(NamespaceError::Missing)?;
            if index + 1 == components.len() {
                return Ok(next);
            }
            current = next;
        }
        Ok(current)
    }

    /// Fetch an object by path and validate its recorded kind.
    pub fn get(&self, path: &str) -> Result<(ObjectKind, &[u8]), NamespaceError> {
        let oid = self.lookup(path)?;
        let bytes = self.store.get(oid).ok_or(NamespaceError::Missing)?;
        let kind = self.store.kind(oid).ok_or(NamespaceError::Missing)?;
        Ok((kind, bytes))
    }

    /// Store bytes and publish them at `path`.
    ///
    /// The parent directories must already exist.
    pub fn put(
        &mut self,
        path: &str,
        kind: ObjectKind,
        bytes: &[u8],
    ) -> Result<ObjectId, NamespaceError> {
        // Resolve the path before publishing the object so a missing parent
        // does not leave an unreachable blob behind.
        let oid = object_oid(kind, bytes);
        self.insert(path, oid)?;
        let stored = self.store.put(kind, bytes);
        debug_assert_eq!(stored, oid);
        Ok(oid)
    }

    /// Publish an existing OID at `path`.
    pub fn insert(&mut self, path: &str, oid: ObjectId) -> Result<(), NamespaceError> {
        let components = components(path)?;
        if components.is_empty() {
            return Err(NamespaceError::InvalidPath);
        }
        let root = self.insert_at(self.root, &components, oid)?;
        self.root = root;
        Ok(())
    }

    /// Create `path` and any missing parents.
    pub fn create_dir_all(&mut self, path: &str) -> Result<ObjectId, NamespaceError> {
        let components = components(path)?;
        let root = self.ensure_dir_at(self.root, &components)?;
        self.root = root;
        Ok(self.root)
    }

    /// List direct children of a directory path.
    pub fn list(&self, path: &str) -> Result<Vec<String>, NamespaceError> {
        let oid = self.lookup(path)?;
        let directory = self.load_directory(oid)?;
        Ok(directory
            .iter()
            .map(|(name, _)| String::from(name))
            .collect())
    }

    fn load_directory(&self, oid: ObjectId) -> Result<Directory, NamespaceError> {
        if self.store.kind(oid) != Some(ObjectKind::Tree) {
            return Err(NamespaceError::NotDirectory);
        }
        let bytes = self.store.get(oid).ok_or(NamespaceError::Missing)?;
        Directory::decode(bytes).map_err(NamespaceError::from)
    }

    fn insert_at(
        &mut self,
        dir_oid: ObjectId,
        components: &[&str],
        oid: ObjectId,
    ) -> Result<ObjectId, NamespaceError> {
        let mut directory = self.load_directory(dir_oid)?;
        let name = components[0];
        if components.len() == 1 {
            directory.insert(name, oid)?;
            return Ok(self.store.put(ObjectKind::Tree, &directory.encode()));
        }
        let child = directory.get(name).ok_or(NamespaceError::Missing)?;
        let child = self.insert_at(child, &components[1..], oid)?;
        directory.insert(name, child)?;
        Ok(self.store.put(ObjectKind::Tree, &directory.encode()))
    }

    fn ensure_dir_at(
        &mut self,
        dir_oid: ObjectId,
        components: &[&str],
    ) -> Result<ObjectId, NamespaceError> {
        if components.is_empty() {
            return Ok(dir_oid);
        }
        let mut directory = self.load_directory(dir_oid)?;
        let name = components[0];
        let child = match directory.get(name) {
            Some(oid) if self.store.kind(oid) == Some(ObjectKind::Tree) => oid,
            Some(_) => return Err(NamespaceError::NotDirectory),
            None => self.store.put(ObjectKind::Tree, &Directory::new().encode()),
        };
        let child = self.ensure_dir_at(child, &components[1..])?;
        directory.insert(name, child)?;
        Ok(self.store.put(ObjectKind::Tree, &directory.encode()))
    }
}

impl Default for Namespace {
    fn default() -> Self {
        Self::new()
    }
}

fn components(path: &str) -> Result<Vec<&str>, NamespaceError> {
    if path.is_empty() {
        return Err(NamespaceError::InvalidPath);
    }
    let mut result = Vec::new();
    for part in path.split('/') {
        if part.is_empty() || part == "." {
            continue;
        }
        if part == ".." || part.len() > MAX_DIRECTORY_NAME {
            return Err(NamespaceError::InvalidPath);
        }
        result.push(part);
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

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
    #[test]
    fn namespace_creates_directories_and_resolves_paths() {
        let mut namespace = Namespace::new();
        let root = namespace.root();
        assert_eq!(namespace.lookup("/"), Ok(root));
        assert!(namespace.list("/").expect("root list").is_empty());

        let etc = namespace.create_dir_all("/etc").expect("etc");
        assert_eq!(etc, namespace.root());
        namespace
            .create_dir_all("/etc/ssl")
            .expect("nested directory");
        let motd = namespace
            .put("/etc/motd", ObjectKind::Blob, b"Welcome to IRIX 7.\n")
            .expect("motd");
        assert_eq!(namespace.lookup("/etc/motd"), Ok(motd));
        let (kind, bytes) = namespace.get("/etc/motd").expect("get");
        assert_eq!(kind, ObjectKind::Blob);
        assert_eq!(bytes, b"Welcome to IRIX 7.\n");
        assert_eq!(
            namespace.list("/etc").expect("list"),
            vec![String::from("motd"), String::from("ssl")]
        );
        assert_eq!(
            namespace.lookup("/etc/missing"),
            Err(NamespaceError::Missing)
        );
    }

    #[test]
    fn namespace_updates_are_copy_on_write() {
        let mut namespace = Namespace::new();
        let first = namespace
            .put("/hello.txt", ObjectKind::Blob, b"one")
            .expect("first");
        let old_root = namespace.root();
        let second = namespace
            .put("/hello.txt", ObjectKind::Blob, b"two")
            .expect("second");
        assert_ne!(first, second);
        assert_ne!(old_root, namespace.root());
        assert_eq!(namespace.lookup("/hello.txt"), Ok(second));
        assert_eq!(namespace.get("/hello.txt").expect("get").1, b"two");
        // The old root is still a valid historical directory object.
        assert!(namespace.store().contains(old_root));
    }

    #[test]
    fn namespace_rejects_invalid_paths_and_kind_confusion() {
        let mut namespace = Namespace::new();
        assert_eq!(
            namespace.lookup("/../etc"),
            Err(NamespaceError::InvalidPath)
        );
        assert_eq!(namespace.lookup(""), Err(NamespaceError::InvalidPath));
        namespace
            .put("/blob", ObjectKind::Blob, b"not a directory")
            .expect("blob");
        assert_eq!(
            namespace.create_dir_all("/blob/child"),
            Err(NamespaceError::NotDirectory)
        );
        assert_eq!(
            namespace.insert("/blob/child", ObjectId([7; 32])),
            Err(NamespaceError::NotDirectory)
        );
        assert_eq!(
            namespace.insert("/", ObjectId([7; 32])),
            Err(NamespaceError::InvalidPath)
        );
    }
}
