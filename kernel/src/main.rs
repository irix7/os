// SPDX-License-Identifier: GPL-3.0-only
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

pub fn print_hex32(v: u32) {
    print_hex8((v >> 24) as u8);
    print_hex8((v >> 16) as u8);
    print_hex8((v >> 8) as u8);
    print_hex8(v as u8);
}

use core::panic::PanicInfo;
use core::ptr::{read_volatile, write_volatile};

// BootInfo physical address passed in $a0 by the ROM.
// kseg0 maps physical 0x0000_0000–0x1FFF_FFFF to virtual 0x8000_0000–0x9FFF_FFFF.
const KSEG0_OFFSET: u64 = 0x8000_0000;

/// BootInfo fields (offsets from boot-protocol.md §3).
const BOOTINFO_MAGIC: u32 = 0x49374249; // "I7BI" big-endian
const BOOTINFO_SIZE: usize = 256;

const BOOTINFO_MEM_MAP_PHYS: usize = 0x10;
const BOOTINFO_MEM_MAP_COUNT: usize = 0x18;
const BOOTINFO_MEM_MAP_ENTRY_SIZE: usize = 0x1C;
const BOOTINFO_CONSOLE_POLL: usize = 0xF0;
const BOOTINFO_NET_STATE: usize = 0xF4;
const BOOTINFO_SEEQ_STATE: usize = 0xF8;

/// Memory descriptor kind values (matching the ROM).
const MEM_KIND_USABLE_RAM: u32 = 0;
const MEM_KIND_RESERVED: u32 = 1;

/// A memory descriptor copied from BootInfo.
#[derive(Clone, Copy, Debug)]
struct BootMemDesc {
    base: u64,
    size: u64,
    kind: u32,
    flags: u32,
}

/// CRC32C (Castagnoli) for BootInfo checksum verification.
fn crc32c(data: &[u8]) -> u32 {
    let mut crc: u32 = 0xFFFF_FFFF;
    for &byte in data {
        crc ^= byte as u32;
        for _ in 0..8 {
            let mask = if crc & 1 == 1 { 0x82F6_3B78 } else { 0 };
            crc = (crc >> 1) ^ mask;
        }
    }
    crc ^ 0xFFFF_FFFF
}

/// Read a u32 from a kseg0 virtual address.
fn read_u32(addr: u64) -> u32 {
    unsafe { read_volatile(addr as *const u32) }
}

/// Read BootInfo from the physical address passed in $a0.
fn read_bootinfo(phys_addr: u64) -> Option<&'static [u8; BOOTINFO_SIZE]> {
    let kseg0_addr = phys_addr + KSEG0_OFFSET;
    let ptr = kseg0_addr as *const [u8; BOOTINFO_SIZE];
    let data = unsafe { &*ptr };

    // Verify magic.
    let magic = u32::from_le_bytes([data[0x00], data[0x01], data[0x02], data[0x03]]);
    if magic != BOOTINFO_MAGIC {
        return None;
    }

    // Verify size.
    let size = u32::from_le_bytes([data[0x08], data[0x09], data[0x0A], data[0x0B]]) as usize;
    if size != BOOTINFO_SIZE {
        return None;
    }

    // Verify checksum.
    let checksum = u32::from_le_bytes([data[0x0C], data[0x0D], data[0x0E], data[0x0F]]);
    let mut buf = [0u8; BOOTINFO_SIZE];
    buf.copy_from_slice(data);
    buf[0x0C..0x10].copy_from_slice(&0u32.to_le_bytes());
    let computed = crc32c(&buf);
    if computed != checksum {
        return None;
    }

    Some(data)
}

/// Read memory descriptors from BootInfo's mem_map.
fn read_mem_map(info: &[u8; BOOTINFO_SIZE]) -> (BootMemDesc, usize) {
    let map_phys = u64::from_le_bytes([
        info[0x10], info[0x11], info[0x12], info[0x13],
        info[0x14], info[0x15], info[0x16], info[0x17],
    ]);
    let count = u32::from_le_bytes([info[0x18], info[0x19], info[0x1A], info[0x1B]]) as usize;
    let entry_size = u32::from_le_bytes([info[0x1C], info[0x1D], info[0x1E], info[0x1F]]) as usize;

    let mut descs = [BootMemDesc {
        base: 0,
        size: 0,
        kind: 0,
        flags: 0,
    }; 64];

    let kseg0_map = map_phys + KSEG0_OFFSET;
    for i in 0..count.min(64) {
        let off = (i * entry_size) as usize;
        let base = u64::from_le_bytes([
            info[kseg0_map as usize + off],
            info[kseg0_map as usize + off + 1],
            info[kseg0_map as usize + off + 2],
            info[kseg0_map as usize + off + 3],
            info[kseg0_map as usize + off + 4],
            info[kseg0_map as usize + off + 5],
            info[kseg0_map as usize + off + 6],
            info[kseg0_map as usize + off + 7],
        ]);
        let size = u64::from_le_bytes([
            info[kseg0_map as usize + off + 8],
            info[kseg0_map as usize + off + 9],
            info[kseg0_map as usize + off + 10],
            info[kseg0_map as usize + off + 11],
            info[kseg0_map as usize + off + 12],
            info[kseg0_map as usize + off + 13],
            info[kseg0_map as usize + off + 14],
            info[kseg0_map as usize + off + 15],
        ]);
        let kind = u32::from_le_bytes([
            info[kseg0_map as usize + off + 16],
            info[kseg0_map as usize + off + 17],
            info[kseg0_map as usize + off + 18],
            info[kseg0_map as usize + off + 19],
        ]);
        let flags = u32::from_le_bytes([
            info[kseg0_map as usize + off + 20],
            info[kseg0_map as usize + off + 21],
            info[kseg0_map as usize + off + 22],
            info[kseg0_map as usize + off + 23],
        ]);

        descs[i] = BootMemDesc { base, size, kind, flags };
    }

    (descs[0], count.min(64))
}

