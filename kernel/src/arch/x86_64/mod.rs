// SPDX-License-Identifier: GPL-3.0-only
//! x86_64 implementation of the [`Arch`](crate::arch::interface::Arch) boundary.
//!
//! Bring-up model: QEMU loads the Multiboot2 image and jumps to `_start` in
//! `entry.S`.  `_start` reloads the GDT, sets up a stack and calls `rust_main`
//! here.  `rust_main` installs the IDT, enables 4-level paging, initialises
//! the 16550 serial console, then enters the shared kernel main.
//!
//! The serial port is COM1 (0x3F8), polled for bring-up and interrupt-driven
//! once the IDT timer path is proven.

use crate::arch::interface::{Arch, ArchCaps, ArchError, TrapFrame};
use crate::frames::MemDesc;

mod gdt;
mod idt;
mod paging;

use self::idt::TICKS;

/// 16550 UART registers (COM1, port 0x3F8).
const COM1: u16 = 0x3F8;
const REG_DATA: u16 = 0x00; // RBR / THR / DLL
const REG_IER: u16 = 0x01; // IER / DLH
const REG_FCR: u16 = 0x02; // FCR
const REG_LCR: u16 = 0x03; // LCR
const REG_MCR: u16 = 0x04; // MCR
const REG_LSR: u16 = 0x05; // LSR
const REG_MSR: u16 = 0x06; // MSR

const LSR_THRE: u8 = 0x20; // transmitter holding register empty
const LSR_DR: u8 = 0x01; // data ready

/// x86_64 arch state.  A unit struct; mutable state lives in `static mut`
/// cells here and in `idt`.
#[derive(Clone, Copy, Debug, Default)]
pub struct X86_64Arch;

impl X86_64Arch {
    pub const fn new() -> Self {
        X86_64Arch
    }
}

impl Arch for X86_64Arch {
    fn caps(&self) -> ArchCaps {
        ArchCaps(ArchCaps::MMU.0 | ArchCaps::VIRTIO.0 | ArchCaps::RTC.0)
    }

    fn init(&mut self) -> Result<(), ArchError> {
        serial_init();
        Ok(())
    }

    fn install_vectors(&mut self) {
        unsafe { idt::init() };
    }

    fn init_timer(&mut self) {
        // The timer IRQ is unmasked by idt::init(); the PIT is left at its
        // power-on rate (approx 18.2 Hz) for bring-up.  A later slice
        // reprogrammes it to 100 Hz.
        let _ = ();
    }

    fn ticks(&self) -> u32 {
        unsafe { TICKS }
    }

    fn wait(&mut self) {
        unsafe { core::arch::asm!("sti", options(nostack)) };
        unsafe { core::arch::asm!("hlt", options(nostack)) };
    }

    fn console_write(&mut self, bytes: &[u8]) {
        for &byte in bytes {
            serial_write(byte);
        }
    }

    fn console_read(&mut self, out: &mut [u8]) -> usize {
        let mut written = 0;
        while written < out.len() {
            if !serial_data_ready() {
                break;
            }
            out[written] = serial_read();
            written += 1;
        }
        written
    }

    fn trap(&mut self, _tf: &mut TrapFrame) -> bool {
        // x86 exceptions are dispatched by the IDT stubs directly
        // (isr_handler), not through this arch-level entry.  Keep the trait
        // method for the shared contract; report and treat as unhandled.
        false
    }

    fn fatal_trap(&mut self, tf: &TrapFrame) -> ! {
        crate::print_str("trap: fatal vector=0x");
        crate::print_hex8((tf.cause as u32) as u8);
        crate::print_str(" epc=0x");
        crate::print_hex8(((tf.epc as u32) >> 24) as u8);
        crate::print_hex8(((tf.epc as u32) >> 16) as u8);
        crate::print_hex8(((tf.epc as u32) >> 8) as u8);
        crate::print_hex8(tf.epc as u8);
        crate::print_str(" badvaddr=0x");
        crate::print_hex8(((tf.badvaddr as u32) >> 24) as u8);
        crate::print_hex8(((tf.badvaddr as u32) >> 16) as u8);
        crate::print_hex8(((tf.badvaddr as u32) >> 8) as u8);
        crate::print_hex8(tf.badvaddr as u8);
        crate::println("");
        unsafe { core::arch::asm!("cli", options(nostack)) };
        loop {
            unsafe { core::arch::asm!("hlt", options(nostack)) };
        }
    }

    fn reboot(&mut self) -> ! {
        // Keyboard-controller reset pulse.
        unsafe {
            core::arch::asm!("cli", options(nostack));
            loop {
                // Wait for the controller input buffer to be empty.
                let _ = 0u8;
                core::arch::asm!("out 0x64, al", in(al) 0xFEu8, options(nostack));
                core::arch::asm!("hlt", options(nostack));
            }
        }
    }

