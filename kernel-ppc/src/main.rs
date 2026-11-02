// SPDX-License-Identifier: GPL-3.0-only
//! IRIX 7 PowerPC G3 machine.
//!
//! The first slice of #287: QEMU's g3beige machine loads the raw image at
//! 0x01000000 and OpenBIOS enters it with the MMU on and the Open Firmware
//! client interface in r5.  This code reads the machine description from the
//! device tree, brings up the ESCC console, takes decrementer ticks through
//! the 0x900 exception vector, and halts.

#![no_std]
#![no_main]

use core::panic::PanicInfo;

mod of;
mod platform;
mod serial;
mod timer;

core::arch::global_asm!(include_str!("boot.S"));

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

fn halt() -> ! {
    loop {
        core::hint::spin_loop();
    }
}

#[no_mangle]
pub extern "C" fn kernel_main(of_client: u32) -> ! {
    let client = of::Client::new(of_client);
    let escc = match platform::escc(&client) {
        Some(base) => base,
        // Without the console there is nowhere to report the failure.
        None => halt(),
    };
    serial::init(escc);

    print_line(b"IRIX 7 PowerPC G3 bring-up");
    print_line(b"ppc: booted");
    serial::write(b"ppc: of client=");
    print_hex(of_client);
    serial::write_byte(b'\n');
    serial::write(b"ppc: escc=");
    print_hex(escc);
    serial::write_byte(b'\n');

    let memory = match platform::memory(&client) {
        Some(memory) => memory,
        None => {
            print_line(b"ppc: device tree has no /memory reg");
            halt();
        }
    };
    serial::write(b"ppc: memory base=");
    print_hex(memory.base);
    serial::write(b" size=");
    print_dec(memory.size / (1024 * 1024));
    print_line(b" MiB");

    let timebase = match platform::timebase(&client) {
        Some(timebase) => timebase,
        None => {
            print_line(b"ppc: device tree has no timebase-frequency");
            halt();
        }
    };
    serial::write(b"ppc: timebase=");
    print_dec(timebase);
    print_line(b" Hz");

    timer::start(timebase);
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

    print_line(b"ppc: halted");
    print_line(b"BOOT-OK");
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
