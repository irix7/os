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
#![cfg_attr(not(test), no_std)]
#![feature(asm, asm_experimental_arch)]

extern crate alloc;

use core::panic::PanicInfo;
use core::ptr::{read_volatile, write_volatile};

pub mod allocator;
pub mod capability;
pub mod diagnostics;
pub mod dns;
pub mod font;
pub mod frames;
pub mod gfx;
pub mod heap;
pub mod http;
pub mod idle;
pub mod kbd;
pub mod menu;
pub mod net;
pub mod ntp;
pub mod scheduler;
pub mod sha256;
pub mod telnet;
pub mod tlb;
pub mod tls;
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

/// kseg0 offset for MIPS physical-to-virtual address translation.
const KSEG0_OFFSET: u64 = 0x8000_0000;

/// End of the low physical memory reserved for the ROM window alias, the
/// exception vector, the ROM's live state, the kernel image and its stack.
/// The frame allocator must never hand any of it out.
const RESERVED_END: u64 = 0x08B0_0000;

fn wr(reg: u8, data: u8) {
    unsafe {
        write_volatile(Z85C30_BASE, reg);
        write_volatile(Z85C30_BASE, data);
    }
}

fn rd(reg: u8) -> u8 {
    unsafe {
        write_volatile(Z85C30_BASE, reg);
        read_volatile(Z85C30_BASE)
    }
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
const WR0_DATA: u8 = 8;

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
        write_volatile(Z85C30_BASE, WR0_DATA);
        write_volatile(Z85C30_BASE, b);
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

// ---------------------------------------------------------------------------
// Console output: serial + telnet
// ---------------------------------------------------------------------------

pub fn console_write(s: &str) {
    write_str(s);
    crate::telnet::write_output(s.as_bytes());
}

pub fn console_write_str(s: &str) {
    console_write(s);
}

pub fn console_println(s: &str) {
    console_write(s);
    write_byte(b'\r');
    write_byte(b'\n');
}

// ---------------------------------------------------------------------------
// BootInfo fields (offsets from boot-protocol.md §3).
const BOOTINFO_MAGIC: u32 = 0x49374249; // "I7BI" big-endian
const BOOTINFO_SIZE: usize = 256;

const BOOTINFO_MEM_MAP_PHYS: usize = 0x10;
const BOOTINFO_MEM_MAP_COUNT: usize = 0x18;
const BOOTINFO_MEM_MAP_ENTRY_SIZE: usize = 0x1C;
const BOOTINFO_CONSOLE_POLL: usize = 0xF0;
const BOOTINFO_NET_STATE: usize = 0xF4;
const BOOTINFO_SEEQ_STATE: usize = 0xF8;

// Memory descriptor kind values (matching the ROM).
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
fn read_u32(addr: u32) -> u32 {
    unsafe { read_volatile(addr as *const u32) }
}

/// Read BootInfo from the physical address passed in $a0.
fn read_bootinfo(phys_addr: u32) -> Option<&'static [u8; BOOTINFO_SIZE]> {
    let kseg0_addr = phys_addr.wrapping_add(0x8000_0000);
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
        info[0x10], info[0x11], info[0x12], info[0x13], info[0x14], info[0x15], info[0x16],
        info[0x17],
    ]);
    let count = u32::from_le_bytes([info[0x18], info[0x19], info[0x1A], info[0x1B]]) as usize;
    let entry_size = u32::from_le_bytes([info[0x1C], info[0x1D], info[0x1E], info[0x1F]]) as usize;

    let mut descs = [BootMemDesc {
        base: 0,
        size: 0,
        kind: 0,
        flags: 0,
    }; 64];

    let kseg0_map = (map_phys as u32).wrapping_add(0x8000_0000) as usize;
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

        descs[i] = BootMemDesc {
            base,
            size,
            kind,
            flags,
        };
    }

    (descs[0], count.min(64))
}

