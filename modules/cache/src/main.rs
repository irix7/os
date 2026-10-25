// SPDX-License-Identifier: GPL-3.0-only
//! Optional content-addressed SCSI cache probe.
//!
//! Reads one 512-byte sector from SCSI target 1 into module RAM, hashes it
//! with BLAKE3, and during the idle tick writes a small header and the sector
//! to a cache-owned target 3. A later tick reads both back and verifies the
//! digest. The module is deliberately one-shot: it is a proof of the WRITE(6)
//! path and the ABI v11 block service, not the final package cache.

#![no_std]
#![no_main]

use module_abi::{ModuleVtable, Services};

const SOURCE_TARGET: u8 = 1;
const CACHE_TARGET: u8 = 3;
const HEADER_LBA: u32 = 0;
const DATA_LBA: u32 = 1;
const MAGIC: &[u8; 8] = b"I7CACHE1";

static NONE: ModuleVtable = ModuleVtable {
    status: 0,
    tick: None,
};
static BAD: ModuleVtable = ModuleVtable {
    status: 1,
    tick: None,
};
static RUN: ModuleVtable = ModuleVtable {
    status: 0,
    tick: Some(tick),
};

static mut DATA: [u8; 512] = [0; 512];
static mut HASH: [u8; 32] = [0; 32];
static mut STATE: u8 = 0;

fn log(services: &Services, text: &[u8]) {
    (services.write)(text.as_ptr(), text.len());
}

fn log_hash(services: &Services, prefix: &[u8], hash: &[u8; 32]) {
    let mut line = [0u8; 96];
    let mut n = 0usize;
    for &byte in prefix {
        if n < line.len() {
            line[n] = byte;
            n += 1;
        }
    }
    const HEX: &[u8; 16] = b"0123456789abcdef";
    for &byte in hash.iter().take(8) {
        if n + 2 > line.len() {
            break;
        }
        line[n] = HEX[(byte >> 4) as usize];
        line[n + 1] = HEX[(byte & 0x0f) as usize];
        n += 2;
    }
    if n < line.len() {
        line[n] = b'\n';
        n += 1;
    }
    log(services, &line[..n]);
}

/// # Safety
/// `services` must point to a valid kernel service table for this module.
#[no_mangle]
pub unsafe extern "C" fn _start(services: *const Services) -> *const ModuleVtable {
    let Some(services) = (unsafe { services.as_ref() }) else {
        return &BAD;
    };
    if services.version < 11 {
        log(services, b"cache: ABI too old\n");
        return &BAD;
    }

    let data = unsafe { &mut *core::ptr::addr_of_mut!(DATA) };
    if unsafe { (services.block_read)(SOURCE_TARGET, 0, data.as_mut_ptr()) } != 0 {
        log(services, b"cache: source read failed\n");
        return &NONE;
    }
    let mut probe = [0u8; 512];
    if unsafe { (services.block_read)(CACHE_TARGET, 0, probe.as_mut_ptr()) } != 0 {
        log(services, b"cache: cache target unavailable\n");
        return &NONE;
    }
    let hash = blake3::hash(data);
    unsafe {
        let dst = core::ptr::addr_of_mut!(HASH) as *mut u8;
        core::ptr::copy_nonoverlapping(hash.as_bytes().as_ptr(), dst, 32);
        STATE = 0;
    }
    log_hash(services, b"cache: queued ", hash.as_bytes());
    &RUN
}

unsafe extern "C" fn tick(services: *const Services) -> i32 {
    let Some(services) = (unsafe { services.as_ref() }) else {
        return 1;
    };
    let state = unsafe { STATE };
    match state {
        0 => {
            let mut header = [0u8; 512];
            header[..8].copy_from_slice(MAGIC);
            unsafe {
                let hash = *core::ptr::addr_of!(HASH);
                header[8..40].copy_from_slice(&hash);
                header[40..44].copy_from_slice(&(512u32).to_be_bytes());
                STATE = 1;
            }
            log(services, b"cache: writing header\n");
            if unsafe { (services.block_write)(CACHE_TARGET, HEADER_LBA, header.as_ptr()) } != 0 {
                log(services, b"cache: header write failed\n");
                return 1;
            }
            log(services, b"cache: header stored\n");
            0
        }
        1 => {
            let data = unsafe { &*core::ptr::addr_of!(DATA) };
            log(services, b"cache: writing data\n");
            if unsafe { (services.block_write)(CACHE_TARGET, DATA_LBA, data.as_ptr()) } != 0 {
                log(services, b"cache: data write failed\n");
                return 1;
            }
            unsafe { STATE = 2 };
            log(services, b"cache: data stored\n");
            0
        }
        2 => {
            let mut header = [0u8; 512];
            let mut data = [0u8; 512];
            if unsafe { (services.block_read)(CACHE_TARGET, HEADER_LBA, header.as_mut_ptr()) } != 0
                || unsafe { (services.block_read)(CACHE_TARGET, DATA_LBA, data.as_mut_ptr()) } != 0
            {
                log(services, b"cache: readback failed\n");
                return 1;
            }
            let magic_ok = &header[..8] == MAGIC;
            let hash_ok = header[8..40] == unsafe { *core::ptr::addr_of!(HASH) };
            let data_ok = blake3::hash(&data).as_bytes() == &header[8..40];
            if magic_ok && hash_ok && data_ok {
                log(services, b"cache: verified\n");
            } else {
                log(services, b"cache: verification failed\n");
            }
            unsafe { STATE = 3 };
            1
        }
        _ => 1,
    }
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
    dst.cast()
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
    dst.cast()
}

/// # Safety
/// `a` and `b` must be valid for reads of `n` bytes.
#[no_mangle]
pub unsafe extern "C" fn memcmp(
    a: *const core::ffi::c_void,
    b: *const core::ffi::c_void,
    n: usize,
) -> i32 {
    let a = a as *const u8;
    let b = b as *const u8;
    let mut i = 0;
    while i < n {
        let av = unsafe { *a.add(i) };
        let bv = unsafe { *b.add(i) };
        if av != bv {
            return av as i32 - bv as i32;
        }
        i += 1;
    }
    0
}
