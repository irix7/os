// SPDX-License-Identifier: GPL-3.0-only
//! Exact-version module manifests and dependency resolution for IRIX 7.
//!
//! This is the host-testable first slice of the runtime module dependency work
//! tracked in #217. It deliberately does **not** load, relocate or execute an
//! image. It defines the manifest records, the exact-version graph rules and a
//! deterministic resolver that the future module manager can use.
//!
//! Version 1 rules:
//!
//! - dependencies name an exact `name` and `version`;
//! - every dependency record carries a SHA-256 digest that must match the
//!   manifest's declared dependency digest;
//! - ABI compatibility is a major equality plus a host minor at least as new as
//!   the module's declared minor;
//! - cycles, missing records and digest/ABI mismatches fail closed;
//! - the root module is last in the returned order, so a loader can invoke
//!   dependencies before dependents.
//!
//! No version ranges, solver, signature validation or slot allocation is
//! implemented here. Those are later slices.

#![cfg_attr(not(test), no_std)]
#![forbid(unsafe_code)]
#![deny(missing_docs)]

extern crate alloc;

use alloc::collections::{BTreeMap, BTreeSet};
use alloc::string::String;
use alloc::vec::Vec;
use core::fmt;

/// A SHA-256 digest.
pub type Digest = [u8; 32];

/// Module ABI version.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Abi {
    /// Breaking ABI generation.
    pub major: u16,
    /// Additive ABI revision.
    pub minor: u16,
}

impl Abi {
    /// Construct an ABI version.
    pub const fn new(major: u16, minor: u16) -> Self {
        Self { major, minor }
    }

    /// Whether a module built for `self` can run on `host`.
    pub const fn is_compatible_with(self, host: Abi) -> bool {
        self.major == host.major && self.minor <= host.minor
    }
}

/// Stable module name and exact version.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct ModuleId {
    /// Stable ASCII package name.
    pub name: String,
    /// Exact version selected from the package index.
    pub version: String,
}

impl ModuleId {
    /// Construct a module identity.
    pub fn new(name: impl Into<String>, version: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            version: version.into(),
        }
    }
}

/// One exact-version dependency and its expected image digest.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Dependency {
    /// Dependency identity.
    pub module: ModuleId,
    /// Required SHA-256 of the dependency image.
    pub sha256: Digest,
}

impl Dependency {
    /// Construct an exact dependency.
    pub fn new(name: impl Into<String>, version: impl Into<String>, sha256: Digest) -> Self {
        Self {
            module: ModuleId::new(name, version),
            sha256,
        }
    }
}

/// A versioned interface exported by a module.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Export {
    /// Interface name.
    pub interface: String,
    /// Interface version.
    pub version: u16,
}

impl Export {
    /// Construct an export declaration.
    pub fn new(interface: impl Into<String>, version: u16) -> Self {
        Self {
            interface: interface.into(),
            version,
        }
    }
}

/// Module manifest, excluding the image bytes themselves.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Manifest {
    /// Module identity.
    pub id: ModuleId,
    /// ABI the image was built for.
    pub abi: Abi,
    /// Exact runtime dependencies.
    pub dependencies: Vec<Dependency>,
    /// Interfaces this module provides.
    pub exports: Vec<Export>,
}

impl Manifest {
    /// Construct a manifest with no dependencies or exports.
    pub fn new(name: impl Into<String>, version: impl Into<String>, abi: Abi) -> Self {
        Self {
            id: ModuleId::new(name, version),
            abi,
            dependencies: Vec::new(),
            exports: Vec::new(),
        }
    }

    /// Add an exact dependency declaration.
    pub fn with_dependency(mut self, dependency: Dependency) -> Self {
        self.dependencies.push(dependency);
        self
    }

    /// Add an export declaration.
    pub fn with_export(mut self, export: Export) -> Self {
        self.exports.push(export);
        self
    }
}

/// A manifest plus the digest of the image that contains it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Record {
    /// Parsed manifest.
    pub manifest: Manifest,
    /// SHA-256 over the whole image, including the manifest.
    pub digest: Digest,
}

impl Record {
    /// Construct a registry record.
    pub fn new(manifest: Manifest, digest: Digest) -> Self {
        Self { manifest, digest }
    }
}

/// A collection of verified module records.
#[derive(Clone, Debug, Default)]
pub struct Registry {
    records: BTreeMap<ModuleId, Record>,
}

impl Registry {
    /// Construct an empty registry.
    pub const fn new() -> Self {
        Self {
            records: BTreeMap::new(),
        }
    }

    /// Insert a record. Re-inserting the same identity is rejected.
    pub fn insert(&mut self, record: Record) -> Result<(), ResolveError> {
        let id = record.manifest.id.clone();
        if self.records.contains_key(&id) {
            return Err(ResolveError::Duplicate(id));
        }
        self.records.insert(id, record);
        Ok(())
    }