/// Read the resident console-poll function pointer from BootInfo.
///
/// The ROM publishes the address of its polled console routine at offset
/// 0xF0 so the kernel can drive the same terminal after handoff.  A zero
/// address means the ROM did not provide one.
fn read_console_poll(info: &[u8; BOOTINFO_SIZE]) -> Option<extern "C" fn(*mut ())> {
    let addr = u32::from_le_bytes([
        info[BOOTINFO_CONSOLE_POLL],
        info[BOOTINFO_CONSOLE_POLL + 1],
        info[BOOTINFO_CONSOLE_POLL + 2],
        info[BOOTINFO_CONSOLE_POLL + 3],
    ]);
    if addr == 0 {
        return None;
    }
    // SAFETY: the ROM populated this field with the address of a routine that
    // is resident in PROM for the lifetime of the machine.
    Some(unsafe { core::mem::transmute::<u32, extern "C" fn(*mut ())>(addr) })
}

/// Kernel entry point.
///
/// Handoff from the ROM (all 32-bit: the n32 ABI sign-extends pointers, and
/// a u64 in a register pair would leave the high half undefined):
///   $a0 = BootInfo physical address (0 when the ROM has none)
///   $a1 = total RAM bytes
///   $a2 = RAM base physical address
/// Linker-provided bounds of the kernel's .bss section.
#[cfg(not(test))]
extern "C" {
    static __bss_start: u8;
    static __bss_end: u8;
}

/// Zero the .bss section.
///
/// The ROM's RAM test scribbles over all of DRAM before the kernel image is
/// copied in, so the kernel's .bss arrives full of the test pattern.  Every
/// static that expects a zero initial value -- the console's own state, the
/// frame allocator's tables -- has to be cleared here.
#[cfg(not(test))]
fn clear_bss() {
    unsafe {
        let mut p = &__bss_start as *const u8 as *mut u32;
        let end = &__bss_end as *const u8 as *mut u32;
        while p < end {
            p.write_volatile(0);
            p = p.add(1);
        }
    }
}

