// SPDX-License-Identifier: GPL-3.0-only
//! x86 port I/O.  Only ring 0 uses these, and the machine is single-threaded.

use core::arch::asm;

pub fn outb(port: u16, value: u8) {
    // SAFETY: an I/O port write, valid in ring 0.
    unsafe {
        asm!("out dx, al", in("dx") port, in("al") value, options(nomem, nostack, preserves_flags));
    }
}

pub fn outw(port: u16, value: u16) {
    // SAFETY: an I/O port write, valid in ring 0.
    unsafe {
        asm!("out dx, ax", in("dx") port, in("ax") value, options(nomem, nostack, preserves_flags));
    }
}

pub fn outl(port: u16, value: u32) {
    // SAFETY: an I/O port write, valid in ring 0.
    unsafe {
        asm!("out dx, eax", in("dx") port, in("eax") value, options(nomem, nostack, preserves_flags));
    }
}

pub fn inb(port: u16) -> u8 {
    let value: u8;
    // SAFETY: an I/O port read, valid in ring 0.
    unsafe {
        asm!("in al, dx", in("dx") port, out("al") value, options(nomem, nostack, preserves_flags));
    }
    value
}

pub fn inw(port: u16) -> u16 {
    let value: u16;
    // SAFETY: an I/O port read, valid in ring 0.
    unsafe {
        asm!("in ax, dx", in("dx") port, out("ax") value, options(nomem, nostack, preserves_flags));
    }
    value
}

pub fn inl(port: u16) -> u32 {
    let value: u32;
    // SAFETY: an I/O port read, valid in ring 0.
    unsafe {
        asm!("in eax, dx", in("dx") port, out("eax") value, options(nomem, nostack, preserves_flags));
    }
    value
}
