// SPDX-License-Identifier: GPL-3.0-only
//! Package-index records with exact runtime dependencies and release fields.
//!
//! The v1 line remains:
//!
//! ```text
//! kind name version url sha256 description...
//! ```
//!
//! Version 2 adds optional whitespace-delimited metadata fields before the
//! human description:
//!
//! ```text
//! kind name version url sha256 \
//!   [depends=name@version:sha256,...] \
//!   [abi=major.minor] [region=N] \
//!   [exports=interface@version:offset,...] \
//!   [sig=epoch:128-hex-digits] description...
//! ```
//!
//! Dependencies are exact versions and 64-hex SHA-256 digests, matching the
//! module manifest. Exports are declarations copied from the image manifest so
//! `module_load` can register them before the image is entered. `region` names
//! the fixed arena region the image was linked for. `sig` is the detached
//! Ed25519 release signature over the record digest. There are no ranges or
//! solver; runtime metadata is not optional when the runtime manager consumes
//! a record.

use alloc::string::{String, ToString};
use alloc::vec::Vec;
use core::fmt;

use module_trust::SignatureRecord;

use crate::{Abi, Dependency, Digest, Export, Manifest, Record};

/// Package-index format accepted by the runtime manager.
pub const INDEX_VERSION: u16 = 2;
/// Maximum index bytes accepted by the manager.
pub const MAX_INDEX_BYTES: usize = 64 * 1024;
/// Maximum records in one index.
pub const MAX_INDEX_RECORDS: usize = 256;
/// Maximum bytes in one index line.
pub const MAX_LINE_BYTES: usize = 8 * 1024;

/// Package record kind.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PackageKind {
    /// Kernel module image.
    Module,
    /// Userland image written to target 2 or run from RAM.
    Userland,
}

impl PackageKind {
    /// Wire spelling.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Module => "module",
            Self::Userland => "userland",
        }
    }
}

impl fmt::Display for PackageKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// One parsed package-index record.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PackageRecord {
    /// Module or userland.
    pub kind: PackageKind,
    /// Stable package name.
    pub name: String,
    /// Exact package version.
    pub version: String,
    /// Immutable HTTPS URL.
    pub url: String,
    /// SHA-256 over the exact published bytes.
    pub sha256: Digest,
    /// ABI the image was built for. Required by the runtime manager.
    pub abi: Option<Abi>,
    /// Arena region the image was linked for. Required by the runtime manager.
    pub region: Option<u32>,
    /// Exact runtime dependencies. Empty for standalone packages.
    pub dependencies: Vec<Dependency>,
    /// Interfaces the image exports, copied from its manifest.
    pub exports: Vec<Export>,
    /// Detached release signature over `sha256`.
    pub signature: Option<SignatureRecord>,
    /// Human description; may be empty.
    pub description: String,
}

impl PackageRecord {
    /// Build the manifest this record describes.
    ///
    /// Returns `None` for a legacy record without an `abi=` field. Entry and
    /// export offsets are filled from the image footer when the manager loads
    /// it; the index copy of the exports exists so they can be registered in
    /// the same `module_load` call.
    pub fn manifest(&self) -> Option<Manifest> {
        let abi = self.abi?;
        let mut manifest = Manifest::new(self.name.as_str(), self.version.as_str(), abi);
        manifest.dependencies = self.dependencies.clone();
        manifest.exports = self.exports.clone();
        Some(manifest)
    }

    /// Build a resolver record for this package.
    pub fn resolver_record(&self) -> Option<Record> {
        Some(Record::new(self.manifest()?, self.sha256))
    }
}

/// A parsed package index.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PackageIndex {
    /// Index format version.
    pub version: u16,
    /// Records in file order.
    pub records: Vec<PackageRecord>,
}

