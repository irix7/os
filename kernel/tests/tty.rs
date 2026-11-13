// SPDX-License-Identifier: GPL-3.0-only
//! Host tests for the console line discipline.
//!
//! `Tty` is pure logic, so it is compiled straight from the kernel's source.
//! The module's console glue reads the keyboard queue, writes the console and
//! signals a process group; this test supplies inert versions of those.

#![no_std]
// The glue functions have no callers in this test binary.
#![allow(dead_code)]

mod kbd {
    pub fn read_input_byte() -> Option<u8> {
        None
    }
}

mod process {
    pub fn signal_group(_group: u32, _signal: i32) -> bool {
        false
    }
}

mod graphics_console {
    pub fn flush() {}
}

fn write_bytes(_bytes: &[u8]) {}

#[path = "../src/tty.rs"]
mod tty;
