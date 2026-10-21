// SPDX-License-Identifier: GPL-3.0-only
//! Serial console driver (Z85C30 ESCC inside the IOC2).
//!
//! Hardware facts (see `docs/research/indy-hardware.md` §15.3):
//! - Z85C30 ESCC base physical 0x1FBD_9830, mapped at kseg1 0xBFBD_9830.
//! - Indirect register access: write the register number to `(reg << 2) + 3`,
//!   then the data byte to the same address (byte in lane 3).
//! - Ports are wired in reverse: channel A = rear-panel "Serial Port 1"
//!   = `serial0`.
//! - PCLK = 3.672 MHz; PROM default is 9600 8N1, the safe bring-up rate.
//!
//! Polled output only; interrupts are left disabled. This is the early
//! bring-up console used before the console cell starts.

#![no_std]

use core::ptr::{read_volatile, write_volatile};

/// Z85C30 ESCC register window, kseg1 virtual base.
const Z85C30_BASE: *mut u8 = 0xBFBD_9830 as *mut u8;

/// Write a Z85C30 control register (indirect, lane 3).
fn wr(reg: u8, data: u8) {
    unsafe {
        write_volatile(Z85C30_BASE.add(((reg as usize) << 2) + 3), data);
    }
}

/// Read a Z85C30 control register (indirect, lane 3).
fn rd(reg: u8) -> u8 {
    unsafe { read_volatile(Z85C30_BASE.add(((reg as usize) << 2) + 3)) }
}

/// Z85C30 register numbers.
#[allow(dead_code)]
const WR0: u8 = 0; // commands
#[allow(dead_code)]
const WR1: u8 = 1; // int enables
#[allow(dead_code)]
const WR3: u8 = 3; // rx control
#[allow(dead_code)]
const WR4: u8 = 4; // x16 clock, stop bits, parity
#[allow(dead_code)]
const WR5: u8 = 5; // tx control
#[allow(dead_code)]
const WR9: u8 = 9; // master int ctrl / reset
#[allow(dead_code)]
const WR11: u8 = 11; // clock mode
#[allow(dead_code)]
const WR12: u8 = 12; // baud rate low
#[allow(dead_code)]
const WR13: u8 = 13; // baud rate high
#[allow(dead_code)]
const WR14: u8 = 14; // misc / BRG source
#[allow(dead_code)]
const RR0: u8 = 0; // status: bit 2 = tx buffer empty

/// Time constant for ~9600 baud at PCLK = 3.672 MHz:
///   baud = PCLK / (32 * (tc + 2))  =>  tc + 2 = 3672000 / (32 * 9600) ≈ 12
///   tc = 10 gives 9562 baud, within tolerance of 9600.
const BAUD_TC: u8 = 10;

/// Initialise channel A for 9600 8N1, polled output only.
pub fn init() {
    // Force a channel reset, then master interrupt enable.
    wr(WR9, 0xB0); // channel reset (channel A)
    wr(WR4, 0x44); // x16 clock, 1 stop bit, no parity
    wr(WR11, 0x50); // txclk = BRG, rxclk = BRG
    wr(WR12, BAUD_TC); // baud rate low byte
    wr(WR13, 0x00); // baud rate high byte
    wr(WR14, 0x01); // BRG source = PCLK
    wr(WR3, 0xC1); // rx 8 bits/char, rx enable
    wr(WR5, 0x68); // tx 8 bits/char, tx enable, DTR
    wr(WR1, 0x00); // all interrupts off
    wr(WR9, 0x08); // master interrupt enable (no vector includes status)
}

/// Wait until the transmitter holding register is empty.
fn tx_ready() -> bool {
    rd(RR0) & 0x04 != 0 // bit 2: tx buffer empty
}

/// Write one byte to the serial console (polled).
pub fn write_byte(b: u8) {
    // Spin until the transmitter can accept another byte.
    while !tx_ready() {
        core::hint::spin_loop();
    }
    unsafe {
        write_volatile(Z85C30_BASE.add(3), b); // data register at lane 3 offset 3
    }
}

/// Write a NUL-terminated string to the serial console.
pub fn write_str(s: &str) {
    for b in s.bytes() {
        write_byte(b);
    }
}

/// Write a string to the serial console.
pub fn print_str(s: &str) {
    write_str(s);
}

/// Write a string followed by a newline.
pub fn println(s: &str) {
    write_str(s);
    write_byte(b'\r');
    write_byte(b'\n');
}

/// Print a hex byte.
pub fn print_hex8(v: u8) {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    write_byte(HEX[(v >> 4) as usize]);
    write_byte(HEX[(v & 0x0F) as usize]);
}

/// Print a string.
#[macro_export]
macro_rules! print {
    ($($arg:tt)*) => {{
        use core::fmt::Write;
        let _ = write!(&mut crate::console::ConsoleWriter, $($arg)*);
    }};
}

/// Print a string followed by a newline.
#[macro_export]
macro_rules! println {
    ($($arg:tt)*) => {{
        use core::fmt::Write;
        let _ = writeln!(&mut crate::console::ConsoleWriter, $($arg)*);
    }};
}

/// A console writer that implements `core::fmt::Write`.
pub struct ConsoleWriter;

impl core::fmt::Write for ConsoleWriter {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        for b in s.bytes() {
            write_byte(b);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn baud_time_constant_is_reasonable() {
        // 3672000 / (32 * (10 + 2)) = 9562.5 Hz, close to 9600.
        let baud = 3672000u32 / (32u32 * (BAUD_TC as u32 + 2));
        assert!(baud > 9000 && baud < 10000);
    }
}