// SPDX-License-Identifier: GPL-3.0-only
//! MIPS cache maintenance.
//!
//! `CP0 Config.K0` is 3 on the Indy, so kseg0 is **cacheable**.  Anything that
//! DMA reads or writes, and any code written to RAM and then executed, needs
//! explicit maintenance.
//!
//! The R4400 has 16 KiB direct-mapped L1 caches with 16-byte lines.

/// L1 cache line size in bytes.
pub const LINE: usize = 16;

/// Write back and invalidate the data cache over `[addr, addr + len)`.
///
/// `cache 0x15` is Hit Writeback Invalidate (D).
pub fn dcache_writeback_invalidate(addr: usize, len: usize) {
    let mut p = addr & !(LINE - 1);
    let end = addr.saturating_add(len);
    while p < end {
        // SAFETY: cache maintenance on a mapped kernel address.
        unsafe {
            core::arch::asm!(".set noat", "cache 0x15, 0({0})", in(reg) p, options(nostack));
        }
        p += LINE;
    }
}

/// Invalidate the data cache over `[addr, addr + len)`.
///
/// `cache 0x11` is Hit Invalidate (D).  Use this for a buffer the device has
/// just written; writeback would copy stale CPU lines over the new data.
pub fn dcache_invalidate(addr: usize, len: usize) {
    let mut p = addr & !(LINE - 1);
    let end = addr.saturating_add(len);
    while p < end {
        // SAFETY: cache maintenance on a mapped kernel address.
        unsafe {
            core::arch::asm!(".set noat", "cache 0x11, 0({0})", in(reg) p, options(nostack));
        }
        p += LINE;
    }
}

/// Invalidate the instruction cache over `[addr, addr + len)`.
///
/// `cache 0x10` is Hit Invalidate (I).
pub fn icache_invalidate(addr: usize, len: usize) {
    let mut p = addr & !(LINE - 1);
    let end = addr.saturating_add(len);
    while p < end {
        // SAFETY: cache maintenance on a mapped kernel address.
        unsafe {
            core::arch::asm!(".set noat", "cache 0x10, 0({0})", in(reg) p, options(nostack));
        }
        p += LINE;
    }
}

/// Complete all outstanding memory operations.
pub fn sync() {
    // SAFETY: `sync` has no operands.
    unsafe {
        core::arch::asm!("sync", options(nostack));
    }
}
