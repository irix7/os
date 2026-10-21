#![allow(dead_code)]

//! R4000-series cache initialisation.
//!
//! The R4000 powers up with caches disabled and in an undefined state. We must:
//!   1. Invalidate the entire I-cache (index-based, all lines)
//!   2. Invalidate (writeback+invalidate) the entire D-cache (index-based)
//!   3. Set Config.K0 to write-back for kseg0
//!   4. Enable caches via Status (CU, IM, IE bits)
//!
//! Cache line size: 16 bytes on R4000/R4400, 32 bytes on R4600/R5000.
//! We assume 16 bytes (R4000/R4400 are the minimum MIPS III target).
//!
//! I-cache size: 8 KB (R4000SC) → 512 lines, or 16 KB (R4400/R4600) → 1024 lines.
//! D-cache size: same as I-cache.
//!
//! We use 16 KB (1024 lines) as a safe upper bound; extra iterations on a
//! smaller cache are harmless (they just re-invalidate already-invalid lines).

//! Cache op codes for the `cache` instruction. The op field is 5 bits
//! (instruction bits 20..=16): op 0 = Index_Invalidate_I, op 1 =
//! Index_Writeback_Invalidate_D. Offset is 0 (address comes from base reg).
const CACHE_OP_INDEX_INVALIDATE_I: u32 = 0;
const CACHE_OP_INDEX_WBACK_INV_D: u32 = 1;

/// Cache line size in bytes (R4000/R4400 = 16).
const CACHE_LINE_SIZE: usize = 16;

/// Number of cache lines to invalidate (16 KB / 16 B = 1024 lines).
const CACHE_LINES: usize = 1024;

/// CP0 register numbers (immediates in mfc0/mtc0).
const CP0_CONFIG: u32 = 16; // $16
const CP0_STATUS: u32 = 12; // $12

/// Status register bits.
const STATUS_CU0: u32 = 1 << 28; // coprocessor 0 usable
const STATUS_IM: u32 = 0xFF00; // interrupt mask
const STATUS_IE: u32 = 1 << 0; // interrupt enable

/// Config.K0 field (bits 2:0) selects kseg0 cacheability:
///   0b010 = uncached, 0b011 = write-back (with write allocation)
const CONFIG_K0_WRITEBACK: u32 = 0b011;

/// Read CP0 register (hardcoded register number as immediate).
#[cfg(target_arch = "mips64")]
unsafe fn cp0_read(cpr: u32) -> u32 {
    let out: u32;
    match cpr {
        CP0_CONFIG => core::arch::asm!("mfc0 {0}, $16", out(reg) out),
        CP0_STATUS => core::arch::asm!("mfc0 {0}, $12", out(reg) out),
        _ => core::arch::asm!("mfc0 {0}, $16", out(reg) out),
    }
    out
}

/// Write CP0 register (hardcoded register number as immediate).
#[cfg(target_arch = "mips64")]
unsafe fn cp0_write(cpr: u32, val: u32) {
    match cpr {
        CP0_CONFIG => core::arch::asm!("mtc0 {0}, $16", in(reg) val),
        CP0_STATUS => core::arch::asm!("mtc0 {0}, $12", in(reg) val),
        _ => core::arch::asm!("mtc0 {0}, $16", in(reg) val),
    }
}

/// Execute the `cache` instruction with a hardcoded op and an address register.
#[cfg(target_arch = "mips64")]
unsafe fn cache_op(op: u32, addr: u32) {
    match op {
        CACHE_OP_INDEX_INVALIDATE_I => {
            core::arch::asm!(".set noat\n\tcache 0, 0({0})", in(reg) addr);
        }
        CACHE_OP_INDEX_WBACK_INV_D => {
            core::arch::asm!(".set noat\n\tcache 1, 0({0})", in(reg) addr);
        }
        _ => {
            core::arch::asm!(".set noat\n\tcache 0, 0({0})", in(reg) addr);
        }
    }
}

/// Initialise the R4000 caches: invalidate I and D caches, set kseg0
/// write-back, enable caches.
///
/// # Safety
/// Must run in kernel mode with interrupts disabled, before any cached
/// access to kseg0, and only on an R4000/R4400-class MIPS III CPU.
#[cfg(target_arch = "mips64")]
pub unsafe fn init() {
    let base: u32 = 0x8000_0000; // kseg0, start of RAM/cacheable region

    // 1. Invalidate the entire I-cache (index-based).
    for i in 0..CACHE_LINES {
        cache_op(
            CACHE_OP_INDEX_INVALIDATE_I,
            base + (i * CACHE_LINE_SIZE) as u32,
        );
    }

    // 2. Writeback+invalidate the entire D-cache (index-based).
    for i in 0..CACHE_LINES {
        cache_op(
            CACHE_OP_INDEX_WBACK_INV_D,
            base + (i * CACHE_LINE_SIZE) as u32,
        );
    }

    // 3. Ensure all writes are visible before changing cache mode.
    core::arch::asm!("sync");

    // 4. Set Config.K0 = write-back for kseg0.
    let mut config = cp0_read(CP0_CONFIG);
    config &= !0b111; // clear K0 (bits 2:0)
    config |= CONFIG_K0_WRITEBACK;
    cp0_write(CP0_CONFIG, config);

    // 5. Enable kseg0 caching.  Interrupts stay disabled through the kernel
    // hand-off; the kernel enables them when it is ready.
    let mut status = cp0_read(CP0_STATUS);
    status |= STATUS_CU0 | STATUS_IM;
    cp0_write(CP0_STATUS, status);

    // 6. Final sync to ensure config takes effect.
    core::arch::asm!("sync");
}

// Host builds only need this module to compile for `cargo test`/`clippy`; the
// real implementation is MIPS-only.
#[cfg(not(target_arch = "mips64"))]
unsafe fn cp0_read(_cpr: u32) -> u32 {
    0
}

#[cfg(not(target_arch = "mips64"))]
unsafe fn cp0_write(_cpr: u32, _val: u32) {}

#[cfg(not(target_arch = "mips64"))]
unsafe fn cache_op(_op: u32, _addr: u32) {}

/// Host stub; cache initialisation is MIPS-only.
///
/// # Safety
/// No-op on non-MIPS hosts.
#[cfg(not(target_arch = "mips64"))]
pub unsafe fn init() {}
