// SPDX-License-Identifier: GPL-3.0-only
//! Host model of the future module manager.
//!
//! The manager consumes a [`Registry`] and a [`ImageSource`], verifies every
//! not-yet-loaded image against its recorded SHA-256, and publishes the whole
//! resolved set atomically. It does **not** decide slots, relocate or execute
//! an image; those belong to the downloadable module manager and its arena.
//!
//! A load failure (missing image or digest mismatch) leaves every existing
//! loaded module and reference count unchanged. Re-loading a root increments
//! the reference count for every module in its resolution; unloading decrements
//! it and removes modules that reach zero.

use alloc::collections::BTreeMap;
use alloc::vec::Vec;
use core::fmt;
use sha2::{Digest as ShaDigest, Sha256};

use crate::{resolve, Abi, Digest, ModuleId, Record, Registry, Resolution, ResolveError};

/// Source of module image bytes, keyed by exact module identity.
pub trait ImageSource {
    /// Fetch the raw image. The manager verifies its digest before use.
    fn fetch(&self, id: &ModuleId) -> Option<Vec<u8>>;
}

/// A loaded module.
struct Loaded {
    record: Record,
    image: Vec<u8>,
    refs: u32,
}

/// Host manager failure.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ManagerError {
    /// Dependency resolution failed.
    Resolve(ResolveError),
    /// The source does not contain the module.
    MissingImage(ModuleId),
    /// The source image does not match the recorded digest.
    DigestMismatch {
        /// Module identity.
        module: ModuleId,
        /// Expected digest from the registry.
        expected: Digest,
        /// Digest of the fetched image.
        actual: Digest,
    },
    /// The module is not loaded.
    NotLoaded(ModuleId),
}

impl fmt::Display for ManagerError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Resolve(error) => write!(f, "resolve failed: {error}"),
            Self::MissingImage(id) => write!(f, "missing image for {}@{}", id.name, id.version),
            Self::DigestMismatch {
                module,
                expected,
                actual,
            } => write!(
                f,
                "image digest mismatch for {}@{}: expected {:02x?}, got {:02x?}",
                module.name, module.version, expected, actual
            ),
            Self::NotLoaded(id) => write!(f, "module {}@{} is not loaded", id.name, id.version),
        }
    }
}

impl From<ResolveError> for ManagerError {
    fn from(error: ResolveError) -> Self {
        Self::Resolve(error)
    }
}

/// Host-side loaded-module manager.
pub struct ModuleManager {
    registry: Registry,
    host: Abi,
    loaded: BTreeMap<ModuleId, Loaded>,
}

impl ModuleManager {
    /// Create a manager over a registry and host ABI.
    pub fn new(registry: Registry, host: Abi) -> Self {
        Self {
            registry,
            host,
            loaded: BTreeMap::new(),
        }
    }

    /// Resolve, fetch, verify and publish a root module and its dependencies.
    ///
    /// All images are fetched and verified before any load state changes.
    pub fn load(
        &mut self,
        root: &Record,
        source: &dyn ImageSource,
    ) -> Result<Resolution, ManagerError> {
        let resolution = resolve(root, self.host, &self.registry)?;

        let mut fetched = Vec::new();
        for id in &resolution.order {
            if self.loaded.contains_key(id) {
                continue;
            }
            let record = self
                .registry
                .get(id)
                .expect("resolver only returns registered modules");
            let image = source
                .fetch(id)
                .ok_or_else(|| ManagerError::MissingImage(id.clone()))?;
            let actual = sha256(&image);
            if actual != record.digest {
                return Err(ManagerError::DigestMismatch {
                    module: id.clone(),
                    expected: record.digest,
                    actual,
                });
            }
            fetched.push((id.clone(), record.clone(), image));
        }

        // Verification completed; publish the whole set and take references.
        for (id, record, image) in fetched {
            self.loaded.insert(
                id,
                Loaded {
                    record,
                    image,
                    refs: 0,
                },
            );
        }
        for id in &resolution.order {
            let loaded = self
                .loaded
                .get_mut(id)
                .expect("all modules were just published");
            loaded.refs = loaded.refs.saturating_add(1);
        }
        Ok(resolution)
    }

