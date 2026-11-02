// SPDX-License-Identifier: GPL-3.0-only
//! Loadable modules on the PowerPC G3 machine.
//!
//! A module is a flat image copied into [`SLOT_BASE`] from the boot disk.  The
//! disk layout is the canonical `IRIX7MOD` header from `kernel-core::image`:
//! magic, big-endian image length and the SHA-256, then the image at a fixed
//! LBA.
//!
//! The header, digest and vtable rules and the entry call are the shared core;
//! this file supplies the ATA read, the slot address and the cache flush.

use irix7_sha256::Hasher;

use kernel_core::image::{self, Host, Image};

use crate::ata::{self, Channel, SECTOR_SIZE};
use crate::{cache, console, print_dec, print_hex_bytes, print_line};

/// The module slot: 2 MiB at physical `0x0180_0000`.
///
/// It sits above the kernel image, which ends below `0x0200_0000`.  The whole
/// slot gets one IBAT2/DBAT2 pair.
pub const SLOT_BASE: u32 = 0x0180_0000;
/// Capacity of the module slot in bytes.
pub const SLOT_SIZE: usize = 0x0020_0000;
/// Disk sector holding the header.
pub const HEADER_LBA: u32 = 6;
/// Disk sector where the flat image starts.
pub const IMAGE_LBA: u32 = 16;

/// What the loader found and how the module answered.
pub struct Report {
    pub length: u32,
    pub status: i32,
    pub ticked: bool,
}

/// Every reason an image is not entered.
pub enum Error {
    Read(&'static str, ata::Error),
    Header,
    Length(u32),
    Digest {
        expected: [u8; 32],
        actual: [u8; 32],
    },
    Vtable,
    Status(i32),
}

impl Error {
    pub fn report(&self) {
        match self {
            Error::Read(phase, error) => {
                console::write(b"ppc: module read phase=");
                console::write(phase.as_bytes());
                console::write_byte(b'\n');
                error.report();
            }
            Error::Header => print_line(b"ppc: module header invalid"),
            Error::Length(length) => {
                console::write(b"ppc: module length out of range: ");
                print_dec(*length);
                console::write_byte(b'\n');
            }
            Error::Digest { expected, actual } => {
                console::write(b"ppc: module digest expected=");
                print_hex_bytes(expected);
                console::write(b" actual=");
                print_hex_bytes(actual);
                console::write_byte(b'\n');
                print_line(b"MODULE-DIGEST-BAD");
            }
            Error::Vtable => print_line(b"ppc: module vtable out of range"),
            Error::Status(status) => {
                console::write(b"ppc: module status=");
                print_dec(*status as u32);
                console::write_byte(b'\n');
            }
        }
    }
}

/// The slot and its cache.
struct PpcHost;

impl Host for PpcHost {
    fn clear(&mut self, _image: &Image) {
        zero_slot();
    }

    fn copy(&mut self, image: &Image, offset: usize, bytes: &[u8]) {
        // SAFETY: `offset + bytes.len()` is inside the reserved slot, and the
        // caller streams within the length it checked.
        unsafe {
            core::ptr::copy_nonoverlapping(
                bytes.as_ptr(),
                (image.base as *mut u8).add(offset),
                bytes.len(),
            );
        }
    }

    fn flush_code(&mut self, _image: &Image, length: usize) {
        cache::flush_code(SLOT_BASE, length);
    }
}

/// Read, verify and enter the module on `channel`.
///
/// The slot is zeroed before the copy so `.bss` starts clear.  A digest
/// mismatch zeroes it again and returns without entering the image.
pub fn load(channel: Channel) -> Result<Report, Error> {
    let mut header = [0u8; SECTOR_SIZE];
    channel
        .read_sector(HEADER_LBA, &mut header)
        .map_err(|error| Error::Read("header", error))?;
    let header = match image::parse_header(&header, SLOT_SIZE) {
        Ok(header) => header,
        Err(image::HeaderError::Length { length, .. }) => return Err(Error::Length(length)),
        Err(_) => return Err(Error::Header),
    };

    zero_slot();

    let length = header.length;
    let sectors = (length as usize).div_ceil(SECTOR_SIZE);
    let mut hasher = Hasher::new();
    let mut sector = [0u8; SECTOR_SIZE];
    for index in 0..sectors {
        channel
            .read_sector(IMAGE_LBA + index as u32, &mut sector)
            .map_err(|error| Error::Read("image", error))?;
        let offset = index * SECTOR_SIZE;
        let take = (length as usize - offset).min(SECTOR_SIZE);
        // SAFETY: `offset + take <= length <= SLOT_SIZE`, and the slot is
        // reserved for this image.
        unsafe {
            core::ptr::copy_nonoverlapping(
                sector.as_ptr(),
                (SLOT_BASE as *mut u8).add(offset),
                take,
            );
        }
        hasher.update(&sector[..take]);
    }
    let actual = hasher.finalize().0;
    if image::verify_digest(&header.digest, &actual).is_err() {
        zero_slot();
        return Err(Error::Digest {
            expected: header.digest,
            actual,
        });
    }

    console::write(b"ppc: module length=");
    print_dec(length);
    console::write(b" digest=");
    print_hex_bytes(&actual);
    console::write_byte(b'\n');

    let window = Image::new(SLOT_BASE as usize, SLOT_SIZE, "ppc");
    // SAFETY: the image was copied into the reserved slot and its digest was
    // checked.  The entry reads the shared service table only during the call.
    let report = match unsafe { image::enter(&window, length as usize, &mut PpcHost) } {
        Ok(report) => report,
        Err(image::Error::Vtable) => return Err(Error::Vtable),
        Err(image::Error::Status(status)) => return Err(Error::Status(status)),
        Err(image::Error::Length { length, .. }) => return Err(Error::Length(length as u32)),
        Err(image::Error::Digest { .. }) => unreachable!("enter does not verify a digest"),
    };

    // SAFETY: the module registered this callback in its own vtable and
    // receives the same table on every call.
    let mut ticked = false;
    if let Some(vtable) = report.vtable() {
        if let Some(tick) = unsafe { (*vtable).tick } {
            let _ = unsafe { tick(&kernel_core::service::SERVICES) };
            ticked = true;
        }
    }
    Ok(Report {
        length,
        status: report.status,
        ticked,
    })
}

/// Clear the whole slot before a copy, so `.bss` and any stale image go away.
fn zero_slot() {
    // SAFETY: the slot is reserved for module images and is not aliased.
    unsafe {
        core::ptr::write_bytes(SLOT_BASE as *mut u8, 0, SLOT_SIZE);
    }
}
