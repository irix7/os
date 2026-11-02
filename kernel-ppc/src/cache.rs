// SPDX-License-Identifier: GPL-3.0-only
//! Cache maintenance for code written into RAM.
//!
//! The module slot is mapped cacheable through DBAT2, so a module image that
//! the loader copies and hashes is still partly in the data cache when the
//! first instruction fetch reaches for it.  The 750 has separate data and
//! instruction caches with 32-byte blocks, and a fetch cannot see a dirty data
//! line.  Write every block back, wait for the stores, invalidate the matching
//! instruction blocks, then synchronise the context.

use core::arch::asm;

/// PowerPC 750 data and instruction cache block size.
pub const CACHE_LINE: u32 = 32;

/// Publish `[base, base + len)` to instruction fetch.
///
/// `base` need not be cache aligned; the first and last blocks are whole-line
/// operations.
pub fn flush_code(base: u32, len: usize) {
    if len == 0 {
        return;
    }
    let start = base & !(CACHE_LINE - 1);
    let end = base.saturating_add(len as u32);
    let mut address = start;
    // SAFETY: dcbst is privileged and operates on a cache block.  The range
    // belongs to the module slot, which the kernel owns.
    unsafe {
        while address < end {
            asm!("dcbst 0, {address}", address = in(reg) address, options(nostack));
            address = address.wrapping_add(CACHE_LINE);
        }
        // Complete the writebacks before the instruction blocks are dropped.
        asm!("sync", options(nostack));
        address = start;
        while address < end {
            asm!("icbi 0, {address}", address = in(reg) address, options(nostack));
            address = address.wrapping_add(CACHE_LINE);
        }
        // isync is a context synchronising event: no stale fetch survives it.
        asm!("sync", "isync", options(nostack));
    }
}
