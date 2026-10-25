// SPDX-License-Identifier: GPL-3.0-only
//! Native IRIX 7 libc v0.
//!
//! This is a deliberately small, freestanding C ABI layer over the provisional
//! user syscalls.  It implements the memory and string primitives, `write`,
//! `_exit`, anonymous `mmap` and a one-way `malloc` so native user images can
//! start sharing a runtime.  It is not yet the POSIX.1-2024 native libc: the
//! kernel now has a small console/ramfs fd table, but this library has no
//! general file system, pthreads, TLS, signals or locale wrapper.
//!
//! The ABI is n32 big-endian.  All syscall entry points live in assembly so the
//! compiler's register allocator cannot clobber the fixed `$a0`-`$a3`/`$v0`
//! conventions.

#![no_std]
#![feature(asm_experimental_arch)]

use core::ffi::{c_char, c_int, c_void};
use core::ptr::{null_mut, read_volatile, write_volatile};

/// Payload header before each allocation.
const HEADER: usize = 16;

/// A pointer value equal to `-1` in the n32 C ABI.
const MAP_FAILED: *mut c_void = usize::MAX as *mut c_void;

/// `EIO`-like value used until native errno numbers are frozen.
const ERRNO_IO: c_int = 5;
const ERRNO_INVAL: c_int = 22;
const ERRNO_NOMEM: c_int = 12;

static mut ERRNO: c_int = 0;

#[no_mangle]
pub extern "C" fn __errno_location() -> *mut c_int {
    core::ptr::addr_of_mut!(ERRNO)
}

core::arch::global_asm!(
    r#"
    .set noreorder
    .set mips3

    .section .text.__irix7_write, "ax", @progbits
    .align 2
    .globl __irix7_write
__irix7_write:
    li      $v0, 1
    syscall
    jr      $ra
    nop

    .section .text.__irix7_read, "ax", @progbits
    .align 2
    .globl __irix7_read
__irix7_read:
    li      $v0, 16
    syscall
    jr      $ra
    nop

    .section .text.__irix7_pipe, "ax", @progbits
    .align 2
    .globl __irix7_pipe
__irix7_pipe:
    li      $v0, 22
    syscall
    jr      $ra
    nop

    .section .text.__irix7_tcgetwinsize, "ax", @progbits
    .align 2
    .globl __irix7_tcgetwinsize
__irix7_tcgetwinsize:
    li      $v0, 23
    syscall
    jr      $ra
    nop

    .section .text.__irix7_tcsetwinsize, "ax", @progbits
    .align 2
    .globl __irix7_tcsetwinsize
__irix7_tcsetwinsize:
    li      $v0, 24
    syscall
    jr      $ra
    nop

    .section .text.__irix7_exit, "ax", @progbits
    .align 2
    .globl __irix7_exit
__irix7_exit:
    li      $v0, 2
    syscall
1:  b       1b
    nop

    .section .text.__irix7_mmap_anon, "ax", @progbits
    .align 2
    .globl __irix7_mmap_anon
__irix7_mmap_anon:
    move    $a1, $4
    move    $a0, $zero
    li      $a2, 3
    li      $a3, 0x22
    li      $v0, 3
    syscall
    jr      $ra
    nop

    .section .text.__irix7_munmap, "ax", @progbits
    .align 2
    .globl __irix7_munmap
__irix7_munmap:
    li      $v0, 4
    syscall
    jr      $ra
    nop

    .section .text.__irix7_mprotect, "ax", @progbits
    .align 2
    .globl __irix7_mprotect
__irix7_mprotect:
    li      $v0, 5
    syscall
    jr      $ra
    nop
"#
);

extern "C" {
    fn __irix7_write(fd: c_int, buf: *const u8, count: usize) -> isize;
    fn __irix7_read(fd: c_int, buf: *mut u8, count: usize) -> isize;
    fn __irix7_pipe(fds: *mut c_int) -> c_int;
    fn __irix7_tcgetwinsize(fd: c_int, ws: *mut winsize) -> c_int;
    fn __irix7_tcsetwinsize(fd: c_int, ws: *const winsize) -> c_int;
    fn __irix7_exit(status: c_int) -> !;
    fn __irix7_mmap_anon(length: usize) -> *mut c_void;
    fn __irix7_munmap(address: *mut c_void, length: usize) -> c_int;
    fn __irix7_mprotect(address: *mut c_void, length: usize, protection: c_int) -> c_int;
}

