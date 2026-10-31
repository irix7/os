// SPDX-License-Identifier: GPL-3.0-only
//! IRIX 7 x86_64 machine.
//!
//! The first slice of #266: QEMU loads this ELF through the Xen PVH path, the
//! assembly stub in `boot.S` enters long mode with the first 4 GiB identity
//! mapped, and this code brings up the serial console, the PVH memory map, the
//! IDT and the PIT.  A deliberate breakpoint proves the trap path reports
//! rather than hanging.  The shared kernel core is not linked in yet; the
//! machine layer is being built first so the core can move onto it without a
//! second copy of kernel logic.

#![no_std]
#![no_main]

extern crate alloc;

use alloc::vec::Vec;
use core::arch::asm;
use core::panic::PanicInfo;

mod heap;
mod idt;
mod memory;
mod pvh;
mod serial;
mod timer;

core::arch::global_asm!(include_str!("boot.S"), options(att_syntax));

/// The 16550 debug port, used before anything else exists.
pub const QEMU_DEBUG_EXIT: u16 = 0x501;

/// Leave QEMU through `isa-debug-exit`.
///
/// The device encodes the guest's value as `(value << 1) | 1`, so a caller that
/// passes 0 makes QEMU exit with status 1.  Without the device the write is a
/// no-op and the machine halts instead, which keeps the image usable under a
/// plain `-kernel` invocation.
pub fn qemu_exit(value: u32) -> ! {
    // SAFETY: an I/O port write, valid in ring 0.
    unsafe {
        asm!("out dx, eax", in("dx") QEMU_DEBUG_EXIT, in("eax") value, options(nomem, nostack));
    }
    halt()
}

/// Stop the machine until an interrupt arrives.
pub fn halt() -> ! {
    loop {
        // SAFETY: `hlt` is a ring-0 instruction; interrupts stay enabled so a
        // wake-up is possible.
        unsafe {
            asm!("hlt", options(nomem, nostack));
        }
    }
}

fn print_hex(value: u64) {
    serial::write(b"0x");
    let mut started = false;
    for shift in (0..16).rev() {
        let digit = ((value >> (shift * 4)) & 0xF) as u8;
        if digit != 0 || started || shift == 0 {
            started = true;
            serial::write_byte(if digit < 10 {
                b'0' + digit
            } else {
                b'a' + digit - 10
            });
        }
    }
}

fn print_dec(mut value: u64) {
    let mut digits = [0u8; 20];
    let mut n = 0;
    if value == 0 {
        serial::write_byte(b'0');
        return;
    }
    while value > 0 {
        digits[n] = b'0' + (value % 10) as u8;
        value /= 10;
        n += 1;
    }
    while n > 0 {
        n -= 1;
        serial::write_byte(digits[n]);
    }
}

fn print_line(parts: &[&[u8]]) {
    for part in parts {
        serial::write(part);
    }
    serial::write_byte(b'\n');
}

/// The PVH entry point, called from `start64` with the physical address of
/// `hvm_start_info`.
#[no_mangle]
pub extern "C" fn kernel_main(start_info: u64) -> ! {
    serial::init();
    print_line(&[b"x86: booted"]);

    serial::write(b"x86: pvh start_info=");
    print_hex(start_info);
    serial::write_byte(b'\n');

    let ram = pvh::describe(start_info);
    serial::write(b"x86: ram=");
    print_dec(ram / (1024 * 1024));
    serial::write(b" MiB\n");

    memory::init(start_info);
    let (heap_base, heap_size) = heap::init();
    serial::write(b"x86: frames total=");
    print_dec(memory::total_ram() / (1024 * 1024));
    serial::write(b" MiB free=");
    print_dec(memory::free_frames() as u64);
    serial::write(b" frames\n");
    serial::write(b"x86: heap base=");
    print_hex(heap_base as u64);
    serial::write(b" size=");
    print_dec(heap_size as u64);
    serial::write(b"\n");

    // Prove both layers before anything depends on them: one frame must come
    // back page-aligned and above the kernel image, and a growing collection
    // must sum correctly.
    let frame = memory::alloc_frame().expect("x86: frame allocation");
    if !frame.is_multiple_of(4096) || frame < heap_base as u64 + heap_size as u64 {
        print_line(&[b"x86: frame allocation outside the free range"]);
        qemu_exit(1);
    }
    serial::write(b"x86: frame alloc=");
    print_hex(frame);
    serial::write_byte(b'\n');

    let mut values: Vec<u64> = Vec::new();
    for value in 0..1000u64 {
        values.push(value);
    }
    let sum: u64 = values.iter().sum();
    serial::write(b"x86: heap sum=");
    print_dec(sum);
    serial::write_byte(b'\n');
    if sum != 499_500 {
        print_line(&[b"x86: heap sum mismatch"]);
        qemu_exit(1);
    }

    idt::init();
    timer::init();
    print_line(&[b"x86: timer enabled"]);

    let mut spins = 0u64;
    while timer::ticks() < 10 {
        if spins >= 500_000_000 {
            print_line(&[b"x86: timer stalled"]);
            qemu_exit(1);
        }
        spins += 1;
        core::hint::spin_loop();
    }
    serial::write(b"x86: timer ticks=");
    print_dec(timer::ticks() as u64);
    serial::write_byte(b'\n');

    print_line(&[b"x86: deliberate fault: int3"]);
    // SAFETY: the IDT installs a handler for vector 3.
    unsafe {
        asm!("int3", options(nomem, nostack));
    }
    unreachable!("int3 returned");
}

#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    print_line(&[
        b"x86: panic: ",
        info.message().as_str().unwrap_or("?").as_bytes(),
    ]);
    qemu_exit(2)
}
