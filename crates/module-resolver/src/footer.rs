// SPDX-License-Identifier: GPL-3.0-only
//! Versioned manifest footer for module images.
//!
//! The footer lets a loader find a manifest without changing the module entry
//! at offset 0. The enclosing module image digest is the integrity boundary;
//! the footer itself has no checksum.
//!
//! ```text
//! image bytes | manifest record | footer
//! footer: I7MF | u16 version=1 | u16 reserved=0 | u32 manifest offset | u32 manifest length
//! ```
//!
//! A standalone image has no footer. A footer that is present but malformed
//! fails closed rather than being treated as an old standalone image.

use alloc::vec::Vec;
use core::fmt;

use crate::codec::{DecodeError, EncodeError, MAX_RECORD};
use crate::Manifest;

/// Footer magic.
pub const MAGIC: [u8; 4] = *b"I7MF";
/// Footer format version.
pub const FORMAT_VERSION: u16 = 1;
/// Encoded footer length.
pub const FOOTER_LEN: usize = 16;

/// Footer construction failure.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FooterError {
    /// The manifest record could not be encoded.
    Encode(EncodeError),
    /// The image or manifest is too large for the u32 footer fields.
    TooLarge,
}

impl fmt::Display for FooterError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Encode(error) => write!(f, "manifest encoding failed: {error}"),
            Self::TooLarge => f.write_str("image or manifest exceeds u32"),
        }
    }
}

/// Manifest footer parsing failure.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ParseError {
    /// The footer version is not supported.
    UnsupportedVersion(u16),
    /// The reserved field is non-zero.
    Reserved,
    /// The manifest range is outside the image or does not end at the footer.
    OutOfRange,
    /// The manifest record is malformed.
    Manifest(DecodeError),
    /// The manifest record exceeds the 64 KiB manifest bound.
    ManifestTooLarge,
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnsupportedVersion(version) => {
                write!(f, "manifest footer version {version} is unsupported")
            }
            Self::Reserved => f.write_str("manifest footer reserved field is non-zero"),
            Self::OutOfRange => f.write_str("manifest footer range is out of bounds"),
            Self::Manifest(error) => write!(f, "manifest footer record: {error}"),
            Self::ManifestTooLarge => f.write_str("manifest footer record exceeds 64 KiB"),
        }
    }
}

impl From<EncodeError> for FooterError {
    fn from(error: EncodeError) -> Self {
        Self::Encode(error)
    }
}

impl From<DecodeError> for ParseError {
    fn from(error: DecodeError) -> Self {
        Self::Manifest(error)
    }
}

/// Append a manifest record and footer to an image.
pub fn wrap(image: &[u8], manifest: &Manifest) -> Result<Vec<u8>, FooterError> {
    let record = manifest.encode()?;
    let offset = u32::try_from(image.len()).map_err(|_| FooterError::TooLarge)?;
    let len = u32::try_from(record.len()).map_err(|_| FooterError::TooLarge)?;
    let mut out = Vec::with_capacity(image.len() + record.len() + FOOTER_LEN);
    out.extend_from_slice(image);
    out.extend_from_slice(&record);
    out.extend_from_slice(&MAGIC);
    out.extend_from_slice(&FORMAT_VERSION.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    out.extend_from_slice(&offset.to_le_bytes());
    out.extend_from_slice(&len.to_le_bytes());
    Ok(out)
}

/// Parse a footer and its manifest, if present.
///
/// Returns `Ok(None)` for a standalone image without the footer magic. A
/// present footer with invalid fields returns an error.
pub fn parse(image: &[u8]) -> Result<Option<(Manifest, &[u8])>, ParseError> {
    if image.len() < FOOTER_LEN {
        return Ok(None);
    }
    let footer_start = image.len() - FOOTER_LEN;
    let footer = &image[footer_start..];
    if footer[..4] != MAGIC {
        return Ok(None);
    }
    let version = u16::from_le_bytes([footer[4], footer[5]]);
    if version != FORMAT_VERSION {
        return Err(ParseError::UnsupportedVersion(version));
    }
    if footer[6] != 0 || footer[7] != 0 {
        return Err(ParseError::Reserved);
    }
    let offset = u32::from_le_bytes([footer[8], footer[9], footer[10], footer[11]]) as usize;
    let len = u32::from_le_bytes([footer[12], footer[13], footer[14], footer[15]]) as usize;
    if len > MAX_RECORD {
        return Err(ParseError::ManifestTooLarge);
    }
    let end = offset.checked_add(len).ok_or(ParseError::OutOfRange)?;
    if offset > footer_start || end != footer_start {
        return Err(ParseError::OutOfRange);
    }
    let manifest = Manifest::decode(&image[offset..end])?;
    Ok(Some((manifest, &image[..offset])))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Abi, Dependency, Export};

    fn manifest() -> Manifest {
        Manifest::new("mesh.transport", "1.2.3", Abi::new(1, 0))
            .with_dependency(Dependency::new("mesh.codec", "1.0.0", [0x11; 32]))
            .with_export(Export::new("irix7.mesh.transport", 1))
    }

    #[test]
    fn round_trip_preserves_image_and_manifest() {
        let image = b"flat module bytes";
        let wrapped = wrap(image, &manifest()).expect("wrap");
        let (decoded, payload) = parse(&wrapped).expect("parse").expect("footer");
        assert_eq!(decoded, manifest());
        assert_eq!(payload, image);
    }

    #[test]
    fn standalone_image_has_no_footer() {
        assert_eq!(parse(b"no footer here").expect("parse"), None);
    }

    #[test]
    fn bad_footer_version_fails_closed() {
        let mut wrapped = wrap(b"image", &manifest()).expect("wrap");
        let start = wrapped.len() - FOOTER_LEN;
        wrapped[start + 4..start + 6].copy_from_slice(&2u16.to_le_bytes());
        assert_eq!(parse(&wrapped), Err(ParseError::UnsupportedVersion(2)));
    }

    #[test]
    fn reserved_field_fails_closed() {
        let mut wrapped = wrap(b"image", &manifest()).expect("wrap");
        let start = wrapped.len() - FOOTER_LEN;
        wrapped[start + 6] = 1;
        assert_eq!(parse(&wrapped), Err(ParseError::Reserved));
    }

    #[test]
    fn range_must_end_at_footer() {
        let mut wrapped = wrap(b"image", &manifest()).expect("wrap");
        let start = wrapped.len() - FOOTER_LEN;
        wrapped[start + 12..start + 16].copy_from_slice(&0u32.to_le_bytes());
        assert_eq!(parse(&wrapped), Err(ParseError::OutOfRange));
    }

    #[test]
    fn corrupt_manifest_is_rejected() {
        let mut wrapped = wrap(b"image", &manifest()).expect("wrap");
        let start = wrapped.len() - FOOTER_LEN;
        let offset = u32::from_le_bytes([
            wrapped[start + 8],
            wrapped[start + 9],
            wrapped[start + 10],
            wrapped[start + 11],
        ]) as usize;
        wrapped[offset] = b'X';
        assert!(matches!(parse(&wrapped), Err(ParseError::Manifest(_))));
    }

    #[test]
    fn oversized_manifest_is_rejected() {
        let mut wrapped = wrap(b"image", &manifest()).expect("wrap");
        let start = wrapped.len() - FOOTER_LEN;
        wrapped[start + 12..start + 16].copy_from_slice(&((MAX_RECORD + 1) as u32).to_le_bytes());
        assert_eq!(parse(&wrapped), Err(ParseError::ManifestTooLarge));
    }
}
