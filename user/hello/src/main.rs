// SPDX-License-Identifier: GPL-3.0-only
//! Minimal native IRIX 7 user smoke image.
//!
//! It is linked at 0x1000_0000 and uses the provisional syscall codes:
//! `v0 = 1` write (reported by the kernel and returned from), `v0 = 2` exit.
//! This is a loader/syscall test, not a libc or a general user ABI.

#![no_std]
#![no_main]
#![feature(asm_experimental_arch)]

/// Forces the loader to map and zero a BSS range beyond the file data.
#[no_mangle]
static mut BSS_PAGE: [u8; 8192] = [0; 8192];

#[no_mangle]
pub extern "C" fn _start() -> ! {
    // If the loader forgot to map or zero the BSS pages this faults or exits 43.
    let first = unsafe { core::ptr::read_volatile(core::ptr::addr_of!(BSS_PAGE[0])) };
    let second = unsafe { core::ptr::read_volatile(core::ptr::addr_of!(BSS_PAGE[4096])) };
    let last = unsafe { core::ptr::read_volatile(core::ptr::addr_of!(BSS_PAGE[8191])) };
    if first != 0 || second != 0 || last != 0 {
        exit(43);
    }

    unsafe {
        // Exercise the highest mapped stack page, then the provisional write.
        core::arch::asm!(
            "sw $zero, -4($sp)",
            "li $v0, 1",
            "syscall",
            options(nostack)
        );
    }
    exit(42)
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
