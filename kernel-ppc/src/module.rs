// SPDX-License-Identifier: GPL-3.0-only
//! Loadable modules on the PowerPC G3 machine.
//!
//! A module is a flat image linked at [`SLOT_BASE`] and copied there from the
//! boot disk.  The disk layout matches the x86 machine's intent: one header
//! sector holds the magic `IRIX7MOD`, the image length as a big-endian u32 and
//! the image's SHA-256; the image itself starts at a fixed LBA.  The digest in
//! the header is what this machine trusts.  A real boot takes it from the
//! signed boot configuration, which the PowerPC machine does not have yet.
//!
//! The slot is mapped cacheable by IBAT2/DBAT2, so the loader writes every
//! block back and invalidates the instruction cache before the first fetch of
//! module code.

use core::mem::{align_of, size_of};

use irix7_sha256::{Hasher, Sha256};
use module_abi::{ModuleVtable, Services};

use crate::ata::{self, Channel, SECTOR_SIZE};
use crate::services::SERVICES;
use crate::{cache, print_dec, print_hex_bytes, print_line, serial};

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

const MAGIC: &[u8; 8] = b"IRIX7MOD";
const LENGTH_OFFSET: usize = 8;
const DIGEST_OFFSET: usize = 12;
const HEADER_LEN: usize = DIGEST_OFFSET + 32;

/// A module entry point.
type Entry = unsafe extern "C" fn(*const Services) -> *const ModuleVtable;

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
                serial::write(b"ppc: module read phase=");
                serial::write(phase.as_bytes());
                serial::write_byte(b'\n');
                error.report();
            }
            Error::Header => print_line(b"ppc: module header invalid"),
            Error::Length(length) => {
                serial::write(b"ppc: module length out of range: ");
                print_dec(*length);
                serial::write_byte(b'\n');
            }
            Error::Digest { expected, actual } => {
                serial::write(b"ppc: module digest expected=");
                print_hex_bytes(expected);
                serial::write(b" actual=");
                print_hex_bytes(actual);
                serial::write_byte(b'\n');
                print_line(b"MODULE-DIGEST-BAD");
            }
            Error::Vtable => print_line(b"ppc: module vtable out of range"),
            Error::Status(status) => {
                serial::write(b"ppc: module status=");
                print_dec(*status as u32);
                serial::write_byte(b'\n');
            }
        }
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
    if &header[..MAGIC.len()] != MAGIC {
        return Err(Error::Header);
    }
    let length = u32::from_be_bytes([
        header[LENGTH_OFFSET],
        header[LENGTH_OFFSET + 1],
        header[LENGTH_OFFSET + 2],
        header[LENGTH_OFFSET + 3],
    ]);
    if length == 0 || length as usize > SLOT_SIZE {
        return Err(Error::Length(length));
    }
    let mut expected = [0u8; 32];
    expected.copy_from_slice(&header[DIGEST_OFFSET..HEADER_LEN]);

    zero_slot();

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
    if !Sha256(actual).eq_digest(&Sha256(expected)) {
        zero_slot();
        return Err(Error::Digest { expected, actual });
    }

    cache::flush_code(SLOT_BASE, length as usize);

    serial::write(b"ppc: module length=");
    print_dec(length);
    serial::write(b" digest=");
    print_hex_bytes(&actual);
    serial::write_byte(b'\n');

    // SAFETY: the image was copied into the reserved slot and its digest and
    // length were checked.  The entry reads the service table only during this
    // call.
    let entry: Entry = unsafe { core::mem::transmute(SLOT_BASE) };
    let vtable = unsafe { entry(&SERVICES) };
    if vtable.is_null() {
        return Ok(Report {
            length,
            status: 0,
            ticked: false,
        });
    }
    let base = SLOT_BASE as usize;
    let image_end = base + length as usize;
    let address = vtable as usize;
    if address < base
        || !address.is_multiple_of(align_of::<ModuleVtable>())
        || address
            .checked_add(size_of::<ModuleVtable>())
            .is_none_or(|end| end > image_end)
    {
        return Err(Error::Vtable);
    }
    // SAFETY: the vtable pointer was checked against the verified image.
    let status = unsafe { (*vtable).status };
    if status != 0 {
        return Err(Error::Status(status));
    }
    // SAFETY: the module registered this callback in its own vtable and
    // receives the same table on every call.
    let mut ticked = false;
    if let Some(tick) = unsafe { (*vtable).tick } {
        let _ = unsafe { tick(&SERVICES) };
        ticked = true;
    }
    Ok(Report {
        length,
        status,
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