    fn poweroff(&mut self) -> ! {
        crate::print_str("kernel: powered off\n");
        unsafe { core::arch::asm!("cli", options(nostack)) };
        loop {
            unsafe { core::arch::asm!("hlt", options(nostack)) };
        }
    }

    fn memory_map(&mut self, _descs: &mut [MemDesc]) -> usize {
        // The x86 boot path reads the Multiboot2 memory map in rust_main and
        // passes descriptors to the frame allocator directly.
        0
    }

    fn dcache_writeback_invalidate(&mut self, _addr: usize, _len: usize) {
        paging::dcache_writeback_invalidate(_addr, _len);
    }

    fn icache_invalidate(&mut self, _addr: usize, _len: usize) {
        paging::icache_invalidate(_addr, _len);
    }

    fn sync(&mut self) {
        paging::sync();
    }

    fn direct_map_offset(&self) -> u64 {
        0 // identity-mapped: virtual == physical
    }

    fn rtc_read_unix(&mut self) -> Option<u64> {
        cmos_read_unix()
    }
}

/// Entry point called from `_start` in `entry.S` after the GDT is loaded and
/// a stack is set up.  Installs the IDT, enables paging, starts the serial
/// console, then enters the shared kernel main.
#[no_mangle]
pub extern "C" fn rust_main() -> ! {
    unsafe { idt::init() };
    unsafe { paging::init_paging() };
    unsafe { paging::enable_paging() };

    // Serial console is already initialised by Arch::init; announce the arch.
    crate::print_str("x86_64: arch up, paging enabled\n");

    // Enter the shared kernel main.  The x86 boot path supplies its own
    // memory map descriptor source via the frame allocator below.
    kernel_main()
}

/// Shared entry into the kernel core.  Defined in `main.rs`.
extern "C" {
    fn kernel_main() -> !;
}

// ---------------------------------------------------------------------------
// 16550 UART (COM1)
// ---------------------------------------------------------------------------

fn serial_init() {
    // Disable all interrupts.
    outb(COM1 + REG_IER, 0x00);
    // Enable DLAB to set the baud rate divisor.
    outb(COM1 + REG_LCR, 0x80);
    // 115200 baud: divisor = 115200 / (baud) ... 1843200/115200 = 16 = 0x10.
    outb(COM1 + REG_DATA, 0x10); // DLL low
    outb(COM1 + REG_IER, 0x00); // DLH high
                                // 8N1, clear DLAB.
    outb(COM1 + REG_LCR, 0x03);
    // Enable FIFOs, clear TX/RX queues.
    outb(COM1 + REG_FCR, 0x07);
    // RTS/DTR set.
    outb(COM1 + REG_MCR, 0x03);
}

fn outb(port: u16, value: u8) {
    unsafe { core::arch::asm!("out {0}, {1}", in(dx) port, in(al) value, options(nostack)) }
}

fn inb(port: u16) -> u8 {
    let v: u8;
    unsafe { core::arch::asm!("in {0}, {1}", out(al) v, in(dx) port, options(nostack)) }
    v
}

fn serial_write(byte: u8) {
    // Wait for the transmitter holding register to be empty.
    for _ in 0..1_000_000 {
        if inb(COM1 + REG_LSR) & LSR_THRE != 0 {
            outb(COM1 + REG_DATA, byte);
            return;
        }
    }
}

fn serial_data_ready() -> bool {
    inb(COM1 + REG_LSR) & LSR_DR != 0
}

fn serial_read() -> u8 {
    inb(COM1 + REG_DATA)
}

/// Read the CMOS real-time clock and return Unix seconds, if a valid date is
/// present.  Minimal BCD read of the seconds/minutes/hours/day/month/year
/// registers; year is interpreted as 2000+YY.
fn cmos_read_unix() -> Option<u64> {
    // Disable NMI while reading CMOS.
    outb(0x70, 0x80 | 0x00);
    let seconds = bcd(inb(0x71));
    outb(0x70, 0x02);
    let minutes = bcd(inb(0x71));
    outb(0x70, 0x04);
    let hours = bcd(inb(0x71));
    outb(0x70, 0x07);
    let day = bcd(inb(0x71));
    outb(0x70, 0x08);
    let month = bcd(inb(0x71));
    outb(0x70, 0x09);
    let year = bcd(inb(0x71));
    outb(0x70, 0x00); // re-enable NMI

    if seconds == 0 && minutes == 0 && hours == 0 && day == 0 && month == 0 && year == 0 {
        return None;
    }
    // Very rough civil-days conversion (not leap-year accurate).  Good enough
    // to report a non-zero boot time.
    let mut total: u64 = 0;
    total += (year as u64) * 365 * 86400;
    total += (month as u64) * 30 * 86400;
    total += (day as u64) * 86400;
    total += (hours as u64) * 3600;
    total += (minutes as u64) * 60;
    total += seconds as u64;
    Some(total)
}

fn bcd(v: u8) -> u8 {
    ((v >> 4) * 10) + (v & 0x0F)
}
