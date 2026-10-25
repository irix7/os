// SPDX-License-Identifier: GPL-3.0-only
//! `guest-abi` — what the WebAssembly core may call.
//!
//! This crate is compiled for `wasm32-unknown-unknown` (or `wasm32v1-none`).
//! Every pointer is a 32-bit offset into the guest's linear memory; the host
//! must validate it against the current memory size.  No native pointer ever
//! crosses this boundary.
//!
//! The ABI is intentionally one generic submission call plus a small number
//! of synchronous helpers.  Operations are numbered by `sys_primitives::OpCode`
//! and completions are written to a guest-owned ring by `host_poll`.

#![no_std]
#![forbid(unsafe_code)]
#![deny(missing_docs)]

/// A pointer and length in the guest's linear memory.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(C)]
pub struct GuestSlice {
    /// Byte offset into linear memory.
    pub ptr: u32,
    /// Byte length.
    pub len: u32,
}

impl GuestSlice {
    /// The empty slice at offset zero.
    pub const EMPTY: Self = Self { ptr: 0, len: 0 };
}

/// A guest-visible completion record.  This is the byte layout the host
/// writes; it mirrors `sys_primitives::Completion`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(C)]
pub struct GuestCompletion {
    /// Submission token.
    pub event: u64,
    /// Request tag.
    pub tag: u32,
    /// Status (negative is an error).
    pub status: i32,
    /// Result kind.
    pub kind: u8,
    /// Completion flags.
    pub flags: u8,
    /// Reserved; zero.
    pub reserved: u16,
    /// Kind-specific value 0.
    pub value0: u32,
    /// Kind-specific value 1.
    pub value1: u64,
}

/// ABI version understood by this crate.
pub const HOST_ABI_VERSION: u32 = 1;

#[link(wasm_import_module = "i7host")]
extern "C" {
    /// Return the host ABI version.  The core refuses to run if the major
    /// version differs.
    pub fn host_version() -> u32;

    /// Submit an operation.
    ///
    /// * `op` is a `sys_primitives::OpCode` discriminant.
    /// * `a0..a3` are operation-specific scalar arguments (handles, offsets,
    ///   lengths, flags).
    /// * `payload` is an optional inline payload in guest memory.  The host
    ///   copies it before returning; it must not retain the pointer.
    ///
    /// Returns a non-zero submission token, or zero if the operation could not
    /// be queued.  The completion is delivered through `host_poll`.
    pub fn host_submit(op: u32, a0: u64, a1: u64, a2: u64, a3: u64, payload: GuestSlice) -> u64;

    /// Drain up to `max` completions into guest memory at `out`.
    /// Returns the number written.
    pub fn host_poll(out: *mut GuestCompletion, max: u32) -> u32;

    /// Cancel a submission by token.  Returns 0 if the request was found,
    /// a negative status otherwise.  Cancellation is best-effort: the
    /// operation may already have committed.
    pub fn host_cancel(token: u64) -> i32;

    /// Check a capability handle synchronously.  `object` points at a 32-byte
    /// `ObjectId` in guest memory.  Returns 0 on success, negative on failure.
    pub fn host_cap_check(handle: u64, rights: u32, object: *const u8) -> i32;

    /// Derive an attenuated capability.  The output is a 320-byte token in
    /// guest memory.  Returns the token length, or a negative status.
    pub fn host_cap_derive(
        parent: u64,
        rights: u32,
        not_after: u64,
        out: *mut u8,
        out_len: u32,
    ) -> i32;

    /// Monotonic nanoseconds since boot.
    pub fn host_now() -> u64;

    /// Fill guest memory with entropy.  Returns 0 on success.
    pub fn host_random(out: *mut u8, len: u32) -> i32;

    /// Write to the shim console.  Bounded and may block briefly.
    pub fn host_console_write(ptr: *const u8, len: u32);

    /// Read console input without blocking.  Returns bytes read.
    pub fn host_console_read(ptr: *mut u8, len: u32) -> u32;

    /// Yield the current actor's fuel slice.  The scheduler will return to it
    /// at the next safepoint; this is a cooperative hint, not a trap.
    pub fn host_yield();

    /// Terminate the current actor with a status.  Does not return.
    pub fn host_exit(status: i32) -> !;
}
