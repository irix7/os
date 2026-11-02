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

/// Number of 16-byte L1 lines in the R4400's 16 KiB direct-mapped cache.
const LINES: u32 = 16 * 1024 / LINE as u32;

/// Invalidate the whole L1 data cache by index.
///
/// The index operation names every line, so it clears a stale line even when
/// the new user alias has a different physical tag.  The emulator writes back
/// a dirty line before invalidating it.
pub fn dcache_invalidate_all() {
    for line in 0..LINES {
        let index = line << 4;
        // SAFETY: index operations take the address as an index; no memory is
        // dereferenced.
        unsafe {
            core::arch::asm!(".set noat", "cache 0x01, 0({0})", in(reg) index, options(nostack));
        }
    }
    sync();
}

/// Invalidate the L2 lines covering `[physical, physical + length)`.
///
/// Hit_Invalidate(SD) drops a line without writing it back.  A frame the
/// allocator just reused may still have an L2 line with the old virtual PIdx;
/// the next access through the new user alias would otherwise take a Virtual
/// Coherency Exception on the R4400.
pub fn invalidate_l2(physical: u64, length: usize) {
    const L2_LINE: u64 = 128;
    let start = physical & !(L2_LINE - 1);
    let end = physical.saturating_add(length as u64);
    let mut line = start;
    while line < end {
        let alias = ((line as u32) | 0xA000_0000) as usize;
        // SAFETY: kseg1 is the uncached direct map and the cache op only
        // touches the addressed line.
        unsafe {
            core::arch::asm!(".set noat", "cache 0x13, 0({0})", in(reg) alias, options(nostack));
        }
        line += L2_LINE;
    }
    sync();
}

/// Invalidate the whole L1 instruction cache by index.
pub fn icache_invalidate_all() {
    for line in 0..LINES {
        let index = line << 4;
        // SAFETY: index operations take the address as an index; no memory is
        // dereferenced.
        unsafe {
            core::arch::asm!(".set noat", "cache 0x00, 0({0})", in(reg) index, options(nostack));
        }
    }
    sync();
}

/// Complete all outstanding memory operations.
pub fn sync() {
    // SAFETY: `sync` has no operands.
    unsafe {
        core::arch::asm!("sync", options(nostack));
    }
}
