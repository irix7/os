// SPDX-License-Identifier: GPL-3.0-only
//! Interrupt Descriptor Table and exception/interrupt entry for x86_64.
//!
//! Vectors 0-31 are processor exceptions; 32 is the timer IRQ (PIC IRQ0).
//! Each exception vector has a small assembly stub that saves registers,
//! calls the Rust handler, restores registers and returns with `iretq`.
//! Vectors with an error code (8, 10, 11, 12, 13, 14) adjust the stack before
//! `iretq`.  All other vectors share an "unexpected interrupt" stub.

use crate::arch::interface::Arch;

const IDT_ENTRIES: usize = 256;
const IDT_PRESENT: u64 = 1 << 7;
const IDT_INTERRUPT_GATE: u64 = 0xE; // type bits 3:0

// Vectors that push an error code on the exception stack frame.
const HAS_ERROR_CODE: [u8; 7] = [8, 10, 11, 12, 13, 14, 17];

fn has_error_code(vector: u8) -> bool {
    HAS_ERROR_CODE.contains(&vector)
}

#[repr(C, align(16))]
struct IdtEntry {
    offset_low: u16,
    selector: u16,
    ist: u8,
    type_attr: u8,
    offset_mid: u16,
    offset_high: u32,
    reserved: u32,
}

impl IdtEntry {
    const fn null() -> Self {
        IdtEntry {
            offset_low: 0,
            selector: 0,
            ist: 0,
            type_attr: 0,
            offset_mid: 0,
            offset_high: 0,
            reserved: 0,
        }
    }

    fn set(&mut self, offset: u64, selector: u16, has_err: bool) {
        self.offset_low = offset as u16;
        self.selector = super::gdt::KERNEL_CODE_SEL;
        self.ist = 0;
        // Present, ring 0, interrupt gate.
        self.type_attr = (IDT_PRESENT | (IDT_INTERRUPT_GATE << 8)) as u8;
        self.offset_mid = (offset >> 16) as u16;
        self.offset_high = (offset >> 32) as u32;
        self.reserved = 0;
        let _ = has_err;
    }
}

#[repr(C, align(8))]
struct IdtPtr {
    limit: u16,
    base: u64,
}

static mut IDT: [IdtEntry; IDT_ENTRIES] = [IdtEntry::null(); IDT_ENTRIES];

/// Tick counter incremented by the timer interrupt handler.
#[no_mangle]
static mut TICKS: u32 = 0;

/// PIC command/data ports.
const PIC1_CMD: u16 = 0x20;
const PIC1_DATA: u16 = 0x21;
const PIC2_CMD: u16 = 0xA0;
const PIC2_DATA: u16 = 0xA1;
const PIC_EOI: u8 = 0x20;

fn pic_write(port: u16, value: u8) {
    unsafe { core::arch::asm!("out {0}, {1}", in(dx) port, in(al) value, options(nostack)) }
}

fn pic_read(port: u16) -> u8 {
    let v: u8;
    unsafe { core::arch::asm!("in {0}, {1}", out(al) v, in(dx) port, options(nostack)) }
    v
}

/// Remap the PIC to IRQs 32-47 and mask all interrupts.
fn pic_remap() {
    let a1 = pic_read(PIC1_DATA);
    let b1 = pic_read(PIC2_DATA);
    pic_write(PIC1_CMD, 0x11); // ICW1: init, ICW4 needed
    pic_write(PIC2_CMD, 0x11);
    pic_write(PIC1_DATA, 32); // ICW2: IRQ0 -> vector 32
    pic_write(PIC2_DATA, 40); // ICW2: IRQ8 -> vector 40
    pic_write(PIC1_DATA, 4); // ICW3: slave PIC at IRQ2
    pic_write(PIC2_DATA, 2); // ICW3: cascade identity
    pic_write(PIC1_DATA, 0x01); // ICW4: 8086 mode
    pic_write(PIC2_DATA, 0x01);
    pic_write(PIC1_DATA, a1);
    pic_write(PIC2_DATA, b1);
    pic_write(PIC1_DATA, 0xFF); // mask all
    pic_write(PIC2_DATA, 0xFF);
}

/// Send EOI to the PIC(s).
fn pic_eoi(irq: u8) {
    if irq >= 8 {
        pic_write(PIC2_CMD, PIC_EOI);
    }
    pic_write(PIC1_CMD, PIC_EOI);
}

