// SPDX-License-Identifier: GPL-3.0-only
//! Host tests for the DS1386 environment validation and repair rules.
//!
//! The NVRAM accessors themselves are hardware-only; these tests cover the
//! structural checks that decide whether the kernel rewrites a blank or
//! corrupt environment.

#![no_std]
#![allow(dead_code)]

fn console_println(_: &str) {}

#[path = "../src/nvram.rs"]
mod nvram;
