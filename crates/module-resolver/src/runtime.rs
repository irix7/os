// SPDX-License-Identifier: GPL-3.0-only
//! Runtime resolution and fail-closed loading for the downloadable manager.
//!
//! This module is the host-testable core of `modules/manager`. It consumes a
//! parsed [`PackageIndex`], resolves the exact graph, verifies every release
//! signature, loads every image into its declared arena region, validates the
//! image manifest footer against the index record, and only then enters the
//! modules in dependency order.
//!
//! All loads happen before the first entry call. Any failure unloads every
//! image loaded by this transaction, so a missing dependency, digest/ABI/cycle
//! failure, region limit or stale handle cannot leave a dependent running.

use alloc::collections::BTreeSet;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use core::fmt;
use sha2::{Digest as ShaDigest, Sha256};

use module_trust::{SignatureRecord, SignatureState, TrustError, TrustPolicy};

use crate::footer;
use crate::index::{PackageIndex, PackageKind, PackageRecord};
use crate::{resolve, Abi, Digest, Export, ModuleId, Record, Registry, ResolveError};

/// Opaque module handle from the loader.
pub type Handle = u64;

/// The bounded arena reported by the kernel loader.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ArenaInfo {
    /// First arena byte.
    pub base: usize,
    /// Total arena bytes.
    pub length: usize,
    /// Bytes in one region.
    pub region_size: usize,
    /// Number of regions.
    pub region_count: u32,
    /// Bit `i` set when region `i` is loaded.
    pub used_lo: u32,
    /// Bit `i` set when region `32 + i` is loaded.
    pub used_hi: u32,
}

/// Loader status returned by a backend.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BackendError(pub i32);

/// One `module_load` request.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LoadRequest<'a> {
    /// Immutable image URL.
    pub url: &'a str,
    /// Expected SHA-256 of the complete image.
    pub sha256: Digest,
    /// Exact module name.
    pub name: &'a str,
    /// Exact module version.
    pub version: &'a str,
    /// Export declarations to register at load.
    pub exports: &'a [Export],
    /// Arena region chosen by the package index.
    pub region: u32,
    /// Maximum accepted image bytes.
    pub max_size: u32,
}

/// Metadata for a loaded handle.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ImageInfo {
    /// Address of the first image byte.
    pub base: usize,
    /// Verified image bytes written into the region.
    pub image_size: u32,
    /// Arena region index.
    pub region: u32,
    /// Loader state.
    pub state: u32,
}

/// A resolved export binding.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ExportBinding {
    /// Handle that owns the export.
    pub handle: Handle,
    /// Absolute table address, or `None` for a declaration-only export.
    pub table: Option<usize>,
}

/// The kernel services the runtime manager needs.
pub trait RuntimeBackend {
    /// Read the arena geometry.
    fn arena(&mut self) -> Result<ArenaInfo, BackendError>;
    /// Fetch, verify and copy one image; returns its handle.
    fn load(&mut self, request: &LoadRequest<'_>) -> Result<Handle, BackendError>;
    /// Read the verified image bytes.
    fn image(&self, handle: Handle) -> Result<&[u8], BackendError>;
    /// Read loaded-image metadata.
    fn info(&mut self, handle: Handle) -> Result<ImageInfo, BackendError>;
    /// Enter a loaded image and return its vtable status.
    fn execute(&mut self, handle: Handle, entry_offset: u32) -> Result<i32, BackendError>;
    /// Find an export by exact interface and version.
    fn find_export(
        &mut self,
        interface: &str,
        version: u16,
    ) -> Result<Option<ExportBinding>, BackendError>;
    /// Release one region.
    fn unload(&mut self, handle: Handle) -> Result<(), BackendError>;
}

/// One node in a resolved runtime graph.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Node {
    /// Resolver record (identity, ABI, dependencies and digest).
    pub record: Record,
    /// Immutable image URL.
    pub url: String,
    /// Arena region the image was linked for.
    pub region: u32,
    /// Detached release signature.
    pub signature: Option<SignatureRecord>,
    /// Export declarations copied from the index.
    pub exports: Vec<Export>,
}

/// A resolved, ordered runtime graph. The root is last.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Plan {
    /// Root identity.
    pub root: ModuleId,
    /// Dependencies first, root last.
    pub nodes: Vec<Node>,
}

impl Plan {
    /// Resolve a root from a parsed package index.
    pub fn from_index(
        index: &PackageIndex,
        root_name: &str,
        root_version: &str,
        host: Abi,
    ) -> Result<Self, RuntimeError> {
        let root_id = ModuleId::new(root_name, root_version);
        let root_record = index
            .find_module(root_name, root_version)
            .ok_or_else(|| RuntimeError::MissingRoot(root_id.clone()))?;
        let root = root_record
            .resolver_record()
            .ok_or_else(|| RuntimeError::LegacyRecord(root_id.clone(), "abi"))?;
        let registry = registry(index)?;
        let resolution = resolve(&root, host, &registry).map_err(RuntimeError::Resolve)?;

        let mut nodes = Vec::with_capacity(resolution.order.len());
        let mut regions = BTreeSet::new();
        for id in &resolution.order {
            let package = index
                .find_module(&id.name, &id.version)
                .ok_or_else(|| RuntimeError::MissingRoot(id.clone()))?;
            let node = node_from_package(package)?;
            if !regions.insert(node.region) {
                return Err(RuntimeError::RegionDuplicate(node.region));
            }
            nodes.push(node);
        }
        Ok(Self {
            root: root_id,
            nodes,
        })
    }
}

fn node_from_package(package: &PackageRecord) -> Result<Node, RuntimeError> {
    let id = ModuleId::new(package.name.as_str(), package.version.as_str());
    let record = package
        .resolver_record()
        .ok_or_else(|| RuntimeError::LegacyRecord(id.clone(), "abi"))?;
    let region = package
        .region
        .ok_or_else(|| RuntimeError::LegacyRecord(id.clone(), "region"))?;
    check_limit(&id, "url", package.url.len(), 511)?;
    check_limit(&id, "name", package.name.len(), 63)?;
    check_limit(&id, "version", package.version.len(), 31)?;
    check_limit(&id, "exports", package.exports.len(), 16)?;
    Ok(Node {
        record,
        url: package.url.clone(),
        region,
        signature: package.signature,
        exports: package.exports.clone(),
    })
}

fn check_limit(
    module: &ModuleId,
    field: &'static str,
    value: usize,
    max: usize,
) -> Result<(), RuntimeError> {
    if value > max {
        return Err(RuntimeError::Limit {
            module: module.clone(),
            field,
            value,
            max,
        });
    }
    Ok(())
}

