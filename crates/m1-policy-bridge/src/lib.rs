// SPDX-License-Identifier: GPL-3.0-only
//! M1 bridge from the slot-B Wasm policy to the native kernel.
//!
//! The bridge occupies the final [`Bridge`]-sized bytes of slot B. The module
//! writes it only after the guest has committed; the kernel reads it after the
//! module returns. It is deliberately a small read-only service surface, not
//! a general module ABI.

#![no_std]
#![forbid(unsafe_code)]

/// Slot B base, matching the module linker and `kernel/src/module.rs`.
pub const SLOT_B_BASE: usize = 0x8848_0000;
/// Slot B capacity, matching the module linker and `kernel/src/module.rs`.
pub const SLOT_B_SIZE: usize = 0x0018_0000;

/// Magic identifying a published M1 policy bridge.
pub const MAGIC: u64 = 0x4d31_504f_4c49_4359; // "M1POLICY"
/// Bridge layout version.
pub const VERSION: u32 = 1;

/// Read-only operations exposed by the committed Wasm policy.
#[repr(C)]
pub struct Bridge {
    /// [`MAGIC`] when published.
    pub magic: u64,
    /// [`VERSION`] when published.
    pub version: u32,
    /// Reserved, must be zero.
    pub reserved: u32,
    /// Opaque `wasm_policy_host::Policy` pointer.
    pub context: usize,
    /// Write the current root OID.
    pub root: unsafe extern "C" fn(usize, *mut u8) -> i32,
    /// Resolve a path and write its OID.
    pub lookup: unsafe extern "C" fn(usize, *const u8, usize, *mut u8) -> i32,
    /// Fetch an object by OID. Returns length or a negative status.
    pub get: unsafe extern "C" fn(usize, *const u8, *mut i32, *mut u8, usize) -> isize,
    /// Check a live capability for an object and rights set.
    pub authorize: unsafe extern "C" fn(usize, *const u8, u32) -> i32,
}

/// Address of the bridge record at the end of slot B.
pub const fn bridge_address() -> usize {
    SLOT_B_BASE + SLOT_B_SIZE - core::mem::size_of::<Bridge>()
}
