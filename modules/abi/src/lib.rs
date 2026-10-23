// SPDX-License-Identifier: GPL-3.0-only
//! The loadable-module ABI shared by the kernel and every module image.
//!
//! The table is append-only: new services go at the end and [`ABI_VERSION`]
//! is bumped.  A module links this crate, so the kernel and the module cannot
//! disagree about a field offset.  The compile-time assertions below cover the
//! n32 layout (32-bit pointers); a host build is 64-bit and skips them.

#![no_std]

/// Load address of the module area (kseg0, physical `0x0840_0000`).
pub const MODULE_BASE: usize = 0x8840_0000;

/// Maximum module image size.
pub const MODULE_MAX: usize = 0x0020_0000; // 2 MiB

/// Service-table version understood by this ABI.
pub const ABI_VERSION: u32 = 9;

/// Service table passed to a module entry point.
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
    /// Listen on `port` in slot `slot` (0..2).  Returns 0 or -1.
    pub tcp_listen: unsafe extern "C" fn(u32, u16) -> i32,
    /// 0 closed, 1 listening, 2 established, 3 closing.
    pub tcp_state: extern "C" fn(u32) -> u32,
    /// Receive into `ptr`; returns bytes, 0 for none, -1 for no socket.
    pub tcp_recv: unsafe extern "C" fn(u32, *mut u8, usize) -> isize,
    /// Send from `ptr`; returns bytes accepted, -1 for no socket.
    pub tcp_send: unsafe extern "C" fn(u32, *const u8, usize) -> isize,
    /// Close the connection (send FIN).
    pub tcp_close: extern "C" fn(u32),
    /// Abort the connection (send RST).
    pub tcp_abort: extern "C" fn(u32),
    /// Copy serial output from absolute `offset`; returns bytes copied.
    pub console_read: unsafe extern "C" fn(u64, *mut u8, usize) -> isize,
    /// Total bytes ever written to the serial console.
    pub console_total: extern "C" fn() -> u64,
    /// Restart from the PROM entry point.  Does not return.
    pub reset: extern "C" fn(),
    /// Halt the machine.  Does not return.
    pub poweroff: extern "C" fn(),
    /// Format the machine status block; returns bytes written.
    pub status: unsafe extern "C" fn(*mut u8, usize) -> isize,
    /// Read a PROM environment value by name; returns bytes or -1.
    pub config_get: unsafe extern "C" fn(*const u8, usize, *mut u8, usize) -> isize,
    /// Format an information block: 0 status, 1 net, 2 mem, 3 time.
    pub info: unsafe extern "C" fn(u32, *mut u8, usize) -> isize,
    /// Copy BGR shadow pixels from `offset`; returns bytes copied.
    pub screen_read: unsafe extern "C" fn(u32, *mut u8, usize) -> isize,
}

/// What a module returns from its entry point.
///
/// A null pointer means "success, no callback".  Otherwise the pointer must
/// point inside the module image and stay valid while the module is loaded.
#[repr(C)]
pub struct ModuleVtable {
    /// Result of the module's one-shot initialisation; 0 is success.
    pub status: i32,
    /// Called from the kernel's idle loop at most every 10 ms.  Return 0 to
    /// keep running, non-zero to unregister.
    pub tick: Option<unsafe extern "C" fn(*const Services) -> i32>,
}

#[cfg(target_pointer_width = "32")]
const _: () = {
    use core::mem::{offset_of, size_of};
    assert!(size_of::<Services>() == 0x4c);
    assert!(offset_of!(Services, version) == 0x00);
    assert!(offset_of!(Services, write) == 0x04);
    assert!(offset_of!(Services, clock_ms) == 0x08);
    assert!(offset_of!(Services, alloc) == 0x0c);
    assert!(offset_of!(Services, poll) == 0x10);
    assert!(offset_of!(Services, tcp_listen) == 0x14);
    assert!(offset_of!(Services, tcp_state) == 0x18);
    assert!(offset_of!(Services, tcp_recv) == 0x1c);
    assert!(offset_of!(Services, tcp_send) == 0x20);
    assert!(offset_of!(Services, tcp_close) == 0x24);
    assert!(offset_of!(Services, tcp_abort) == 0x28);
    assert!(offset_of!(Services, console_read) == 0x2c);
    assert!(offset_of!(Services, console_total) == 0x30);
    assert!(offset_of!(Services, reset) == 0x34);
    assert!(offset_of!(Services, poweroff) == 0x38);
    assert!(offset_of!(Services, status) == 0x3c);
    assert!(offset_of!(Services, config_get) == 0x40);
    assert!(offset_of!(Services, info) == 0x44);
    assert!(offset_of!(Services, screen_read) == 0x48);
    assert!(size_of::<ModuleVtable>() == 8);
    assert!(offset_of!(ModuleVtable, status) == 0);
    assert!(offset_of!(ModuleVtable, tick) == 4);
};