/// One entered module.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NodeReport {
    /// Module identity.
    pub id: ModuleId,
    /// Arena region.
    pub region: u32,
    /// Loader handle.
    pub handle: Handle,
    /// Image-relative entry offset.
    pub entry_offset: u32,
    /// Status returned by the module vtable.
    pub status: i32,
    /// Registered exports.
    pub exports: Vec<Export>,
}

/// Successful load and execution report.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Report {
    /// Root identity.
    pub root: ModuleId,
    /// Loaded and entered nodes in dependency order.
    pub nodes: Vec<NodeReport>,
    /// Signature mode accepted by the policy.
    pub signatures: SignatureState,
}

impl Report {
    /// Format the bounded status block read by MCP `GET /modules`.
    pub fn status_text(&self) -> String {
        let mut out = String::new();
        let _ = fmt::write(
            &mut out,
            format_args!(
                "status=ok root={}@{} nodes={} signatures={}\n",
                self.root.name,
                self.root.version,
                self.nodes.len(),
                signature_name(self.signatures)
            ),
        );
        out.push_str("order=");
        for (index, node) in self.nodes.iter().enumerate() {
            if index > 0 {
                out.push(',');
            }
            let _ = fmt::write(
                &mut out,
                format_args!("{}@{}", node.id.name, node.id.version),
            );
        }
        out.push('\n');
        for node in &self.nodes {
            let _ = fmt::write(
                &mut out,
                format_args!(
                    "{}@{} region={} handle={} entry={} status={}\n",
                    node.id.name,
                    node.id.version,
                    node.region,
                    node.handle,
                    node.entry_offset,
                    node.status
                ),
            );
            for export in &node.exports {
                let _ = fmt::write(
                    &mut out,
                    format_args!(
                        "export={}@{} interface={} version={} offset={}\n",
                        node.id.name,
                        node.id.version,
                        export.interface,
                        export.version,
                        export.offset
                    ),
                );
            }
        }
        out
    }
}

fn signature_name(state: SignatureState) -> &'static str {
    match state {
        SignatureState::Signed => "ed25519",
        SignatureState::UnsignedAllowed => "unsigned",
    }
}

/// Runtime manager failure.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RuntimeError {
    /// The root is not in the package index.
    MissingRoot(ModuleId),
    /// A record lacks metadata the runtime manager requires.
    LegacyRecord(ModuleId, &'static str),
    /// Graph resolution failed.
    Resolve(ResolveError),
    /// A field exceeds a loader limit.
    Limit {
        /// Module identity.
        module: ModuleId,
        /// Field name.
        field: &'static str,
        /// Actual value.
        value: usize,
        /// Maximum value.
        max: usize,
    },
    /// Two nodes declare the same region.
    RegionDuplicate(u32),
    /// A node declares a region the manager must not use.
    RegionOutOfRange {
        /// Module identity.
        module: ModuleId,
        /// Declared region.
        region: u32,
        /// Highest usable region index.
        limit: u32,
    },
    /// The graph does not fit the arena after reserving the metadata region.
    TooManyModules {
        /// Number of nodes.
        count: usize,
        /// Number of usable regions.
        limit: usize,
    },
    /// Release signature verification failed.
    Trust {
        /// Module identity.
        module: ModuleId,
        /// Trust-store error.
        error: TrustError,
    },
    /// The arena service failed.
    Arena(i32),
    /// A loader service failed.
    Backend {
        /// Module identity.
        module: ModuleId,
        /// Operation name.
        stage: &'static str,
        /// Loader status.
        status: i32,
    },
    /// `module_load` rejected the image.
    Load {
        /// Module identity.
        module: ModuleId,
        /// Loader status.
        status: i32,
    },
    /// Loaded-image metadata is inconsistent.
    ImageInfo {
        /// Module identity.
        module: ModuleId,
        /// Inconsistent field.
        field: &'static str,
    },
    /// The bytes in the region do not match the requested digest.
    DigestMismatch(ModuleId),
    /// A runtime image has no manifest footer.
    MissingFooter(ModuleId),
    /// The manifest footer is malformed.
    Footer {
        /// Module identity.
        module: ModuleId,
        /// Footer error.
        error: String,
    },
    /// The manifest does not match the index record.
    ManifestMismatch {
        /// Module identity.
        module: ModuleId,
        /// Mismatched field.
        field: &'static str,
    },
    /// The entry offset is outside the image.
    EntryOutOfRange {
        /// Module identity.
        module: ModuleId,
        /// Declared entry offset.
        offset: u32,
        /// Verified image size.
        size: u32,
    },
    /// A declared export was not registered.
    ExportMissing {
        /// Module identity.
        module: ModuleId,
        /// Interface.
        interface: String,
        /// Interface version.
        version: u16,
    },
    /// A declared export is owned by another handle.
    ExportOwner {
        /// Module identity.
        module: ModuleId,
        /// Interface.
        interface: String,
        /// Interface version.
        version: u16,
    },
    /// A declared export table is not at the manifest offset.
    ExportTable {
        /// Module identity.
        module: ModuleId,
        /// Interface.
        interface: String,
        /// Interface version.
        version: u16,
    },
    /// The module entry returned a non-zero vtable status.
    ModuleStatus {
        /// Module identity.
        module: ModuleId,
        /// Vtable status.
        status: i32,
    },
}

impl RuntimeError {
    /// Pipeline stage for bounded status text.
    pub fn stage(&self) -> &'static str {
        match self {
            Self::MissingRoot(_) => "index",
            Self::LegacyRecord(_, _) => "index",
            Self::Resolve(_) => "resolve",
            Self::Limit { .. } => "limit",
            Self::RegionDuplicate(_) | Self::RegionOutOfRange { .. } => "region",
            Self::TooManyModules { .. } => "limit",
            Self::Trust { .. } => "signature",
            Self::Arena(_) => "arena",
            Self::Backend { .. } | Self::Load { .. } => "load",
            Self::ImageInfo { .. } => "image",
            Self::DigestMismatch(_) => "digest",
            Self::MissingFooter(_) | Self::Footer { .. } => "manifest",
            Self::ManifestMismatch { .. } => "manifest",
            Self::EntryOutOfRange { .. } => "entry",
            Self::ExportMissing { .. } | Self::ExportOwner { .. } | Self::ExportTable { .. } => {
                "export"
            }
            Self::ModuleStatus { .. } => "execute",
        }
    }

    /// Format bounded failure text for the status block.
    pub fn status_text(&self, root: Option<&ModuleId>) -> String {
        let mut out = String::new();
        out.push_str("status=failed");
        if let Some(root) = root {
            let _ = fmt::write(
                &mut out,
                format_args!(" root={}@{}", root.name, root.version),
            );
        }
        let _ = fmt::write(
            &mut out,
            format_args!(" stage={} error={}", self.stage(), self),
        );
        out
    }
}

