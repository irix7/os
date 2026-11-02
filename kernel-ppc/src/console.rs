// SPDX-License-Identifier: GPL-3.0-only
//! The kernel console.
//!
//! Every byte goes to the ESCC first, so the serial transcript is unchanged,
//! and then to the framebuffer mirror.  The framebuffer holds bytes printed
//! before it was mapped and replays them when it is initialised, so the boot
//! log appears on the LCD from its first line.

use crate::{fb, serial};

pub fn write_byte(byte: u8) {
    serial::write_byte(byte);
    fb::write_byte(byte);
}

pub fn write(bytes: &[u8]) {
    serial::write(bytes);
    for byte in bytes {
        fb::write_byte(*byte);
    }
}
