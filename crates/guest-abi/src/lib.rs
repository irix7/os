// SPDX-License-Identifier: GPL-3.0-only
//! The OS-side re-export of the shared Wasm host-call ABI.
//!
//! The SDK owns the ABI numbers, guest slice/completion layouts and the
//! `i7host` import surface. This crate is the stable OS import path so the
//! core, shim and SDK cannot drift.

#![no_std]

pub use wasm_abi::*;