/// Read console_poll function pointer from BootInfo.
fn read_console_poll(info: &[u8; BOOTINFO_SIZE]) -> extern "C" fn(*mut ()) {
    let addr = u32::from_le_bytes([
        info[0xF0], info[0xF1], info[0xF2], info[0xF3],
    ]);
    unsafe { core::mem::transmute(addr) }
}

/// Read net_state pointer from BootInfo.
fn read_net_state(info: &[u8; BOOTINFO_SIZE]) -> *mut () {
    let addr = u64::from_le_bytes([
        info[0xF4], info[0xF5], info[0xF6], info[0xF7],
        info[0xF8], info[0xF9], info[0xFA], info[0xFB],
    ]);
    addr as *mut ()
}

/// Read seeq_state pointer from BootInfo.
fn read_seeq_state(info: &[u8; BOOTINFO_SIZE]) -> *mut () {
    let addr = u64::from_le_bytes([
        info[0xF8], info[0xF9], info[0xFA], info[0xFB],
        info[0xFC], info[0xFD], info[0xFE], info[0xFF],
    ]);
    addr as *mut ()
}

/// Kernel entry point.
#[cfg_attr(not(test), no_mangle)]
pub extern "C" fn _start() -> ! {
    init();

    traps::init();
    traps::install_vectors();
    tlb::init();

    // Read BootInfo from $a0 (physical address passed by the ROM).
    let bootinfo_phys: u64;
    unsafe { core::arch::asm!("move {0}, $a0", out(reg) bootinfo_phys); }

    let info = match read_bootinfo(bootinfo_phys) {
        Some(info) => info,
        None => {
            println("kernel: bad BootInfo");
            loop { core::hint::spin_loop(); }
        }
    };

    println("kernel: BootInfo verified");

    // Read memory map and pass usable RAM descriptors to the frame allocator.
    let (_first_desc, count) = read_mem_map(info);
    let mut usable_descs = [frames::MemDesc {
        base: 0,
        size: 0,
        kind: 0,
        flags: 0,
    }; 64];
    let kseg0_map = u64::from_le_bytes([
        info[0x10], info[0x11], info[0x12], info[0x13],
        info[0x14], info[0x15], info[0x16], info[0x17],
    ]) + KSEG0_OFFSET;
    let entry_size = u32::from_le_bytes([info[0x1C], info[0x1D], info[0x1E], info[0x1F]]) as usize;
    let mut usable_count = 0;
    for i in 0..count.min(64) {
        let off = (i * entry_size) as usize;
        let kind = u32::from_le_bytes([
            info[kseg0_map as usize + off + 16],
            info[kseg0_map as usize + off + 17],
            info[kseg0_map as usize + off + 18],
            info[kseg0_map as usize + off + 19],
        ]);
        if kind == MEM_KIND_USABLE_RAM {
            let base = u64::from_le_bytes([
                info[kseg0_map as usize + off],
                info[kseg0_map as usize + off + 1],
                info[kseg0_map as usize + off + 2],
                info[kseg0_map as usize + off + 3],
                info[kseg0_map as usize + off + 4],
                info[kseg0_map as usize + off + 5],
                info[kseg0_map as usize + off + 6],
                info[kseg0_map as usize + off + 7],
            ]);
            let size = u64::from_le_bytes([
                info[kseg0_map as usize + off + 8],
                info[kseg0_map as usize + off + 9],
                info[kseg0_map as usize + off + 10],
                info[kseg0_map as usize + off + 11],
                info[kseg0_map as usize + off + 12],
                info[kseg0_map as usize + off + 13],
                info[kseg0_map as usize + off + 14],
                info[kseg0_map as usize + off + 15],
            ]);
            usable_descs[usable_count] = frames::MemDesc { base, size, kind, flags: 0 };
            usable_count += 1;
        }
    }
    frames::init(&usable_descs, usable_count);

    scheduler::init();
    capability::init();
    traps::init_timer();

    println("kernel: up");

    // Idle loop: poll the resident console service and the timer.
    let console_poll = read_console_poll(info);
    let net_state = read_net_state(info);
    idle_loop(console_poll, net_state);
}

/// Idle loop: drive the resident console/network service and yield on timer.
fn idle_loop(console_poll: extern "C" fn(*mut ()), net_state: *mut ()) -> ! {
    loop {
        // Drive the resident console/network service.
        unsafe { console_poll(net_state); }

        // Yield to the scheduler.
        scheduler::yield_cpu();

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