// SPDX-License-Identifier: GPL-3.0-only
//! IRIX 7 PowerPC G3 machine.
//!
//! The second slice of #287, tracked as #291.  QEMU's g3beige machine loads
//! the raw image at 0x01000000 and OpenBIOS enters it with the MMU on and the
//! Open Firmware client interface in r5.  The kernel reads the machine
//! description from the device tree, installs its own BAT entries, points
//! SDR1 at an empty table, and from then on runs on translations it owns.  It
//! takes decrementer ticks through the 0x900 vector, then takes a deliberate
//! unmapped load through the 0x300 vector to prove fault reporting.

#![no_std]
#![no_main]

use core::panic::PanicInfo;

mod mmu;
mod of;
mod platform;
mod serial;
mod timer;
mod trap;

core::arch::global_asm!(include_str!("boot.S"));

/// Outside every BAT: 512 MiB, above the largest RAM size the gate uses and
/// below the PCI hole.
const UNMAPPED: u32 = 0x2000_0000;

extern "C" {
    static __kernel_start: u8;
    static __kernel_end: u8;
}

fn print_hex(value: u32) {
    serial::write(b"0x");
    irix7_fmt::hex(u64::from(value), 1, false, &mut |byte| {
        serial::write_byte(byte)
    });
}

fn print_dec(value: u32) {
    irix7_fmt::decimal(u64::from(value), &mut |byte| serial::write_byte(byte));
}

fn print_line(bytes: &[u8]) {
    serial::write(bytes);
    serial::write_byte(b'\n');
}

fn print_range(label: &[u8], base: u32, size: u32) {
    serial::write(label);
    serial::write(b" base=");
    print_hex(base);
    serial::write(b" size=");
    print_dec(size);
    serial::write_byte(b'\n');
}

fn halt() -> ! {
    loop {
        core::hint::spin_loop();
    }
}

/// Take one load from an address no BAT covers.
///
/// If the DSI vector is not installed, this returns and the gate fails on the
/// missing report.
fn deliberate_fault() -> ! {
    print_line(b"ppc: deliberate unmapped load");
    // SAFETY: the access is the point of the test.  Every BAT is identity
    // mapped and none covers 0x2000_0000, so the load must raise a DSI.
    unsafe {
        core::ptr::read_volatile(UNMAPPED as *const u32);
    }
    print_line(b"ppc: dsi did not fire");
    print_line(b"ppc: halted");
    halt()
}

#[no_mangle]
pub extern "C" fn kernel_main(of_client: u32) -> ! {
    let client = of::Client::new(of_client);
    let macio = match platform::macio(&client) {
        Some(macio) => macio,
        // The ESCC lives inside the Mac I/O window, so without it there is
        // no console to report on.
        None => halt(),
    };
    let escc = match platform::escc(&client, &macio) {
        Some(base) => base,
        // Without the console there is nowhere to report the failure.
        None => halt(),
    };
    serial::init(escc);

    let memory = match platform::memory(&client) {
        Some(memory) => memory,
        None => {
            print_line(b"ppc: device tree has no /memory reg");
            halt();
        }
    };
    let timebase = match platform::timebase(&client) {
        Some(timebase) => timebase,
        None => {
            print_line(b"ppc: device tree has no timebase-frequency");
            halt();
        }
    };

    let io_end = match macio.base.checked_add(macio.size) {
        Some(end) => end,
        None => {
            print_line(b"ppc: mac-io window wraps the address space");
            halt();
        }
    };
    let image = mmu::cover(
        core::ptr::addr_of!(__kernel_start) as u32,
        core::ptr::addr_of!(__kernel_end) as u32,
    );
    let io = mmu::cover(macio.base, io_end);

    // The vectors must be in place before the firmware mapping goes away.
    trap::install();
    timer::prepare(timebase);
    mmu::install(image, io);
    let sdr1 = mmu::activate(image, io);

    serial::write(b"ppc: mmu enabled sdr1=");
    print_hex(sdr1);
    serial::write_byte(b'\n');
    print_range(b"ppc: mmu image", image.base, image.size);
    print_range(b"ppc: mmu io", io.base, io.size);
    serial::write(b"ppc: mmu io wimg=");
    print_hex(mmu::WIMG_IO);
    serial::write_byte(b'\n');

    print_line(b"IRIX 7 PowerPC G3 bring-up");
    print_line(b"ppc: booted");
    serial::write(b"ppc: of client=");
    print_hex(of_client);
    serial::write_byte(b'\n');
    serial::write(b"ppc: escc=");
    print_hex(escc);
    serial::write_byte(b'\n');

    serial::write(b"ppc: memory base=");
    print_hex(memory.base);
    serial::write(b" size=");
    print_dec(memory.size / (1024 * 1024));
    print_line(b" MiB");

    serial::write(b"ppc: timebase=");
    print_dec(timebase);
    print_line(b" Hz");

    timer::enable();
    let started = timer::timebase();
    let deadline = timebase.saturating_mul(2);
    while timer::ticks() < 10 {
        if timer::timebase().wrapping_sub(started) > deadline {
            print_line(b"ppc: decrementer stalled");
            halt();
        }
        core::hint::spin_loop();
    }
    serial::write(b"ppc: decrementer ticks=");
    print_dec(timer::ticks());
    serial::write_byte(b'\n');

    print_line(b"BOOT-OK");
    deliberate_fault()
}

#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    print_line(b"ppc: panic");
    if let Some(message) = info.message().as_str() {
        print_line(message.as_bytes());
    }
    halt()
}