impl PackageIndex {
    /// Parse one complete index image.
    pub fn parse(bytes: &[u8]) -> Result<Self, IndexError> {
        if bytes.len() > MAX_INDEX_BYTES {
            return Err(IndexError::TooLarge);
        }
        let text = core::str::from_utf8(bytes).map_err(|_| IndexError::InvalidUtf8)?;
        let mut version = None;
        let mut records = Vec::new();
        for raw_line in text.lines() {
            if raw_line.len() > MAX_LINE_BYTES {
                return Err(IndexError::LineTooLong);
            }
            let line = raw_line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            if let Some(value) = line.strip_prefix("version=") {
                if version.is_some() {
                    return Err(IndexError::DuplicateField("version"));
                }
                let value = value
                    .parse::<u16>()
                    .map_err(|_| IndexError::BadVersion(line.to_string()))?;
                if value != INDEX_VERSION {
                    return Err(IndexError::UnsupportedVersion(value));
                }
                version = Some(value);
                continue;
            }
            if records.len() >= MAX_INDEX_RECORDS {
                return Err(IndexError::TooManyRecords);
            }
            records.push(parse_record(line)?);
        }
        Ok(Self {
            version: version.unwrap_or(1),
            records,
        })
    }

    /// Find an exact module record.
    pub fn find_module(&self, name: &str, version: &str) -> Option<&PackageRecord> {
        self.records.iter().find(|record| {
            record.kind == PackageKind::Module && record.name == name && record.version == version
        })
    }
}

/// Package-index parsing failure.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum IndexError {
    /// A required field is missing.
    Missing(&'static str),
    /// The kind is not module or userland.
    UnknownKind(String),
    /// A digest is not 64 lowercase hex digits.
    BadDigest,
    /// A dependency is not `name@version:sha256`.
    BadDependency(String),
    /// The same dependency name appears twice in one record.
    DuplicateDependency(String),
    /// A field is not understood.
    UnknownField(String),
    /// A field appears twice in one record.
    DuplicateField(&'static str),
    /// A description begins with a field-looking token.
    BadField(String),
    /// The ABI field is not `major.minor`.
    BadAbi(String),
    /// The region field is not a decimal u32.
    BadRegion(String),
    /// An export is not `interface@version[:offset]`.
    BadExport(String),
    /// The same interface/version is exported twice.
    DuplicateExport(String),
    /// The signature field is not `epoch:128-hex-digits`.
    BadSignature(String),
    /// The index is larger than [`MAX_INDEX_BYTES`].
    TooLarge,
    /// The index has more than [`MAX_INDEX_RECORDS`] records.
    TooManyRecords,
    /// One line is longer than [`MAX_LINE_BYTES`].
    LineTooLong,
    /// The index is not UTF-8.
    InvalidUtf8,
    /// The version header is not a number.
    BadVersion(String),
    /// The index format version is not supported.
    UnsupportedVersion(u16),
}

impl fmt::Display for IndexError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Missing(field) => write!(f, "missing package field: {field}"),
            Self::UnknownKind(kind) => write!(f, "unknown package kind: {kind}"),
            Self::BadDigest => f.write_str("bad SHA-256 digest"),
            Self::BadDependency(dep) => write!(f, "bad dependency: {dep}"),
            Self::DuplicateDependency(name) => write!(f, "duplicate dependency: {name}"),
            Self::UnknownField(field) => write!(f, "unknown package field: {field}"),
            Self::DuplicateField(field) => write!(f, "duplicate package field: {field}"),
            Self::BadField(field) => write!(f, "unexpected field token: {field}"),
            Self::BadAbi(value) => write!(f, "bad ABI field: {value}"),
            Self::BadRegion(value) => write!(f, "bad region field: {value}"),
            Self::BadExport(value) => write!(f, "bad export field: {value}"),
            Self::DuplicateExport(value) => write!(f, "duplicate export: {value}"),
            Self::BadSignature(value) => write!(f, "bad signature field: {value}"),
            Self::TooLarge => f.write_str("package index exceeds 64 KiB"),
            Self::TooManyRecords => f.write_str("package index has too many records"),
            Self::LineTooLong => f.write_str("package index line is too long"),
            Self::InvalidUtf8 => f.write_str("package index is not UTF-8"),
            Self::BadVersion(value) => write!(f, "bad package index version: {value}"),
            Self::UnsupportedVersion(version) => {
                write!(f, "package index version {version} is unsupported")
            }
        }
    }
}

