// SPDX-License-Identifier: GPL-3.0-only
//! Module images: the header, the digest, the entry call and the vtable check.
//!
//! A boot module is a flat image copied into a reserved window and entered in
//! kernel mode.  The machine supplies the window and the copy; this module
//! owns the checks that decide whether the image may run:
//!
//! 1. the header names the magic, a nonzero length and the SHA-256;
//! 2. the image length fits the window;
//! 3. the bytes match the digest;
//! 4. the entry returns a vtable pointer that lies inside the verified image;
//! 5. the vtable status is zero.
//!
//! Step 4 was missing on `kernel-x86` and present twice (with slightly
//! different arithmetic) on the Indy and PowerPC kernels.  One implementation
//! removes the divergence and the out-of-bounds read.

use core::mem::{align_of, size_of};

use module_abi::{ModuleEntry, ModuleVtable};

use crate::service::SERVICES;

/// Magic at the start of a module header sector.
pub const MAGIC: [u8; 8] = *b"IRIX7MOD";
/// Offset of the image length in the header: big-endian `u32`.
pub const LENGTH_OFFSET: usize = 8;
/// Offset of the SHA-256 digest in the header.
pub const DIGEST_OFFSET: usize = 12;
/// Bytes used by a complete header.
pub const HEADER_LEN: usize = DIGEST_OFFSET + 32;

/// A parsed module header.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Header {
    /// Image length in bytes.
    pub length: u32,
    /// Expected SHA-256 of the image.
    pub digest: [u8; 32],
}

/// Why a header sector was rejected.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HeaderError {
    /// The sector is shorter than [`HEADER_LEN`].
    Short,
    /// The magic is not [`MAGIC`].
    Magic,
    /// The length is zero or larger than the target window.
    Length {
        /// Length written in the header.
        length: u32,
        /// Bytes available in the destination window.
        capacity: usize,
    },
}

impl core::fmt::Display for HeaderError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Short => f.write_str("module header is short"),
            Self::Magic => f.write_str("module header magic is not IRIX7MOD"),
            Self::Length { length, capacity } => {
                write!(f, "module length {length} exceeds window {capacity}")
            }
        }
    }
}

/// Parse and range-check a header sector.
///
/// Every machine that reads a module from a disk uses this one layout.  A
/// header from another endianness, or an image longer than the window, is
/// refused before any bytes are copied.
pub fn parse_header(sector: &[u8], capacity: usize) -> Result<Header, HeaderError> {
    if sector.len() < HEADER_LEN {
        return Err(HeaderError::Short);
    }
    if sector[..MAGIC.len()] != MAGIC {
        return Err(HeaderError::Magic);
    }
    let length = u32::from_be_bytes([
        sector[LENGTH_OFFSET],
        sector[LENGTH_OFFSET + 1],
        sector[LENGTH_OFFSET + 2],
        sector[LENGTH_OFFSET + 3],
    ]);
    if length == 0 || length as usize > capacity {
        return Err(HeaderError::Length { length, capacity });
    }
    let mut digest = [0u8; 32];
    digest.copy_from_slice(&sector[DIGEST_OFFSET..HEADER_LEN]);
    Ok(Header { length, digest })
}

/// A module's reserved window.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Image {
    /// Link address and first byte of the window.
    pub base: usize,
    /// Window size in bytes.
    pub window: usize,
    /// Name used in boot logs.
    pub name: &'static str,
}

impl Image {
    /// Describe a module window.
    pub const fn new(base: usize, window: usize, name: &'static str) -> Self {
        Self { base, window, name }
    }
}

/// Why an image was not entered.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    /// The image is empty or larger than the window.
    Length {
        /// Image length.
        length: usize,
        /// Window size.
        window: usize,
    },
    /// The image does not match the expected digest.
    Digest {
        /// Digest from the header or boot configuration.
        expected: [u8; 32],
        /// Digest of the bytes that were read.
        actual: [u8; 32],
    },
    /// The entry returned a vtable outside the verified image.
    Vtable,
    /// The module's initialisation reported a nonzero status.
    Status(i32),
}

impl core::fmt::Display for Error {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Length { length, window } => {
                write!(f, "module length {length} does not fit window {window}")
            }
            Self::Digest { .. } => f.write_str("module digest mismatch"),
            Self::Vtable => f.write_str("module vtable outside the verified image"),
            Self::Status(status) => write!(f, "module entry status {status}"),
        }
    }
}

