// SPDX-License-Identifier: GPL-3.0-only
//! Provisional native IRIX 7 (n32) C ABI types.
//!
//! This crate is the single Rust home for the native libc's fixed-width C
//! types, public layouts and the constants that `std`/`libc` need.  It is the
//! source of truth that the `libc` `irix7` module and the native libc
//! implementation will be generated from or checked against.
//!
//! It is deliberately **not** an implementation and not a frozen ABI yet:
//!
//! - values marked “open” in [`docs/spec/native-libc.md`][spec] are provisional;
//! - the C oracle and the `native-abi` layout assertions still have to review
//!   every field and constant;
//! - the crate contains no `extern` functions, no syscalls, no libc and no
//!   allocation.
//!
//! [spec]: https://github.com/irix7/os/blob/main/docs/spec/native-libc.md
//!
//! The target is n32: 32-bit pointers and `long`, 64-bit `off_t`, `ino_t`,
//! `dev_t`, `time_t` and `blkcnt_t`, big-endian.  All structs are `repr(C)`
//! and all 64-bit fields are naturally aligned.
//!
//! ```text
//! assert_eq!(core::mem::size_of::<native_abi::stat>(), 128);
//! assert_eq!(core::mem::offset_of!(native_abi::stat, st_mtim), 80);
//! ```

#![no_std]
#![forbid(unsafe_code)]
#![allow(non_camel_case_types)]

pub mod constants;
pub mod types;

pub use constants::*;
pub use types::*;
