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
pub mod autoload;
pub mod boot_config;
pub mod cache;
pub mod capability;
pub mod diagnostics;
pub mod dns;
#[cfg(feature = "erofs-debug")]
pub mod erofs_cmd;
pub mod font;
pub mod frames;
pub mod gfx;
pub mod graphics_console;
pub mod heap;
pub mod http;
pub mod idle;
pub mod kbd;
pub mod module;
pub mod net;
pub mod ntp;
pub mod nvram;
pub mod ping;
pub mod range;
pub mod readahead;
pub mod rtc;
pub mod scheduler;
pub mod scsi;
pub mod sha256;
pub mod status;
pub mod telnet;
pub mod tlb;
pub mod tls;
pub mod traps;
pub mod user;

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
pub(crate) const KSEG0_OFFSET: u64 = 0x8000_0000;

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
        // The register-select write must reach the SCC before the read.  The
        // ROM's uncached instruction fetch gave it time; the kernel's cached
        // loop can otherwise read the previous register (RR8), whose bit 2 is
        // zero, and spin forever.
        core::arch::asm!("sync", options(nostack));
        // The IOC PIO needs a few cycles between the register select and the
        // read; without this the read can return the previous register (RR8,
        // whose bit 2 is zero) and the transmitter poll never terminates.
        for _ in 0..16 {
            core::arch::asm!("nop", options(nostack));
        }
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
    // The ROM brought the Z85C30 up at 9600 8N1 before it decompressed the
    // kernel; the kernel inherits that state.  Do not reset the SCC here: a
    // reset races with the ROM's final characters and can leave the
    // transmitter ready bit stuck (observed intermittently under iris).
}

fn tx_ready() -> bool {
    rd(RR0) & 0x04 != 0
}

/// Serial output ring, so a module (the MCP server) can read the console
/// without a second driver.  It records every byte written to the Z85C30.
const CONSOLE_LOG_SIZE: usize = 8192;
static mut CONSOLE_LOG: [u8; CONSOLE_LOG_SIZE] = [0; CONSOLE_LOG_SIZE];
static mut CONSOLE_HEAD: usize = 0;
static mut CONSOLE_TOTAL: u64 = 0;

fn console_log_byte(b: u8) {
    // SAFETY: single-threaded kernel; the ring is only touched here and in
    // console_read, which never runs concurrently.
    unsafe {
        CONSOLE_LOG[CONSOLE_HEAD] = b;
        CONSOLE_HEAD = (CONSOLE_HEAD + 1) % CONSOLE_LOG_SIZE;
        CONSOLE_TOTAL = CONSOLE_TOTAL.wrapping_add(1);
    }
}

/// Total bytes ever written to the serial console.
pub fn console_total() -> u64 {
    // SAFETY: single-threaded.
    unsafe { CONSOLE_TOTAL }
}

/// Copy serial output starting at absolute `offset` into `dst`.
///
/// Offsets older than the ring are clamped to the oldest retained byte.
pub fn console_read(offset: u64, dst: &mut [u8]) -> usize {
    // SAFETY: single-threaded; `offset` is bounded below.
    unsafe {
        let total = CONSOLE_TOTAL;
        let size = CONSOLE_LOG_SIZE as u64;
        let oldest = total.saturating_sub(size);
        let start = offset.max(oldest);
        if start >= total {
            return 0;
        }
        let count = ((total - start) as usize).min(dst.len());
        for (i, slot) in dst.iter_mut().take(count).enumerate() {
            *slot = CONSOLE_LOG[((start + i as u64) % size) as usize];
        }
        count
    }
}

/// Write bytes to the serial sink and queue the same bytes for telnet.
///
/// This is the one console service.  The serial sink also records the MCP
/// telemetry ring; telnet output is queued so a print from inside the socket
/// service cannot re-enter the socket set.
pub fn write_bytes(data: &[u8]) {
    for &byte in data {
        graphics_console::write_byte(byte);
        console_log_byte(byte);
        serial_byte(byte);
    }
    crate::telnet::write_output(data);
}

fn serial_byte(b: u8) {
    // Wait for the transmitter, but never forever: the IOC can return a
    // stale register and the CP0 Count is not yet guaranteed to run this
    // early.  A bounded spin followed by the write is safe at 9600 baud.
    let mut spins = 0u32;
    while !tx_ready() {
        spins += 1;
        if spins >= 200_000 {
            break;
        }
        core::hint::spin_loop();
    }
    unsafe {
        write_volatile(Z85C30_BASE, WR0_DATA);
        write_volatile(Z85C30_BASE, b);
        core::arch::asm!("sync", options(nostack));
    }
    // Pace the next byte so the four-deep transmitter FIFO cannot overflow
    // even when the status read is stale.
    for _ in 0..20_000 {
        core::hint::spin_loop();
    }
}

