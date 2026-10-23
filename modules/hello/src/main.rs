// SPDX-License-Identifier: GPL-3.0-only
//! Demo loadable module: prints a line and returns a code.
//!
//! The kernel downloads the flat image into `0x8840_0000`, verifies its
//! SHA-256, and calls `_start` with the service table.

#![no_std]
#![no_main]

/// Prefix of `kernel::module::Services` (version 3).
#[repr(C)]
pub struct Services {
    pub version: u32,
    pub write: extern "C" fn(*const u8, usize),
    pub clock_ms: extern "C" fn() -> u64,
    pub alloc: unsafe extern "C" fn(usize, usize) -> *mut u8,
    pub poll: extern "C" fn(),
}

#[repr(C)]
pub struct ModuleVtable {
    pub status: i32,
    pub tick: Option<unsafe extern "C" fn(*const Services) -> i32>,
}

static OK: ModuleVtable = ModuleVtable {
    status: 0,
    tick: None,
};
static BAD_PTR: ModuleVtable = ModuleVtable {
    status: 1,
    tick: None,
};
static BAD_VERSION: ModuleVtable = ModuleVtable {
    status: 2,
    tick: None,
};

/// Module entry.
///
/// # Safety
/// `services` must point to a valid service table supplied by the kernel.
#[no_mangle]
pub unsafe extern "C" fn _start(services: *const Services) -> *const ModuleVtable {
    let Some(services) = (unsafe { services.as_ref() }) else {
        return &BAD_PTR;
    };
    if services.version < 2 {
        return &BAD_VERSION;
    }

    let message = b"hello from a downloaded module\n";
    (services.write)(message.as_ptr(), message.len());

    let _uptime = (services.clock_ms)();
    &OK
}

#[cfg(not(test))]
#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    loop {
        core::hint::spin_loop();
    }
}
