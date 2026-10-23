// SPDX-License-Identifier: GPL-3.0-only
//! Loadable modules: flat images fetched into reserved RAM and called through
//! a small C service table.
//!
//! The compressed PROM image is meant to stop growing.  Code that belongs to
//! the running system but is not needed to boot (the TLS client is the first
//! candidate) can be linked at [`MODULE_BASE`], downloaded over the network,
//! verified, and called in place.  Modules run in kernel mode and use the
//! service table rather than linking against kernel symbols, so the core and
//! a module can be rebuilt independently.

use core::sync::atomic::{compiler_fence, Ordering};

/// Load address of the module area (kseg0, physical `0x0840_0000`).
///
/// This sits inside the reserved low window, above the kernel stack top at
/// `0x8820_0000`, and below the frame allocator's first frame at `0x08B0_0000`.
pub const MODULE_BASE: usize = 0x8840_0000;

/// Maximum module image size.
pub const MODULE_MAX: usize = 0x0020_0000; // 2 MiB

/// Service table passed to a module entry point.
///
/// The layout is versioned; append new function pointers and bump `version`.
#[repr(C)]
pub struct Services {
    pub version: u32,
    /// Write bytes to the serial and telnet consoles.
    pub write: extern "C" fn(*const u8, usize),
    /// Milliseconds since boot (monotonic).
    pub clock_ms: extern "C" fn() -> u64,
}

/// A module entry point.
pub type Entry = extern "C" fn(*const Services) -> i32;

static SERVICES: Services = Services {
    version: 1,
    write: service_write,
    clock_ms: service_clock_ms,
};

/// The module area as a writable slice, for downloading straight into place.
pub fn area() -> &'static mut [u8] {
    // SAFETY: the region is reserved for exactly this purpose; the kernel is
    // single-threaded and only one module runs at a time.
    unsafe { core::slice::from_raw_parts_mut(MODULE_BASE as *mut u8, MODULE_MAX) }
}

/// Call the module currently loaded at [`MODULE_BASE`].
///
/// # Safety
/// The caller must have verified the image and its hash; arbitrary code runs
/// in kernel mode from here.
pub unsafe fn call_entry() -> i32 {
    compiler_fence(Ordering::SeqCst);
    let entry: Entry = core::mem::transmute(MODULE_BASE);
    entry(&SERVICES)
}

extern "C" fn service_write(ptr: *const u8, len: usize) {
    if ptr.is_null() || len == 0 || len > 8192 {
        return;
    }
    let data = unsafe { core::slice::from_raw_parts(ptr, len) };
    for &byte in data {
        crate::write_byte(byte);
    }
    crate::telnet::write_output(data);
}

extern "C" fn service_clock_ms() -> u64 {
    let ms = crate::net::clock_now().total_millis();
    if ms < 0 {
        0
    } else {
        ms as u64
    }
}
