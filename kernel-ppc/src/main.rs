// SPDX-License-Identifier: GPL-3.0-only
//! IRIX 7 PowerPC G3 machine.
//!
//! The third slice of #287, tracked as #295.  QEMU's g3beige machine loads
//! the raw image at 0x01000000 and OpenBIOS enters it with the MMU on and the
//! Open Firmware client interface in r5.  The kernel reads the machine
//! description from the device tree, installs its own BAT entries, points
//! SDR1 at an empty table, and from then on runs on translations it owns.  It
//! initialises both ESCC channels, mirrors the boot log to both, takes
//! decrementer ticks through the 0x900 vector, then proves every exception
//! path it owns: a DSI, an ISI, an alignment fault, an illegal instruction and
//! one system call round-trip.  Finally it programs the CUDA timer, takes
//! external interrupts through the Heathrow controller at the 0x500 vector,
//! acknowledges each one and reports the count, then identifies the ATA disk
//! named by the Mac I/O device tree and reads one PIO sector.

#![no_std]
#![no_main]

use core::panic::PanicInfo;

mod ata;
mod cache;
mod irq;
mod mmu;
mod module;
mod of;
mod pic;
mod platform;
mod probe;
mod serial;
mod services;
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

pub(crate) fn print_dec64(value: u64) {
    irix7_fmt::decimal(value, &mut |byte| serial::write_byte(byte));
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

pub(crate) fn print_hex_bytes(bytes: &[u8]) {
    for byte in bytes {
        irix7_fmt::hex(u64::from(*byte), 2, false, &mut |digit| {
            serial::write_byte(digit)
        });
    }
}

fn print_model(model: &[u8; 40]) {
    let mut end = model.len();
    while end > 0 && (model[end - 1] == b' ' || model[end - 1] == 0) {
        end -= 1;
    }
    serial::write(&model[..end]);
}

fn print_channel(channel: Option<u32>) {
    match channel {
        Some(address) => print_hex(address),
        None => serial::write(b"none"),
    }
}

fn print_property(property: &platform::Property) {
    if !property.present() {
        serial::write(b"none");
        return;
    }
    for byte in property.bytes() {
        let printable = (0x20..=0x7e).contains(byte);
        serial::write_byte(if printable { *byte } else { b'?' });
    }
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
        Some(escc) => escc,
        // Without the console there is nowhere to report the failure.
        None => halt(),
    };
    serial::init(escc.channels);

    let memory = match platform::memory(&client) {
        Some(memory) => memory,
        None => {
            print_line(b"ppc: device tree has no /memory reg");
            halt();
        }
    };
    let slot_end = module::SLOT_BASE + module::SLOT_SIZE as u32;
    match memory.base.checked_add(memory.size) {
        Some(end) if end >= slot_end => {}
        _ => {
            print_line(b"ppc: memory does not cover the module slot");
            print_line(b"ppc: halted");
            halt();
        }
    }
    let timebase = match platform::timebase(&client) {
        Some(timebase) => timebase,
        None => {
            print_line(b"ppc: device tree has no timebase-frequency");
            halt();
        }
    };
    // These lookups use the Open Firmware client and must run before the BAT
    // switch.  The results are kept until the boot log has reported the
    // selected Mac I/O node, so a machine with a missing child still shows
    // which controller was chosen.
    let controller = platform::pic(&client, &macio);
    let cuda = platform::cuda(&client, &macio);
    let disk = platform::ata(&client, &macio);

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
    let module_slot = mmu::cover(module::SLOT_BASE, slot_end);

    // The vectors must be in place before the firmware mapping goes away.
    trap::install();
    timer::prepare(timebase);
    mmu::install(image, io, module_slot);
    let sdr1 = mmu::activate(image, io, module_slot);

    serial::write(b"ppc: mmu enabled sdr1=");
    print_hex(sdr1);
    serial::write_byte(b'\n');
    print_range(b"ppc: mmu image", image.base, image.size);
    print_range(b"ppc: mmu io", io.base, io.size);
    serial::write(b"ppc: mmu io wimg=");
    print_hex(mmu::WIMG_IO);
    serial::write_byte(b'\n');
    print_range(b"ppc: mmu module", module_slot.base, module_slot.size);

    print_line(b"IRIX 7 PowerPC G3 bring-up");
    print_line(b"ppc: booted");
    serial::write(b"ppc: of client=");
    print_hex(of_client);
    serial::write_byte(b'\n');
    print_range(b"ppc: macio", macio.base, macio.size);
    serial::write(b"ppc: macio path=");
    serial::write(macio.path.bytes());
    serial::write_byte(b'\n');
    serial::write(b"ppc: macio children escc=");
    print_dec(macio.escc);
    serial::write(b" ata=");
    print_dec(macio.ata);
    serial::write(b" disks=");
    print_dec(macio.disks);
    serial::write_byte(b'\n');
    serial::write(b"ppc: escc=");
    print_hex(escc.base);
    serial::write(b" ch-a=");
    print_channel(escc.channels[0]);
    serial::write(b" ch-b=");
    print_channel(escc.channels[1]);
    serial::write_byte(b'\n');
    serial::write(b"ppc: console output-device=");
    print_property(&escc.output_device);
    serial::write(b" stdout-path=");
    print_property(&escc.stdout_path);
    serial::write(b" selected=");
    match escc.selected {
        Some(channel) => serial::write_byte(channel.letter()),
        None => serial::write(b"none"),
    }
    print_line(b" mirror=a+b");

    serial::write(b"ppc: memory base=");
    print_hex(memory.base);
    serial::write(b" size=");
    print_dec(memory.size / (1024 * 1024));
    print_line(b" MiB");

    serial::write(b"ppc: timebase=");
    print_dec(timebase);
    print_line(b" Hz");

    let controller = match controller {
        Some(controller) => controller,
        None => {
            print_line(b"ppc: Mac I/O is not a Heathrow or Gatwick controller");
            halt();
        }
    };
    let cuda = match cuda {
        Some(cuda) => cuda,
        None => {
            print_line(b"ppc: device tree has no CUDA timer");
            halt();
        }
    };
    let disk = match disk {
        Some(disk) => disk,
        None => {
            print_line(b"ppc: device tree has no ATA disk");
            print_line(b"ppc: halted");
            halt();
        }
    };

    print_range(b"ppc: ata reg", disk.base, disk.size);
    serial::write(b"ppc: ata irq=");
    print_hex(disk.line);
    serial::write_byte(b'\n');

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

    let channel = ata::Channel::new(disk.base);
    let identify = match channel.identify() {
        Ok(identify) => identify,
        Err(error) => {
            error.report();
            print_line(b"ppc: halted");
            halt();
        }
    };
    serial::write(b"ppc: ata model=");
    print_model(&identify.model);
    serial::write(b" sectors=");
    print_dec64(identify.sectors);
    serial::write(b" lba48=");
    print_dec(u32::from(identify.lba48));
    serial::write_byte(b'\n');

    const SIGNATURE_LBA: u32 = 4;
    let mut sector = [0u8; ata::SECTOR_SIZE];
    if let Err(error) = channel.read_sector(SIGNATURE_LBA, &mut sector) {
        error.report();
        print_line(b"ppc: halted");
        halt();
    }
    serial::write(b"ppc: ata lba=");
    print_dec(SIGNATURE_LBA);
    serial::write(b" sector=");
    print_hex_bytes(&sector[..16]);
    serial::write_byte(b'\n');
    print_line(b"ATA-OK");

    match module::load(channel) {
        Ok(report) => {
            serial::write(b"ppc: module status=");
            print_dec(report.status as u32);
            serial::write(b" tick=");
            print_dec(u32::from(report.ticked));
            serial::write(b" length=");
            print_dec(report.length);
            serial::write_byte(b'\n');
            print_line(b"MODULE-OK");
        }
        Err(error) => {
            error.report();
            print_line(b"ppc: halted");
            halt();
        }
    }

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