impl fmt::Display for RuntimeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingRoot(id) => {
                write!(f, "root {}@{} is not in the index", id.name, id.version)
            }
            Self::LegacyRecord(id, field) => write!(
                f,
                "{}@{} has no runtime {field} record",
                id.name, id.version
            ),
            Self::Resolve(error) => write!(f, "{error}"),
            Self::Limit {
                module,
                field,
                value,
                max,
            } => write!(
                f,
                "{}@{} {field} {value} exceeds {max}",
                module.name, module.version
            ),
            Self::RegionDuplicate(region) => write!(f, "region {region} is declared twice"),
            Self::RegionOutOfRange {
                module,
                region,
                limit,
            } => write!(
                f,
                "{}@{} region {region} is outside 0..{limit}",
                module.name, module.version
            ),
            Self::TooManyModules { count, limit } => {
                write!(f, "{count} modules exceed {limit} usable regions")
            }
            Self::Trust { module, error } => {
                write!(f, "{}@{}: {error}", module.name, module.version)
            }
            Self::Arena(status) => write!(f, "module_arena failed ({status})"),
            Self::Backend {
                module,
                stage,
                status,
            } => write!(
                f,
                "{}@{} {stage} failed ({status})",
                module.name, module.version
            ),
            Self::Load { module, status } => {
                write!(
                    f,
                    "{}@{} load failed ({status})",
                    module.name, module.version
                )
            }
            Self::ImageInfo { module, field } => write!(
                f,
                "{}@{} has inconsistent {field}",
                module.name, module.version
            ),
            Self::DigestMismatch(id) => {
                write!(
                    f,
                    "{}@{} arena bytes do not match the digest",
                    id.name, id.version
                )
            }
            Self::MissingFooter(id) => {
                write!(f, "{}@{} has no manifest footer", id.name, id.version)
            }
            Self::Footer { module, error } => {
                write!(f, "{}@{} footer: {error}", module.name, module.version)
            }
            Self::ManifestMismatch { module, field } => write!(
                f,
                "{}@{} manifest {field} differs from the index",
                module.name, module.version
            ),
            Self::EntryOutOfRange {
                module,
                offset,
                size,
            } => write!(
                f,
                "{}@{} entry {offset} is outside {size} bytes",
                module.name, module.version
            ),
            Self::ExportMissing {
                module,
                interface,
                version,
            } => write!(
                f,
                "{}@{} did not register {interface}@{version}",
                module.name, module.version
            ),
            Self::ExportOwner {
                module,
                interface,
                version,
            } => write!(
                f,
                "{}@{} {interface}@{version} is owned by another handle",
                module.name, module.version
            ),
            Self::ExportTable {
                module,
                interface,
                version,
            } => write!(
                f,
                "{}@{} {interface}@{version} table is not at the manifest offset",
                module.name, module.version
            ),
            Self::ModuleStatus { module, status } => write!(
                f,
                "{}@{} entry returned {status}",
                module.name, module.version
            ),
        }
    }
}

struct LoadedNode {
    entry_offset: u32,
}

