// SPDX-License-Identifier: GPL-3.0-only
//! Minimal C ABI shim for bringing up Rust `std`.
//!
//! This is deliberately not the final POSIX libc.  It implements the small
//! symbol set a static, single-threaded `std` hello needs, so the toolchain
//! and loader can be validated before the relibc fork is brought up.  The
//! functions use the provisional kernel syscall ABI.
#![cfg(target_arch = "mips64")]
#![no_std]
#![feature(asm_experimental_arch)]
#![allow(clippy::missing_safety_doc)]

pub mod mem;
pub mod pthread;
pub mod start;
pub mod sys;
pub mod unwind;

pub use sys::*;
