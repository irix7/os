// SPDX-License-Identifier: GPL-3.0-only
//! The PowerPC decrementer as the first tick source.
//!
//! The exception vector at 0x900 branches to `decrementer_handler` in
//! `boot.S`; `trap` installs it.  The handler counts the tick and reloads the
//! decrementer.  This module programs the reload value and enables MSR[EE]
//! once the kernel's own address translation is active.

use core::arch::asm;
use core::sync::atomic::AtomicU32;

pub const MSR_EE: u32 = 1 << 15;

/// The interrupted code's tick count.  `boot.S` increments this directly.
#[no_mangle]
pub static TICKS: AtomicU32 = AtomicU32::new(0);

/// The value `boot.S` writes back to the decrementer after each tick.
#[no_mangle]
pub static mut TICK_RELOAD: u32 = 0;

pub fn ticks() -> u32 {
    TICKS.load(core::sync::atomic::Ordering::Relaxed)
}

/// The low word of the timebase, used to bound waits.
pub fn timebase() -> u32 {
    let value: u32;
    // SAFETY: TBL is SPR 268 on the 750; reading it is privileged but valid.
    unsafe {
        asm!("mfspr {value}, 268", value = out(reg) value, options(nomem, nostack));
    }
    value
}

/// Program a 10 ms period so ten ticks take a tenth of a second at any
/// plausible timebase frequency.  Interrupts stay disabled.
pub fn prepare(timebase_hz: u32) {
    let reload = timebase_hz / 100;
    unsafe {
        TICK_RELOAD = reload;
    }
    write_decrementer(reload);
}

/// Enable decrementer interrupts; the vector and handler are already mapped.
pub fn enable() {
    enable_interrupts();
}

fn write_decrementer(value: u32) {
    // SAFETY: the decrementer is a privileged SPR, available in kernel mode.
    unsafe {
        asm!("mtspr 22, {value}", value = in(reg) value, options(nostack));
    }
}

fn enable_interrupts() {
    let msr: u32;
    // SAFETY: reading and writing MSR is privileged and valid here.
    unsafe {
        asm!("mfmsr {msr}", msr = out(reg) msr, options(nomem, nostack));
        asm!("mtmsr {msr}", "isync", msr = in(reg) msr | MSR_EE, options(nostack));
    }
}