    /// Release one reference for a root and its dependencies.
    ///
    /// A module is removed when its reference count reaches zero.
    pub fn unload(&mut self, root: &Record) -> Result<(), ManagerError> {
        let resolution = resolve(root, self.host, &self.registry)?;
        // Validate the whole set before changing any reference count, so a
        // partial unload cannot leave the manager inconsistent.
        for id in &resolution.order {
            let loaded = self
                .loaded
                .get(id)
                .ok_or_else(|| ManagerError::NotLoaded(id.clone()))?;
            if loaded.refs == 0 {
                return Err(ManagerError::NotLoaded(id.clone()));
            }
        }
        for id in resolution.order.iter().rev() {
            let loaded = self.loaded.get_mut(id).expect("prechecked");
            loaded.refs -= 1;
            if loaded.refs == 0 {
                self.loaded.remove(id);
            }
        }
        Ok(())
    }

    /// Whether a module is loaded.
    pub fn is_loaded(&self, id: &ModuleId) -> bool {
        self.loaded.contains_key(id)
    }

    /// Reference count of a module, or zero if unloaded.
    pub fn refs(&self, id: &ModuleId) -> u32 {
        self.loaded.get(id).map_or(0, |loaded| loaded.refs)
    }

    /// The verified image bytes of a loaded module.
    pub fn image(&self, id: &ModuleId) -> Option<&[u8]> {
        self.loaded.get(id).map(|loaded| loaded.image.as_slice())
    }

    /// The registry record behind a loaded module.
    pub fn record(&self, id: &ModuleId) -> Option<&Record> {
        self.loaded.get(id).map(|loaded| &loaded.record)
    }

    /// Number of loaded modules.
    pub fn len(&self) -> usize {
        self.loaded.len()
    }

    /// Whether no module is loaded.
    pub fn is_empty(&self) -> bool {
        self.loaded.is_empty()
    }
}

