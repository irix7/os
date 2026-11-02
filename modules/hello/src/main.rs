// SPDX-License-Identifier: GPL-3.0-only
//! Demo loadable module: prints a line, reads the machine information, and
//! returns a one-shot tick callback.
//!
//! The kernel copies the flat image into its module slot, verifies its SHA-256
//! and calls `_start` with the service table.  The source is shared by the
//! Indy and PowerPC machines; only the slot linker script differs.

#![no_std]
#![no_main]

use module_abi::{ModuleVtable, Services};

static OK: ModuleVtable = ModuleVtable {
    status: 0,
    tick: Some(tick),
};
static BAD_PTR: ModuleVtable = ModuleVtable {
    status: 1,
    tick: None,
};
static BAD_VERSION: ModuleVtable = ModuleVtable {
    status: 2,
    tick: None,
};

#[cfg(target_arch = "powerpc")]
const MARKER: &[u8] = b"PPC-MODULE-RUN\n";
#[cfg(not(target_arch = "powerpc"))]
const MARKER: &[u8] = b"hello from a downloaded module\n";

/// Module entry.
///
/// # Safety
/// `services` must point to a valid service table supplied by the kernel.
#[no_mangle]
pub unsafe extern "C" fn _start(services: *const Services) -> *const ModuleVtable {
    let Some(services) = (unsafe { services.as_ref() }) else {
        return &BAD_PTR;
    };
    // `info` is an ABI 11 field; an older table ends before offset 0x44.
    if services.version < 11 {
        return &BAD_VERSION;
    }

    (services.write)(MARKER.as_ptr(), MARKER.len());

    // Exercise the information service.  A short buffer proves the kernel
    // bounds the copy to the caller's length.
    let mut info = [0u8; 64];
    let count = (services.info)(0, info.as_mut_ptr(), info.len());
    if count > 0 {
        let count = count as usize;
        let prefix = b"module info: ";
        (services.write)(prefix.as_ptr(), prefix.len());
        (services.write)(info.as_ptr(), count);
        if info[count - 1] != b'\n' {
            (services.write)(b"\n".as_ptr(), 1);
        }
    }

    let _uptime = (services.clock_ms)();
    &OK
}

/// The one-shot tick.  Returning non-zero unregisters the callback.
unsafe extern "C" fn tick(services: *const Services) -> i32 {
    // SAFETY: the kernel passes the same valid table on every call.
    if let Some(services) = unsafe { services.as_ref() } {
        let message = b"module tick\n";
        (services.write)(message.as_ptr(), message.len());
    }
    1
}

#[cfg(not(test))]
#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    loop {
        core::hint::spin_loop();
    }
}
