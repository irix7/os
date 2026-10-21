// SPDX-License-Identifier: GPL-3.0-only
//! IRIX 7 kernel core — privileged framework (the unsafe TCB).
//!
//! This crate provides the privileged framework for the IRIX 7 kernel:
//! traps, TLB/cache maintenance, frame allocation, IRQ/timers, MMIO/DMA
//! brokering, scheduler, capability table, and panic path.
//!
//! Built for `mips3-sgi-irix7-n32` (n32/MIPS III) with `-Z build-std`. The
//! kernel is a `no_main` MIPS binary that is never host-tested.
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

core::arch::global_asm!(include_str!("boot.S"));

// ---------------------------------------------------------------------------
// Serial console driver (Z85C30 ESCC inside the IOC2).
// The PROM console is DUART1B: HPC3_SERIAL1_CMD/DATA at kseg1
// 0xBFBD9830/0xBFBD9834.  Registers are accessed indirectly through the
// control port: write the register number, then read or write the data.
// Polled output only; interrupts stay disabled until the kernel enables them.

const Z85C30_CTRL: *mut u8 = 0xBFBD_9830 as *mut u8;

fn wr(reg: u8, data: u8) {
    unsafe {
        write_volatile(Z85C30_CTRL, reg);
        write_volatile(Z85C30_CTRL, data);
    }
}

fn rd(reg: u8) -> u8 {
    unsafe {
        write_volatile(Z85C30_CTRL, reg);
        read_volatile(Z85C30_CTRL)
    }
}

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
    wr(WR9, 0xC0); // hardware reset
    wr(WR4, 0x44); // x16 clock, 1 stop bit, no parity
    wr(WR3, 0xC0); // 8-bit Rx, receiver disabled for now
    wr(WR5, 0x28); // 8-bit Tx, RTS+DTR, transmitter disabled
    wr(WR11, 0x50); // Rx/Tx clock from BRG
    wr(WR12, BAUD_TC);
    wr(WR13, 0x00);
    wr(WR14, 0x01); // BRG enable
    wr(WR3, 0xC1); // receiver enable
    wr(WR5, 0x68); // transmitter enable
}

fn tx_ready() -> bool {
    rd(RR0) & 0x04 != 0
}

pub fn write_byte(b: u8) {
    while !tx_ready() {
        core::hint::spin_loop();
    }
    unsafe {
        write_volatile(Z85C30_CTRL, 0x08); // select WR8 (transmit data)
        write_volatile(Z85C30_CTRL, b);
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

/// Print an unsigned decimal number (no allocation).
pub fn print_dec(mut v: usize) {
    if v == 0 {
        write_byte(b'0');
        return;
    }
    let mut buf = [0u8; 20];
    let mut n = 0;
    while v > 0 {
        buf[n] = b'0' + (v % 10) as u8;
        v /= 10;
        n += 1;
    }
    while n > 0 {
        n -= 1;
        write_byte(buf[n]);
    }
}

use core::panic::PanicInfo;
use core::ptr::{read_volatile, write_volatile};

/// Resident ROM console/network poll routine, from `BootInfo` offset 0xF0.
/// It is part of the same program at a different address; calling it keeps
/// the ROM-initialised network stack and telnet session live.
static mut CONSOLE_POLL: u32 = 0;
static mut NET_STATE: u32 = 0;

/// Kernel entry point, called from `boot.S` with the physical BootInfo address.
#[no_mangle]
pub extern "C" fn kernel_main(boot_info: u32) -> ! {
    // BootInfo is a physical address; the kernel reaches it through kseg0.
    let bi = boot_info.wrapping_add(0x8000_0000) as *const u32;
    let magic = unsafe { read_volatile(bi) };
    if magic == 0x4937_4249 {
        println("kernel: bootinfo ok");
    } else {
        print_str("kernel: bootinfo magic ");
        print_hex8((magic >> 24) as u8);
        print_hex8((magic >> 16) as u8);
        print_hex8((magic >> 8) as u8);
        print_hex8(magic as u8);
        println("");
    }

    let console_poll = unsafe { read_volatile((bi as usize + 0xF0) as *const u32) };
    let net_state = unsafe { read_volatile((bi as usize + 0xF4) as *const u32) };
    unsafe {
        CONSOLE_POLL = console_poll;
        NET_STATE = net_state;
    }

    // Pull the memory map out of BootInfo (offsets 0x10 and 0x18) and give it
    // to the frame allocator as the ARCS descriptor list.
    let map_phys = unsafe { read_volatile((bi as usize + 0x10) as *const u64) } as u32;
    let map_count = unsafe { read_volatile((bi as usize + 0x18) as *const u32) } as usize;
    let count = map_count.min(64);
    let descs: &[frames::MemDesc] = if count > 0 && map_phys != 0 {
        unsafe {
            core::slice::from_raw_parts(
                map_phys.wrapping_add(0x8000_0000) as *const frames::MemDesc,
                count,
            )
        }
    } else {
        &[]
    };

    init();

    traps::init();
    traps::install_vectors();
    tlb::init();
    frames::init(descs, count);
    scheduler::init();
    capability::init();
    traps::init_timer();

    println("kernel: up");

    idle_loop();
}

fn idle_loop() -> ! {
    loop {
        let poll = unsafe { CONSOLE_POLL };
        let net = unsafe { NET_STATE };
        if poll != 0 && net != 0 {
            let service: extern "C" fn(u32) = unsafe { core::mem::transmute(poll as usize) };
            service(net);
        }
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