fn sha256(bytes: &[u8]) -> Digest {
    let digest = Sha256::digest(bytes);
    let mut out = [0u8; 32];
    out.copy_from_slice(&digest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Dependency, Manifest};
    use alloc::collections::BTreeMap;

    const ABI: Abi = Abi::new(1, 0);

    struct Source {
        images: BTreeMap<ModuleId, Vec<u8>>,
    }

    impl Source {
        fn new(items: &[(&str, &str, &[u8])]) -> Self {
            let mut images = BTreeMap::new();
            for (name, version, image) in items {
                images.insert(ModuleId::new(*name, *version), image.to_vec());
            }
            Self { images }
        }
    }

    impl ImageSource for Source {
        fn fetch(&self, id: &ModuleId) -> Option<Vec<u8>> {
            self.images.get(id).cloned()
        }
    }

    fn record(name: &str, version: &str, deps: &[(&str, &str)], image: &[u8]) -> Record {
        let mut manifest = Manifest::new(name, version, ABI);
        for (dep_name, dep_version) in deps {
            let dep_image = dep_image(dep_name, dep_version);
            manifest = manifest.with_dependency(Dependency::new(
                *dep_name,
                *dep_version,
                sha256(dep_image),
            ));
        }
        Record::new(manifest, sha256(image))
    }

    fn dep_image(name: &str, version: &str) -> &'static [u8] {
        match (name, version) {
            ("b", "1.0") => b"image-b",
            ("c", "1.0") => b"image-c",
            ("d", "1.0") => b"image-d",
            _ => b"image-x",
        }
    }

    fn source(records: &[(&str, &str, &[u8])]) -> Source {
        let mut items = Vec::new();
        for (name, version, image) in records {
            items.push((*name, *version, *image));
        }
        Source::new(&items)
    }

    fn registry(records: &[Record]) -> Registry {
        let mut registry = Registry::new();
        for record in records {
            registry.insert(record.clone()).expect("unique");
        }
        registry
    }

    #[test]
    fn chain_loads_and_unloads() {
        let a = record("a", "1.0", &[("b", "1.0")], b"image-a");
        let b = record("b", "1.0", &[], b"image-b");
        let mut manager = ModuleManager::new(registry(&[a.clone(), b.clone()]), ABI);
        let src = source(&[("a", "1.0", b"image-a"), ("b", "1.0", b"image-b")]);
        manager.load(&a, &src).expect("load");
        assert!(manager.is_loaded(&a.manifest.id));
        assert!(manager.is_loaded(&b.manifest.id));
        assert_eq!(manager.refs(&b.manifest.id), 1);
        assert_eq!(manager.image(&a.manifest.id), Some(&b"image-a"[..]));
        manager.unload(&a).expect("unload");
        assert!(manager.is_empty());
    }

    #[test]
    fn shared_dependency_keeps_one_reference() {
        let c = record("c", "1.0", &[], b"image-c");
        let a = record("a", "1.0", &[("c", "1.0")], b"image-a");
        let b = record("b", "1.0", &[("c", "1.0")], b"image-b");
        let mut manager = ModuleManager::new(registry(&[a.clone(), b.clone(), c.clone()]), ABI);
        let src = source(&[
            ("a", "1.0", b"image-a"),
            ("b", "1.0", b"image-b"),
            ("c", "1.0", b"image-c"),
        ]);
        manager.load(&a, &src).expect("load a");
        manager.load(&b, &src).expect("load b");
        assert_eq!(manager.refs(&c.manifest.id), 2);
        manager.unload(&a).expect("unload a");
        assert!(manager.is_loaded(&c.manifest.id));
        manager.unload(&b).expect("unload b");
        assert!(manager.is_empty());
    }

    #[test]
    fn failed_fetch_is_atomic() {
        let a = record("a", "1.0", &[("b", "1.0")], b"image-a");
        let b = record("b", "1.0", &[], b"image-b");
        let mut manager = ModuleManager::new(registry(&[a.clone(), b.clone()]), ABI);
        let src = source(&[("a", "1.0", b"image-a")]);
        assert_eq!(
            manager.load(&a, &src),
            Err(ManagerError::MissingImage(b.manifest.id))
        );
        assert!(manager.is_empty());
    }

    #[test]
    fn digest_mismatch_is_atomic() {
        let a = record("a", "1.0", &[], b"image-a");
        let mut manager = ModuleManager::new(registry(core::slice::from_ref(&a)), ABI);
        let src = source(&[("a", "1.0", b"wrong")]);
        assert!(matches!(
            manager.load(&a, &src),
            Err(ManagerError::DigestMismatch { .. })
        ));
        assert!(manager.is_empty());
    }

    #[test]
    fn unload_unloaded_module_fails() {
        let a = record("a", "1.0", &[], b"image-a");
        let src = source(&[("a", "1.0", b"image-a")]);
        let mut manager = ModuleManager::new(registry(core::slice::from_ref(&a)), ABI);
        manager.load(&a, &src).expect("load");
        manager.unload(&a).expect("unload");
        assert_eq!(
            manager.unload(&a),
            Err(ManagerError::NotLoaded(a.manifest.id))
        );
    }

    #[test]
    fn repeated_load_increments_reference_count() {
        let a = record("a", "1.0", &[], b"image-a");
        let src = source(&[("a", "1.0", b"image-a")]);
        let mut manager = ModuleManager::new(registry(core::slice::from_ref(&a)), ABI);
        manager.load(&a, &src).expect("first");
        manager.load(&a, &src).expect("second");
        assert_eq!(manager.refs(&a.manifest.id), 2);
        manager.unload(&a).expect("first unload");
        assert!(manager.is_loaded(&a.manifest.id));
        manager.unload(&a).expect("second unload");
        assert!(manager.is_empty());
    }

    #[test]
    fn missing_dependency_is_atomic() {
        let a = record("a", "1.0", &[("missing", "1.0")], b"image-a");
        let mut manager = ModuleManager::new(registry(core::slice::from_ref(&a)), ABI);
        let src = source(&[("a", "1.0", b"image-a")]);
        assert!(matches!(
            manager.load(&a, &src),
            Err(ManagerError::Resolve(_))
        ));
        assert!(manager.is_empty());
    }

    #[test]
    fn source_does_not_need_to_supply_loaded_images_again() {
        let a = record("a", "1.0", &[], b"image-a");
        let src = source(&[("a", "1.0", b"image-a")]);
        let empty = Source::new(&[]);
        let mut manager = ModuleManager::new(registry(core::slice::from_ref(&a)), ABI);
        manager.load(&a, &src).expect("load");
        manager.load(&a, &empty).expect("reload uses loaded image");
        assert_eq!(manager.refs(&a.manifest.id), 2);
    }
}
