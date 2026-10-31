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

use crate::net;
use crate::print_line;
use crate::serial;
use crate::services::SERVICES;
use crate::timer;

/// A module's place on the machine and on the disk.
pub struct Image {
    /// Link address; also the start of the reserved window.
    pub base: usize,
    /// Window size in bytes.
    pub window: usize,
    /// The name used in the boot log.
    pub name: &'static str,
}

/// Disk sector holding the MCP module's digest header.
pub const MCP_HEADER_SECTOR: u64 = 8;
/// Disk sector holding the wasm module's digest header.
pub const WASM_HEADER_SECTOR: u64 = 10;

/// The MCP control-plane module.
pub const MCP: Image = Image {
    base: 0x0200_0000,
    window: 0x0020_0000,
    name: "mcp",
};

/// The wasm policy module, run once at boot.
pub const WASM: Image = Image {
    base: 0x0400_0000,
    window: 0x0020_0000,
    name: "wasm",
};

/// How long the MCP module serves before the boot ends: 60 s at 100 Hz.
pub const SERVE_TICKS: u32 = 6000;

type Entry = unsafe extern "C" fn(*const module_abi::Services) -> *const ModuleVtable;

/// Copy a verified image into its window and call its entry.  Returns the
/// module's vtable.  The caller has checked the digest.  Stops the machine on
/// any failure.
pub fn load(image: &Image, bytes: &[u8]) -> *const ModuleVtable {
    if bytes.is_empty() || bytes.len() > image.window {
        fail(image, "size out of range");
    }
    // The loader clears the window so .bss starts zeroed.
    // SAFETY: the window is reserved, so nothing else is using it.
    unsafe { core::ptr::write_bytes(image.base as *mut u8, 0, image.window) };
    // SAFETY: `bytes.len() <= window`, so the copy stays inside the window.
    unsafe {
        core::ptr::copy_nonoverlapping(bytes.as_ptr(), image.base as *mut u8, bytes.len());
    }

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
