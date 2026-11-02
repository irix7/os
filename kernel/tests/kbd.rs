// SPDX-License-Identifier: GPL-3.0-only
//! Host tests for the PS/2 scancode decoder.
//!
//! The decoder is pure logic, so it is compiled straight from the kernel's
//! source.  The console queue in the same file drains the serial receiver;
//! this test supplies an empty source because the kernel's receiver is not
//! part of the decoder.

#![no_std]
// The decoder is compiled on its own here, so its public entry points have
// no callers in this test binary.
#![allow(dead_code)]

fn serial_read_byte() -> Option<u8> {
    None
}

#[path = "../src/kbd.rs"]
mod kbd;
