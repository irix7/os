// SPDX-License-Identifier: GPL-3.0-only
//! Loadable modules on the x86_64 machine: a flat image in a reserved window,
//! checked against a SHA-256 digest and called through the module ABI.
//!
//! The image comes from the boot disk.  Sector 8 holds a header: the magic
//! `IRIX7MOD`, the image length as a little-endian u64, and the image's SHA-256.
//! The image starts at sector 16.  The digest in the header is what this slice
//! trusts; a real boot takes it from the signed boot configuration, which the
//! x86 machine does not have yet.

use module_abi::ModuleVtable;
use sha2::{Digest, Sha256};

use crate::net;
use crate::print_line;
use crate::serial;
use crate::services::SERVICES;
use crate::virtio_blk::{Disk, SECTOR};

/// The module window: the link address of `modules/mcp/linker-x86.ld` and the
/// reservation `memory.rs` makes.
pub const BASE: usize = 0x0200_0000;
pub const WINDOW: usize = 0x0020_0000;

/// How long the machine serves before its boot ends: 60 s at 100 Hz.
const SERVE_TICKS: u32 = 6000;

const HEADER_SECTOR: u64 = 8;
const IMAGE_SECTOR: u64 = 16;
const MAGIC: &[u8; 8] = b"IRIX7MOD";

type Entry = unsafe extern "C" fn(*const module_abi::Services) -> *const ModuleVtable;

/// Load and call the module on the boot disk.  Stops the machine on any failure.
pub fn run(disk: &mut Disk) {
    let mut sector = [0u8; SECTOR];
    if !disk.read(HEADER_SECTOR, &mut sector) || &sector[..8] != MAGIC {
        fail("x86: module header missing");
    }
    let size = u64::from_le_bytes(sector[8..16].try_into().unwrap_or([0; 8])) as usize;
    let mut digest = [0u8; 32];
    digest.copy_from_slice(&sector[16..48]);
    if size == 0 || size > WINDOW {
        fail("x86: module size out of range");
    }

    // The loader clears the window so .bss starts zeroed.
    // SAFETY: the window is reserved, so nothing else is using it.
    unsafe { core::ptr::write_bytes(BASE as *mut u8, 0, WINDOW) };
    let sectors = size.div_ceil(SECTOR) as u64;
    for index in 0..sectors {
        if !disk.read(IMAGE_SECTOR + index, &mut sector) {
            fail("x86: module read failed");
        }
        let offset = index as usize * SECTOR;
        let count = (size - offset).min(SECTOR);
        // SAFETY: `offset + count <= size <= WINDOW`, inside the window.
        unsafe {
            core::ptr::copy_nonoverlapping(sector.as_ptr(), (BASE + offset) as *mut u8, count);
        }
    }

    // SAFETY: the window holds `size` bytes just written.
    let image = unsafe { core::slice::from_raw_parts(BASE as *const u8, size) };
    let actual = Sha256::digest(image);
    if actual.as_slice() != digest {
        fail("x86: module digest mismatch");
    }
    serial::write(b"x86: module sha256 ok size=");
    crate::print_dec(size as u64);
    serial::write_byte(b'\n');

    // SAFETY: the digest matched the image the machine was built from, and the
    // image begins with its legacy entry, `_start`, at `BASE`.
    let entry: Entry = unsafe { core::mem::transmute::<usize, Entry>(BASE) };
    // SAFETY: the entry reads the service table only during this call.
    let vtable = unsafe { entry(&SERVICES) };
    if vtable.is_null() {
        fail("x86: module entry returned no vtable");
    }
    // SAFETY: the module returned a pointer into its own image.
    let status = unsafe { (*vtable).status };
    serial::write(b"x86: module entry status=");
    crate::print_dec(status as u32 as u64);
    serial::write_byte(b'\n');
    if status != 0 {
        fail("x86: module init failed");
    }

    // Serve: poll the stack and call the module's tick until the window ends.
    // The module's HTTP server runs from its tick and its listener from the
    // stack's poll.  A tick that returns non-zero unregisters it, as the Indy
    // kernel does.
    serial::write(b"x86: module serving\n");
    let tick = unsafe { (*vtable).tick };
    let start = crate::timer::ticks();
    let mut ticking = tick.is_some();
    while crate::timer::ticks().wrapping_sub(start) < SERVE_TICKS {
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

fn fail(message: &str) -> ! {
    print_line(&[message.as_bytes()]);
    crate::qemu_exit(1)
}