pub fn write_str(s: &str) {
    write_bytes(s.as_bytes());
}

pub fn write_byte(b: u8) {
    write_bytes(&[b]);
}

pub fn print_str(s: &str) {
    write_bytes(s.as_bytes());
}

pub fn println(s: &str) {
    write_bytes(s.as_bytes());
    write_bytes(b"\r\n");
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
}

pub fn console_write_str(s: &str) {
    console_write(s);
}

pub fn console_println(s: &str) {
    println(s);
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

/// The ROM handoff, captured by the assembly entry stub before any Rust
/// code can clobber `$a0`-`$a2`.
#[repr(C)]
struct BootHandoff {
    bootinfo_phys: u32,
    ram_bytes: u32,
    ram_base: u32,
}

// Non-zero initial values keep this in `.data`, which `clear_bss` does not
// touch.  The assembly stub overwrites all three words on entry.
#[no_mangle]
static mut BOOT_HANDOFF: BootHandoff = BootHandoff {
    bootinfo_phys: 0xFFFF_FFFF,
    ram_bytes: 0xFFFF_FFFF,
    ram_base: 0xFFFF_FFFF,
};

// The C ABI's argument registers are not visible to the Rust compiler through
// an `asm!` block, so capture them in a tiny real entry stub.  The earlier
// inline-asm capture could be scheduled after `$a0` had been reused as a
// string pointer, which handed `read_bootinfo` a rodata address.
#[cfg(not(test))]
core::arch::global_asm!(
    r#"
    .section .text._start, "ax", @progbits
    .globl _start
    .type _start, @function
_start:
    lui     $t0, %hi(BOOT_HANDOFF)
    addiu   $t0, $t0, %lo(BOOT_HANDOFF)
    sw      $a0, 0($t0)
    sw      $a1, 4($t0)
    sw      $a2, 8($t0)
    j       _start_rust
    nop
    .size _start, .-_start
"#
);

/// Rust entry point, called by the assembly stub above.
///
/// Handoff registers:
///   $a0 = BootInfo physical address (0 when the ROM has none)
///   $a1 = total RAM bytes
///   $a2 = RAM base physical address
#[cfg_attr(not(test), no_mangle)]
pub extern "C" fn _start_rust() -> ! {
    clear_bss();

    // SAFETY: the assembly stub wrote all three words before entering Rust.
    let handoff = core::ptr::addr_of!(BOOT_HANDOFF);
    let bootinfo_phys =
        unsafe { core::ptr::read_volatile(core::ptr::addr_of!((*handoff).bootinfo_phys)) };
    let ram_bytes = unsafe { core::ptr::read_volatile(core::ptr::addr_of!((*handoff).ram_bytes)) };
    let ram_base = unsafe { core::ptr::read_volatile(core::ptr::addr_of!((*handoff).ram_base)) };

    init();

    traps::init();
    traps::install_vectors();
    tlb::init();
    nvram::init();

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

    // Bring up the display and the local text console.
    gfx::init();
    graphics_console::init();
    kbd::init();
    net::init();
    telnet::init();

    // The DS1386 keeps the time across a power cycle; seed the wall clock
    // before any NTP query so status reports a real date.
    match rtc::read_unix() {
        Some(unix) => {
            ntp::set_unix(unix);
            let mut buf = [0u8; 20];
            let n = ntp::format_utc(unix, &mut buf);
            console_write("rtc: ");
            for &b in &buf[..n] {
                write_byte(b);
            }
            console_println(" UTC");
        }
        None => console_println("rtc: no valid time"),
    }

    // Boot-time module configuration from the PROM environment.
    let autoload = autoload::boot();
    announce_boot(autoload);

    // The boot log is the local display now.  Paint it before handing over so
    // the last kernel messages are visible even if userland never returns.
    start_userland();

    // Userland returned (normally after exit, or because target 2 had no
    // loadable image).  Keep the console, network and module services alive.
    // Both the BootInfo path and the register-handoff path keep the console
    // and network alive; only the resident poll routine is conditional.
    let console_poll = info.and_then(|info| read_console_poll(info));
    idle_loop(console_poll);
}

/// Hand over to the native user image on SCSI target 2.
///
/// This is the same one-way entry used by the telnet `userelf` command.  It
/// saves the kernel console context, enters user mode, and returns here only
/// on a load failure or after the user process exits.
fn start_userland() {
    console_println("kernel: starting userland from SCSI target 2");
    graphics_console::flush();

    // SAFETY: single-threaded kernel; the trampoline saves and restores the
    // caller's $sp/$ra and callee-saved registers around the one-way entry.
    unsafe {
        traps::user_from_console_elf();
    }

    user::cleanup();
    console_println("kernel: userland returned to kernel console");
    graphics_console::flush();
}

/// Idle loop: poll the resident console service, network, and telnet.
fn idle_loop(console_poll: Option<extern "C" fn(*mut ())>) -> ! {
    loop {
        // Drive the resident console service when the ROM provided one.
        if let Some(poll) = console_poll {
            poll(core::ptr::null_mut());
        }

        // A telnet `boot` retries the target-2 userland image.
        if telnet::boot_requested() {
            start_userland();
        }

        // Drive the network stack, module callbacks and telnet console.
        graphics_console::flush();
        net::poll();
        module::poll();
        telnet::poll();
        if telnet::reboot_requested() {
            reboot();
        }

        // Park until the next timer tick; iris releases the host core while
        // wait is outstanding and interrupts are enabled.
        idle::wait();
    }
}

/// Emit the single machine-readable boot readiness marker.
///
/// The serial ring and the telnet console both receive this line.  Tests must
/// poll for this marker rather than incidental boot messages.  `boot` is the
/// RTC Unix second at the marker (or zero when the RTC is unset); it is
/// monotonic across normal reboots but can repeat if a machine is reset
/// within the same second.
fn announce_boot(autoload: autoload::Status) {
    let boot_id = crate::ntp::now_unix().unwrap_or(0);
    let network_up = crate::net::ip_addr().is_some();
    let ready = autoload == autoload::Status::Loaded && network_up;
    console_write(if ready {
        "BOOT-READY v=1 boot="
    } else {
        "BOOT-NOT-READY v=1 boot="
    });
    write_u64_dec(boot_id);
    console_write(" kernel=up network=");
    console_write(if network_up { "up" } else { "down" });
    console_write(" mcp=");
    match autoload {
        autoload::Status::Loaded => console_write("listening"),
        autoload::Status::Disabled => console_write("disabled"),
        autoload::Status::Failed => console_write("failed"),
    }
    if ready {
        console_println("");
    } else {
        console_write(" reason=");
        console_println(if !network_up {
            "network-down"
        } else if autoload == autoload::Status::Disabled {
            "no-mcp-url"
        } else {
            "autoload-failed"
        });
    }
}

fn write_u64_dec(mut value: u64) {
    let mut digits = [0u8; 20];
    let mut count = 0usize;
    if value == 0 {
        write_byte(b'0');
        return;
    }
    while value > 0 && count < digits.len() {
        digits[count] = b'0' + (value % 10) as u8;
        value /= 10;
        count += 1;
    }
    while count > 0 {
        count -= 1;
        write_byte(digits[count]);
    }
}

/// Halt the machine.  Interrupts are disabled and the CPU waits.
pub fn poweroff() -> ! {
    console_println("kernel: powered off");
    unsafe {
        let status = traps::status_read();
        traps::status_write(status & !1);
    }
    loop {
        unsafe {
            core::arch::asm!("wait", options(nomem, nostack));
        }
    }
}

/// Restart from the PROM entry point.
pub fn reboot() -> ! {
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
fn panic(info: &PanicInfo) -> ! {
    use core::fmt::Write as _;

    struct Console;
    impl core::fmt::Write for Console {
        fn write_str(&mut self, text: &str) -> core::fmt::Result {
            write_str(text);
            Ok(())
        }
    }

    let mut out = Console;
    let _ = writeln!(out, "kernel panic: {info}");

    let cause = traps::cause_read();
    let epc = traps::epc_read();
    let status = traps::status_read();
    let badvaddr = traps::badvaddr_read();
    let _ = writeln!(
        out,
        "  cause=0x{cause:08x} epc=0x{epc:08x} status=0x{status:08x} badvaddr=0x{badvaddr:08x}"
    );

    let sp: usize;
    let ra: usize;
    // SAFETY: reading $sp and $ra is side-effect free.  The values are only
    // dereferenced below after a range and alignment check.
    unsafe {
        core::arch::asm!("addiu {0}, $sp, 0", out(reg) sp);
        core::arch::asm!("addiu {0}, $ra, 0", out(reg) ra);
    }
    let _ = writeln!(out, "  sp=0x{sp:08x} ra=0x{ra:08x}");
    if (0x8000_0000..0x9000_0000).contains(&sp) && sp % 4 == 0 {
        let _ = write!(out, "  stack:");
        for index in 0..8 {
            let address = sp + index * 4;
            // SAFETY: the range check above keeps this in the kernel's direct
            // mapped window; a corrupted but in-range stack may still fault,
            // so the dump is deliberately short.
            let word = unsafe { core::ptr::read_volatile(address as *const u32) };
            let _ = write!(out, " {word:08x}");
        }
        let _ = writeln!(out);
    }

    let _ = traps::status_write(status & !1);
    loop {
        // SAFETY: interrupts are disabled; `wait` is a parking instruction.
        unsafe {
            core::arch::asm!("wait", options(nomem, nostack));
        }
    }
}

#[cfg(test)]
mod tests {
    // Kernel tests run on the host; no hardware access.
}
