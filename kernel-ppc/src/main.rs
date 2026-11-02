// SPDX-License-Identifier: GPL-3.0-only
//! IRIX 7 PowerPC G3 machine.
//!
//! The third slice of #287, tracked as #295.  QEMU's g3beige machine loads
//! the raw image at 0x01000000 and OpenBIOS enters it with the MMU on and the
//! Open Firmware client interface in r5.  The kernel reads the machine
//! description from the device tree, installs its own BAT entries, points
//! SDR1 at an empty table, and from then on runs on translations it owns.  It
//! takes decrementer ticks through the 0x900 vector, then proves every
//! exception path it owns: a DSI, an ISI, an alignment fault, an illegal
//! instruction and one system call round-trip.  Finally it programs the CUDA
//! timer, takes external interrupts through the Heathrow controller at the
//! 0x500 vector, acknowledges each one and reports the count.

#![no_std]
#![no_main]

use core::panic::PanicInfo;

mod irq;
mod mmu;
mod of;
mod pic;
mod platform;
mod probe;
mod serial;
mod timer;
mod trap;
mod via;

core::arch::global_asm!(include_str!("boot.S"));

extern "C" {
    static __kernel_start: u8;
    static __kernel_end: u8;
}

pub(crate) fn print_hex(value: u32) {
    serial::write(b"0x");
    irix7_fmt::hex(u64::from(value), 1, false, &mut |byte| {
        serial::write_byte(byte)
    });
}

pub(crate) fn print_dec(value: u32) {
    irix7_fmt::decimal(u64::from(value), &mut |byte| serial::write_byte(byte));
}

pub(crate) fn print_line(bytes: &[u8]) {
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

pub(crate) fn halt() -> ! {
    loop {
        core::hint::spin_loop();
    }
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
    let controller = match platform::pic(&client, &macio) {
        Some(controller) => controller,
        None => {
            print_line(b"ppc: Mac I/O is not a Heathrow controller");
            halt();
        }
    };
    let cuda = match platform::cuda(&client, &macio) {
        Some(cuda) => cuda,
        None => {
            print_line(b"ppc: device tree has no CUDA timer");
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

    irq::prepare(controller.base, cuda.base, cuda.line);

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
    probe::run();
    print_line(b"TRAPS-OK");

    irq::start();
    let irq_started = timer::timebase();
    let irq_deadline = timebase.saturating_mul(2);
    while irq::count() < irq::TARGET {
        if timer::timebase().wrapping_sub(irq_started) > irq_deadline {
            serial::write(b"ppc: irq stalled count=");
            print_dec(irq::count());
            serial::write_byte(b'\n');
            print_line(b"ppc: halted");
            halt();
        }
        core::hint::spin_loop();
    }
    let (count, pending, ifr) = irq::stop();
    serial::write(b"ppc: irq pending=");
    print_hex(pending);
    serial::write(b" ifr=");
    print_hex(ifr);
    serial::write_byte(b'\n');
    serial::write(b"IRQ-OK count=");
    print_dec(count);
    serial::write_byte(b'\n');

    print_line(b"ppc: halted");
    halt()
}

#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    print_line(b"ppc: panic");
    if let Some(message) = info.message().as_str() {
        print_line(message.as_bytes());
    }
    halt()
}