/// POSIX `write` over the provisional fd ABI.
#[inline(never)]
#[no_mangle]
pub unsafe extern "C" fn write(fd: c_int, buffer: *const c_void, count: usize) -> isize {
    let result = __irix7_write(fd, buffer as *const u8, count);
    if result < 0 {
        *__errno_location() = ERRNO_IO;
    }
    result
}

/// POSIX `read` over the provisional fd ABI.
#[inline(never)]
#[no_mangle]
pub unsafe extern "C" fn read(fd: c_int, buffer: *mut c_void, count: usize) -> isize {
    let result = __irix7_read(fd, buffer as *mut u8, count);
    if result < 0 {
        *__errno_location() = ERRNO_IO;
    }
    result
}

/// POSIX `pipe`: two descriptors, read first and write second.
#[no_mangle]
pub unsafe extern "C" fn pipe(fds: *mut c_int) -> c_int {
    let result = __irix7_pipe(fds);
    if result < 0 {
        *__errno_location() = ERRNO_NOMEM;
        -1
    } else {
        0
    }
}

/// POSIX `struct winsize`.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct winsize {
    pub ws_row: u16,
    pub ws_col: u16,
    pub ws_xpixel: u16,
    pub ws_ypixel: u16,
}

/// POSIX `tcgetwinsize`.
#[no_mangle]
pub unsafe extern "C" fn tcgetwinsize(fd: c_int, ws: *mut winsize) -> c_int {
    let result = __irix7_tcgetwinsize(fd, ws);
    if result < 0 {
        *__errno_location() = ERRNO_INVAL;
        -1
    } else {
        0
    }
}

/// POSIX `tcsetwinsize`.
#[no_mangle]
pub unsafe extern "C" fn tcsetwinsize(fd: c_int, ws: *const winsize) -> c_int {
    let result = __irix7_tcsetwinsize(fd, ws);
    if result < 0 {
        *__errno_location() = ERRNO_INVAL;
        -1
    } else {
        0
    }
}

/// Terminate the user image; the kernel returns to the console.
#[no_mangle]
pub unsafe extern "C" fn _exit(status: c_int) -> ! {
    __irix7_exit(status)
}

/// POSIX `_Exit`, an alias for `_exit`.
#[no_mangle]
pub unsafe extern "C" fn _Exit(status: c_int) -> ! {
    _exit(status)
}

/// POSIX `exit` without atexit handlers in v0.
#[no_mangle]
pub unsafe extern "C" fn exit(status: c_int) -> ! {
    _exit(status)
}

/// Anonymous private mapping; the provisional kernel ignores address/prot/flags.
#[no_mangle]
pub unsafe extern "C" fn mmap(
    _address: *mut c_void,
    length: usize,
    _protection: c_int,
    _flags: c_int,
    _fd: c_int,
    _offset: i64,
) -> *mut c_void {
    let result = __irix7_mmap_anon(length);
    if result.is_null() || result as usize == usize::MAX {
        *__errno_location() = ERRNO_NOMEM;
        MAP_FAILED
    } else {
        result
    }
}

/// POSIX `munmap`; v0 mappings are released at process exit.
#[no_mangle]
pub unsafe extern "C" fn munmap(_address: *mut c_void, _length: usize) -> c_int {
    0
}

/// POSIX `mprotect`; every user page is already read/write.
#[no_mangle]
pub unsafe extern "C" fn mprotect(
    _address: *mut c_void,
    _length: usize,
    _protection: c_int,
) -> c_int {
    0
}

/// A one-way malloc: each allocation is a separate anonymous mapping with a
/// 16-byte size header.  `free` is a no-op until the process exits.
#[no_mangle]
pub unsafe extern "C" fn malloc(size: usize) -> *mut c_void {
    let Some(total) = size.checked_add(HEADER) else {
        *__errno_location() = ERRNO_NOMEM;
        return null_mut();
    };
    let mapping = mmap(null_mut(), total, 3, 0x22, -1, 0);
    if mapping == MAP_FAILED || mapping.is_null() {
        return null_mut();
    }
    write_volatile(mapping as *mut usize, total);
    mapping.add(HEADER)
}

/// Free is a no-op in the v0 bump allocator.
#[no_mangle]
pub unsafe extern "C" fn free(_pointer: *mut c_void) {}

