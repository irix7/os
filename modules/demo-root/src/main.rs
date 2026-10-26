// SPDX-License-Identifier: GPL-3.0-only
//! Runtime graph root for the downloadable manager.
//!
//! The manager loads `demo-dep` first, registers its `irix7.demo.service@1`
//! export and then enters this image. This module resolves the dependency by
//! export, checks the owner and table, and calls it. The printed value proves
//! dependency order and the verified export binding.
//!
//! The ABI v12 `module_find` service is not used: the current kernel loader
//! fails to store `LoadedModule::version_len`, so an exact name/version lookup
//! cannot match. `module_find_export` carries the owner handle and is not
//! affected by that kernel defect.

#![no_std]
#![no_main]

use module_abi::{ModuleContext, ModuleExportBinding, ModuleVtable, Services, MODULE_OK};

#[repr(C)]
struct DemoApi {
    abi: u32,
    magic: unsafe extern "C" fn() -> u32,
}

static VTABLE: ModuleVtable = ModuleVtable {
    status: 0,
    tick: None,
};

/// Module entry.
///
/// # Safety
/// `services` must point to the kernel service table; `_context` is the
/// manager's opaque context pointer and may be null.
#[no_mangle]
pub unsafe extern "C" fn _start(
    services: *const Services,
    _context: *const ModuleContext,
) -> *const ModuleVtable {
    let Some(services) = (unsafe { services.as_ref() }) else {
        return &VTABLE;
    };
    let interface = b"irix7.demo.service";
    let mut binding = ModuleExportBinding::EMPTY;
    let status = unsafe {
        (services.module_find_export)(interface.as_ptr(), interface.len() as u32, 1, &mut binding)
    };
    if status != MODULE_OK || binding.table.is_null() {
        write(services, b"demo-root: dependency export missing status=");
        write_dec(services, status);
        write(services, b"\n");
        return &VTABLE;
    }
    let api = unsafe { &*(binding.table as *const DemoApi) };
    if api.abi != 1 {
        write(services, b"demo-root: dependency ABI mismatch\n");
        return &VTABLE;
    }
    let magic = unsafe { (api.magic)() };
    let mut line = *b"demo-root: dep=0x000 owner=0x0000000000000000\n";
    line[17] = HEX[((magic >> 8) & 0xf) as usize];
    line[18] = HEX[((magic >> 4) & 0xf) as usize];
    line[19] = HEX[(magic & 0xf) as usize];
    let owner = binding.handle;
    let owner_at = find(&line, b"owner=0x").expect("owner marker") + 8;
    for index in 0..16 {
        let digit = ((owner >> ((15 - index) * 4)) & 0xf) as usize;
        line[owner_at + index] = HEX[digit];
    }
    write(services, &line);
    &VTABLE
}

const HEX: &[u8; 16] = b"0123456789abcdef";

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

fn write(services: &Services, bytes: &[u8]) {
    (services.write)(bytes.as_ptr(), bytes.len());
}

fn write_dec(services: &Services, value: i32) {
    let negative = value < 0;
    let mut value = value.unsigned_abs();
    let mut buf = [0u8; 12];
    let mut index = buf.len();
    loop {
        index -= 1;
        buf[index] = b'0' + (value % 10) as u8;
        value /= 10;
        if value == 0 {
            break;
        }
    }
    if negative {
        index -= 1;
        buf[index] = b'-';
    }
    write(services, &buf[index..]);
}

#[cfg(not(test))]
#[panic_handler]
fn panic(_info: &core::panic::PanicInfo<'_>) -> ! {
    loop {
        core::hint::spin_loop();
    }
}