/// Resolve, verify, load and execute a graph.
///
/// Every image is loaded and validated before the first entry call. On any
/// failure, every handle loaded by this call is unloaded in reverse order.
pub fn run(
    plan: &Plan,
    policy: &TrustPolicy,
    backend: &mut dyn RuntimeBackend,
) -> Result<Report, RuntimeError> {
    let arena = backend
        .arena()
        .map_err(|error| RuntimeError::Arena(error.0))?;
    if arena.region_size == 0 || arena.region_count == 0 {
        return Err(RuntimeError::Arena(-1));
    }
    let metadata_region = arena.region_count - 1;
    let limit = metadata_region as usize;
    if plan.nodes.len() > limit {
        return Err(RuntimeError::TooManyModules {
            count: plan.nodes.len(),
            limit,
        });
    }
    let mut regions = BTreeSet::new();
    for node in &plan.nodes {
        if node.region >= metadata_region {
            return Err(RuntimeError::RegionOutOfRange {
                module: node.record.manifest.id.clone(),
                region: node.region,
                limit: metadata_region,
            });
        }
        if !regions.insert(node.region) {
            return Err(RuntimeError::RegionDuplicate(node.region));
        }
    }

    let mut signatures = Vec::with_capacity(plan.nodes.len());
    for node in &plan.nodes {
        let state = policy
            .verify(&node.record.digest, node.signature.as_ref())
            .map_err(|error| RuntimeError::Trust {
                module: node.record.manifest.id.clone(),
                error,
            })?;
        signatures.push(state);
    }
    let signature_state = if signatures
        .iter()
        .all(|state| *state == SignatureState::Signed)
    {
        SignatureState::Signed
    } else {
        SignatureState::UnsignedAllowed
    };

    let mut loaded: Vec<(Handle, LoadedNode)> = Vec::with_capacity(plan.nodes.len());
    for node in &plan.nodes {
        let id = &node.record.manifest.id;
        let request = LoadRequest {
            url: node.url.as_str(),
            sha256: node.record.digest,
            name: id.name.as_str(),
            version: id.version.as_str(),
            exports: &node.exports,
            region: node.region,
            max_size: arena.region_size as u32,
        };
        let handle = match backend.load(&request) {
            Ok(handle) => handle,
            Err(error) => {
                cleanup(backend, &loaded);
                return Err(RuntimeError::Load {
                    module: id.clone(),
                    status: error.0,
                });
            }
        };
        loaded.push((handle, LoadedNode { entry_offset: 0 }));

        let info = match backend.info(handle) {
            Ok(info) => info,
            Err(error) => {
                cleanup(backend, &loaded);
                return Err(RuntimeError::Backend {
                    module: id.clone(),
                    stage: "query",
                    status: error.0,
                });
            }
        };
        if info.region != node.region {
            cleanup(backend, &loaded);
            return Err(RuntimeError::ImageInfo {
                module: id.clone(),
                field: "region",
            });
        }
        if info.image_size as usize > arena.region_size {
            cleanup(backend, &loaded);
            return Err(RuntimeError::ImageInfo {
                module: id.clone(),
                field: "size",
            });
        }

        let (_manifest, entry_offset) = {
            let image = match backend.image(handle) {
                Ok(image) => image,
                Err(error) => {
                    cleanup(backend, &loaded);
                    return Err(RuntimeError::Backend {
                        module: id.clone(),
                        stage: "image",
                        status: error.0,
                    });
                }
            };
            if image.len() != info.image_size as usize {
                cleanup(backend, &loaded);
                return Err(RuntimeError::ImageInfo {
                    module: id.clone(),
                    field: "image",
                });
            }
            if sha256(image) != node.record.digest {
                cleanup(backend, &loaded);
                return Err(RuntimeError::DigestMismatch(id.clone()));
            }
            let (manifest, _payload) = match footer::parse(image) {
                Ok(Some(value)) => value,
                Ok(None) => {
                    cleanup(backend, &loaded);
                    return Err(RuntimeError::MissingFooter(id.clone()));
                }
                Err(error) => {
                    cleanup(backend, &loaded);
                    return Err(RuntimeError::Footer {
                        module: id.clone(),
                        error: error.to_string(),
                    });
                }
            };
            let entry_offset = manifest.entry_offset;
            if entry_offset % 4 != 0 || (entry_offset != 0 && entry_offset + 4 > info.image_size) {
                cleanup(backend, &loaded);
                return Err(RuntimeError::EntryOutOfRange {
                    module: id.clone(),
                    offset: entry_offset,
                    size: info.image_size,
                });
            }
            if manifest.id != node.record.manifest.id {
                cleanup(backend, &loaded);
                return Err(RuntimeError::ManifestMismatch {
                    module: id.clone(),
                    field: "identity",
                });
            }
            if manifest.abi != node.record.manifest.abi {
                cleanup(backend, &loaded);
                return Err(RuntimeError::ManifestMismatch {
                    module: id.clone(),
                    field: "abi",
                });
            }
            if manifest.dependencies != node.record.manifest.dependencies {
                cleanup(backend, &loaded);
                return Err(RuntimeError::ManifestMismatch {
                    module: id.clone(),
                    field: "dependencies",
                });
            }
            if manifest.exports != node.exports {
                cleanup(backend, &loaded);
                return Err(RuntimeError::ManifestMismatch {
                    module: id.clone(),
                    field: "exports",
                });
            }
            (manifest, entry_offset)
        };

        for export in &node.exports {
            let binding = match backend.find_export(&export.interface, export.version) {
                Ok(binding) => binding,
                Err(error) => {
                    cleanup(backend, &loaded);
                    return Err(RuntimeError::Backend {
                        module: id.clone(),
                        stage: "find_export",
                        status: error.0,
                    });
                }
            };
            let Some(binding) = binding else {
                cleanup(backend, &loaded);
                return Err(RuntimeError::ExportMissing {
                    module: id.clone(),
                    interface: export.interface.clone(),
                    version: export.version,
                });
            };
            if binding.handle != handle {
                cleanup(backend, &loaded);
                return Err(RuntimeError::ExportOwner {
                    module: id.clone(),
                    interface: export.interface.clone(),
                    version: export.version,
                });
            }
            let expected = if export.offset == 0 {
                None
            } else {
                Some(info.base + export.offset as usize)
            };
            if binding.table != expected {
                cleanup(backend, &loaded);
                return Err(RuntimeError::ExportTable {
                    module: id.clone(),
                    interface: export.interface.clone(),
                    version: export.version,
                });
            }
        }

        let last = loaded.last_mut().expect("just pushed");
        last.1 = LoadedNode { entry_offset };
    }

    let mut reports = Vec::with_capacity(plan.nodes.len());
    for (index, node) in plan.nodes.iter().enumerate() {
        let id = &node.record.manifest.id;
        let (handle, loaded_node) = &loaded[index];
        let status = match backend.execute(*handle, loaded_node.entry_offset) {
            Ok(status) => status,
            Err(error) => {
                cleanup(backend, &loaded);
                return Err(RuntimeError::Backend {
                    module: id.clone(),
                    stage: "execute",
                    status: error.0,
                });
            }
        };
        if status != 0 {
            cleanup(backend, &loaded);
            return Err(RuntimeError::ModuleStatus {
                module: id.clone(),
                status,
            });
        }
        reports.push(NodeReport {
            id: id.clone(),
            region: node.region,
            handle: *handle,
            entry_offset: loaded_node.entry_offset,
            status,
            exports: node.exports.clone(),
        });
    }

    Ok(Report {
        root: plan.root.clone(),
        nodes: reports,
        signatures: signature_state,
    })
}

fn cleanup(backend: &mut dyn RuntimeBackend, loaded: &[(Handle, LoadedNode)]) {
    for (handle, _) in loaded.iter().rev() {
        let _ = backend.unload(*handle);
    }
}

/// SHA-256 of a complete image, manifest and footer.
pub fn sha256(bytes: &[u8]) -> Digest {
    let digest = Sha256::digest(bytes);
    let mut out = [0u8; 32];
    out.copy_from_slice(&digest);
    out
}

