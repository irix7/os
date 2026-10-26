// SPDX-License-Identifier: GPL-3.0-only
//! Package-index v2 records with exact runtime dependencies.
//!
//! The v1 index line remains:
//!
//! ```text
//! kind name version url sha256 description...
//! ```
//!
//! Version 2 adds an optional dependency field before the human description:
//!
//! ```text
//! kind name version url sha256 \
//!   depends=name@version:sha256,name@version:sha256,... description...
//! ```
//!
//! The `depends=` field is a single whitespace-delimited token. Dependencies
//! are exact versions and 64-hex SHA-256 digests, matching the module manifest.
//! There are no ranges or solver in v2.

use alloc::string::{String, ToString};
use alloc::vec::Vec;
use core::fmt;

use crate::{Dependency, Digest};

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
    /// Exact runtime dependencies. Empty for standalone packages.
    pub dependencies: Vec<Dependency>,
    /// Human description; may be empty.
    pub description: String,
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
    /// A description begins with a field-looking token.
    BadField(String),
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
            Self::BadField(field) => write!(f, "unexpected field token: {field}"),
        }
    }
}

/// Parse one v2 package-index line.
///
/// Blank lines and comments are the caller's responsibility; this function
/// expects one record. A trailing `depends=` token is parsed before the
/// remaining whitespace-separated words are joined as the description.
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

    let mut dependencies = Vec::new();
    let mut description = Vec::new();
    let mut in_description = false;
    for field in fields {
        if let Some(rest) = field.strip_prefix("depends=") {
            if in_description {
                return Err(IndexError::BadField(field.to_string()));
            }
            if !dependencies.is_empty() {
                return Err(IndexError::DuplicateDependency("depends".to_string()));
            }
            dependencies = parse_depends(rest)?;
            continue;
        }
        if field.contains('=') && !in_description {
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
        dependencies,
        description: description.join(" "),
    })
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
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn digest(byte: u8) -> String {
        const HEX: &[u8; 16] = b"0123456789abcdef";
        let mut out = String::with_capacity(64);
        for _ in 0..32 {
            out.push(HEX[(byte >> 4) as usize] as char);
            out.push(HEX[(byte & 0x0f) as usize] as char);
        }
        out
    }

    #[test]
    fn parses_module_with_exact_dependencies() {
        let line = format!(
            "module mesh.transport 1.2.3 https://example/M  {} depends=mesh.codec@1.0.0:{},mcp@0.1.3:{} transport module",
            digest(0x11),
            digest(0x22),
            digest(0x33)
        );
        let record = parse_record(&line).expect("parse");
        assert_eq!(record.kind, PackageKind::Module);
        assert_eq!(record.name, "mesh.transport");
        assert_eq!(record.version, "1.2.3");
        assert_eq!(record.dependencies.len(), 2);
        assert_eq!(record.dependencies[0].module.name, "mesh.codec");
        assert_eq!(record.dependencies[1].module.version, "0.1.3");
        assert_eq!(record.description, "transport module");
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
    fn rejects_empty_depends() {
        let line = format!("module x 1 https://example/X {} depends=", digest(0x11));
        assert!(matches!(
            parse_record(&line),
            Err(IndexError::BadDependency(_))
        ));
    }
}
