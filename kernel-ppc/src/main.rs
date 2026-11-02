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
//! named by the Mac I/O device tree and reads one PIO sector.  After the
//! module check it waits one second for a key on the selected ESCC channel;
//! a key enters a small `ppc>` monitor, silence prints a timeout.  It then
//! synchronises the ADB controller, waits for one key on the keyboard the
//! device tree names, and prints the scancode, the US ASCII character and
//! `KEY-OK`.

#![no_std]
#![no_main]

use core::panic::PanicInfo;

mod adb;
mod ata;
mod cache;
mod console;
mod cuda;
mod fb;
mod font;
mod irq;
mod keyboard;
mod mmu;
mod module;
mod monitor;
mod of;
mod pic;
mod platform;
mod pmu;
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
    console::write(b"0x");
    irix7_fmt::hex(u64::from(value), 1, false, &mut |byte| {
        console::write_byte(byte)
    });
}

pub(crate) fn print_dec(value: u32) {
    irix7_fmt::decimal(u64::from(value), &mut |byte| console::write_byte(byte));
}

pub(crate) fn print_dec64(value: u64) {
    irix7_fmt::decimal(value, &mut |byte| console::write_byte(byte));
}

pub(crate) fn print_line(bytes: &[u8]) {
    console::write(bytes);
    console::write_byte(b'\n');
}

fn print_range(label: &[u8], base: u32, size: u32) {
    console::write(label);
    console::write(b" base=");
    print_hex(base);
    console::write(b" size=");
    print_dec(size);
    console::write_byte(b'\n');
}

/// The BAT block for the display aperture.
///
/// The block covers both the aperture the firmware assigned and the visible
/// frame, so a display whose `assigned-addresses` size is smaller than the
/// mode still gets a mapped frame.  A 750 BAT block is at most 256 MiB; a
/// larger aperture is refused rather than truncated.
fn framebuffer_bat(display: Option<&platform::Display>) -> Option<mmu::Bat> {
    let display = display?;
    if !fb::supported(display) {
        return None;
    }
    let size = display.size.max(display.frame_size());
    if size > 0x1000_0000 {
        return None;
    }
    let end = display.base.checked_add(size)?;
    Some(mmu::cover(display.base, end))
}

pub(crate) fn print_hex_bytes(bytes: &[u8]) {
    for byte in bytes {
        irix7_fmt::hex(u64::from(*byte), 2, false, &mut |digit| {
            console::write_byte(digit)
        });
    }
}

pub(crate) fn print_model(model: &[u8; 40]) {
    let mut end = model.len();
    while end > 0 && (model[end - 1] == b' ' || model[end - 1] == 0) {
        end -= 1;
    }
    console::write(&model[..end]);
}

fn print_channel(channel: Option<u32>) {
    match channel {
        Some(address) => print_hex(address),
        None => console::write(b"none"),
    }
}

fn print_property(property: &platform::Property) {
    if !property.present() {
        console::write(b"none");
        return;
    }
    for byte in property.bytes() {
        let printable = (0x20..=0x7e).contains(byte);
        console::write_byte(if printable { *byte } else { b'?' });
    }
}

