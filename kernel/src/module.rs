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

use alloc::alloc::{alloc, Layout};
use core::mem::{align_of, size_of};

/// Load address of the module area (kseg0, physical `0x0840_0000`).
///
/// This sits inside the reserved low window, above the kernel stack top at
/// `0x8820_0000`, and below the frame allocator's first frame at `0x08B0_0000`.
pub const MODULE_BASE: usize = 0x8840_0000;

/// Maximum module image size.
pub const MODULE_MAX: usize = 0x0020_0000; // 2 MiB

/// Service table ABI version.
pub const ABI_VERSION: u32 = 2;

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
    /// Allocate `size` bytes aligned to `align`.  The kernel heap never frees.
    pub alloc: unsafe extern "C" fn(usize, usize) -> *mut u8,
    /// Drive the shared network stack once.
    pub poll: extern "C" fn(),
}

/// What a module returns from its entry point.
///
/// A null pointer means "success, no callback".  Otherwise the pointer must
/// point inside the module image and stay valid for as long as the module is
/// loaded.
#[repr(C)]
pub struct ModuleVtable {
    /// Result of the module's one-shot initialisation; 0 is success.
    pub status: i32,
    /// Called from the kernel's idle loop at most every 10 ms.  Return 0 to
    /// keep running, non-zero to unregister.
    pub tick: Option<unsafe extern "C" fn(*const Services) -> i32>,
}

/// A module entry point.
pub type Entry = unsafe extern "C" fn(*const Services) -> *const ModuleVtable;

static SERVICES: Services = Services {
    version: ABI_VERSION,
    write: service_write,
    clock_ms: service_clock_ms,
    alloc: service_alloc,
    poll: service_poll,
};

/// The periodic callback of the loaded module, if it registered one.
static mut TICK: Option<unsafe extern "C" fn(*const Services) -> i32> = None;
static mut LAST_TICK: u64 = 0;

/// The module area as a writable slice, for downloading straight into place.
pub fn area() -> &'static mut [u8] {
    // SAFETY: the region is reserved for exactly this purpose; the kernel is
    // single-threaded and only one module runs at a time.
    unsafe { core::slice::from_raw_parts_mut(MODULE_BASE as *mut u8, MODULE_MAX) }
}

/// Zero the module area before a download.
///
/// The flat image contains `.text`, `.rodata` and `.data` but not `.bss`, so
/// zeroing the area gives the module's `.bss` a defined initial value and
/// discards the previous module's code.
pub fn clear() {
    // SAFETY: the region is reserved for modules and is not aliased.
    unsafe {
        core::ptr::write_bytes(MODULE_BASE as *mut u8, 0, MODULE_MAX);
    }
    crate::cache::dcache_writeback_invalidate(MODULE_BASE, MODULE_MAX);
    crate::cache::sync();
}

fn vtable_in_area(p: *const ModuleVtable) -> bool {
    let a = p as usize;
    if a < MODULE_BASE || a % align_of::<ModuleVtable>() != 0 {
        return false;
    }
    match a.checked_add(size_of::<ModuleVtable>()) {
        Some(end) => end <= MODULE_BASE + MODULE_MAX,
        None => false,
    }
}

/// Call the module currently loaded at [`MODULE_BASE`].
///
/// # Safety
/// The caller must have verified the image and its hash; arbitrary code runs
/// in kernel mode from here.
pub unsafe fn call_entry(len: usize) -> i32 {
    // The image was written through the cacheable kseg0 mapping.  Write the
    // data cache back to RAM and invalidate stale instruction lines before the
    // first fetch of module code.
    crate::cache::dcache_writeback_invalidate(MODULE_BASE, len);
    crate::cache::icache_invalidate(MODULE_BASE, len);
    crate::cache::sync();

    // A new image replaces any previous callback.
    TICK = None;

    let entry: Entry = core::mem::transmute(MODULE_BASE);
    let vtable = entry(&SERVICES);
    if vtable.is_null() {
        return 0;
    }
    if !vtable_in_area(vtable) {
        return -1;
    }
    // The vtable lives inside the verified image; read it directly.
    let status = core::ptr::read(vtable).status;
    if status != 0 {
        return status;
    }
    let tick = core::ptr::read(vtable).tick;
    if let Some(tick) = tick {
        LAST_TICK = service_clock_ms();
        TICK = Some(tick);
    }
    0
}

/// Run the loaded module's periodic callback, if any.
pub fn poll() {
    // SAFETY: single-threaded kernel; TICK is only written here and in
    // call_entry, which is not re-entered.
    let tick = unsafe { TICK };
    let Some(tick) = tick else {
        return;
    };
    let now = service_clock_ms();
    let last = unsafe { LAST_TICK };
    if now.saturating_sub(last) < 10 {
        return;
    }
    unsafe {
        LAST_TICK = now;
    }
    let status = unsafe { tick(&SERVICES) };
    if status != 0 {
        unsafe {
            TICK = None;
        }
    }
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

unsafe extern "C" fn service_alloc(size: usize, align: usize) -> *mut u8 {
    if size == 0 {
        return core::ptr::null_mut();
    }
    let align = align.max(1).next_power_of_two();
    match Layout::from_size_align(size, align) {
        Ok(layout) => alloc(layout),
        Err(_) => core::ptr::null_mut(),
    }
}

extern "C" fn service_poll() {
    crate::net::poll();
}