/// Parse one v2 package-index line.
///
/// Blank lines and comments are the caller's responsibility; this function
/// expects one record. Metadata fields are parsed before the remaining
/// whitespace-separated words are joined as the description.
pub fn parse_record(line: &str) -> Result<PackageRecord, IndexError> {
    let mut fields = line.split_whitespace();
    let kind = match fields.next().ok_or(IndexError::Missing("kind"))? {
        "module" => PackageKind::Module,
        "userland" => PackageKind::Userland,
        other => return Err(IndexError::UnknownKind(other.to_string())),
    };
    let name = fields
        .next()
        .ok_or(IndexError::Missing("name"))?
        .to_string();
    let version = fields
        .next()
        .ok_or(IndexError::Missing("version"))?
        .to_string();
    let url = fields.next().ok_or(IndexError::Missing("url"))?.to_string();
    let digest_text = fields.next().ok_or(IndexError::Missing("sha256"))?;
    let sha256 = parse_digest(digest_text).ok_or(IndexError::BadDigest)?;

    let mut abi = None;
    let mut region = None;
    let mut dependencies = Vec::new();
    let mut exports = Vec::new();
    let mut signature = None;
    let mut description = Vec::new();
    let mut in_description = false;
    let mut seen_depends = false;
    let mut seen_abi = false;
    let mut seen_region = false;
    let mut seen_exports = false;
    let mut seen_signature = false;
    for field in fields {
        if in_description {
            description.push(field);
            continue;
        }
        if let Some(rest) = field.strip_prefix("depends=") {
            if seen_depends {
                return Err(IndexError::DuplicateField("depends"));
            }
            seen_depends = true;
            dependencies = parse_depends(rest)?;
            continue;
        }
        if let Some(rest) = field.strip_prefix("abi=") {
            if seen_abi {
                return Err(IndexError::DuplicateField("abi"));
            }
            seen_abi = true;
            abi = Some(parse_abi(rest)?);
            continue;
        }
        if let Some(rest) = field.strip_prefix("region=") {
            if seen_region {
                return Err(IndexError::DuplicateField("region"));
            }
            seen_region = true;
            region = Some(
                rest.parse::<u32>()
                    .map_err(|_| IndexError::BadRegion(rest.to_string()))?,
            );
            continue;
        }
        if let Some(rest) = field.strip_prefix("exports=") {
            if seen_exports {
                return Err(IndexError::DuplicateField("exports"));
            }
            seen_exports = true;
            exports = parse_exports(rest)?;
            continue;
        }
        if let Some(rest) = field.strip_prefix("sig=") {
            if seen_signature {
                return Err(IndexError::DuplicateField("sig"));
            }
            seen_signature = true;
            signature = Some(
                SignatureRecord::parse(rest)
                    .map_err(|_| IndexError::BadSignature(rest.to_string()))?,
            );
            continue;
        }
        if field.contains('=') {
            return Err(IndexError::UnknownField(field.to_string()));
        }
        in_description = true;
        description.push(field);
    }

    Ok(PackageRecord {
        kind,
        name,
        version,
        url,
        sha256,
        abi,
        region,
        dependencies,
        exports,
        signature,
        description: description.join(" "),
    })
}

fn parse_abi(text: &str) -> Result<Abi, IndexError> {
    let (major, minor) = text
        .split_once('.')
        .ok_or_else(|| IndexError::BadAbi(text.to_string()))?;
    let major = major
        .parse::<u16>()
        .map_err(|_| IndexError::BadAbi(text.to_string()))?;
    let minor = minor
        .parse::<u16>()
        .map_err(|_| IndexError::BadAbi(text.to_string()))?;
    Ok(Abi::new(major, minor))
}

