//! Minimal memory primitives required by smoltcp and core when compiled for a
//! freestanding target without libc. compiler_builtins does not always emit
//! these as linkable symbols on MIPS targets.

use core::ptr::{read_volatile, write_volatile};

/// Copy `n` bytes from `src` to `dst`, handling overlap correctly.
#[no_mangle]
pub unsafe extern "C" fn memcpy(dst: *mut u8, src: *const u8, n: usize) -> *mut u8 {
    let mut dst = dst as *mut u8;
    let src = src as *const u8;
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
    dst as *mut u8
}

/// Fill `n` bytes of `dst` with `val`.
#[no_mangle]
pub unsafe extern "C" fn memset(dst: *mut u8, val: i32, n: usize) -> *mut u8 {
    let mut dst = dst as *mut u8;
    let v = val as u8;
    for i in 0..n {
        write_volatile(dst.add(i), v);
    }
    dst as *mut u8
}

/// Compare `n` bytes at `s1` and `s2`, returning `< 0`, `0`, or `> 0`.
#[no_mangle]
pub unsafe extern "C" fn memcmp(s1: *const u8, s2: *const u8, n: usize) -> i32 {
    let s1 = s1 as *const u8;
    let s2 = s2 as *const u8;
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
#[no_mangle]
pub unsafe extern "C" fn memmove(dst: *mut u8, src: *const u8, n: usize) -> *mut u8 {
    let mut dst = dst as *mut u8;
    let src = src as *const u8;
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
    dst as *mut u8
}
