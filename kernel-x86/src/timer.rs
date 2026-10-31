// SPDX-License-Identifier: GPL-3.0-only
//! The 8259 PIC remap and the 8254 PIT tick.
//!
//! Both are legacy devices that QEMU's `q35` still provides without any PCI
//! discovery, which makes them the right first interrupt source: the trap path
//! and the timer are proven before virtio or the IO-APIC exist.

use core::arch::asm;
use core::sync::atomic::{AtomicU32, Ordering};

const PIC1_COMMAND: u16 = 0x20;
const PIC1_DATA: u16 = 0x21;
const PIC2_COMMAND: u16 = 0xA0;
const PIC2_DATA: u16 = 0xA1;
const PIT_CHANNEL0: u16 = 0x40;
const PIT_COMMAND: u16 = 0x43;
const PIT_HZ: u32 = 100;

static TICKS: AtomicU32 = AtomicU32::new(0);

fn outb(port: u16, value: u8) {
    // SAFETY: an I/O port write, valid in ring 0.
    unsafe {
        asm!("out dx, al", in("dx") port, in("al") value, options(nomem, nostack));
    }
}

/// Remap the PIC to vectors 32..48, unmask only the timer, and start the PIT.
pub fn init() {
    // SAFETY: single-threaded bring-up; interrupts are still disabled.
    unsafe {
        // ICW1: initialise, ICW4 needed.
        outb(PIC1_COMMAND, 0x11);
        outb(PIC2_COMMAND, 0x11);
        // ICW2: vector bases.
        outb(PIC1_DATA, 0x20);
        outb(PIC2_DATA, 0x28);
        // ICW3: master has a slave on IRQ2, slave is on it.
        outb(PIC1_DATA, 0x04);
        outb(PIC2_DATA, 0x02);
        // ICW4: 8086 mode.
        outb(PIC1_DATA, 0x01);
        outb(PIC2_DATA, 0x01);
        // Mask everything but IRQ0.
        outb(PIC1_DATA, 0xFE);
        outb(PIC2_DATA, 0xFF);

        // PIT channel 0, mode 3, binary, lo/hi byte.
        let divisor = (1_193_182 / PIT_HZ) as u16;
        outb(PIT_COMMAND, 0x36);
        outb(PIT_CHANNEL0, divisor as u8);
        outb(PIT_CHANNEL0, (divisor >> 8) as u8);

        asm!("sti", options(nomem, nostack));
    }
}

pub fn ticks() -> u32 {
    TICKS.load(Ordering::Relaxed)
}

/// Called from the assembly IRQ0 stub for every PIT tick.
#[no_mangle]
pub extern "C" fn timer_tick() {
    TICKS.fetch_add(1, Ordering::Relaxed);
    // End of interrupt to the master PIC.
    outb(PIC1_COMMAND, 0x20);
}