    /// Fetch a record by exact identity.
    pub fn get(&self, id: &ModuleId) -> Option<&Record> {
        self.records.get(id)
    }

    /// Number of records.
    pub fn len(&self) -> usize {
        self.records.len()
    }

    /// Whether the registry is empty.
    pub fn is_empty(&self) -> bool {
        self.records.is_empty()
    }
}

/// Deterministic dependency order. The root module is last.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Resolution {
    /// Modules in load order; each dependency precedes its dependents.
    pub order: Vec<ModuleId>,
}

impl Resolution {
    /// Whether the resolution contains a module.
    pub fn contains(&self, id: &ModuleId) -> bool {
        self.order.iter().any(|entry| entry == id)
    }
}

/// Resolver failure. All failures leave the caller without a load order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ResolveError {
    /// A dependency is absent from the registry.
    Missing(ModuleId),
    /// The dependency graph contains a cycle at this module.
    Cycle(ModuleId),
    /// The same exact module identity was registered twice.
    Duplicate(ModuleId),
    /// The record digest does not match the dependency's expected digest.
    DigestMismatch {
        /// Dependency identity.
        module: ModuleId,
        /// Digest declared by the dependent.
        expected: Digest,
        /// Digest recorded in the registry.
        actual: Digest,
    },
    /// The module's ABI cannot run on the host.
    AbiMismatch {
        /// Module identity.
        module: ModuleId,
        /// ABI declared by the module.
        module_abi: Abi,
        /// ABI offered by the host.
        host: Abi,
    },
}

impl fmt::Display for ResolveError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Missing(id) => write!(f, "missing module {}@{}", id.name, id.version),
            Self::Cycle(id) => write!(f, "dependency cycle at {}@{}", id.name, id.version),
            Self::Duplicate(id) => write!(f, "duplicate module {}@{}", id.name, id.version),
            Self::DigestMismatch {
                module,
                expected,
                actual,
            } => write!(
                f,
                "digest mismatch for {}@{}: expected {:02x?}, got {:02x?}",
                module.name, module.version, expected, actual
            ),
            Self::AbiMismatch {
                module,
                module_abi,
                host,
            } => write!(
                f,
                "ABI mismatch for {}@{}: module {}.{}, host {}.{}",
                module.name,
                module.version,
                module_abi.major,
                module_abi.minor,
                host.major,
                host.minor
            ),
        }
    }
}

/// Resolve `root` and all of its exact dependencies.
///
/// The root record is not itself required to be in the registry. Every
/// dependency must be present, its declared digest must match the registry
/// record and its ABI must be compatible with `host`.
pub fn resolve(root: &Record, host: Abi, registry: &Registry) -> Result<Resolution, ResolveError> {
    let mut visiting = BTreeSet::new();
    let mut visited = BTreeSet::new();
    let mut order = Vec::new();
    visit(
        &root.manifest.id,
        host,
        registry,
        &mut visiting,
        &mut visited,
        &mut order,
    )?;
    Ok(Resolution { order })
}