#[cfg_attr(not(test), no_mangle)]
pub extern "C" fn _start() -> ! {
    clear_bss();

    // Capture the ROM handoff FIRST: init() below clobbers the argument
    // registers, so reading $a0/$a1/$a2 afterwards would pick up whatever
    // the serial driver left behind.
    let bootinfo_phys: u32;
    let ram_bytes: u32;
    let ram_base: u32;
    unsafe {
        core::arch::asm!(
            "move {0}, $a0",
            "move {1}, $a1",
            "move {2}, $a2",
            out(reg) bootinfo_phys,
            out(reg) ram_bytes,
            out(reg) ram_base,
        );
    }

    init();

    traps::init();
    traps::install_vectors();
    tlb::init();

    let info = read_bootinfo(bootinfo_phys);

    // Read memory map and pass usable RAM descriptors to the frame allocator.
    let mut usable_descs = [frames::MemDesc {
        base: 0,
        size: 0,
        kind: 0,
        flags: 0,
    }; 64];
    let mut usable_count = 0;

    match info {
        Some(info) => {
            console_println("kernel: BootInfo verified");
            let (_first_desc, count) = read_mem_map(info);
            let kseg0_map = u64::from_le_bytes([
                info[0x10], info[0x11], info[0x12], info[0x13], info[0x14], info[0x15], info[0x16],
                info[0x17],
            ]) + KSEG0_OFFSET;
            let entry_size =
                u32::from_le_bytes([info[0x1C], info[0x1D], info[0x1E], info[0x1F]]) as usize;
            for i in 0..count.min(64) {
                let off = (i * entry_size) as usize;
                let kind = u32::from_le_bytes([
                    info[kseg0_map as usize + off + 16],
                    info[kseg0_map as usize + off + 17],
                    info[kseg0_map as usize + off + 18],
                    info[kseg0_map as usize + off + 19],
                ]);
                if kind == MEM_KIND_USABLE_RAM {
                    let mut base = u64::from_le_bytes([
                        info[kseg0_map as usize + off],
                        info[kseg0_map as usize + off + 1],
                        info[kseg0_map as usize + off + 2],
                        info[kseg0_map as usize + off + 3],
                        info[kseg0_map as usize + off + 4],
                        info[kseg0_map as usize + off + 5],
                        info[kseg0_map as usize + off + 6],
                        info[kseg0_map as usize + off + 7],
                    ]);
                    let mut size = u64::from_le_bytes([
                        info[kseg0_map as usize + off + 8],
                        info[kseg0_map as usize + off + 9],
                        info[kseg0_map as usize + off + 10],
                        info[kseg0_map as usize + off + 11],
                        info[kseg0_map as usize + off + 12],
                        info[kseg0_map as usize + off + 13],
                        info[kseg0_map as usize + off + 14],
                        info[kseg0_map as usize + off + 15],
                    ]);
                    // Never hand the frame allocator memory below the
                    // reserved window, whatever the descriptor says.
                    if base < RESERVED_END {
                        let clip = RESERVED_END - base;
                        if size <= clip {
                            continue;
                        }
                        base = RESERVED_END;
                        size -= clip;
                    }
                    usable_descs[usable_count] = frames::MemDesc {
                        base,
                        size,
                        kind,
                        flags: 0,
                    };
                    usable_count += 1;
                }
            }
        }
        None => {
            // No BootInfo: fall back to the RAM geometry the ROM passed in
            // registers.  Reserve the low window: the exception vector, the
            // ROM's live state, the kernel image and its stack all live there.
            console_println("kernel: no BootInfo; using ROM handoff");
            let start = (ram_base as u64).max(RESERVED_END);
            let end = ram_base as u64 + ram_bytes as u64;
            usable_descs[0] = frames::MemDesc {
                base: start,
                size: end.saturating_sub(start),
                kind: MEM_KIND_USABLE_RAM,
                flags: 0,
            };
            usable_count = 1;
        }
    }
    frames::init(&usable_descs, usable_count);

    scheduler::init();
    capability::init();
    idle::init();

    console_println("kernel: up");

    // Bring up the display and show the boot menu.
    gfx::init();
    kbd::init();
    net::init();
    telnet::init();
    menu::draw();
    loop {
        let mut action: Option<usize> = None;
        if let Some(key) = kbd::poll_key() {
            match key {
                kbd::Key::Up => {
                    menu::move_selection(-1);
                    menu::draw();
                }
                kbd::Key::Down => {
                    menu::move_selection(1);
                    menu::draw();
                }
                kbd::Key::Enter => action = Some(menu::selected()),
                kbd::Key::Char(c) if c.is_ascii_digit() => {
                    let n = (c - b'1') as usize;
                    if n < menu::OPTIONS.len() {
                        menu::select(n);
                        action = Some(n);
                    }
                }
                _ => {}
            }
        }
        net::poll();
        telnet::poll();
        if telnet::reboot_requested() {
            reboot();
        }
        if telnet::boot_requested() {
            break;
        }
        if let Some(index) = action {
            if index == 3 {
                // "Run diagnostics": print the inventory and return to the menu.
                diagnostics::run();
                menu::draw();
                continue;
            }
            break;
        }
        idle::wait();
    }
    console_write("kernel: booting ");
    console_println(menu::OPTIONS[menu::selected()]);

    // Idle loop: poll the resident console service, network, and telnet.
    // Both the BootInfo path and the register-handoff path keep the console
    // and network alive; only the resident poll routine is conditional.
    let console_poll = info.and_then(|info| read_console_poll(info));
    idle_loop(console_poll);
}

/// Idle loop: poll the resident console service, network, and telnet.
fn idle_loop(console_poll: Option<extern "C" fn(*mut ())>) -> ! {
    loop {
        // Drive the resident console service when the ROM provided one.
        if let Some(poll) = console_poll {
            poll(core::ptr::null_mut());
        }

        // Drive the network stack and telnet console.
        net::poll();
        telnet::poll();
        if telnet::reboot_requested() {
            reboot();
        }

        // Park until the next timer tick; iris releases the host core while
        // wait is outstanding and interrupts are enabled.
        idle::wait();
    }
}

/// Restart from the PROM entry point.
fn reboot() -> ! {
    console_println("kernel: rebooting");
    // Interrupts off, then jump to the PROM reset window (kseg1).
    let status = traps::status_read();
    traps::status_write(status & !1);
    unsafe {
        core::arch::asm!(
            ".set noat",
            "jr {0}",
            "nop",
            in(reg) 0xBFC0_0000u32,
            options(noreturn)
        );
    }
}

#[cfg(not(test))]
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
