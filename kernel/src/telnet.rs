// SPDX-License-Identifier: GPL-3.0-only
//! Telnet console server for the kernel (stub).
//!
//! Telnet support will be wired up once the smoltcp API is verified
//! against smoltcp 0.13.

use core::sync::atomic::{AtomicBool, Ordering};

static mut TELNET_ACTIVE: AtomicBool = AtomicBool::new(false);

pub fn init() {
    // Placeholder for telnet server initialization
}

pub fn poll() {
    // stub
}

pub fn write_output(_data: &[u8]) {
    // stub — will send to telnet client when implemented
}

pub fn is_active() -> bool {
    unsafe { TELNET_ACTIVE.load(Ordering::SeqCst) }
}
