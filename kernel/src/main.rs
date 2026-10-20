//! IRIX 7 kernel core — privileged framework (the unsafe TCB).
//!
//! This crate provides the privileged framework for the IRIX 7 kernel:
//! traps, TLB/cache maintenance, frame allocation, IRQ/timers, MMIO/DMA
//! brokering, scheduler, capability table, and panic path.
//!
//! Built for `mips2-sgi-irix7-o32` (o32/MIPS II) and
//! `mips3-sgi-irix7-n32` (n32/MIPS III) with `-Z build-std`. The kernel is a
//! `no_main` MIPS binary that is never host-tested.
//!
//! The kernel occupies kseg0 (unmapped, cached) and executes in kernel mode.
//! Legacy IRIX processes occupy kuseg in their own MMU address spaces.

#![cfg_attr(not(test), no_main)]
#![no_std]
#![feature(asm, asm_experimental_arch)]

pub mod allocator;
pub mod capability;
pub mod frames;
pub mod scheduler;
pub mod tlb;
pub mod traps;

// ---------------------------------------------------------------------------
// Serial console driver (Z85C30 ESCC inside the IOC2).
// Hardware facts (see docs/research/indy-hardware.md §15.3):
// - Z85C30 ESCC base physical 0x1FBD_9830, mapped at kseg1 0xBFBD_9830.
// - Indirect register access: write register number to `(reg << 2) + 3`,
//   then data byte to same address (byte lane 3).
// - PCLK = 3.672 MHz; PROM default is 9600 8N1.
// - Polled output only; interrupts left disabled.

const Z85C30_BASE: *mut u8 = 0xBFBD_9830 as *mut u8;

fn wr(reg: u8, data: u8) {
    unsafe {
        write_volatile(Z85C30_BASE.add(((reg as usize) << 2) + 3), data);
    }
}

fn rd(reg: u8) -> u8 {
    unsafe { read_volatile(Z85C30_BASE.add(((reg as usize) << 2) + 3)) }
}

const WR0: u8 = 0;
const WR1: u8 = 1;
const WR3: u8 = 3;
const WR4: u8 = 4;
const WR5: u8 = 5;
const WR9: u8 = 9;
const WR11: u8 = 11;
const WR12: u8 = 12;
const WR13: u8 = 13;
const WR14: u8 = 14;
const RR0: u8 = 0;

const BAUD_TC: u8 = 10;

pub fn init() {
    wr(WR9, 0xB0);
    wr(WR4, 0x44);
    wr(WR11, 0x50);
    wr(WR12, BAUD_TC);
    wr(WR13, 0x00);
    wr(WR14, 0x01);
    wr(WR3, 0xC1);
    wr(WR5, 0x68);
    wr(WR1, 0x00);
    wr(WR9, 0x08);
}

fn tx_ready() -> bool {
    rd(RR0) & 0x04 != 0
}

pub fn write_byte(b: u8) {
    while !tx_ready() {
        core::hint::spin_loop();
    }
    unsafe {
        write_volatile(Z85C30_BASE.add(3), b);
    }
}

pub fn write_str(s: &str) {
    for b in s.bytes() {
        write_byte(b);
    }
}

pub fn print_str(s: &str) {
    write_str(s);
}

pub fn println(s: &str) {
    write_str(s);
    write_byte(b'\r');
    write_byte(b'\n');
}

pub fn print_hex8(v: u8) {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    write_byte(HEX[(v >> 4) as usize]);
    write_byte(HEX[(v & 0x0F) as usize]);
}

use core::panic::PanicInfo;
use core::ptr::{read_volatile, write_volatile};

/// Kernel entry point.
#[cfg_attr(not(test), no_mangle)]
pub extern "C" fn _start() -> ! {
    init();

    traps::init();
    traps::install_vectors();
    tlb::init();
    frames::init(&[], 0);
    scheduler::init();
    capability::init();
    traps::init_timer();

    println("kernel: up");

    idle_loop();
}

fn idle_loop() -> ! {
    loop {
        core::hint::spin_loop();
    }
}

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    println("kernel panic");
    loop {
        core::hint::spin_loop();
    }
}

#[cfg(test)]
mod tests {
    // Kernel tests run on the host; no hardware access.
}

