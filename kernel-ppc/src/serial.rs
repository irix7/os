// SPDX-License-Identifier: GPL-3.0-only
//! The Z85C30 ESCC in the Heathrow Mac I/O controller.
//!
//! QEMU's g3beige machine wires `-serial` to channel A of the ESCC.  The
//! controller is reached through the Mac I/O PCI BAR; the base address comes
//! from the device tree, not from a compile-time constant, because the
//! firmware assigns the BAR.  Registers use a two-level selector: a write to
//! the control port selects a register, the next control access reads or
//! writes it.

use core::ptr::{read_volatile, write_volatile};

/// Channel A control, relative to the ESCC base.
const CHANNEL_A_CTRL: u32 = 0x20;
/// Channel A data, relative to the ESCC base.
const CHANNEL_A_DATA: u32 = 0x30;

const REG_COMMAND: u8 = 0;
const REG_TXCTRL1: u8 = 4;
const REG_TXCTRL2: u8 = 5;
const REG_MINTR: u8 = 9;

const STATUS_TXEMPTY: u8 = 0x04;
const TXCTRL2_8BITS_TXEN: u8 = 0x68;

static mut BASE: u32 = 0;

pub fn init(base: u32) {
    unsafe {
        BASE = base;
    }
    write_register(REG_MINTR, 0x80); // reset channel A
    write_register(REG_TXCTRL1, 0x44); // x16 clock, one stop bit, no parity
    write_register(REG_TXCTRL2, TXCTRL2_8BITS_TXEN);
    write_register(REG_COMMAND, 0x00);
}

fn write_register(register: u8, value: u8) {
    let base = unsafe { BASE };
    if base == 0 {
        return;
    }
    unsafe {
        write_volatile((base + CHANNEL_A_CTRL) as *mut u8, register);
        write_volatile((base + CHANNEL_A_CTRL) as *mut u8, value);
    }
}

fn read_status() -> u8 {
    let base = unsafe { BASE };
    if base == 0 {
        return STATUS_TXEMPTY;
    }
    unsafe {
        write_volatile((base + CHANNEL_A_CTRL) as *mut u8, REG_COMMAND);
        read_volatile((base + CHANNEL_A_CTRL) as *const u8)
    }
}

pub fn write_byte(byte: u8) {
    if byte == b'\n' {
        write_byte(b'\r');
    }
    let base = unsafe { BASE };
    if base == 0 {
        return;
    }
    while read_status() & STATUS_TXEMPTY == 0 {
        core::hint::spin_loop();
    }
    unsafe {
        write_volatile((base + CHANNEL_A_DATA) as *mut u8, byte);
    }
}

pub fn write(bytes: &[u8]) {
    for byte in bytes {
        write_byte(*byte);
    }
}
