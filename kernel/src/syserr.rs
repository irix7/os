// SPDX-License-Identifier: GPL-3.0-only
//! Native syscall results.
//!
//! Kernel services return a `CELL_E_*` status on failure, never a libc
//! `errno`.  The plain `syscall` bring-up ABI collapses every error to
//! `u32::MAX` at its boundary; the native gate passes the status to the libc.

pub type SysResult<T> = Result<T, u32>;

pub use native_abi::status::*;
