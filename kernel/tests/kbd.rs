// SPDX-License-Identifier: GPL-3.0-only
//! Host tests for the PS/2 scancode decoder.
//!
//! The decoder is pure logic, so it is compiled straight from the kernel's
//! source: `src/kbd.rs` has no kernel-only dependencies.

#![no_std]

#[path = "../src/kbd.rs"]
mod kbd;