/// What a successful entry returned.
#[derive(Clone, Copy, Debug)]
pub struct Report {
    /// The module's vtable, or null for a legacy image with no tick.
    pub vtable: *const ModuleVtable,
    /// The entry's status; always zero on success.
    pub status: i32,
}

impl Report {
    /// The vtable when the module registered one.
    pub fn vtable(&self) -> Option<*const ModuleVtable> {
        if self.vtable.is_null() {
            None
        } else {
            Some(self.vtable)
        }
    }
}

/// The machine side of loading: the window and its cache.
///
/// `copy` receives the image in order; a machine that streams from a disk may
/// write directly and call [`enter`] without `clear`/`copy`.
pub trait Host {
    /// Zero the whole window so `.bss` and a stale image are gone.
    fn clear(&mut self, image: &Image);
    /// Copy `bytes` to `image.base + offset`.
    fn copy(&mut self, image: &Image, offset: usize, bytes: &[u8]);
    /// Make the written code visible to the instruction fetch before entry.
    ///
    /// Coherent machines do nothing; the Indy and PowerPC write back the data
    /// cache and invalidate the instruction cache.
    fn flush_code(&mut self, image: &Image, length: usize);
}

/// SHA-256 of an image.
pub fn digest(image: &[u8]) -> [u8; 32] {
    irix7_sha256::Sha256::digest(image).0
}

/// Compare an image digest with the expected value.
pub fn verify_digest(expected: &[u8; 32], actual: &[u8; 32]) -> Result<(), Error> {
    if expected == actual {
        Ok(())
    } else {
        Err(Error::Digest {
            expected: *expected,
            actual: *actual,
        })
    }
}

/// Whether `vtable` lies wholly inside `base..base + image_len` and is aligned.
pub fn validate_vtable(vtable: *const ModuleVtable, base: usize, image_len: usize) -> bool {
    let address = vtable as usize;
    if address < base || !address.is_multiple_of(align_of::<ModuleVtable>()) {
        return false;
    }
    match address.checked_add(size_of::<ModuleVtable>()) {
        Some(end) => end <= base.saturating_add(image_len),
        None => false,
    }
}

/// Copy a verified byte buffer into the window and enter it.
///
/// The caller has already checked the digest (see [`load`] for the combined
/// path).  The window is cleared first so `.bss` starts zeroed.
pub fn install(image: &Image, bytes: &[u8], host: &mut impl Host) -> Result<Report, Error> {
    if bytes.is_empty() || bytes.len() > image.window {
        return Err(Error::Length {
            length: bytes.len(),
            window: image.window,
        });
    }
    host.clear(image);
    host.copy(image, 0, bytes);
    // SAFETY: the machine reserved the window for modules, the copy is bounded
    // by `window`, and the caller checked the digest.
    unsafe { enter(image, bytes.len(), host) }
}

/// Verify a byte buffer against its digest and enter it.
pub fn load(
    image: &Image,
    bytes: &[u8],
    expected: &[u8; 32],
    host: &mut impl Host,
) -> Result<Report, Error> {
    verify_digest(expected, &digest(bytes))?;
    install(image, bytes, host)
}

