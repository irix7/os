//! Minimal memory primitives required by smoltcp and core when compiled for a
//! freestanding target without libc. compiler_builtins does not always emit
//! these as linkable symbols on MIPS, so we provide them here.

use core::ptr::{read_volatile, write_volatile};

/// Copy `n` bytes from `src` to `dst`, handling overlap correctly.
///
/// # Safety
/// `dst` must point to `n` bytes of writable memory and `src` to `n` bytes of
/// readable memory. The regions may overlap.
#[no_mangle]
pub unsafe extern "C" fn memcpy(dst: *mut u8, src: *const u8, n: usize) -> *mut u8 {
    if (dst as usize) < (src as usize) {
        for i in 0..n {
            write_volatile(dst.add(i), read_volatile(src.add(i)));
        }
    } else {
        let mut i = n;
        while i > 0 {
            i -= 1;
            write_volatile(dst.add(i), read_volatile(src.add(i)));
        }
    }
    dst
}

/// Fill `n` bytes of `dst` with `val`.
///
/// # Safety
/// `dst` must point to `n` bytes of writable memory.
#[no_mangle]
pub unsafe extern "C" fn memset(dst: *mut u8, val: i32, n: usize) -> *mut u8 {
    let v = val as u8;
    for i in 0..n {
        write_volatile(dst.add(i), v);
    }
    dst
}

/// Compare `n` bytes at `s1` and `s2`, returning `< 0`, `0`, or `> 0`.
///
/// # Safety
/// `s1` and `s2` must point to `n` bytes of readable memory.
#[no_mangle]
pub unsafe extern "C" fn memcmp(s1: *const u8, s2: *const u8, n: usize) -> i32 {
    for i in 0..n {
        let a = read_volatile(s1.add(i));
        let b = read_volatile(s2.add(i));
        if a != b {
            return (a as i32) - (b as i32);
        }
    }
    0
}

/// Copy `n` bytes from `src` to `dst`, handling overlap correctly.
///
/// # Safety
/// `dst` must point to `n` bytes of writable memory and `src` to `n` bytes of
/// readable memory. The regions may overlap.
#[no_mangle]
pub unsafe extern "C" fn memmove(dst: *mut u8, src: *const u8, n: usize) -> *mut u8 {
    if (dst as usize) < (src as usize) {
        for i in 0..n {
            write_volatile(dst.add(i), read_volatile(src.add(i)));
        }
    } else {
        let mut i = n;
        while i > 0 {
            i -= 1;
            write_volatile(dst.add(i), read_volatile(src.add(i)));
        }
    }
    dst
}
