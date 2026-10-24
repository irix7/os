// SPDX-License-Identifier: GPL-3.0-only
//! Memory and string primitives for the bring-up libc.

use core::ffi::c_void;

use crate::sys::{syscall2, SYS_MMAP};

const HEADER: usize = 16;

static mut HEAP_NEXT: usize = 0;
static mut HEAP_LEFT: usize = 0;

unsafe fn grow(minimum: usize) -> bool {
    let request = minimum.max(32 * 1024);
    let result = syscall2(SYS_MMAP, 0, request);
    if result == usize::MAX || result == 0 {
        return false;
    }
    HEAP_NEXT = result;
    HEAP_LEFT = request;
    true
}

pub unsafe fn malloc(size: usize) -> *mut u8 {
    let Some(total) = size.checked_add(HEADER + 15) else {
        return core::ptr::null_mut();
    };
    if HEAP_LEFT < total && !grow(total) {
        return core::ptr::null_mut();
    }
    let raw = (HEAP_NEXT + 15) & !15;
    HEAP_NEXT = raw + total;
    HEAP_LEFT = HEAP_LEFT.saturating_sub(total);
    let header = raw as *mut usize;
    *header = size;
    (raw + HEADER) as *mut u8
}

pub unsafe fn realloc(ptr: *mut u8, size: usize) -> *mut u8 {
    if ptr.is_null() {
        return malloc(size);
    }
    let old = *((ptr as usize - HEADER) as *const usize);
    let new = malloc(size);
    if new.is_null() {
        return core::ptr::null_mut();
    }
    core::ptr::copy_nonoverlapping(ptr, new, old.min(size));
    new
}

pub unsafe fn aligned_alloc(align: usize, size: usize) -> Option<*mut u8> {
    let align = align.max(16);
    if align & (align - 1) != 0 {
        return None;
    }
    let total = size.checked_add(HEADER + align - 1)?;
    if HEAP_LEFT < total && !grow(total) {
        return None;
    }
    let raw = (HEAP_NEXT + align - 1) & !(align - 1);
    HEAP_NEXT = raw + total;
    HEAP_LEFT = HEAP_LEFT.saturating_sub(total);
    *((raw - HEADER) as *mut usize) = size;
    Some((raw) as *mut u8)
}

#[no_mangle]
pub unsafe extern "C" fn memcpy(dst: *mut c_void, src: *const c_void, len: usize) -> *mut c_void {
    let dst = dst as *mut u8;
    let src = src as *const u8;
    let mut index = 0;
    while index < len {
        *dst.add(index) = *src.add(index);
        index += 1;
    }
    dst as *mut c_void
}

#[no_mangle]
pub unsafe extern "C" fn memmove(dst: *mut c_void, src: *const c_void, len: usize) -> *mut c_void {
    let dst8 = dst as *mut u8;
    let src8 = src as *const u8;
    if (dst as usize) < (src as usize) {
        return memcpy(dst, src, len);
    }
    let mut index = len;
    while index > 0 {
        index -= 1;
        *dst8.add(index) = *src8.add(index);
    }
    dst
}

#[no_mangle]
pub unsafe extern "C" fn memset(dst: *mut c_void, value: i32, len: usize) -> *mut c_void {
    let dst8 = dst as *mut u8;
    let mut index = 0;
    while index < len {
        *dst8.add(index) = value as u8;
        index += 1;
    }
    dst
}

#[no_mangle]
pub unsafe extern "C" fn memcmp(left: *const c_void, right: *const c_void, len: usize) -> i32 {
    let left = left as *const u8;
    let right = right as *const u8;
    let mut index = 0;
    while index < len {
        let a = *left.add(index);
        let b = *right.add(index);
        if a != b {
            return a as i32 - b as i32;
        }
        index += 1;
    }
    0
}