fn visit(
    id: &ModuleId,
    host: Abi,
    registry: &Registry,
    visiting: &mut BTreeSet<ModuleId>,
    visited: &mut BTreeSet<ModuleId>,
    order: &mut Vec<ModuleId>,
) -> Result<(), ResolveError> {
    if visited.contains(id) {
        return Ok(());
    }
    if !visiting.insert(id.clone()) {
        return Err(ResolveError::Cycle(id.clone()));
    }

    let record = registry
        .get(id)
        .ok_or_else(|| ResolveError::Missing(id.clone()))?;
    if !record.manifest.abi.is_compatible_with(host) {
        return Err(ResolveError::AbiMismatch {
            module: id.clone(),
            module_abi: record.manifest.abi,
            host,
        });
    }

    for dependency in &record.manifest.dependencies {
        let dependency_record = registry
            .get(&dependency.module)
            .ok_or_else(|| ResolveError::Missing(dependency.module.clone()))?;
        if dependency_record.digest != dependency.sha256 {
            return Err(ResolveError::DigestMismatch {
                module: dependency.module.clone(),
                expected: dependency.sha256,
                actual: dependency_record.digest,
            });
        }
        visit(&dependency.module, host, registry, visiting, visited, order)?;
    }

    visiting.remove(id);
    visited.insert(id.clone());
    order.push(id.clone());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    const ABI: Abi = Abi::new(1, 0);

    fn digest(name: &str, version: &str) -> Digest {
        let mut out = [0u8; 32];
        out[0] = name.bytes().next().unwrap_or(0);
        out[1] = version.bytes().next().unwrap_or(0);
        out[2] = name.len() as u8;
        out
    }

    fn dependency(name: &str, version: &str) -> Dependency {
        Dependency::new(name, version, digest(name, version))
    }

    fn record(name: &str, version: &str, abi: Abi, deps: &[(&str, &str)]) -> Record {
        let mut manifest = Manifest::new(name, version, abi);
        for (dep_name, dep_version) in deps {
            manifest = manifest.with_dependency(dependency(dep_name, dep_version));
        }
        Record::new(manifest, digest(name, version))
    }

    fn registry(records: &[Record]) -> Registry {
        let mut registry = Registry::new();
        for record in records {
            registry.insert(record.clone()).expect("unique record");
        }
        registry
    }

    #[test]
    fn chain_loads_dependencies_before_dependents() {
        let a = record("a", "1.0", ABI, &[("b", "1.0")]);
        let b = record("b", "1.0", ABI, &[("c", "1.0")]);
        let c = record("c", "1.0", ABI, &[]);
        let registry = registry(&[a.clone(), b.clone(), c.clone()]);
        let resolved = resolve(&a, ABI, &registry).expect("resolve");
        assert!(resolved.contains(&b.manifest.id));
        assert_eq!(
            resolved.order,
            vec![c.manifest.id, b.manifest.id, a.manifest.id]
        );
    }

    #[test]
    fn diamond_resolves_each_module_once() {
        let a = record("a", "1.0", ABI, &[("b", "1.0"), ("c", "1.0")]);
        let b = record("b", "1.0", ABI, &[("d", "1.0")]);
        let c = record("c", "1.0", ABI, &[("d", "1.0")]);
        let d = record("d", "1.0", ABI, &[]);
        let registry = registry(&[a.clone(), b.clone(), c.clone(), d.clone()]);
        let resolved = resolve(&a, ABI, &registry).expect("resolve");
        assert_eq!(resolved.order.len(), 4);
        let d_position = resolved
            .order
            .iter()
            .position(|id| id == &d.manifest.id)
            .expect("d");
        assert!(d_position < resolved.order.len() - 1);
        assert_eq!(resolved.order.last(), Some(&a.manifest.id));
    }

    #[test]
    fn cycle_fails_closed() {
        let a = record("a", "1.0", ABI, &[("b", "1.0")]);
        let b = record("b", "1.0", ABI, &[("a", "1.0")]);
        let registry = registry(&[a.clone(), b.clone()]);
        assert_eq!(
            resolve(&a, ABI, &registry),
            Err(ResolveError::Cycle(a.manifest.id))
        );
    }

    #[test]
    fn missing_dependency_fails_closed() {
        let a = record("a", "1.0", ABI, &[("missing", "1.0")]);
        let registry = registry(core::slice::from_ref(&a));
        assert_eq!(
            resolve(&a, ABI, &registry),
            Err(ResolveError::Missing(ModuleId::new("missing", "1.0")))
        );
    }

    #[test]
    fn exact_version_mismatch_is_missing() {
        let a = record("a", "1.0", ABI, &[("b", "1.1")]);
        let b = record("b", "1.0", ABI, &[]);
        let registry = registry(&[a.clone(), b]);
        assert_eq!(
            resolve(&a, ABI, &registry),
            Err(ResolveError::Missing(ModuleId::new("b", "1.1")))
        );
    }

    #[test]
    fn digest_mismatch_fails_closed() {
        let a = record("a", "1.0", ABI, &[("b", "1.0")]);
        let mut b = record("b", "1.0", ABI, &[]);
        b.digest = [0xff; 32];
        let registry = registry(&[a.clone(), b]);
        assert!(matches!(
            resolve(&a, ABI, &registry),
            Err(ResolveError::DigestMismatch { .. })
        ));
    }

    #[test]
    fn abi_mismatch_fails_closed() {
        let a = record("a", "1.0", ABI, &[("b", "1.0")]);
        let b = record("b", "1.0", Abi::new(2, 0), &[]);
        let registry = registry(&[a.clone(), b]);
        assert!(matches!(
            resolve(&a, ABI, &registry),
            Err(ResolveError::AbiMismatch { .. })
        ));
    }

    #[test]
    fn duplicate_record_is_rejected() {
        let a = record("a", "1.0", ABI, &[]);
        let mut registry = Registry::new();
        registry.insert(a.clone()).expect("first insert");
        assert_eq!(
            registry.insert(a.clone()),
            Err(ResolveError::Duplicate(a.manifest.id))
        );
    }

    #[test]
    fn manifest_exports_are_preserved() {
        let manifest =
            Manifest::new("a", "1.0", ABI).with_export(Export::new("irix7.mesh.transport", 1));
        assert_eq!(
            manifest.exports,
            vec![Export::new("irix7.mesh.transport", 1)]
        );
    }
}