/// Prove the ADB keyboard once, after the other boot probes.
///
/// The wait is bounded so a machine without a working keyboard still reaches
/// `ppc: halted`; the queue keeps whatever follows the first printable press
/// for the input consumer.
fn keyboard_probe(adb: Option<platform::AdbHost>) {
    let Some(host) = adb else {
        print_line(b"ppc: keyboard absent");
        print_line(b"KEY-SKIP reason=no-adb");
        return;
    };
    keyboard::init(host);
    if !keyboard::start() {
        print_line(b"KEY-SKIP reason=reset");
        return;
    }
    print_line(b"ppc: keyboard waiting");
    match keyboard::wait_for_key(3000) {
        Some(key) => {
            serial::write(b"ppc: adb key code=0x");
            print_hex_bytes(&[key.code]);
            serial::write(b" down=");
            print_dec(u32::from(key.down as u8));
            serial::write(b" ascii=0x");
            print_hex_bytes(&[key.ascii]);
            serial::write(b" char=");
            let printable = key.ascii.is_ascii_graphic() || key.ascii == b' ';
            serial::write_byte(if printable { key.ascii } else { b'?' });
            serial::write_byte(b'\n');
            print_line(b"KEY-OK");
        }
        None => {
            print_line(b"ppc: keyboard timeout");
            print_line(b"KEY-SKIP reason=no-key");
        }
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
    let input = serial::init(escc.channels, escc.selected);

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
    let adb = platform::adb(&client, &macio);
    let display = platform::display(&client);

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
    let framebuffer = framebuffer_bat(display.as_ref());

    // The vectors must be in place before the firmware mapping goes away.
    trap::install();
    timer::prepare(timebase);
    mmu::install(image, io, module_slot, framebuffer);
    let sdr1 = mmu::activate(image, io, module_slot, framebuffer);

    console::write(b"ppc: mmu enabled sdr1=");
    print_hex(sdr1);
    console::write_byte(b'\n');
    print_range(b"ppc: mmu image", image.base, image.size);
    print_range(b"ppc: mmu io", io.base, io.size);
    console::write(b"ppc: mmu io wimg=");
    print_hex(mmu::WIMG_IO);
    console::write_byte(b'\n');
    print_range(b"ppc: mmu module", module_slot.base, module_slot.size);
    if let Some(framebuffer) = framebuffer {
        print_range(b"ppc: mmu fb", framebuffer.base, framebuffer.size);
        console::write(b"ppc: mmu fb wimg=");
        print_hex(mmu::WIMG_FB);
        console::write_byte(b'\n');
    }

    match &display {
        Some(display) => {
            console::write(b"ppc: display path=");
            console::write(display.path.bytes());
            console::write(b" base=");
            print_hex(display.base);
            console::write(b" size=");
            print_dec(display.size);
            console::write(b" width=");
            print_dec(display.width);
            console::write(b" height=");
            print_dec(display.height);
            console::write(b" depth=");
            print_dec(display.depth);
            console::write(b" linebytes=");
            print_dec(display.linebytes);
            console::write_byte(b'\n');
        }
        None => print_line(b"ppc: display absent"),
    }

    // The framebuffer is mapped by the switch above, so initialising it now
    // replays every byte printed since the first MMU line.  A machine with no
    // display, or a mode this console cannot draw, keeps the serial log and
    // reports the refusal.  The pixel self-test waits until the ATA read so
    // its block and glyph are still on the final screen.
    let graphics = display
        .as_ref()
        .is_some_and(|display| fb::supported(display) && fb::init(display));
    if !graphics {
        if let Some(display) = &display {
            console::write(b"ppc: display unsupported depth=");
            print_dec(display.depth);
            console::write(b" linebytes=");
            print_dec(display.linebytes);
            console::write_byte(b'\n');
            print_line(b"GFX-SKIP reason=mode");
        } else {
            print_line(b"GFX-SKIP reason=no-display");
        }
    }

    print_line(b"IRIX 7 PowerPC G3 bring-up");
    print_line(b"ppc: booted");
    console::write(b"ppc: of client=");
    print_hex(of_client);
    console::write_byte(b'\n');
    print_range(b"ppc: macio", macio.base, macio.size);
    console::write(b"ppc: macio path=");
    console::write(macio.path.bytes());
    console::write_byte(b'\n');
    console::write(b"ppc: macio children escc=");
    print_dec(macio.escc);
    console::write(b" ata=");
    print_dec(macio.ata);
    console::write(b" disks=");
    print_dec(macio.disks);
    console::write_byte(b'\n');
    console::write(b"ppc: escc=");
    print_hex(escc.base);
    console::write(b" ch-a=");
    print_channel(escc.channels[0]);
    console::write(b" ch-b=");
    print_channel(escc.channels[1]);
    console::write_byte(b'\n');
    console::write(b"ppc: console output-device=");
    print_property(&escc.output_device);
    console::write(b" stdout-path=");
    print_property(&escc.stdout_path);
    console::write(b" selected=");
    match escc.selected {
        Some(channel) => console::write_byte(channel.letter()),
        None => console::write(b"none"),
    }
    print_line(b" mirror=a+b");

    console::write(b"ppc: memory base=");
    print_hex(memory.base);
    console::write(b" size=");
    print_dec(memory.size / (1024 * 1024));
    print_line(b" MiB");

    console::write(b"ppc: timebase=");
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
        Some(cuda) => Some(cuda),
        None => {
            print_line(b"ppc: timer via=absent external-irq=skipped");
            None
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
    console::write(b"ppc: ata irq=");
    print_hex(disk.line);
    console::write_byte(b'\n');

    match &adb {
        Some(platform::AdbHost::Cuda { base, keyboard }) => {
            serial::write(b"ppc: adb transport=cuda via=");
            print_hex(*base);
            serial::write(b" keyboard=");
            print_dec(u32::from(*keyboard));
            serial::write_byte(b'\n');
        }
        Some(platform::AdbHost::Pmu { base, keyboard }) => {
            serial::write(b"ppc: adb transport=pmu via=");
            print_hex(*base);
            serial::write(b" keyboard=");
            print_dec(u32::from(*keyboard));
            serial::write_byte(b'\n');
        }
        None => print_line(b"ppc: adb absent"),
    }

    if let Some(cuda) = &cuda {
        irq::prepare(controller.base, cuda.base, cuda.line);
    }

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
    console::write(b"ppc: decrementer ticks=");
    print_dec(timer::ticks());
    console::write_byte(b'\n');

    print_line(b"BOOT-OK");
    probe::run();
    print_line(b"TRAPS-OK");

    if let Some(_cuda) = &cuda {
        irq::start();
        let irq_started = timer::timebase();
        let irq_deadline = timebase.saturating_mul(2);
        while irq::count() < irq::TARGET {
            if timer::timebase().wrapping_sub(irq_started) > irq_deadline {
                console::write(b"ppc: irq stalled count=");
                print_dec(irq::count());
                console::write_byte(b'\n');
                print_line(b"ppc: halted");
                halt();
            }
            core::hint::spin_loop();
        }
        let (count, pending, ifr) = irq::stop();
        console::write(b"ppc: irq pending=");
        print_hex(pending);
        console::write(b" ifr=");
        print_hex(ifr);
        console::write_byte(b'\n');
        console::write(b"IRQ-OK count=");
        print_dec(count);
        console::write_byte(b'\n');
    } else {
        print_line(b"IRQ-SKIP reason=no-cuda");
    }

    let channel = ata::Channel::new(disk.base);
    let identify = match channel.identify() {
        Ok(identify) => identify,
        Err(error) => {
            error.report();
            print_line(b"ppc: halted");
            halt();
        }
    };
    console::write(b"ppc: ata model=");
    print_model(&identify.model);
    console::write(b" sectors=");
    print_dec64(identify.sectors);
    console::write(b" lba48=");
    print_dec(u32::from(identify.lba48));
    console::write_byte(b'\n');

    const SIGNATURE_LBA: u32 = 4;
    let mut sector = [0u8; ata::SECTOR_SIZE];
    if let Err(error) = channel.read_sector(SIGNATURE_LBA, &mut sector) {
        error.report();
        print_line(b"ppc: halted");
        halt();
    }
    console::write(b"ppc: ata lba=");
    print_dec(SIGNATURE_LBA);
    console::write(b" sector=");
    print_hex_bytes(&sector[..16]);
    console::write_byte(b'\n');
    print_line(b"ATA-OK");

    // Draw the self-test block and glyph now, so the final screen still shows
    // them after the boot log has scrolled.  The readback is the same mapping
    // the console uses, so a wrong stride, byte order or aperture fails here.
    if graphics {
        let passed = fb::selftest();
        console::write(b"ppc: framebuffer bytes=");
        print_dec(fb::rendered());
        console::write_byte(b'\n');
        if passed {
            print_line(b"GFX-OK");
        } else {
            print_line(b"GFX-FAIL");
        }
    }

    let gate_disk = &sector[..16] == b"IRIX7-ATA-SIG001";
    let mut module_state = monitor::ModuleState::Skipped;
    if gate_disk {
        match module::load(channel) {
            Ok(report) => {
                console::write(b"ppc: module status=");
                print_dec(report.status as u32);
                console::write(b" tick=");
                print_dec(u32::from(report.ticked));
                console::write(b" length=");
                print_dec(report.length);
                console::write_byte(b'\n');
                print_line(b"MODULE-OK");
                module_state = monitor::ModuleState::Loaded {
                    length: report.length,
                    ticked: report.ticked,
                };
            }
            Err(error) => {
                error.report();
                print_line(b"ppc: halted");
                halt();
            }
        }
    } else {
        print_line(b"ppc: module skipped=not-gate-disk");
    }

    let info = monitor::Info {
        macio_base: macio.base,
        macio_size: macio.size,
        macio_path: macio.path,
        console: escc.selected,
        input,
        memory_base: memory.base,
        memory_size: memory.size,
        timebase,
        ata_model: identify.model,
        ata_sectors: identify.sectors,
        module: module_state,
    };

    match input {
        None => print_line(b"ppc: monitor unavailable"),
        Some(channel) => {
            serial::write(b"ppc: monitor wait input=");
            serial::write_byte(channel.letter());
            serial::write_byte(b'\n');
            match monitor::first_key() {
                Some(key) => monitor::run(&info, key),
                None => print_line(b"ppc: monitor timeout"),
            }
        }
    }

    keyboard_probe(adb);

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
