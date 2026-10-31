// SPDX-License-Identifier: GPL-3.0-only
//! Loadable modules on the x86_64 machine: flat images in reserved windows,
//! checked against a SHA-256 digest and called through the module ABI.
//!
//! Each image has a header sector: the magic `IRIX7MOD`, the image length as a
//! little-endian u64, and the image's SHA-256.  The image itself starts at its
//! own sector.  The digest in the header is what this machine trusts; a real
//! boot takes it from the signed boot configuration, which the x86 machine does
//! not have yet.

use module_abi::ModuleVtable;
use sha2::{Digest, Sha256};

use crate::net;
use crate::print_line;
use crate::serial;
use crate::services::SERVICES;
use crate::timer;
use crate::virtio_blk::{Disk, SECTOR};

/// A module's place on the machine and on the disk.
pub struct Image {
    /// Link address; also the start of the reserved window.
    pub base: usize,
    /// Window size in bytes.
    pub window: usize,
    /// Disk sector holding the header.
    pub header_sector: u64,
    /// Disk sector holding the first image byte.
    pub image_sector: u64,
    /// The name used in the boot log.
    pub name: &'static str,
}

/// The MCP control-plane module.
pub const MCP: Image = Image {
    base: 0x0200_0000,
    window: 0x0020_0000,
    header_sector: 8,
    image_sector: 16,
    name: "mcp",
};

/// The wasm policy module, run once at boot.
pub const WASM: Image = Image {
    base: 0x0400_0000,
    window: 0x0020_0000,
    header_sector: 10,
    image_sector: 2048,
    name: "wasm",
};

/// How long the MCP module serves before the boot ends: 60 s at 100 Hz.
pub const SERVE_TICKS: u32 = 6000;

type Entry = unsafe extern "C" fn(*const module_abi::Services) -> *const ModuleVtable;

/// Load `image` from the disk, check its digest, and call its entry.  Returns
/// the module's vtable.  Stops the machine on any failure.
pub fn load(disk: &mut Disk, image: &Image) -> *const ModuleVtable {
    let mut sector = [0u8; SECTOR];
    if !disk.read(image.header_sector, &mut sector) || &sector[..8] != b"IRIX7MOD" {
        fail(image, "header missing");
    }
    let size = u64::from_le_bytes(sector[8..16].try_into().unwrap_or([0; 8])) as usize;
    let mut digest = [0u8; 32];
    digest.copy_from_slice(&sector[16..48]);
    if size == 0 || size > image.window {
        fail(image, "size out of range");
    }

    // The loader clears the window so .bss starts zeroed.
    // SAFETY: the window is reserved, so nothing else is using it.
    unsafe { core::ptr::write_bytes(image.base as *mut u8, 0, image.window) };
    let sectors = size.div_ceil(SECTOR) as u64;
    for index in 0..sectors {
        if !disk.read(image.image_sector + index, &mut sector) {
            fail(image, "read failed");
        }
        let offset = index as usize * SECTOR;
        let count = (size - offset).min(SECTOR);
        // SAFETY: `offset + count <= size <= window`, inside the window.
        unsafe {
            core::ptr::copy_nonoverlapping(
                sector.as_ptr(),
                (image.base + offset) as *mut u8,
                count,
            );
        }
    }

    // SAFETY: the window holds `size` bytes just written.
    let bytes = unsafe { core::slice::from_raw_parts(image.base as *const u8, size) };
    if Sha256::digest(bytes).as_slice() != digest {
        fail(image, "digest mismatch");
    }
    serial::write(image.name.as_bytes());
    serial::write(b": sha256 ok size=");
    crate::print_dec(size as u64);
    serial::write_byte(b'\n');

    // SAFETY: the digest matched the image the machine was built from, and the
    // image begins with its legacy entry, `_start`, at `base`.
    let entry: Entry = unsafe { core::mem::transmute::<usize, Entry>(image.base) };
    // SAFETY: the entry reads the service table only during this call.
    let vtable = unsafe { entry(&SERVICES) };
    if vtable.is_null() {
        fail(image, "entry returned no vtable");
    }
    // SAFETY: the module returned a pointer into its own image.
    let status = unsafe { (*vtable).status };
    serial::write(image.name.as_bytes());
    serial::write(b": entry status=");
    crate::print_dec(status as u32 as u64);
    serial::write_byte(b'\n');
    if status != 0 {
        fail(image, "init failed");
    }
    vtable
}

/// Serve: poll the stack and call the module's tick for `SERVE_TICKS`.  A tick
/// that returns non-zero unregisters it, as the Indy kernel does.
pub fn serve(vtable: *const ModuleVtable) {
    serial::write(b"x86: module serving\n");
    // SAFETY: the vtable points into a loaded module image.
    let tick = unsafe { (*vtable).tick };
    let start = timer::ticks();
    let mut ticking = tick.is_some();
    while timer::ticks().wrapping_sub(start) < SERVE_TICKS {
        net::poll();
        if let (true, Some(tick)) = (ticking, tick) {
            // SAFETY: the module registered this tick and receives the same table.
            if unsafe { tick(&SERVICES) } != 0 {
                ticking = false;
            }
        }
        core::hint::spin_loop();
    }
}

fn fail(image: &Image, message: &str) -> ! {
    serial::write(b"x86: ");
    serial::write(image.name.as_bytes());
    serial::write(b": ");
    print_line(&[message.as_bytes()]);
    crate::qemu_exit(1)
}
