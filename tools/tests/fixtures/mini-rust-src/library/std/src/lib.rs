// SPDX-License-Identifier: GPL-3.0-only
// Minimal stand-in for library/std/src/lib.rs in the audit fixtures.
#![no_std]

mod sys;

pub mod os;

// A platform-only reference: the audit must not report it for irix7.
#[cfg(target_os = "linux")]
pub fn linux_only() -> libc::c_int {
    unsafe { libc::statx(0, core::ptr::null(), 0, 0, core::ptr::null_mut()) }
}

// A build-only reference: cfg(test) is false for the audit.
#[cfg(test)]
pub fn test_only() -> libc::c_int {
    unsafe { libc::pthread_getname_np(core::ptr::null_mut(), core::ptr::null_mut(), 0) }
}
