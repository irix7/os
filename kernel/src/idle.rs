// SPDX-License-Identifier: GPL-3.0-only
//! Timer tick and idle wait.
//!
//! The CP0 Count/Compare pair is the only usable periodic timer on the Indy
//! (the 8254's interrupt output is broken).  The general exception vector in
//! `traps.rs` keeps a fast path for timer interrupts: it acknowledges Compare
//! by adding a fixed interval to Count, increments [`TICKS`] and returns with
//! `eret`.  Only `$k0` and `$k1` are used on that path.
//!
//! With the timer running and `Status.IE` set, the idle loop executes `wait`.
//! iris parks the vCPU while `wait` is outstanding as long as interrupts are
//! enabled, which keeps an idle guest from pegging a host core.

use core::ptr::{read_volatile, write_volatile};

use crate::traps;

/// Timer interval in Count ticks.  At iris's 33 MHz Count rate this is 10 ms.
pub(crate) const INTERVAL: u32 = 330_000;

/// Tick counter incremented by the exception vector fast path.
#[no_mangle]
static mut TICKS: u32 = 0;

/// Start the timer and unmask its interrupt.
pub fn init() {
    // `traps::install_vectors` owns the general and TLB-refill vectors now.
    // Acknowledge any stale Compare match, then arm the next one.
    let count = traps::count_read();
    traps::compare_write(count.wrapping_add(INTERVAL));

    // Unmask the timer interrupt (IP7) and enable interrupts.
    let mut status = traps::status_read();
    status |= 1 << 15; // IM7
    status |= 1; // IE
    traps::status_write(status);
}

/// Number of timer ticks since [`init`].
pub fn ticks() -> u32 {
    // SAFETY: the exception vector writes this word.
    unsafe { read_volatile(core::ptr::addr_of!(TICKS)) }
}

/// Uptime in milliseconds according to the timer tick.
pub fn uptime_ms() -> u64 {
    ticks() as u64 * (INTERVAL as u64 * 1000 / 33_000_000)
}

/// Park until the next interrupt.
pub fn wait() {
    // SAFETY: interrupts are enabled after init().
    unsafe {
        core::arch::asm!("wait", options(nomem, nostack));
    }
}
