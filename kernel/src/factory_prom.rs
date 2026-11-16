// SPDX-License-Identifier: GPL-3.0-only
//! RAM geometry for a kernel entered from the SGI factory PROM.
//!
//! The factory PROM passes ARCS arguments, so the kernel reads the bank layout
//! from the memory controller the PROM programmed.  `MEMCFG0` and `MEMCFG1`
//! each hold two 16-bit bank fields: bit 13 marks the bank valid, bits 12..8
//! are the size in units minus one and bits 7..0 are the base in units.  The
//! unit is 4 MiB on memory controller revisions below 5 and 16 MiB from
//! revision 5, matching `rom/src/boot/memsize.S`.

/// Physical base of DRAM on the Indy.
pub const LOMEM_BASE: u32 = 0x0800_0000;

const MC_SYSID: *const u32 = 0xBFA0_001C as *const u32;
const MEMCFG0: *const u32 = 0xBFA0_00C4 as *const u32;
const MEMCFG1: *const u32 = 0xBFA0_00CC as *const u32;

const BANK_VALID: u32 = 0x2000;
const BANK_SIZE_MASK: u32 = 0x1f00;
const BANK_BASE_MASK: u32 = 0x00ff;

/// Bytes of DRAM mapped contiguously from [`LOMEM_BASE`].
///
/// A bank mapped above a hole is left out: the register handoff describes one
/// extent.
pub fn contiguous_bytes(mc_revision: u32, memcfg0: u32, memcfg1: u32) -> u32 {
    let shift = if mc_revision >= 5 { 24 } else { 22 };
    let fields = [
        memcfg0 >> 16,
        memcfg0 & 0xffff,
        memcfg1 >> 16,
        memcfg1 & 0xffff,
    ];
    let mut end = LOMEM_BASE as u64;
    // Four passes place four banks in any register order.
    for _ in 0..fields.len() {
        for field in fields {
            if field & BANK_VALID == 0 {
                continue;
            }
            let base = ((field & BANK_BASE_MASK) as u64) << shift;
            let size = ((((field & BANK_SIZE_MASK) >> 8) + 1) as u64) << shift;
            if base <= end && base + size > end {
                end = base + size;
            }
        }
    }
    (end - LOMEM_BASE as u64).min(u32::MAX as u64) as u32
}

/// Read the memory controller and return the contiguous DRAM size.
pub fn ram_bytes() -> u32 {
    // SAFETY: the memory controller registers are a fixed kseg1 MMIO range.
    let (revision, memcfg0, memcfg1) = unsafe {
        (
            core::ptr::read_volatile(MC_SYSID) & 0x0f,
            core::ptr::read_volatile(MEMCFG0),
            core::ptr::read_volatile(MEMCFG1),
        )
    };
    contiguous_bytes(revision, memcfg0, memcfg1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_32_mib_bank_on_an_old_controller() {
        // Valid, 8 units of 4 MiB, base 0x0800_0000 (0x20 units).
        let bank = BANK_VALID | (7 << 8) | 0x20;
        assert_eq!(contiguous_bytes(3, bank << 16, 0), 32 << 20);
    }

    #[test]
    fn two_banks_in_reverse_register_order() {
        let low = BANK_VALID | (7 << 8) | 0x20;
        let high = BANK_VALID | (7 << 8) | 0x28;
        assert_eq!(contiguous_bytes(3, (high << 16) | low, 0), 64 << 20);
    }

    #[test]
    fn a_bank_above_a_hole_is_left_out() {
        let low = BANK_VALID | (7 << 8) | 0x20;
        let far = BANK_VALID | (7 << 8) | 0x40;
        assert_eq!(contiguous_bytes(3, low << 16, far), 32 << 20);
    }

    #[test]
    fn revision_5_uses_16_mib_units() {
        let bank = BANK_VALID | (1 << 8) | 0x08;
        assert_eq!(contiguous_bytes(5, bank, 0), 32 << 20);
    }

    #[test]
    fn no_valid_bank_is_zero() {
        assert_eq!(contiguous_bytes(3, 0, 0), 0);
    }
}
