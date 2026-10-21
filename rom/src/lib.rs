// SPDX-License-Identifier: GPL-3.0-only
//! ROM serial console driver (Z85C30 ESCC inside the IOC2).
//!
//! Hardware facts (from the original IP22 ARCS PROM source):
//! - The PROM console is `DUART1B` = `HPC3_SERIAL1_CMD/DATA`, physical
//!   0x1FBD_9830/0x1FBD_9834, kseg1 0xBFBD_9830/0xBFBD_9834.
//! - In iris those are SCC channel B, exposed on TCP port 8881 (tty1, the
//!   console).  Channel A (0xBFBD_9838/3C) is tty2 on port 8880.
//! - PCLK = 3.672 MHz; PROM default is 9600 8N1, the safe bring-up rate.
//!
//! Only polled output is needed for the ROM's early console; interrupts are
//! left disabled. This driver is deliberately tiny and correct rather than
//! complete: the ROM only needs to print its progress marker and errors.

#![no_std]
#![allow(static_mut_refs)]
#![allow(suspicious_runtime_symbol_definitions)]
#![allow(unused_features)]
#![allow(unused_unsafe)]
#![feature(asm_experimental_arch)]

extern crate alloc;

pub mod aes;
pub mod allocator;
pub mod boot;
pub mod cache;
pub mod dns;
pub mod gloss;
pub mod hex;
pub mod http;
pub mod net;
pub mod ntp;
pub mod ramtest;
pub mod sha256;
pub mod telnet;

/// TLS 1.3 / HTTPS client.  Only compiled with the `tls` feature.
#[cfg(feature = "tls")]
pub mod tls;

use core::ptr::{read_volatile, write_volatile};

/// Z85C30 ESCC register window, kseg1 virtual base.
const Z85C30_BASE: *mut u8 = 0xBFBD_9830 as *mut u8;

/// Channel B control port (DUART1B, the console): base+0.
const CHAN_CTRL: usize = 0;

/// Write a Z85C30 control register on channel B (indirect access through the
/// control port: first select the register, then write its data).
fn wr(reg: u8, data: u8) {
    unsafe {
        let select = if reg < 8 { reg } else { (reg - 8) | 0x08 };
        write_volatile(Z85C30_BASE.add(CHAN_CTRL), select); // select register
        write_volatile(Z85C30_BASE.add(CHAN_CTRL), data); // write data via control port
    }
}

/// Read a Z85C30 control register on channel B.
fn rd(reg: u8) -> u8 {
    unsafe {
        let select = if reg < 8 { reg } else { (reg - 8) | 0x08 };
        write_volatile(Z85C30_BASE.add(CHAN_CTRL), select); // select register
        read_volatile(Z85C30_BASE.add(CHAN_CTRL)) // read from control port
    }
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

/// Exact sequence from the original IP22prom `pon_initio` (pon/pon_io.s):
/// hardware reset, then 9600 8N1 with both clocks from the baud-rate
/// generator, receiver enabled, transmitter enabled last.
pub fn serial_init() {
    wr(WR9, 0xC0); // WR9_HW_RST: reset both SCC channels
    wr(WR4, 0x44); // x16 clock, 1 stop bit, no parity
    wr(WR3, 0xC0); // 8 bits/char, receiver disabled for now
    wr(WR5, 0x28); // 8 bits/char, RTS+DTR, transmitter disabled
    wr(WR11, 0x50); // Rx clock <- BRG, Tx clock <- BRG
    wr(WR12, BAUD_TC); // BRG time constant low (9600)
    wr(WR13, 0x00); // BRG time constant high
    wr(WR14, 0x01); // WR14_BRG_ENBL
    wr(WR3, 0xC1); // 8 bits/char, receiver enable
    wr(WR5, 0x68); // 8 bits/char, Tx enable, RTS+DTR
}

/// Wait until the transmitter holding register is empty.
fn tx_ready() -> bool {
    rd(RR0) & 0x04 != 0 // bit 2: tx buffer empty
}

/// Rolling boot log mirrored to the telnet console when a client connects.
/// The log is linear and truncates at 4 KiB; that is ample for the ROM's
/// progress messages.
const BOOT_LOG_SIZE: usize = 4096;
static mut BOOT_LOG: [u8; BOOT_LOG_SIZE] = [0; BOOT_LOG_SIZE];
static mut BOOT_LOG_LEN: usize = 0;

/// Return the boot messages captured since `rom_main` started.
pub fn boot_log() -> &'static [u8] {
    unsafe { &BOOT_LOG[..BOOT_LOG_LEN] }
}

/// Write one byte to the serial console (polled).
pub fn serial_write_byte(b: u8) {
    unsafe {
        let len = BOOT_LOG_LEN;
        if len < BOOT_LOG_SIZE {
            BOOT_LOG[len] = b;
            BOOT_LOG_LEN = len + 1;
        }
    }
    // Spin until the transmitter can accept another byte.
    while !tx_ready() {
        core::hint::spin_loop();
    }
    unsafe {
        // Select WR8 (transmit data register) and write the byte through the
        // control port; the Z85C30 resets its register pointer afterwards.
        write_volatile(Z85C30_BASE.add(CHAN_CTRL), 0x08);
        write_volatile(Z85C30_BASE.add(CHAN_CTRL), b);
    }
}

/// Write a NUL-terminated string to the serial console.
pub fn serial_write_str(s: &str) {
    for b in s.bytes() {
        serial_write_byte(b);
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
