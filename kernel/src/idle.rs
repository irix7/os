// SPDX-License-Identifier: GPL-3.0-only
//! Timer tick and idle wait.
//!
//! The CP0 Count/Compare pair is the only usable periodic timer on the Indy
//! (the 8254's interrupt output is broken).  The general exception vector in
//! `traps.rs` keeps a fast path for timer interrupts: it acknowledges Compare
//! by adding a fixed interval to Count, increments [`TICKS`] and returns with
//! `eret`.  Only `$k0` and `$k1` are used on that path.
//!
//! An external interrupt (IP2, the Seeq receive line) is *not* this path.  It
//! takes the full frame, is counted and deferred, and never moves Compare or
//! [`TICKS`]; the 10 ms tick therefore keeps running join, route, lease and
//! migration deadlines at the same rate whether or not frames are arriving.
//!
//! With the timer running and `Status.IE` set, the idle loop executes `wait`.
//! iris parks the vCPU while `wait` is outstanding as long as interrupts are
//! enabled, which keeps an idle guest from pegging a host core.

use core::ptr::read_volatile;

use crate::traps;

/// Timer interval in Count ticks.  At iris's 33 MHz Count rate this is 10 ms.
pub(crate) const INTERVAL: u32 = 330_000;

/// Tick counter incremented by the exception vector fast path.
#[no_mangle]
static mut TICKS: u32 = 0;

/// Completed idle-loop passes since [`init`].
static mut PASSES: u32 = 0;
/// Count ticks spent inside the loop body, excluding time parked in `wait`.
static mut BUSY_COUNTS: u64 = 0;
/// Count ticks covered by the measured window.
static mut TOTAL_COUNTS: u64 = 0;
/// Count at the end of the previous pass, or 0 before the first.
static mut LAST_PARK: u32 = 0;
/// Whether [`LAST_PARK`] holds a real sample.
static mut MEASURING: bool = false;

/// Start the timer and unmask its interrupt.
pub fn init() {
    // `traps::install_vectors` owns the general and TLB-refill vectors now.
    // Acknowledge any stale Compare match, then arm the next one.
    let count = traps::count_read();
    traps::compare_write(count.wrapping_add(INTERVAL));

    // Unmask the timer interrupt (IP7) and enable interrupts.
    let mut status = traps::status_read();
    status |= traps::STATUS_IM7;
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

/// Record one idle-loop pass.
///
/// `entered` is CP0 Count at the top of the pass.  The parked sample is taken
/// here, so the caller does not need to read Count twice.  The counters give
/// the guest's own idle CPU share: the fraction of wall time the loop spends
/// doing work instead of parked in [`wait`].  They say nothing about the host
/// process, which spins on `wait` by design in iris.
pub fn note_pass(entered: u32) {
    let parked = traps::count_read();
    // SAFETY: single-threaded idle thread.
    unsafe {
        PASSES = PASSES.wrapping_add(1);
        if MEASURING {
            let total = parked.wrapping_sub(LAST_PARK) as u64;
            let busy = parked.wrapping_sub(entered) as u64;
            // A pass that wrapped Count contributes a meaningless interval;
            // skip it rather than fold it into the totals.
            if total < (1u64 << 31) && busy <= total {
                TOTAL_COUNTS = TOTAL_COUNTS.saturating_add(total);
                BUSY_COUNTS = BUSY_COUNTS.saturating_add(busy);
            }
        }
        LAST_PARK = parked;
        MEASURING = true;
    }
}

/// What [`note_pass`] has measured.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Load {
    /// Completed passes.
    pub passes: u32,
    /// Microseconds inside the loop body.
    pub busy_us: u64,
    /// Microseconds covered by the window.
    pub total_us: u64,
}

impl Load {
    /// Busy share in whole percent, or 0 before a full window is measured.
    pub fn percent(&self) -> u32 {
        if self.total_us == 0 {
            0
        } else {
            (self.busy_us.saturating_mul(100) / self.total_us) as u32
        }
    }
}

/// The measured idle load, in microseconds at the 33 MHz Count rate.
pub fn load() -> Load {
    // SAFETY: single-threaded; the idle thread writes these counters.
    unsafe {
        Load {
            passes: read_volatile(core::ptr::addr_of!(PASSES)),
            busy_us: read_volatile(core::ptr::addr_of!(BUSY_COUNTS)) / 33,
            total_us: read_volatile(core::ptr::addr_of!(TOTAL_COUNTS)) / 33,
        }
    }
}

/// Park until the next interrupt.
pub fn wait() {
    // SAFETY: interrupts are enabled after init().
    unsafe {
        core::arch::asm!("wait", options(nomem, nostack));
    }
}

/// Park for at most about 0.5 ms.
///
/// The timer fast path re-arms the normal 10 ms interval after this wake, so
/// the short wait is one early observation, not a new polling cadence.  It is
/// used only while a module TCP socket has readiness or has just done work.
pub fn wait_short() {
    let count = traps::count_read();
    traps::compare_write(count.wrapping_add(16_500));
    wait();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn load_percent_is_bounded() {
        assert_eq!(Load::default().percent(), 0);
        let idle = Load {
            passes: 10,
            busy_us: 1,
            total_us: 100,
        };
        assert_eq!(idle.percent(), 1);
        let busy = Load {
            passes: 10,
            busy_us: 100,
            total_us: 100,
        };
        assert_eq!(busy.percent(), 100);
    }
}
