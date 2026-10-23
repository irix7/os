// SPDX-License-Identifier: GPL-3.0-only
//! Host tests for the shared byte-range helper.
//!
//! `src/range.rs` is pure logic with no kernel dependencies, so the HTTP and
//! TLS clients' range parsing, length check and header formatting can be
//! exercised on the host without the emulator.

#![no_std]
// The helper is compiled on its own here, so its public entry points have no
// callers in this test binary.
#![allow(dead_code)]

#[path = "../src/range.rs"]
mod range;