/// Build a registry from all runtime records in an index.
pub fn registry(index: &PackageIndex) -> Result<Registry, RuntimeError> {
    let mut registry = Registry::new();
    for package in &index.records {
        if package.kind != PackageKind::Module {
            continue;
        }
        if let Some(record) = package.resolver_record() {
            registry.insert(record).map_err(RuntimeError::Resolve)?;
        }
    }
    Ok(registry)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::index::PackageIndex;
    use crate::{Dependency, Manifest};
    use alloc::collections::BTreeMap;
    use alloc::format;
    use alloc::vec;
    use ed25519_dalek::{Signer, SigningKey};
    use module_trust::image_message;

    const ABI: Abi = Abi::new(12, 0);

    fn digest(bytes: &[u8]) -> Digest {
        sha256(bytes)
    }

    fn hex(bytes: &[u8]) -> String {
        const HEX: &[u8; 16] = b"0123456789abcdef";
        let mut out = String::new();
        for byte in bytes {
            out.push(HEX[(byte >> 4) as usize] as char);
            out.push(HEX[(byte & 0x0f) as usize] as char);
        }
        out
    }

    fn image(
        name: &str,
        version: &str,
        deps: &[(&str, &str, Digest)],
        exports: &[Export],
    ) -> Vec<u8> {
        let mut manifest = Manifest::new(name, version, ABI);
        for (dep_name, dep_version, digest) in deps {
            manifest = manifest.with_dependency(Dependency::new(*dep_name, *dep_version, *digest));
        }
        for export in exports {
            manifest = manifest.with_export(export.clone());
        }
        footer::wrap(b"payload", &manifest).expect("wrap")
    }

    fn signature(key: &SigningKey, epoch: u32, digest: &Digest) -> SignatureRecord {
        SignatureRecord::new(epoch, key.sign(&image_message(digest)).to_bytes())
    }

    fn line(
        name: &str,
        version: &str,
        region: u32,
        bytes: &[u8],
        deps: &[(&str, &str, Digest)],
        exports: &[Export],
        signature: Option<&SignatureRecord>,
    ) -> String {
        let mut out = format!(
            "module {name} {version} https://example/{name}.module {} abi=12.0 region={region}",
            hex(&digest(bytes))
        );
        if !deps.is_empty() {
            out.push_str(" depends=");
            for (index, (dep_name, dep_version, digest)) in deps.iter().enumerate() {
                if index > 0 {
                    out.push(',');
                }
                out.push_str(&format!("{dep_name}@{dep_version}:{}", hex(digest)));
            }
        }
        if !exports.is_empty() {
            out.push_str(" exports=");
            for (index, export) in exports.iter().enumerate() {
                if index > 0 {
                    out.push(',');
                }
                out.push_str(&format!(
                    "{}@{}:{}",
                    export.interface, export.version, export.offset
                ));
            }
        }
        if let Some(signature) = signature {
            out.push_str(" sig=");
            out.push_str(&signature.to_text());
        }
        out.push_str(" runtime fixture");
        out
    }

    fn index(lines: &[String]) -> PackageIndex {
        let mut text = String::from("version=2\n");
        for line in lines {
            text.push_str(line);
            text.push('\n');
        }
        PackageIndex::parse(text.as_bytes()).expect("parse index")
    }

    struct FakeLoaded {
        id: ModuleId,
        region: u32,
        base: usize,
        image: Vec<u8>,
        exports: Vec<Export>,
    }

    struct Fake {
        arena: ArenaInfo,
        images: BTreeMap<(String, String), Vec<u8>>,
        loaded: BTreeMap<Handle, FakeLoaded>,
        generation: u32,
        load_log: Vec<ModuleId>,
        exec_log: Vec<ModuleId>,
        unload_log: Vec<Handle>,
        fail_load: Option<ModuleId>,
        fail_status: i32,
        corrupt: Option<ModuleId>,
        stale_execute: bool,
        drop_exports: bool,
        export_owner: Option<Handle>,
    }

    impl Fake {
        fn new(region_count: u32) -> Self {
            Self {
                arena: ArenaInfo {
                    base: 0x884c_0000,
                    length: 0x14_0000,
                    region_size: 0x4_0000,
                    region_count,
                    used_lo: 0,
                    used_hi: 0,
                },
                images: BTreeMap::new(),
                loaded: BTreeMap::new(),
                generation: 0,
                load_log: Vec::new(),
                exec_log: Vec::new(),
                unload_log: Vec::new(),
                fail_load: None,
                fail_status: -5,
                corrupt: None,
                stale_execute: false,
                drop_exports: false,
                export_owner: None,
            }
        }

        fn add(&mut self, name: &str, version: &str, image: Vec<u8>) {
            self.images
                .insert((name.to_string(), version.to_string()), image);
        }
    }

    impl RuntimeBackend for Fake {
        fn arena(&mut self) -> Result<ArenaInfo, BackendError> {
            Ok(self.arena)
        }

        fn load(&mut self, request: &LoadRequest<'_>) -> Result<Handle, BackendError> {
            let id = ModuleId::new(request.name, request.version);
            if self.fail_load.as_ref() == Some(&id) {
                return Err(BackendError(self.fail_status));
            }
            let mut image = self
                .images
                .get(&(request.name.to_string(), request.version.to_string()))
                .cloned()
                .ok_or(BackendError(-7))?;
            if self.corrupt.as_ref() == Some(&id) {
                let last = image.len() - 1;
                image[last] ^= 0xff;
            }
            if image.len() > request.max_size as usize {
                return Err(BackendError(-5));
            }
            self.generation = self.generation.wrapping_add(1).max(1);
            let handle = ((self.generation as u64) << 32) | request.region as u64;
            let base = self.arena.base + request.region as usize * self.arena.region_size;
            let exports = if self.drop_exports {
                Vec::new()
            } else {
                request.exports.to_vec()
            };
            self.loaded.insert(
                handle,
                FakeLoaded {
                    id: id.clone(),
                    region: request.region,
                    base,
                    image,
                    exports,
                },
            );
            self.load_log.push(id);
            Ok(handle)
        }

        fn image(&self, handle: Handle) -> Result<&[u8], BackendError> {
            self.loaded
                .get(&handle)
                .map(|loaded| loaded.image.as_slice())
                .ok_or(BackendError(-9))
        }

        fn info(&mut self, handle: Handle) -> Result<ImageInfo, BackendError> {
            let loaded = self.loaded.get(&handle).ok_or(BackendError(-9))?;
            Ok(ImageInfo {
                base: loaded.base,
                image_size: loaded.image.len() as u32,
                region: loaded.region,
                state: 1,
            })
        }

        fn execute(&mut self, handle: Handle, _entry: u32) -> Result<i32, BackendError> {
            if self.stale_execute {
                return Err(BackendError(-9));
            }
            let loaded = self.loaded.get(&handle).ok_or(BackendError(-9))?;
            self.exec_log.push(loaded.id.clone());
            Ok(0)
        }

        fn find_export(
            &mut self,
            interface: &str,
            version: u16,
        ) -> Result<Option<ExportBinding>, BackendError> {
            for (handle, loaded) in &self.loaded {
                if let Some(export) = loaded
                    .exports
                    .iter()
                    .find(|export| export.interface == interface && export.version == version)
                {
                    return Ok(Some(ExportBinding {
                        handle: self.export_owner.unwrap_or(*handle),
                        table: (export.offset != 0).then(|| loaded.base + export.offset as usize),
                    }));
                }
            }
            Ok(None)
        }

        fn unload(&mut self, handle: Handle) -> Result<(), BackendError> {
            self.loaded.remove(&handle);
            self.unload_log.push(handle);
            Ok(())
        }
    }

    fn graph() -> (PackageIndex, Vec<u8>, Vec<u8>) {
        let dep = image("demo-dep", "1.0.0", &[], &[]);
        let root = image(
            "demo-root",
            "1.0.0",
            &[("demo-dep", "1.0.0", digest(&dep))],
            &[],
        );
        let index = index(&[
            line("demo-dep", "1.0.0", 0, &dep, &[], &[], None),
            line(
                "demo-root",
                "1.0.0",
                1,
                &root,
                &[("demo-dep", "1.0.0", digest(&dep))],
                &[],
                None,
            ),
        ]);
        (index, dep, root)
    }

    fn policy(require: bool) -> TrustPolicy {
        TrustPolicy::new(
            1,
            SigningKey::from_bytes(&[9u8; 32]).verifying_key(),
            require,
        )
    }

    fn signed_index() -> (PackageIndex, Vec<u8>, Vec<u8>, SigningKey) {
        let key = SigningKey::from_bytes(&[9u8; 32]);
        let dep = image("demo-dep", "1.0.0", &[], &[]);
        let root = image(
            "demo-root",
            "1.0.0",
            &[("demo-dep", "1.0.0", digest(&dep))],
            &[],
        );
        let dep_sig = signature(&key, 1, &digest(&dep));
        let root_sig = signature(&key, 1, &digest(&root));
        let index = index(&[
            line("demo-dep", "1.0.0", 0, &dep, &[], &[], Some(&dep_sig)),
            line(
                "demo-root",
                "1.0.0",
                1,
                &root,
                &[("demo-dep", "1.0.0", digest(&dep))],
                &[],
                Some(&root_sig),
            ),
        ]);
        (index, dep, root, key)
    }

    #[test]
    fn dependencies_load_and_execute_before_the_root() {
        let (index, dep, root, _key) = signed_index();
        let plan = Plan::from_index(&index, "demo-root", "1.0.0", ABI).expect("plan");
        let mut fake = Fake::new(5);
        fake.add("demo-dep", "1.0.0", dep);
        fake.add("demo-root", "1.0.0", root);
        let report = run(&plan, &policy(true), &mut fake).expect("run");
        assert_eq!(report.nodes.len(), 2);
        assert_eq!(fake.exec_log.len(), 2);
        assert_eq!(fake.exec_log[0].name, "demo-dep");
        assert_eq!(fake.exec_log[1].name, "demo-root");
        assert_eq!(fake.load_log.len(), 2);
        assert!(report
            .status_text()
            .contains("status=ok root=demo-root@1.0.0"));
        assert!(report.status_text().contains("signatures=ed25519"));
        assert!(fake.unload_log.is_empty());
    }

    #[test]
    fn missing_dependency_fails_before_any_load() {
        let root = image(
            "demo-root",
            "1.0.0",
            &[("missing", "1.0.0", [0x11; 32])],
            &[],
        );
        let index = index(&[line(
            "demo-root",
            "1.0.0",
            1,
            &root,
            &[("missing", "1.0.0", [0x11; 32])],
            &[],
            None,
        )]);
        assert!(matches!(
            Plan::from_index(&index, "demo-root", "1.0.0", ABI),
            Err(RuntimeError::Resolve(ResolveError::Missing(_)))
        ));
    }

    #[test]
    fn abi_mismatch_and_cycle_fail_resolution() {
        let dep = image("demo-dep", "1.0.0", &[], &[]);
        let root = image(
            "demo-root",
            "1.0.0",
            &[("demo-dep", "1.0.0", digest(&dep))],
            &[],
        );
        let mut abi_index = index(&[
            line("demo-dep", "1.0.0", 0, &dep, &[], &[], None),
            line(
                "demo-root",
                "1.0.0",
                1,
                &root,
                &[("demo-dep", "1.0.0", digest(&dep))],
                &[],
                None,
            ),
        ]);
        abi_index.records[0].abi = Some(Abi::new(13, 0));
        assert!(matches!(
            Plan::from_index(&abi_index, "demo-root", "1.0.0", ABI),
            Err(RuntimeError::Resolve(ResolveError::AbiMismatch { .. }))
        ));

        // The cycle test only needs matching record digests, not real images.
        let a = crate::index::parse_record(&format!(
            "module a 1.0.0 https://example/a {} abi=12.0 region=0 depends=b@1.0.0:{}",
            hex(&[0x33; 32]),
            hex(&[0x22; 32])
        ))
        .expect("a");
        let b = crate::index::parse_record(&format!(
            "module b 1.0.0 https://example/b {} abi=12.0 region=1 depends=a@1.0.0:{}",
            hex(&[0x22; 32]),
            hex(&[0x33; 32])
        ))
        .expect("b");
        let cycle = PackageIndex {
            version: 2,
            records: vec![a, b],
        };
        assert!(matches!(
            Plan::from_index(&cycle, "a", "1.0.0", ABI),
            Err(RuntimeError::Resolve(ResolveError::Cycle(_)))
        ));
    }

    #[test]
    fn load_failure_unloads_everything_and_never_executes() {
        let (index, dep, root, _key) = signed_index();
        let plan = Plan::from_index(&index, "demo-root", "1.0.0", ABI).expect("plan");
        let mut fake = Fake::new(5);
        fake.add("demo-dep", "1.0.0", dep);
        fake.add("demo-root", "1.0.0", root);
        fake.fail_load = Some(ModuleId::new("demo-root", "1.0.0"));
        fake.fail_status = -5;
        assert!(matches!(
            run(&plan, &policy(true), &mut fake),
            Err(RuntimeError::Load { status: -5, .. })
        ));
        assert!(fake.exec_log.is_empty());
        assert_eq!(fake.unload_log.len(), 1);
        assert!(fake.loaded.is_empty());
    }

    #[test]
    fn corrupt_arena_bytes_fail_the_digest_and_unload() {
        let (index, dep, root, _key) = signed_index();
        let plan = Plan::from_index(&index, "demo-root", "1.0.0", ABI).expect("plan");
        let mut fake = Fake::new(5);
        fake.add("demo-dep", "1.0.0", dep);
        fake.add("demo-root", "1.0.0", root);
        fake.corrupt = Some(ModuleId::new("demo-dep", "1.0.0"));
        assert!(matches!(
            run(&plan, &policy(true), &mut fake),
            Err(RuntimeError::DigestMismatch(_))
        ));
        assert!(fake.exec_log.is_empty());
        assert!(fake.loaded.is_empty());
    }

    #[test]
    fn oversized_image_is_rejected_without_execution() {
        let payload = alloc::vec![0u8; 300 * 1024];
        let dep = footer::wrap(&payload, &Manifest::new("demo-dep", "1.0.0", ABI))
            .expect("wrap oversized image");
        let index = index(&[line("demo-dep", "1.0.0", 0, &dep, &[], &[], None)]);
        let plan = Plan::from_index(&index, "demo-dep", "1.0.0", ABI).expect("plan");
        let mut fake = Fake::new(5);
        fake.add("demo-dep", "1.0.0", dep);
        assert!(matches!(
            run(&plan, &policy(false), &mut fake),
            Err(RuntimeError::Load { status: -5, .. })
        ));
        assert!(fake.exec_log.is_empty());
        assert!(fake.loaded.is_empty());
    }

    #[test]
    fn stale_handle_on_execute_fails_closed() {
        let (index, dep, root, _key) = signed_index();
        let plan = Plan::from_index(&index, "demo-root", "1.0.0", ABI).expect("plan");
        let mut fake = Fake::new(5);
        fake.add("demo-dep", "1.0.0", dep);
        fake.add("demo-root", "1.0.0", root);
        fake.stale_execute = true;
        assert!(matches!(
            run(&plan, &policy(true), &mut fake),
            Err(RuntimeError::Backend {
                stage: "execute",
                status: -9,
                ..
            })
        ));
        assert_eq!(fake.unload_log.len(), 2);
        assert!(fake.loaded.is_empty());
    }

    #[test]
    fn missing_signature_is_rejected_unless_policy_allows_unsigned() {
        let (index, dep, root) = graph();
        let plan = Plan::from_index(&index, "demo-root", "1.0.0", ABI).expect("plan");
        let mut fake = Fake::new(5);
        fake.add("demo-dep", "1.0.0", dep.clone());
        fake.add("demo-root", "1.0.0", root.clone());
        assert!(matches!(
            run(&plan, &policy(true), &mut fake),
            Err(RuntimeError::Trust {
                error: TrustError::MissingSignature,
                ..
            })
        ));
        assert!(fake.load_log.is_empty());

        let mut fake = Fake::new(5);
        fake.add("demo-dep", "1.0.0", dep);
        fake.add("demo-root", "1.0.0", root);
        let report = run(&plan, &policy(false), &mut fake).expect("unsigned run");
        assert_eq!(report.signatures, SignatureState::UnsignedAllowed);
    }

    #[test]
    fn too_many_modules_and_reserved_region_fail_before_load() {
        let (index, dep, root) = graph();
        let plan = Plan::from_index(&index, "demo-root", "1.0.0", ABI).expect("plan");
        let mut fake = Fake::new(2);
        fake.add("demo-dep", "1.0.0", dep);
        fake.add("demo-root", "1.0.0", root);
        assert!(matches!(
            run(&plan, &policy(false), &mut fake),
            Err(RuntimeError::TooManyModules { .. })
        ));
        assert!(fake.load_log.is_empty());

        let mut reserved = plan.clone();
        reserved.nodes[0].region = 4;
        let mut fake = Fake::new(5);
        assert!(matches!(
            run(&reserved, &policy(false), &mut fake),
            Err(RuntimeError::RegionOutOfRange { .. })
        ));
        assert!(fake.load_log.is_empty());
    }

    #[test]
    fn manifest_exports_are_registered_and_table_checked() {
        let export = Export::new("irix7.demo.service", 1).with_offset(4);
        let dep = image("demo-dep", "1.0.0", &[], core::slice::from_ref(&export));
        let root = image(
            "demo-root",
            "1.0.0",
            &[("demo-dep", "1.0.0", digest(&dep))],
            &[],
        );
        let index = index(&[
            line(
                "demo-dep",
                "1.0.0",
                0,
                &dep,
                &[],
                core::slice::from_ref(&export),
                None,
            ),
            line(
                "demo-root",
                "1.0.0",
                1,
                &root,
                &[("demo-dep", "1.0.0", digest(&dep))],
                &[],
                None,
            ),
        ]);
        let plan = Plan::from_index(&index, "demo-root", "1.0.0", ABI).expect("plan");
        let mut fake = Fake::new(5);
        fake.add("demo-dep", "1.0.0", dep);
        fake.add("demo-root", "1.0.0", root);
        let report = run(&plan, &policy(false), &mut fake).expect("run");
        assert_eq!(report.nodes[0].exports, vec![export]);
        assert!(report
            .status_text()
            .contains("export=demo-dep@1.0.0 interface=irix7.demo.service version=1 offset=4"));
    }

    #[test]
    fn index_export_mismatch_fails_before_execution() {
        let export = Export::new("irix7.demo.service", 1).with_offset(4);
        let dep = image("demo-dep", "1.0.0", &[], core::slice::from_ref(&export));
        let index = index(&[line(
            "demo-dep",
            "1.0.0",
            0,
            &dep,
            &[],
            &[Export::new("other.interface", 1).with_offset(4)],
            None,
        )]);
        let plan = Plan::from_index(&index, "demo-dep", "1.0.0", ABI).expect("plan");
        let mut fake = Fake::new(5);
        fake.add("demo-dep", "1.0.0", dep);
        assert!(matches!(
            run(&plan, &policy(false), &mut fake),
            Err(RuntimeError::ManifestMismatch {
                field: "exports",
                ..
            })
        ));
        assert!(fake.exec_log.is_empty());
        assert!(fake.loaded.is_empty());
    }

    fn m2_export() -> Export {
        Export::new("irix7.mesh.transport", 1).with_offset(8)
    }

    fn m2_graph() -> (PackageIndex, Vec<u8>, Vec<u8>) {
        let export = m2_export();
        let mesh = image("m2-mesh", "1.0.0", &[], core::slice::from_ref(&export));
        let root = image(
            "m2-root",
            "1.0.0",
            &[("m2-mesh", "1.0.0", digest(&mesh))],
            &[],
        );
        let index = index(&[
            line(
                "m2-mesh",
                "1.0.0",
                2,
                &mesh,
                &[],
                core::slice::from_ref(&export),
                None,
            ),
            line(
                "m2-root",
                "1.0.0",
                3,
                &root,
                &[("m2-mesh", "1.0.0", digest(&mesh))],
                &[],
                None,
            ),
        ]);
        (index, mesh, root)
    }

    #[test]
    fn m2_runtime_graph_loads_mesh_before_root() {
        let (index, mesh, root) = m2_graph();
        let plan = Plan::from_index(&index, "m2-root", "1.0.0", ABI).expect("plan");
        assert_eq!(plan.nodes.len(), 2);
        assert_eq!(plan.nodes[0].region, 2);
        assert_eq!(plan.nodes[1].region, 3);
        assert_eq!(plan.nodes[0].exports, vec![m2_export()]);

        let mut fake = Fake::new(5);
        fake.add("m2-mesh", "1.0.0", mesh);
        fake.add("m2-root", "1.0.0", root);
        let report = run(&plan, &policy(false), &mut fake).expect("run");

        assert_eq!(fake.load_log[0].name, "m2-mesh");
        assert_eq!(fake.load_log[1].name, "m2-root");
        assert_eq!(fake.exec_log[0].name, "m2-mesh");
        assert_eq!(fake.exec_log[1].name, "m2-root");
        let mesh_exec = fake
            .exec_log
            .iter()
            .position(|id| id.name == "m2-mesh")
            .expect("mesh executed");
        let root_exec = fake
            .exec_log
            .iter()
            .position(|id| id.name == "m2-root")
            .expect("root executed");
        assert!(mesh_exec < root_exec, "root entered before m2-mesh");
        assert!(report
            .status_text()
            .contains("order=m2-mesh@1.0.0,m2-root@1.0.0"));
        assert!(report
            .status_text()
            .contains("export=m2-mesh@1.0.0 interface=irix7.mesh.transport version=1 offset=8"));
        assert!(fake.unload_log.is_empty());
    }

    #[test]
    fn m2_bad_mesh_digest_fails_closed_before_root_entry() {
        let (index, mesh, root) = m2_graph();
        let plan = Plan::from_index(&index, "m2-root", "1.0.0", ABI).expect("plan");
        let mut fake = Fake::new(5);
        fake.add("m2-mesh", "1.0.0", mesh);
        fake.add("m2-root", "1.0.0", root);
        fake.corrupt = Some(ModuleId::new("m2-mesh", "1.0.0"));
        assert!(matches!(
            run(&plan, &policy(false), &mut fake),
            Err(RuntimeError::DigestMismatch(_))
        ));
        assert!(fake.exec_log.is_empty());
        assert!(fake.loaded.is_empty());
    }

    #[test]
    fn m2_missing_export_fails_closed_before_root_entry() {
        let (index, mesh, root) = m2_graph();
        let plan = Plan::from_index(&index, "m2-root", "1.0.0", ABI).expect("plan");
        let mut fake = Fake::new(5);
        fake.add("m2-mesh", "1.0.0", mesh);
        fake.add("m2-root", "1.0.0", root);
        fake.drop_exports = true;
        assert!(matches!(
            run(&plan, &policy(false), &mut fake),
            Err(RuntimeError::ExportMissing {
                interface,
                version: 1,
                ..
            }) if interface == "irix7.mesh.transport"
        ));
        assert!(fake.exec_log.is_empty());
        assert!(fake.loaded.is_empty());
    }

    #[test]
    fn m2_stale_handle_fails_closed_before_root_entry() {
        let (index, mesh, root) = m2_graph();
        let plan = Plan::from_index(&index, "m2-root", "1.0.0", ABI).expect("plan");
        let mut fake = Fake::new(5);
        fake.add("m2-mesh", "1.0.0", mesh);
        fake.add("m2-root", "1.0.0", root);
        fake.stale_execute = true;
        assert!(matches!(
            run(&plan, &policy(false), &mut fake),
            Err(RuntimeError::Backend {
                stage: "execute",
                status: -9,
                ..
            })
        ));
        assert!(fake.exec_log.is_empty());
        assert_eq!(fake.unload_log.len(), 2);
        assert!(fake.loaded.is_empty());
    }

    #[test]
    fn m2_oversized_mesh_fails_closed_before_root_entry() {
        let oversized = footer::wrap(
            &alloc::vec![0u8; 300 * 1024],
            &Manifest::new("m2-mesh", "1.0.0", ABI),
        )
        .expect("wrap oversized mesh");
        let root = image(
            "m2-root",
            "1.0.0",
            &[("m2-mesh", "1.0.0", digest(&oversized))],
            &[],
        );
        let index = index(&[
            line("m2-mesh", "1.0.0", 2, &oversized, &[], &[], None),
            line(
                "m2-root",
                "1.0.0",
                3,
                &root,
                &[("m2-mesh", "1.0.0", digest(&oversized))],
                &[],
                None,
            ),
        ]);
        let plan = Plan::from_index(&index, "m2-root", "1.0.0", ABI).expect("plan");
        let mut fake = Fake::new(5);
        fake.add("m2-mesh", "1.0.0", oversized);
        fake.add("m2-root", "1.0.0", root);
        assert!(matches!(
            run(&plan, &policy(false), &mut fake),
            Err(RuntimeError::Load { status: -5, .. })
        ));
        assert!(fake.exec_log.is_empty());
        assert!(fake.loaded.is_empty());
    }

    #[test]
    fn m2_export_owner_mismatch_is_rejected_before_root_entry() {
        let (index, mesh, root) = m2_graph();
        let plan = Plan::from_index(&index, "m2-root", "1.0.0", ABI).expect("plan");
        let mut fake = Fake::new(5);
        fake.add("m2-mesh", "1.0.0", mesh);
        fake.add("m2-root", "1.0.0", root);
        fake.export_owner = Some(0x0000_0001_0000_0003);
        assert!(matches!(
            run(&plan, &policy(false), &mut fake),
            Err(RuntimeError::ExportOwner {
                interface,
                version: 1,
                ..
            }) if interface == "irix7.mesh.transport"
        ));
        assert!(fake.exec_log.is_empty());
        assert!(fake.loaded.is_empty());
    }

    #[test]
    fn m2_root_declares_the_exact_mesh_digest() {
        let export = m2_export();
        let mesh = image("m2-mesh", "1.0.0", &[], core::slice::from_ref(&export));
        let wrong = [0x55u8; 32];
        let root = image("m2-root", "1.0.0", &[("m2-mesh", "1.0.0", wrong)], &[]);
        let index = index(&[
            line(
                "m2-mesh",
                "1.0.0",
                2,
                &mesh,
                &[],
                core::slice::from_ref(&export),
                None,
            ),
            line(
                "m2-root",
                "1.0.0",
                3,
                &root,
                &[("m2-mesh", "1.0.0", wrong)],
                &[],
                None,
            ),
        ]);
        assert!(matches!(
            Plan::from_index(&index, "m2-root", "1.0.0", ABI),
            Err(RuntimeError::Resolve(ResolveError::DigestMismatch { .. }))
        ));
    }
}
