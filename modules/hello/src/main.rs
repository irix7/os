// SPDX-License-Identifier: GPL-3.0-only
//! Demo loadable module: prints a line and returns a code.
//!
//! The kernel downloads the flat image into `0x8840_0000`, verifies its
//! SHA-256, and calls `_start` with the service table.

#![no_std]
#![no_main]

use core::panic::PanicInfo;

/// Must match `kernel::module::Services` (version 1).
#[repr(C)]
pub struct Services {
    pub version: u32,
    pub write: extern "C" fn(*const u8, usize),
    pub clock_ms: extern "C" fn() -> u64,
}

#[no_mangle]
pub extern "C" fn _start(services: *const Services) -> i32 {
    let Some(services) = (unsafe { services.as_ref() }) else {
        return 1;
    };
    if services.version != 1 {
        return 2;
    }

    let message = b"hello from a downloaded module\n";
    (services.write)(message.as_ptr(), message.len());

    let _uptime = (services.clock_ms)();
    0
}

#[cfg(not(test))]
#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    loop {
        core::hint::spin_loop();
    }
}