/// Resize an allocation, preserving the smaller of the old and new sizes.
#[no_mangle]
pub unsafe extern "C" fn realloc(pointer: *mut c_void, size: usize) -> *mut c_void {
    if pointer.is_null() {
        return malloc(size);
    }
    if size == 0 {
        return null_mut();
    }
    let header = (pointer as *mut u8).sub(HEADER);
    let total = read_volatile(header as *const usize);
    let old_size = total.saturating_sub(HEADER);
    let new_pointer = malloc(size);
    if new_pointer.is_null() {
        return null_mut();
    }
    let copy = old_size.min(size);
    memcpy(new_pointer, pointer, copy);
    new_pointer
}

/// Allocate and zero `count * size` bytes.
#[no_mangle]
pub unsafe extern "C" fn calloc(count: usize, size: usize) -> *mut c_void {
    let Some(total) = count.checked_mul(size) else {
        *__errno_location() = ERRNO_NOMEM;
        return null_mut();
    };
    let pointer = malloc(total);
    if !pointer.is_null() {
        memset(pointer, 0, total);
    }
    pointer
}

/// Allocate at least `alignment`-aligned storage.
#[no_mangle]
pub unsafe extern "C" fn memalign(alignment: usize, size: usize) -> *mut c_void {
    if alignment == 0 {
        *__errno_location() = ERRNO_INVAL;
        return null_mut();
    }
    // v0 malloc returns 16-byte aligned payloads; larger alignments are not
    // yet supported.
    if alignment > HEADER {
        *__errno_location() = ERRNO_INVAL;
        return null_mut();
    }
    malloc(size)
}

#[no_mangle]
pub unsafe extern "C" fn memcpy(
    destination: *mut c_void,
    source: *const c_void,
    count: usize,
) -> *mut c_void {
    let dst = destination as *mut u8;
    let src = source as *const u8;
    for index in 0..count {
        write_volatile(dst.add(index), read_volatile(src.add(index)));
    }
    destination
}

#[no_mangle]
pub unsafe extern "C" fn memmove(
    destination: *mut c_void,
    source: *const c_void,
    count: usize,
) -> *mut c_void {
    let dst = destination as *mut u8;
    let src = source as *const u8;
    if (dst as usize) <= (src as usize) {
        for index in 0..count {
            write_volatile(dst.add(index), read_volatile(src.add(index)));
        }
    } else {
        for index in (0..count).rev() {
            write_volatile(dst.add(index), read_volatile(src.add(index)));
        }
    }
    destination
}

#[no_mangle]
pub unsafe extern "C" fn memset(
    destination: *mut c_void,
    value: c_int,
    count: usize,
) -> *mut c_void {
    let dst = destination as *mut u8;
    let byte = value as u8;
    for index in 0..count {
        write_volatile(dst.add(index), byte);
    }
    destination
}

#[no_mangle]
pub unsafe extern "C" fn memcmp(left: *const c_void, right: *const c_void, count: usize) -> c_int {
    let left = left as *const u8;
    let right = right as *const u8;
    for index in 0..count {
        let a = read_volatile(left.add(index));
        let b = read_volatile(right.add(index));
        if a != b {
            return a as c_int - b as c_int;
        }
    }
    0
}

#[no_mangle]
pub unsafe extern "C" fn strlen(string: *const c_char) -> usize {
    let mut length = 0;
    while read_volatile((string as *const u8).add(length)) != 0 {
        length += 1;
    }
    length
}

#[no_mangle]
pub unsafe extern "C" fn strcmp(left: *const c_char, right: *const c_char) -> c_int {
    let mut index = 0;
    loop {
        let a = read_volatile((left as *const u8).add(index));
        let b = read_volatile((right as *const u8).add(index));
        if a != b {
            return a as c_int - b as c_int;
        }
        if a == 0 {
            return 0;
        }
        index += 1;
    }
}

#[no_mangle]
pub unsafe extern "C" fn strncmp(left: *const c_char, right: *const c_char, count: usize) -> c_int {
    for index in 0..count {
        let a = read_volatile((left as *const u8).add(index));
        let b = read_volatile((right as *const u8).add(index));
        if a != b {
            return a as c_int - b as c_int;
        }
        if a == 0 {
            return 0;
        }
    }
    0
}

#[no_mangle]
pub unsafe extern "C" fn strcpy(destination: *mut c_char, source: *const c_char) -> *mut c_char {
    let mut index = 0;
    loop {
        let byte = read_volatile((source as *const u8).add(index));
        write_volatile((destination as *mut u8).add(index), byte);
        if byte == 0 {
            return destination;
        }
        index += 1;
    }
}

#[no_mangle]
pub unsafe extern "C" fn abort() -> ! {
    _exit(134)
}
