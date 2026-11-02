// SPDX-License-Identifier: GPL-3.0-only
//! Loadable modules on the x86_64 machine: the machine's windows, the disk
//! sectors and the serve loop.
//!
//! The header format, the digest check, the vtable bounds check and the entry
//! call are `kernel-core::image`, the same code the Indy and PowerPC kernels
//! run.  This file supplies what is specific to the machine: where the windows
//! are, which sectors hold the headers and how long the boot serves.

use kernel_core::image::{self, Host, Image};
use module_abi::ModuleVtable;

use crate::net;
use crate::print_line;
use crate::serial;
use crate::timer;

/// Disk sector holding the MCP module's digest header.
pub const MCP_HEADER_SECTOR: u64 = 8;
/// Disk sector holding the wasm module's digest header.
pub const WASM_HEADER_SECTOR: u64 = 10;

/// The MCP control-plane module.
pub const MCP: Image = Image::new(0x0200_0000, 0x0020_0000, "mcp");

/// The wasm policy module, run once at boot.
pub const WASM: Image = Image::new(0x0400_0000, 0x0020_0000, "wasm");

/// Serve window for a standalone boot: 60 s at 100 Hz.
///
/// The smoke gate runs QEMU under a 150 s cap and expects the guest to power
/// off on its own.
pub const SMOKE_SERVE_TICKS: u32 = 6_000;

/// Serve window for a mesh run: 300 s at 100 Hz.
///
/// Join, homes, route, migration and the two restarts share one boot.
pub const MESH_SERVE_TICKS: u32 = 30_000;

/// The machine's window and cache.
struct X86Host;

impl Host for X86Host {
    fn clear(&mut self, image: &Image) {
        // SAFETY: the window is reserved for this module.
        unsafe { core::ptr::write_bytes(image.base as *mut u8, 0, image.window) };
    }

    fn copy(&mut self, image: &Image, offset: usize, bytes: &[u8]) {
        // SAFETY: the caller bounded `offset + bytes.len()` by the window.
        unsafe {
            core::ptr::copy_nonoverlapping(
                bytes.as_ptr(),
                (image.base as *mut u8).add(offset),
                bytes.len(),
            );
        }
    }

    fn flush_code(&mut self, _image: &Image, _length: usize) {
        // The x86 page tables map the window write-back and there is no
        // separate instruction cache to invalidate.
    }
}

/// Copy a verified image into its window and call its entry.  Returns the
/// module's vtable.  Stops the machine on any failure.
pub fn load(image: &Image, bytes: &[u8]) -> *const ModuleVtable {
    match image::install(image, bytes, &mut X86Host) {
        Ok(report) => {
            serial::write(image.name.as_bytes());
            serial::write(b": entry status=");
            crate::print_dec(report.status as u32 as u64);
            serial::write_byte(b'\n');
            let Some(vtable) = report.vtable() else {
                fail(image, "entry returned no vtable");
            };
            vtable
        }
        Err(error) => fail(image, error_text(error)),
    }
}

fn error_text(error: image::Error) -> &'static str {
    match error {
        image::Error::Length { .. } => "size out of range",
        image::Error::Digest { .. } => "digest mismatch",
        image::Error::Vtable => "vtable outside the verified image",
        image::Error::Status(_) => "init failed",
    }
}

/// Serve: poll the stack and call the module's tick for the boot's serve
/// window.  A tick that returns non-zero unregisters it, as the Indy kernel
/// does.
///
/// A peer address in the configuration means the rig is running the mesh
/// scenario, which needs the long window; otherwise the machine powers off
/// soon after the standalone markers.
pub fn serve(vtable: *const ModuleVtable) {
    serial::write(b"x86: module serving\n");
    // SAFETY: the vtable points into a loaded module image.
    let tick = unsafe { (*vtable).tick };
    // Run one tick so the module arms its listeners, then announce readiness.
    if let Some(tick) = tick {
        // SAFETY: the module registered this tick and receives the same table.
        let _ = unsafe { tick(&kernel_core::service::SERVICES) };
    }
    serial::write(b"BOOT-READY v=1 kernel=up network=up mcp=listening mesh=single\n");
    let mut peer = [0u8; 64];
    let serve_ticks = if crate::config::get(b"mesh.peer", &mut peer) > 0 {
        MESH_SERVE_TICKS
    } else {
        SMOKE_SERVE_TICKS
    };
    let start = timer::ticks();
    let mut ticking = tick.is_some();
    while timer::ticks().wrapping_sub(start) < serve_ticks {
        net::poll();
        if let (true, Some(tick)) = (ticking, tick) {
            // SAFETY: the module registered this tick and receives the same table.
            if unsafe { tick(&kernel_core::service::SERVICES) } != 0 {
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