fn parse_exports(text: &str) -> Result<Vec<Export>, IndexError> {
    if text.is_empty() {
        return Err(IndexError::BadExport(String::new()));
    }
    let mut exports = Vec::new();
    for spec in text.split(',') {
        let (interface, rest) = spec
            .split_once('@')
            .ok_or_else(|| IndexError::BadExport(spec.to_string()))?;
        if interface.is_empty() {
            return Err(IndexError::BadExport(spec.to_string()));
        }
        let (version, offset) = match rest.split_once(':') {
            Some((version, offset)) => (
                version,
                offset
                    .parse::<u32>()
                    .map_err(|_| IndexError::BadExport(spec.to_string()))?,
            ),
            None => (rest, 0),
        };
        if offset % 4 != 0 {
            return Err(IndexError::BadExport(spec.to_string()));
        }
        let version = version
            .parse::<u16>()
            .map_err(|_| IndexError::BadExport(spec.to_string()))?;
        if exports
            .iter()
            .any(|export: &Export| export.interface == interface && export.version == version)
        {
            return Err(IndexError::DuplicateExport(spec.to_string()));
        }
        exports.push(Export::new(interface, version).with_offset(offset));
    }
    Ok(exports)
}

fn parse_depends(text: &str) -> Result<Vec<Dependency>, IndexError> {
    if text.is_empty() {
        return Err(IndexError::BadDependency(String::new()));
    }
    let mut dependencies = Vec::new();
    for spec in text.split(',') {
        let (name_version, digest_text) = spec
            .rsplit_once(':')
            .ok_or_else(|| IndexError::BadDependency(spec.to_string()))?;
        let (name, version) = name_version
            .split_once('@')
            .ok_or_else(|| IndexError::BadDependency(spec.to_string()))?;
        if name.is_empty() || version.is_empty() {
            return Err(IndexError::BadDependency(spec.to_string()));
        }
        let sha256 =
            parse_digest(digest_text).ok_or_else(|| IndexError::BadDependency(spec.to_string()))?;
        if dependencies
            .iter()
            .any(|dependency: &Dependency| dependency.module.name == name)
        {
            return Err(IndexError::DuplicateDependency(name.to_string()));
        }
        dependencies.push(Dependency::new(name, version, sha256));
    }
    Ok(dependencies)
}

fn parse_digest(text: &str) -> Option<Digest> {
    if text.len() != 64 {
        return None;
    }
    let mut out = [0u8; 32];
    for (index, slot) in out.iter_mut().enumerate() {
        let hi = hex(text.as_bytes()[index * 2])?;
        let lo = hex(text.as_bytes()[index * 2 + 1])?;
        *slot = (hi << 4) | lo;
    }
    Some(out)
}

