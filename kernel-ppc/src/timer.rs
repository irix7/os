// SPDX-License-Identifier: GPL-3.0-only
//! The PowerPC decrementer as the first tick source.
//!
//! The exception vector at 0x900 branches to `decrementer_handler` in
//! `boot.S`.  The handler counts the tick and reloads the decrementer; this
//! module installs the vector, programs the reload value and enables MSR[EE].

use core::arch::asm;
use core::ptr::write_volatile;
use core::sync::atomic::AtomicU32;

pub const MSR_EE: u32 = 1 << 15;
pub const DECREMENTER_VECTOR: u32 = 0x900;

/// The interrupted code's tick count.  `boot.S` increments this directly.
#[no_mangle]
pub static TICKS: AtomicU32 = AtomicU32::new(0);

/// The value `boot.S` writes back to the decrementer after each tick.
#[no_mangle]
pub static mut TICK_RELOAD: u32 = 0;

extern "C" {
    fn decrementer_handler();
}

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

pub fn start(timebase_hz: u32) {
    // A 10 ms period, so ten ticks take a tenth of a second at any plausible
    // timebase frequency.
    let reload = timebase_hz / 100;
    unsafe {
        TICK_RELOAD = reload;
    }
    install();
    write_decrementer(reload);
    enable_interrupts();
}

fn install() {
    let handler = decrementer_handler as *const () as usize as u32;
    let offset = handler.wrapping_sub(DECREMENTER_VECTOR);
    assert!(
        offset < 0x0200_0000,
        "decrementer handler out of branch range"
    );
    // `b` (opcode 18) with the word offset in LI.
    let branch = 0x4800_0000 | (offset & 0x03ff_fffc);
    // SAFETY: 0x900 is the decrementer vector in the firmware's one-to-one
    // mapping.  It is writable, and the cache maintenance below makes the new
    // instruction visible to the instruction fetch.
    unsafe {
        write_volatile(DECREMENTER_VECTOR as *mut u32, branch);
        asm!(
            "dcbst 0, {vector}",
            "sync",
            "icbi 0, {vector}",
            "sync",
            "isync",
            vector = in(reg) DECREMENTER_VECTOR,
            options(nostack),
        );
    }
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
