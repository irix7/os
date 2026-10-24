// SPDX-License-Identifier: GPL-3.0-only
//! Minimal native IRIX 7 user smoke image.
//!
//! It is linked at 0x1000_0000 and uses the provisional syscall codes:
//! `v0 = 1` write (`a0` fd, `a1` buffer, `a2` length), `v0 = 2` exit.
//! This is a loader/syscall test, not a libc or a general user ABI.

#![no_std]
#![no_main]
#![feature(asm_experimental_arch)]

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

    if write(1, MESSAGE.as_ptr(), MESSAGE.len() as u32) != MESSAGE.len() as u32 {
        exit(44);
    }

    // The kernel places argv[0] ("hello") on the initial stack.
    let arg0 = unsafe { core::ptr::read_volatile(argv as *const u32) };
    if arg0 == 0 || write(1, arg0 as *const u8, 5) != 5 {
        exit(45);
    }
    if write(1, b"\n".as_ptr(), 1) != 1 {
        exit(46);
    }

    // The kernel places PATH=/ and HOME=/ on the initial environment.
    let env0 = unsafe { core::ptr::read_volatile(envp as *const u32) };
    if env0 == 0 || write(1, env0 as *const u8, 6) != 6 {
        exit(48);
    }
    if write(1, b"\n".as_ptr(), 1) != 1 {
        exit(49);
    }

    unsafe {
        // Exercise the highest mapped stack page.
        core::arch::asm!("sw $zero, -4($sp)", options(nostack));
    }
    exit(42)
}

core::arch::global_asm!(
    r#"
    .set noreorder
    .set mips3
    .section .text.user_write, "ax", @progbits
    .align 2
    .globl user_write
user_write:
    li      $v0, 1
    syscall
    jr      $ra
    nop
"#
);

extern "C" {
    /// The n32 C ABI already places fd/buffer/length in $a0/$a1/$a2.
    fn user_write(fd: u32, buffer: *const u8, length: u32) -> u32;
}

#[inline(never)]
fn write(fd: u32, buffer: *const u8, length: u32) -> u32 {
    unsafe { user_write(fd, buffer, length) }
}

fn exit(code: u32) -> ! {
    unsafe {
        core::arch::asm!(
            "move $a0, {code}",
            "li $v0, 2",
            "syscall",
            "2:",
            "b 2b",
            "nop",
            code = in(reg) code,
            options(noreturn)
        );
    }
}

#[cfg(not(test))]
#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    loop {
        core::hint::spin_loop();
    }
}
