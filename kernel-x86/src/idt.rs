// SPDX-License-Identifier: GPL-3.0-only
//! The interrupt descriptor table and the exception report path.
//!
//! The first slice installs the vectors it can prove: `int3`, #GP, #PF and the
//! timer.  Every other entry points at one shared stub, so an unexpected
//! vector is reported with a sentinel number instead of running through a
//! zeroed descriptor.

use core::arch::asm;

#[repr(C, packed)]
#[derive(Clone, Copy)]
struct IdtEntry {
    offset_low: u16,
    selector: u16,
    ist: u8,
    flags: u8,
    offset_mid: u16,
    offset_high: u32,
    zero: u32,
}

impl IdtEntry {
    const MISSING: Self = Self {
        offset_low: 0,
        selector: 0,
        ist: 0,
        flags: 0,
        offset_mid: 0,
        offset_high: 0,
        zero: 0,
    };

    fn handler(handler: u64) -> Self {
        Self {
            offset_low: handler as u16,
            selector: 0x08,
            ist: 0,
            // Present, DPL 0, 64-bit interrupt gate.
            flags: 0x8E,
            offset_mid: (handler >> 16) as u16,
            offset_high: (handler >> 32) as u32,
            zero: 0,
        }
    }
}

#[repr(C, packed)]
struct Descriptor {
    limit: u16,
    base: u64,
}

static mut IDT: [IdtEntry; 256] = [IdtEntry::MISSING; 256];

extern "C" {
    /// Breakpoint, #GP, #PF, the unexpected-vector stub and IRQ0, in that
    /// order; `boot.S` builds the table so the IDT setup stays data-driven.
    static isr_table: [u64; 5];
}

pub fn init() {
    // SAFETY: single-threaded bring-up; no interrupts are enabled yet, so no
    // handler can observe a half-written table.
    unsafe {
        let table = &isr_table;
        let idt = &mut *core::ptr::addr_of_mut!(IDT);
        for entry in idt.iter_mut() {
            *entry = IdtEntry::handler(table[3]);
        }
        idt[3] = IdtEntry::handler(table[0]);
        idt[13] = IdtEntry::handler(table[1]);
        idt[14] = IdtEntry::handler(table[2]);
        idt[32] = IdtEntry::handler(table[4]);

        let descriptor = Descriptor {
            limit: (core::mem::size_of::<[IdtEntry; 256]>() - 1) as u16,
            base: core::ptr::addr_of!(IDT) as u64,
        };
        asm!("lidt [{}]", in(reg) &descriptor, options(readonly, nostack, preserves_flags));
    }
}

/// Report a fault and stop.  `frame` points at the vector, error code and
/// faulting instruction pointer, in that order, as pushed by the stubs.
#[no_mangle]
pub extern "C" fn exception_dispatch(frame: *const u64) -> ! {
    let (vector, error, rip) = unsafe { (*frame, *frame.add(1), *frame.add(2)) };
    let name: &[u8] = match vector {
        3 => b"breakpoint",
        13 => b"general-protection",
        14 => b"page-fault",
        0xFF => b"unexpected",
        _ => b"unknown",
    };
    crate::serial::write(b"x86: exception vector=");
    crate::print_dec(vector);
    crate::serial::write(b" (");
    crate::serial::write(name);
    crate::serial::write(b") error=");
    crate::print_hex(error);
    crate::serial::write(b" rip=");
    crate::print_hex(rip);
    crate::serial::write_byte(b'\n');
    crate::print_line(&[b"x86: halted"]);
    crate::qemu_exit(0)
}
