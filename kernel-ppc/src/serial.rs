// SPDX-License-Identifier: GPL-3.0-only
//! The Z85C30 ESCC in the Heathrow Mac I/O controller.
//!
//! The ESCC has two independent channels.  QEMU's g3beige wires `-serial` to
//! channel A and the second `-serial` to channel B; the PowerBook G3 M4753
//! wires its external Geoport and its internal infrared link to the same two
//! channels, but the firmware console property and the cable wiring vary by
//! model and configuration.  The kernel therefore initialises both channels
//! and mirrors every byte to both, so a boot is visible on either wiring.
//!
//! The controller is reached through the Mac I/O PCI BAR; the base address
//! comes from the device tree, not from a compile-time constant, because the
//! firmware assigns the BAR.  Registers use a two-level selector: a write to
//! the control port selects a register, the next control access reads or
//! writes it.

use core::sync::atomic::{AtomicU32, Ordering::Relaxed};

/// A channel's data port is 16 bytes above its control port.
const CHANNEL_DATA_OFFSET: u32 = 0x10;

const REG_COMMAND: u8 = 0;
const REG_RXCTRL: u8 = 3;
const REG_TXCTRL1: u8 = 4;
const REG_TXCTRL2: u8 = 5;
const REG_MINTR: u8 = 9;
const REG_CLOCK: u8 = 11;
const REG_BRGLO: u8 = 12;
const REG_BRGHI: u8 = 13;
const REG_MISC2: u8 = 14;

const STATUS_TXEMPTY: u8 = 0x04;

/// A channel reset is commanded through WR9: bit 7 for A, bit 6 for B.
const MINTR_RST_A: u8 = 0x80;
const MINTR_RST_B: u8 = 0x40;

/// 8 bits per character, transmit enable, RTS and DTR asserted.
const TXCTRL2_8BITS_TXEN: u8 = 0xea;
/// Receive enable with 8 bits per character.
const RXCTRL_8BITS_RXEN: u8 = 0xc1;
/// x16 clock, one stop bit, no parity.
const TXCTRL1_16X_1STOP: u8 = 0x44;
/// Transmit and receive clocks both come from the baud-rate generator.
const CLOCK_BRG: u8 = 0x50;
/// Baud-rate generator enabled with RTxC as its source.
const MISC2_BRG_ENABLE: u8 = 0x01;

/// Input clock of the ESCC on the Mac I/O bus.
const ESCC_CLOCK: u32 = 3_686_400;
/// The console rate used by Open Firmware and the gate.
const CONSOLE_BAUD: u32 = 38_400;
/// WR4 selects a divide-by-16 clock, so the BRG sees clock / 16.
const CLOCK_DIVISOR: u32 = 16;
/// The Z85C30 program value is `clock / (2 * baud * divisor) - 2`.
const BAUD_DIVISOR: u32 = ESCC_CLOCK / (2 * CONSOLE_BAUD * CLOCK_DIVISOR) - 2;

/// The channel control ports, indexed the same way as
/// [`crate::platform::Escc::channels`]: zero is channel A, one is channel B.
/// Zero means the channel was not described by the firmware.
static PORTS: [AtomicU32; 2] = [AtomicU32::new(0), AtomicU32::new(0)];

/// Initialise every channel the device tree described and mirror output to
/// all of them.  A channel that is absent is left alone.
pub fn init(channels: [Option<u32>; 2]) {
    for (index, channel) in channels.iter().enumerate() {
        let Some(control) = *channel else {
            continue;
        };
        let reset = if index == 0 { MINTR_RST_A } else { MINTR_RST_B };
        PORTS[index].store(control, Relaxed);
        init_channel(control, reset);
    }
}

fn init_channel(control: u32, reset: u8) {
    write_register(control, REG_MINTR, reset);
    write_register(control, REG_MINTR, 0x00);
    write_register(control, REG_TXCTRL1, TXCTRL1_16X_1STOP);
    write_register(control, REG_CLOCK, CLOCK_BRG);
    write_register(control, REG_BRGLO, (BAUD_DIVISOR & 0xff) as u8);
    write_register(control, REG_BRGHI, ((BAUD_DIVISOR >> 8) & 0xff) as u8);
    write_register(control, REG_MISC2, MISC2_BRG_ENABLE);
    write_register(control, REG_RXCTRL, RXCTRL_8BITS_RXEN);
    write_register(control, REG_TXCTRL2, TXCTRL2_8BITS_TXEN);
    write_register(control, REG_COMMAND, 0x00);
}

fn write_register(control: u32, register: u8, value: u8) {
    unsafe {
        core::ptr::write_volatile(control as *mut u8, register);
        core::ptr::write_volatile(control as *mut u8, value);
    }
}

fn read_status(control: u32) -> u8 {
    unsafe {
        core::ptr::write_volatile(control as *mut u8, REG_COMMAND);
        core::ptr::read_volatile(control as *const u8)
    }
}

fn write_byte_to(control: u32, byte: u8) {
    while read_status(control) & STATUS_TXEMPTY == 0 {
        core::hint::spin_loop();
    }
    unsafe {
        core::ptr::write_volatile((control + CHANNEL_DATA_OFFSET) as *mut u8, byte);
    }
}

pub fn write_byte(byte: u8) {
    for port in PORTS.iter() {
        let control = port.load(Relaxed);
        if control == 0 {
            continue;
        }
        if byte == b'\n' {
            write_byte_to(control, b'\r');
        }
        write_byte_to(control, byte);
    }
}

pub fn write(bytes: &[u8]) {
    for byte in bytes {
        write_byte(*byte);
    }
}
