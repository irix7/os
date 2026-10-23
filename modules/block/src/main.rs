// SPDX-License-Identifier: GPL-3.0-only
//! Demo loadable module: read SCSI block 0 through the ABI.

#![no_std]
#![no_main]

use module_abi::{ModuleVtable, Services};

static OK: ModuleVtable = ModuleVtable {
    status: 0,
    tick: None,
};
static BAD: ModuleVtable = ModuleVtable {
    status: 1,
    tick: None,
};

/// # Safety
/// `services` must point to a valid kernel service table for this module.
#[no_mangle]
pub unsafe extern "C" fn _start(services: *const Services) -> *const ModuleVtable {
    let Some(services) = (unsafe { services.as_ref() }) else {
        return &BAD;
    };
    if services.version < 10 {
        (services.write)(b"block: ABI too old\n".as_ptr(), 18);
        return &BAD;
    }

    let mut sector = [0u8; 512];
    if unsafe { (services.block_read)(1, 0, sector.as_mut_ptr()) } != 0 {
        (services.write)(b"block: read failed\n".as_ptr(), 18);
        return &BAD;
    }
    let magic = u32::from_be_bytes([sector[0], sector[1], sector[2], sector[3]]);
    let mut line = *b"block: magic 0x00000000\n";
    const HEX: &[u8; 16] = b"0123456789abcdef";
    for (index, byte) in magic.to_be_bytes().iter().enumerate() {
        line[15 + index * 2] = HEX[(byte >> 4) as usize];
        line[16 + index * 2] = HEX[(byte & 0x0f) as usize];
    }
    (services.write)(line.as_ptr(), line.len());
    &OK
}

#[cfg(not(test))]
#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    loop {
        core::hint::spin_loop();
    }
}

// The module links only core and compiler_builtins; provide the byte
// primitives the compiler may emit for array initialisation and copies.
//
/// # Safety
/// `dst` must be valid for writes of `n` bytes.
#[no_mangle]
pub unsafe extern "C" fn memset(
    dst: *mut core::ffi::c_void,
    c: i32,
    n: usize,
) -> *mut core::ffi::c_void {
    let dst = dst as *mut u8;
    let mut i = 0;
    while i < n {
        unsafe {
            *dst.add(i) = c as u8;
        }
        i += 1;
    }
    dst as *mut core::ffi::c_void
}

/// # Safety
/// `dst` must be valid for writes of `n` bytes and `src` for reads of `n`
/// bytes; the regions must not overlap.
#[no_mangle]
pub unsafe extern "C" fn memcpy(
    dst: *mut core::ffi::c_void,
    src: *const core::ffi::c_void,
    n: usize,
) -> *mut core::ffi::c_void {
    let d = dst as *mut u8;
    let s = src as *const u8;
    let mut i = 0;
    while i < n {
        unsafe {
            *d.add(i) = *s.add(i);
        }
        i += 1;
    }
    dst
}
