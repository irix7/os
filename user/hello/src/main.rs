// SPDX-License-Identifier: GPL-3.0-only
//! Minimal native IRIX 7 user smoke image.
//!
//! It is linked at 0x1000_0000 and uses the provisional kernel syscall ABI
//! through the v0 native libc.  This is a loader/libc test, not a POSIX
//! application environment yet.

#![no_std]
#![no_main]
#![feature(asm_experimental_arch)]

use core::ffi::c_void;
use native_libc::{_exit, calloc, free, malloc, memalign, memcmp, memcpy, realloc, strlen, write};

/// Forces the loader to map and zero a BSS range beyond the file data.
#[no_mangle]
static mut BSS_PAGE: [u8; 8192] = [0; 8192];

const MESSAGE: &[u8] = b"hello from user\n";

#[no_mangle]
pub extern "C" fn _start() -> ! {
    let mut argc: u32;
    let mut argv: u32;
    let mut envp: u32;
    unsafe {
        core::arch::asm!(
            "move {argc}, $a0",
            "move {argv}, $a1",
            "move {envp}, $a2",
            argc = out(reg) argc,
            argv = out(reg) argv,
            envp = out(reg) envp,
        );
    }
    if argc != 1 {
        exit(47);
    }

    // If the loader forgot to map or zero the BSS pages this faults or exits 43.
    let first = unsafe { core::ptr::read_volatile(core::ptr::addr_of!(BSS_PAGE[0])) };
    let second = unsafe { core::ptr::read_volatile(core::ptr::addr_of!(BSS_PAGE[4096])) };
    let last = unsafe { core::ptr::read_volatile(core::ptr::addr_of!(BSS_PAGE[8191])) };
    if first != 0 || second != 0 || last != 0 {
        exit(43);
    }

    if !write_bytes(MESSAGE) {
        exit(44);
    }

    // The kernel places argv[0] ("hello") on the initial stack.
    let arg0 = unsafe { core::ptr::read_volatile(argv as *const u32) };
    if arg0 == 0 || !write_user(arg0, 5) {
        exit(45);
    }
    if !write_bytes(b"\n") {
        exit(46);
    }

    // The kernel places PATH=/ and HOME=/ on the initial environment.
    let env0 = unsafe { core::ptr::read_volatile(envp as *const u32) };
    if env0 == 0 {
        exit(48);
    }
    let mut env_line = [0u8; 7];
    unsafe {
        core::ptr::copy_nonoverlapping(env0 as *const u8, env_line.as_mut_ptr(), 6);
    }
    env_line[6] = b'\n';
    // The path/argv checks above already exercise the write return; do not
    // fold this one into another conditional in the smoke image.
    let _ = write_bytes(&env_line);

    // libc malloc maps anonymous pages; check zeroing and copy bytes into it.
    let heap = unsafe { malloc(8192) };
    if heap.is_null() {
        exit(50);
    }
    if unsafe { memcmp(heap, b"\0".as_ptr() as *const c_void, 1) } != 0 {
        exit(51);
    }
    let heap_message = b"heap\n";
    unsafe {
        memcpy(
            heap,
            heap_message.as_ptr() as *const c_void,
            heap_message.len(),
        );
    }
    if !write_user(heap as u32, heap_message.len() as u32) {
        exit(52);
    }
    unsafe {
        free(heap);
    }

    // Exercise the remaining v0 libc primitives.
    let zeroed = unsafe { calloc(4, 4) };
    if zeroed.is_null() {
        exit(53);
    }
    if unsafe { memcmp(zeroed, b"\0".as_ptr() as *const c_void, 1) } != 0 {
        exit(54);
    }
    unsafe { free(zeroed) };

    let resized = unsafe { realloc(heap, 16) };
    if resized.is_null() {
        exit(55);
    }
    if unsafe { memcmp(resized, b"heap\n\0".as_ptr() as *const c_void, 5) } != 0 {
        exit(56);
    }
    unsafe { free(resized) };

    if unsafe { strlen(b"hello\0".as_ptr() as *const core::ffi::c_char) } != 5 {
        exit(57);
    }
    let aligned = unsafe { memalign(16, 32) };
    if aligned.is_null() || (aligned as usize) & 15 != 0 {
        exit(58);
    }
    unsafe { free(aligned) };

    unsafe {
        // Exercise the highest mapped stack page.
        core::arch::asm!("sw $zero, -4($sp)", options(nostack));
    }
    exit(42)
}

fn write_bytes(bytes: &[u8]) -> bool {
    let pointer = core::hint::black_box(bytes.as_ptr()) as *const c_void;
    let length = core::hint::black_box(bytes.len());
    unsafe { write(1, pointer, length) == length as isize }
}

fn write_user(address: u32, length: u32) -> bool {
    let pointer = core::hint::black_box(address) as *const c_void;
    let length = core::hint::black_box(length) as usize;
    unsafe { write(1, pointer, length) == length as isize }
}

fn exit(status: u32) -> ! {
    unsafe { _exit(status as i32) }
}

#[cfg(not(test))]
#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    exit(134)
}
