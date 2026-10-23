// SPDX-License-Identifier: GPL-3.0-only
//! Timer tick and idle wait.
//!
//! The CP0 Count/Compare pair is the only usable periodic timer on the Indy
//! (the 8254's interrupt output is broken).  A small exception trampoline is
//! copied to the general exception vector at kseg0 `0x8000_0180`; it
//! acknowledges Compare by adding a fixed interval to Count and returns with
//! `eret`.  Only `$k0` and `$k1` are used, so no general register context
//! needs saving.
//!
//! With the timer running and `Status.IE` set, the idle loop executes `wait`.
//! iris parks the vCPU while `wait` is outstanding as long as interrupts are
//! enabled, which keeps an idle guest from pegging a host core.

use core::ptr::{read_volatile, write_volatile};

use crate::traps;

/// Timer interval in Count ticks.  At iris's 33 MHz Count rate this is 10 ms.
const INTERVAL: u32 = 330_000;

/// Tick counter incremented by the trampoline.
#[no_mangle]
static mut TICKS: u32 = 0;

core::arch::global_asm!(
    r#"
    .set noreorder
    .set mips3
    .set noat

    .section .text.timer_vector, "ax", @progbits
    .align 4
    .globl timer_vector
timer_vector:
    mfc0    $k0, $13              /* Cause */
    andi    $k0, $k0, 0x7c        /* ExcCode */
    bnez    $k0, 9f
    nop

    /* Compare = Count + INTERVAL (0x50910 = 330000) */
    mfc0    $k0, $9
    lui     $k1, 0x0005
    ori     $k1, $k1, 0x0910
    addu    $k0, $k0, $k1
    mtc0    $k0, $11

    /* TICKS += 1 */
    lui     $k0, %hi(TICKS)
    ori     $k0, $k0, %lo(TICKS)
    lw      $k1, 0($k0)
    addiu   $k1, $k1, 1
    sw      $k1, 0($k0)

    eret
    nop

9:  /* Not an interrupt: report and halt from Rust. */
    lui     $k0, %hi(unexpected_exception)
    ori     $k0, $k0, %lo(unexpected_exception)
    jr      $k0
    nop
    .globl timer_vector_end
timer_vector_end:
"#
);

extern "C" {
    static timer_vector: u8;
    static timer_vector_end: u8;
}

/// Address of the general exception vector (BEV = 0).
const VECTOR: *mut u8 = 0x8000_0180 as *mut u8;

/// Copy the trampoline into the exception vector and start the timer.
pub fn init() {
    // SAFETY: the two symbols bracket the trampoline in kernel text.
    let (src, len) = unsafe {
        let src = &timer_vector as *const u8;
        let len = (&timer_vector_end as *const u8 as usize) - (src as usize);
        (src, len)
    };
    for i in 0..len {
        // SAFETY: the vector lives in DRAM below the kernel image.
        unsafe {
            write_volatile(VECTOR.add(i), read_volatile(src.add(i)));
        }
    }
    // kseg0 is cacheable: publish the written bytes to memory and drop any
    // stale instruction lines before the vector can be taken.
    crate::cache::dcache_writeback_invalidate(VECTOR as usize, len);
    crate::cache::icache_invalidate(VECTOR as usize, len);
    crate::cache::sync();

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
    // SAFETY: the trampoline writes this word from the exception handler.
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

/// Called from the trampoline for any non-interrupt exception.
#[no_mangle]
pub extern "C" fn unexpected_exception() -> ! {
    let cause = traps::cause_read();
    let epc = traps::epc_read();
    let status = traps::status_read();
    crate::print_str("kernel: unexpected exception\r\n  cause=0x");
    crate::print_hex32(cause);
    crate::print_str("\r\n  epc=0x");
    crate::print_hex32(epc);
    crate::print_str("\r\n  status=0x");
    crate::print_hex32(status);
    crate::println("");
    loop {
        core::hint::spin_loop();
    }
}