fn hex(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    fn digest(byte: u8) -> String {
        const HEX: &[u8; 16] = b"0123456789abcdef";
        let mut out = String::with_capacity(64);
        for _ in 0..32 {
            out.push(HEX[(byte >> 4) as usize] as char);
            out.push(HEX[(byte & 0x0f) as usize] as char);
        }
        out
    }

    fn signature(byte: u8) -> String {
        format!("7:{}", digest(byte).repeat(2))
    }

    #[test]
    fn parses_module_with_exact_dependencies_and_release_fields() {
        let line = format!(
            "module mesh.transport 1.2.3 https://example/M {} \
             depends=mesh.codec@1.0.0:{},mcp@0.1.3:{} \
             abi=12.0 region=2 exports=irix7.mesh.transport@1:4096,irix7.mesh.aux@2 \
             sig={} transport module",
            digest(0x11),
            digest(0x22),
            digest(0x33),
            signature(0x44)
        );
        let record = parse_record(&line).expect("parse");
        assert_eq!(record.kind, PackageKind::Module);
        assert_eq!(record.name, "mesh.transport");
        assert_eq!(record.version, "1.2.3");
        assert_eq!(record.abi, Some(Abi::new(12, 0)));
        assert_eq!(record.region, Some(2));
        assert_eq!(record.dependencies.len(), 2);
        assert_eq!(record.dependencies[0].module.name, "mesh.codec");
        assert_eq!(record.dependencies[1].module.version, "0.1.3");
        assert_eq!(record.exports.len(), 2);
        assert_eq!(record.exports[0].offset, 4096);
        assert_eq!(record.exports[1].offset, 0);
        assert_eq!(record.signature.expect("signature").key_epoch, 7);
        assert_eq!(record.description, "transport module");
        assert_eq!(record.manifest().expect("manifest").abi, Abi::new(12, 0));
    }

    #[test]
    fn index_parses_versions_comments_and_bounds() {
        let text = format!(
            "# runtime index\nversion=2\n\nmodule a 1.0 https://example/A {} abi=12.0 region=0\n",
            digest(0x11)
        );
        let index = PackageIndex::parse(text.as_bytes()).expect("parse index");
        assert_eq!(index.version, 2);
        assert_eq!(index.records.len(), 1);
        assert!(index.find_module("a", "1.0").is_some());
        assert!(index.find_module("a", "1.1").is_none());
        assert_eq!(
            PackageIndex::parse(b"version=1\n"),
            Err(IndexError::UnsupportedVersion(1))
        );
        assert_eq!(
            PackageIndex::parse(b"version=2\nversion=2\n"),
            Err(IndexError::DuplicateField("version"))
        );
        assert_eq!(
            PackageIndex::parse(&vec![b'x'; MAX_INDEX_BYTES + 1]),
            Err(IndexError::TooLarge)
        );
    }

    #[test]
    fn parses_standalone_and_userland() {
        let line = format!(
            "userland std-hello 1.0 https://example/S {} fixture",
            digest(0x44)
        );
        let record = parse_record(&line).expect("parse");
        assert_eq!(record.kind, PackageKind::Userland);
        assert!(record.dependencies.is_empty());
        assert!(record.abi.is_none());
        assert_eq!(record.description, "fixture");
    }

    #[test]
    fn rejects_bad_digest() {
        let line = "module x 1 https://example/X nothex";
        assert_eq!(parse_record(line), Err(IndexError::BadDigest));
    }

    #[test]
    fn rejects_bad_dependency() {
        let line = format!("module x 1 https://example/X {} depends=x", digest(0x11));
        assert!(matches!(
            parse_record(&line),
            Err(IndexError::BadDependency(_))
        ));
    }

    #[test]
    fn rejects_duplicate_dependency() {
        let line = format!(
            "module x 1 https://example/X {} depends=a@1:{},a@2:{}",
            digest(0x11),
            digest(0x22),
            digest(0x33)
        );
        assert_eq!(
            parse_record(&line),
            Err(IndexError::DuplicateDependency("a".to_string()))
        );
    }

    #[test]
    fn rejects_unknown_kind_and_field() {
        assert!(matches!(
            parse_record("bundle x 1 https://example/X 0000"),
            Err(IndexError::UnknownKind(_))
        ));
        let line = format!(
            "module x 1 https://example/X {} unknown=value",
            digest(0x11)
        );
        assert!(matches!(
            parse_record(&line),
            Err(IndexError::UnknownField(_))
        ));
    }

    #[test]
    fn rejects_missing_fields() {
        assert_eq!(
            parse_record("module x"),
            Err(IndexError::Missing("version"))
        );
    }

    #[test]
    fn rejects_empty_depends_and_exports() {
        let line = format!("module x 1 https://example/X {} depends=", digest(0x11));
        assert!(matches!(
            parse_record(&line),
            Err(IndexError::BadDependency(_))
        ));
        let line = format!("module x 1 https://example/X {} exports=", digest(0x11));
        assert!(matches!(parse_record(&line), Err(IndexError::BadExport(_))));
    }

    #[test]
    fn rejects_bad_release_fields() {
        let base = format!("module x 1 https://example/X {}", digest(0x11));
        assert!(matches!(
            parse_record(&format!("{base} abi=12")),
            Err(IndexError::BadAbi(_))
        ));
        assert!(matches!(
            parse_record(&format!("{base} region=-1")),
            Err(IndexError::BadRegion(_))
        ));
        assert!(matches!(
            parse_record(&format!("{base} exports=a@1:3")),
            Err(IndexError::BadExport(_))
        ));
        assert!(matches!(
            parse_record(&format!("{base} sig=bad")),
            Err(IndexError::BadSignature(_))
        ));
        assert_eq!(
            parse_record(&format!("{base} abi=12.0 abi=12.0")),
            Err(IndexError::DuplicateField("abi"))
        );
    }
}
