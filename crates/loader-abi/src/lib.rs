// SPDX-License-Identifier: GPL-3.0-only
//! Fixed loader/arena service table for the runtime module manager.
//!
//! This is the M1 post-freeze exception surface. It is intentionally small and
//! read-only to the manager: the manager chooses a verified URL and an arena
//! destination, while the kernel performs fetch, SHA-256 verification, copy and
//! cache-safe entry. Resolution, signatures, package metadata and policy live
//! in the downloadable manager module.

#![no_std]
#![forbid(unsafe_code)]

/// Magic identifying the live loader table.
pub const MAGIC: u64 = 0x4c4f_4144_4552_5631; // "LOADERV1"
/// Loader table version.
pub const VERSION: u32 = 1;
/// Fixed kseg0 address of the loader table.
pub const TABLE_ADDR: usize = 0x883f_f000;

/// Arena base, after the 256 KiB runtime-manager image in slot B.
pub const ARENA_BASE: usize = 0x884c_0000;
/// Arena length; slot B ends at `0x8860_0000`.
pub const ARENA_LEN: usize = 0x0014_0000;

/// Fixed status block used by the runtime manager and MCP.
pub const STATUS_ADDR: usize = 0x883f_e000;
/// Status text capacity.
pub const STATUS_TEXT: usize = 1024;
/// Magic written by a live manager.
pub const STATUS_MAGIC: u64 = 0x4d4f_4453_5441_5431; // "MODSTAT1"

/// Runtime module graph status written by the manager.
#[repr(C)]
pub struct ModuleStatus {
    /// [`STATUS_MAGIC`] when populated.
    pub magic: u64,
    /// Status format version.
    pub version: u32,
    /// Valid bytes in `text`.
    pub len: u32,
    /// Human-readable resolved graph and load results.
    pub text: [u8; STATUS_TEXT],
}

/// A runtime module entry.
///
/// The first argument is the loader table (so dependency modules can load or
/// inspect their arena), the second is the manager-provided context pointer.
pub type Entry = unsafe extern "C" fn(*const LoaderTable, *const u8) -> i32;

/// Loader/arena service table.
#[repr(C)]
pub struct LoaderTable {
    /// [`MAGIC`].
    pub magic: u64,
    /// [`VERSION`].
    pub version: u32,
    /// Reserved, must be zero.
    pub reserved: u32,
    /// First writable arena byte.
    pub arena_base: usize,
    /// Arena length.
    pub arena_len: usize,
    /// Fetch a URL into `dest`, verify its SHA-256 and return 0.
    ///
    /// Arguments: URL pointer/length, 32-byte digest pointer, destination
    /// pointer/length. The destination must lie wholly inside the arena.
    pub load: unsafe extern "C" fn(*const u8, usize, *const u8, usize, usize) -> i32,
    /// Fetch a URL into `out` without a digest; returns bytes or -1.
    ///
    /// This is for the package index or other bounded metadata. The destination
    /// is not required to be in the arena.
    pub fetch: unsafe extern "C" fn(*const u8, usize, *mut u8, usize) -> isize,
    /// Flush caches and enter a loaded image: (entry, code length, context).
    pub enter: unsafe extern "C" fn(usize, usize, *const u8) -> i32,
}

/// Constraint for a destination range.
pub const fn in_arena(address: usize, length: usize) -> bool {
    match address.checked_add(length) {
        Some(end) => address >= ARENA_BASE && end <= ARENA_BASE + ARENA_LEN,
        None => false,
    }
}