extern "C" {
    // Assembly ISR stubs.  One per exception vector plus the timer stub and a
    // shared unexpected-interrupt stub.
    fn isr_0();
    fn isr_1();
    fn isr_2();
    fn isr_3();
    fn isr_4();
    fn isr_5();
    fn isr_6();
    fn isr_7();
    fn isr_8();
    fn isr_9();
    fn isr_10();
    fn isr_11();
    fn isr_12();
    fn isr_13();
    fn isr_14();
    fn isr_15();
    fn isr_16();
    fn isr_17();
    fn isr_18();
    fn isr_19();
    fn isr_20();
    fn isr_21();
    fn isr_22();
    fn isr_23();
    fn isr_24();
    fn isr_25();
    fn isr_26();
    fn isr_27();
    fn isr_28();
    fn isr_29();
    fn isr_30();
    fn isr_31();
    fn isr_timer(); // vector 32
    fn isr_unexpected(); // catch-all
}

const ISRS: [*const (); 32] = [
    isr_0 as *const (),
    isr_1 as *const (),
    isr_2 as *const (),
    isr_3 as *const (),
    isr_4 as *const (),
    isr_5 as *const (),
    isr_6 as *const (),
    isr_7 as *const (),
    isr_8 as *const (),
    isr_9 as *const (),
    isr_10 as *const (),
    isr_11 as *const (),
    isr_12 as *const (),
    isr_13 as *const (),
    isr_14 as *const (),
    isr_15 as *const (),
    isr_16 as *const (),
    isr_17 as *const (),
    isr_18 as *const (),
    isr_19 as *const (),
    isr_20 as *const (),
    isr_21 as *const (),
    isr_22 as *const (),
    isr_23 as *const (),
    isr_24 as *const (),
    isr_25 as *const (),
    isr_26 as *const (),
    isr_27 as *const (),
    isr_28 as *const (),
    isr_29 as *const (),
    isr_30 as *const (),
    isr_31 as *const (),
];

/// The Rust interrupt/exception handler.  Called from the assembly stubs with
/// `rdi = vector`, `rsi = error_code` (0 when none).  Returns `()`; the stub
/// restores registers and executes `iretq`.  A fatal trap does not return.
extern "C" fn isr_handler(vector: u64, _error_code: u64) {
    let vector = vector as u8;
    if vector == 32 {
        // Timer tick: acknowledge and count.
        unsafe {
            TICKS = TICKS.wrapping_add(1);
        }
        pic_eoi(0);
        return;
    }
    crate::print_str("trap: vector=");
    crate::print_hex8(vector);
    crate::println("");
    // Fatal trap: park.  Do not iretq — disable interrupts and halt.
    unsafe {
        core::arch::asm!("cli", options(nostack));
    }
    loop {
        unsafe {
            core::arch::asm!("hlt", options(nostack));
        }
    }
}

/// Install the IDT, remap the PIC and enable the timer interrupt.
pub unsafe fn init() {
    pic_remap();

    // Exception vectors 0-31.
    for i in 0..32 {
        IDT[i].set(
            ISRS[i] as u64,
            super::gdt::KERNEL_CODE_SEL,
            has_error_code(i as u8),
        );
    }
    // Timer IRQ0 -> vector 32.
    IDT[32].set(isr_timer as u64, super::gdt::KERNEL_CODE_SEL, false);
    // Everything else: unexpected.
    for i in 33..IDT_ENTRIES {
        IDT[i].set(isr_unexpected as u64, super::gdt::KERNEL_CODE_SEL, false);
    }

    let idt_ptr = IdtPtr {
        limit: (core::mem::size_of::<[IdtEntry; IDT_ENTRIES]>() - 1) as u16,
        base: IDT.as_ptr() as u64,
    };
    core::arch::asm!("lidt [{0}]", in(reg) &idt_ptr, options(nostack));

    // Unmask the timer interrupt (IRQ0 = bit 0 of PIC1 data).
    let mut mask = pic_read(PIC1_DATA);
    mask &= !0x01;
    pic_write(PIC1_DATA, mask);

    // Enable interrupts.
    core::arch::asm!("sti", options(nostack));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_code_vectors_are_known() {
        assert!(has_error_code(14)); // page fault
        assert!(!has_error_code(13)); // wait, 13 is general protection
        assert!(has_error_code(13));
        assert!(!has_error_code(0)); // divide by zero
    }
}
