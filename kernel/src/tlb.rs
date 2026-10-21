// SPDX-License-Identifier: GPL-3.0-only
//! MMU/TLB management.
//!
//! The R4400 has a 48-entry JTLB, each entry mapping an even/odd page pair.
//! PageMask selects 4 KiB–16 MiB in powers of four.
//!
//! Address space layout:
//!   kuseg   0x0000_0000–0x7FFF_FFFF  Legacy IRIX processes (MMU, ASIDs)
//!   kseg0   0x8000_0000–0x9FFF_FFFF  Kernel and native cells (unmapped, cached)
//!   kseg1   0xA000_0000–0xBFFF_FFFF  Early boot, uncached MMIO
//!   kseg2   0xC000_0000–0xFFFF_FFFF  Mapped kernel windows (optional)

#![no_std]

use crate::print_hex8;
use core::ptr::{read_volatile, write_volatile};

use crate::traps::{
    entryhi_read, entryhi_write, entrylo0_read, entrylo0_write, entrylo1_read, entrylo1_write,
    index_read, index_write, pagemask_read, pagemask_write,
};

const C0_PAGEMASK: u32 = 5;
const C0_WIRED: u32 = 6;
const C0_ENTRYHI: u32 = 10;
const C0_ENTRYLO0: u32 = 2;
const C0_ENTRYLO1: u32 = 3;
const C0_INDEX: u32 = 0;
const C0_RANDOM: u32 = 1;

const PAGE_SIZE_4K: u32 = 0x1000;
const PAGE_MASK_MASK: u32 = 0x0000_fe00; // PageMask bits 12-23

// PageMask encodings (size = (1 << (12 + 2*n)) for n=0..7).
const PAGE_MASK_4K: u32 = 0x0000_0000;
const PAGE_MASK_16K: u32 = 0x0000_0400;
const PAGE_MASK_64K: u32 = 0x0000_0c00;
const PAGE_MASK_256K: u32 = 0x0000_1400;
const PAGE_MASK_1M: u32 = 0x0000_1c00;
const PAGE_MASK_4M: u32 = 0x0000_2400;
const PAGE_MASK_16M: u32 = 0x0000_2c00;

// EntryLo fields (R4400).
const ENTRYLO_VALID: u32 = 0x0000_0002;
const ENTRYLO_GLOBAL: u32 = 0x0000_0100;

// EntryHi ASID field (8 bits).
const ENTRYHI_ASID_MASK: u32 = 0x0000_00ff;
const ENTRYHI_VPN2_MASK: u32 = 0xffff_e000;

// JTLB has 48 entries.
const JTLB_ENTRIES: usize = 48;

// Wired entries for kseg2 (wired, never replaced).
const WIRED_COUNT: u32 = 4;

// Current ASID for legacy processes.
static mut NEXT_ASID: u32 = 1;

/// Initialise the TLB: wire entries, invalidate all others.
pub fn init() {
    // Clear PageMask.
    pagemask_write(0);

    // Wire the first WIRED_COUNT entries for kseg2 (if used).
    index_write(0);
    // ... wire entries as needed.

    // Invalidate all remaining entries by writing invalid EntryLo.
    for i in WIRED_COUNT as usize..JTLB_ENTRIES {
        index_write(i as u32);
        entrylo0_write(0);
        entrylo1_write(0);
        // TLBWI - write index into TLB.
        unsafe {
            core::arch::asm!("tlbwi");
        }
    }

    crate::println("tlb: initialised (48-entry JTLB)");
}

/// Map a virtual page to a physical page with given attributes.
///
/// `vpn` is the virtual page number (page-aligned).
/// `pfn` is the physical frame number (page-aligned).
/// `asid` is the address space identifier (0 = kernel/native).
/// `mask` selects the page size.
/// `valid` and `global` control the EntryLo valid/global bits.
pub fn map_page(vpn: u32, pfn: u32, asid: u32, mask: u32, valid: bool, global: bool) {
    pagemask_write(mask);

    // EntryHi: VPN2 (bits 31:13) + ASID (bits 7:0).
    let entryhi = (vpn & PAGE_MASK_VPN2_MASK) | (asid & ENTRYHI_ASID_MASK);
    entryhi_write(entryhi);

    // EntryLo0: even page.
    let mut lo0 = pfn & 0xffff_f000;
    if valid {
        lo0 |= ENTRYLO_VALID;
    }
    if global {
        lo0 |= ENTRYLO_GLOBAL;
    }
    entrylo0_write(lo0);

    // EntryLo1: odd page (same as even for now).
    entrylo1_write(lo0);

    // Write the TLB entry.
    unsafe {
        core::arch::asm!("tlbwr");
    }
}

/// Invalidate a TLB entry by index.
pub fn invalidate_index(index: u32) {
    index_write(index);
    entrylo0_write(0);
    entrylo1_write(0);
    unsafe {
        core::arch::asm!("tlbwi");
    }
}

/// Allocate a new ASID for a legacy process.
pub fn alloc_asid() -> u32 {
    unsafe {
        NEXT_ASID += 1;
        if NEXT_ASID == 0 {
            NEXT_ASID = 1; // ASID 0 = kernel/native
        }
        NEXT_ASID
    }
}

/// PageMask VPN2 mask (for extracting VPN from EntryHi).
const PAGE_MASK_VPN2_MASK: u32 = 0xffff_e000;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn page_mask_encodings() {
        assert_eq!(PAGE_MASK_4K, 0x0000_0000);
        assert_eq!(PAGE_MASK_16M, 0x0000_2c00);
    }

    #[test]
    fn asid_allocation() {
        let a1 = alloc_asid();
        let a2 = alloc_asid();
        assert_ne!(a1, a2);
        assert_ne!(a1, 0); // ASID 0 reserved for kernel/native
    }
}