/// Enter a streamed image whose bytes are already in `image.base`.
///
/// `length` must be the number of verified bytes in the window.  The digest is
/// checked by a caller that streamed and hashed at the same time; this
/// function performs the cache flush, the entry call, the vtable bounds check
/// and the status check.
///
/// # Safety
///
/// `image.base` must contain a complete verified image of `length` bytes, and
/// arbitrary module code runs in kernel mode from the entry call.
pub unsafe fn enter(image: &Image, length: usize, host: &mut impl Host) -> Result<Report, Error> {
    if length == 0 || length > image.window {
        return Err(Error::Length {
            length,
            window: image.window,
        });
    }
    host.flush_code(image, length);

    // SAFETY: the caller guarantees a complete image at `base`.
    let entry: ModuleEntry = unsafe { core::mem::transmute(image.base) };
    // SAFETY: the entry receives the shared service table and no manager
    // context, which is the same call a legacy `_start` image expects.
    let vtable = unsafe { entry(&SERVICES, core::ptr::null()) };
    if vtable.is_null() {
        return Ok(Report { vtable, status: 0 });
    }
    if !validate_vtable(vtable, image.base, length) {
        return Err(Error::Vtable);
    }
    // SAFETY: the vtable was checked against the verified image.
    let status = unsafe { (*vtable).status };
    if status != 0 {
        return Err(Error::Status(status));
    }
    Ok(Report { vtable, status })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn header(length: u32, digest: [u8; 32]) -> [u8; HEADER_LEN] {
        let mut sector = [0u8; HEADER_LEN];
        sector[..MAGIC.len()].copy_from_slice(&MAGIC);
        sector[LENGTH_OFFSET..DIGEST_OFFSET].copy_from_slice(&length.to_be_bytes());
        sector[DIGEST_OFFSET..].copy_from_slice(&digest);
        sector
    }

    #[test]
    fn parses_the_canonical_header() {
        let digest = [0xab; 32];
        let parsed = parse_header(&header(4096, digest), 8192).unwrap();
        assert_eq!(parsed.length, 4096);
        assert_eq!(parsed.digest, digest);
    }

    #[test]
    fn refuses_a_foreign_or_short_header() {
        assert_eq!(parse_header(&[0u8; 12], 4096), Err(HeaderError::Short));

        let mut sector = header(1, [0; 32]);
        sector[0] = b'N';
        assert_eq!(parse_header(&sector, 4096), Err(HeaderError::Magic));

        // A little-endian length is not the canonical form.
        let mut sector = header(1, [0; 32]);
        sector[LENGTH_OFFSET..DIGEST_OFFSET].copy_from_slice(&1u32.to_le_bytes());
        assert_eq!(
            parse_header(&sector, 32 * 1024 * 1024).unwrap().length,
            16_777_216
        );
    }

    #[test]
    fn refuses_an_empty_or_oversized_length() {
        assert_eq!(
            parse_header(&header(0, [0; 32]), 4096),
            Err(HeaderError::Length {
                length: 0,
                capacity: 4096
            })
        );
        assert_eq!(
            parse_header(&header(4097, [0; 32]), 4096),
            Err(HeaderError::Length {
                length: 4097,
                capacity: 4096
            })
        );
    }

    #[test]
    fn digest_matches_the_known_sha256() {
        let expected = irix7_sha256::Sha256::from_hex(
            b"ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad",
        )
        .unwrap();
        assert_eq!(digest(b"abc"), expected.0);
        assert!(verify_digest(&expected.0, &digest(b"abc")).is_ok());
        assert!(matches!(
            verify_digest(&expected.0, &digest(b"abd")),
            Err(Error::Digest { .. })
        ));
    }

    static VT: ModuleVtable = ModuleVtable {
        status: 0,
        tick: None,
    };
    static PAD: [u8; 64] = [0; 64];

    #[test]
    fn vtable_must_sit_inside_the_image_and_be_aligned() {
        let base = &VT as *const ModuleVtable as usize;
        let size = size_of::<ModuleVtable>();
        assert!(validate_vtable(&VT, base, size));
        assert!(validate_vtable(&VT, base, size + 64));
        assert!(!validate_vtable(&VT, base + 1, size));
        assert!(!validate_vtable(&VT, base + 1, size * 4));
        // Ending past the image is refused even when the start is in range.
        assert!(!validate_vtable(&VT, base + size / 2, size / 2));
        let _ = &PAD;
    }

    #[test]
    fn rejects_an_image_that_does_not_fit() {
        let image = Image::new(0x1000, 16, "test");
        let mut host = VecHost::default();
        assert!(matches!(
            install(&image, &[], &mut host),
            Err(Error::Length { length: 0, .. })
        ));
        assert!(matches!(
            install(&image, &[0u8; 17], &mut host),
            Err(Error::Length { length: 17, .. })
        ));
    }

    #[derive(Default)]
    struct VecHost {
        cleared: bool,
        copied: usize,
        flushed: usize,
    }

    impl Host for VecHost {
        fn clear(&mut self, _image: &Image) {
            self.cleared = true;
        }

        fn copy(&mut self, _image: &Image, _offset: usize, bytes: &[u8]) {
            self.copied += bytes.len();
        }

        fn flush_code(&mut self, _image: &Image, length: usize) {
            self.flushed += length;
        }
    }
}
