// SPDX-License-Identifier: GPL-3.0-only
//! Long-running module demo: prints a line once a second for five seconds,
//! then unregisters its tick callback.
//!
//! This exercises the version 2 ABI: the entry returns a vtable with a `tick`
//! callback, and the kernel calls it from the idle loop.  The module's `.bss`
//! is zeroed by the loader before the download.

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

static VTABLE: ModuleVtable = ModuleVtable {
    status: 0,
    tick: Some(tick),
};

static mut LAST: u64 = 0;
static mut COUNT: u32 = 0;

/// Module entry.
///
/// # Safety
/// `services` must point to a valid service table supplied by the kernel.
#[no_mangle]
pub unsafe extern "C" fn _start(services: *const Services) -> *const ModuleVtable {
    // SAFETY: the kernel passes a valid table; store it for the callback.
    if let Some(services) = unsafe { services.as_ref() } {
        unsafe {
            LAST = (services.clock_ms)();
            // Exercise the version 2 allocator service.
            let p = (services.alloc)(32, 8);
            if !p.is_null() {
                core::ptr::write_bytes(p, b'A', 32);
                let msg = b"ticker: alloc ok\n";
                (services.write)(msg.as_ptr(), msg.len());
            }
        }
    }
    &VTABLE
}

unsafe extern "C" fn tick(services: *const Services) -> i32 {
    // SAFETY: the kernel passes the same valid table on every call.
    let Some(services) = (unsafe { services.as_ref() }) else {
        return 1;
    };
    let now = (services.clock_ms)();
    let last = unsafe { LAST };
    if now.saturating_sub(last) < 1000 {
        return 0;
    }
    unsafe {
        LAST = now;
        COUNT += 1;
    }
    let count = unsafe { COUNT };
    let message = b"ticker: tick\n";
    (services.write)(message.as_ptr(), message.len());

    if count >= 5 {
        1
    } else {
        0
    }
}

#[cfg(not(test))]
#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    loop {
        core::hint::spin_loop();
    }
}
