//! Results of the stackless DRAM test.
//!
//! `reset.S` runs `memtest.S` before it creates a stack, copies `.data`, or
//! clears `.bss`, so the test can overwrite every byte of DRAM without
//! destroying the code doing the testing (the code runs from the PROM).  The
//! two results are left at a fixed low-memory slot for Rust to report.

use core::ptr::read_volatile;

/// Low-memory result slot used by reset.S: [0] first failing address
/// (0 = pass), [1] total bytes tested.
const RESULT: *const u32 = 0xA800_0400 as *const u32;

/// First address that failed the test, or 0 if every byte passed.
pub fn first_failure() -> u32 {
    unsafe { read_volatile(RESULT) }
}

/// Total size of the DRAM that passed the test, in bytes.
pub fn tested_ram_size() -> usize {
    unsafe { read_volatile(RESULT.add(1)) as usize }
}
