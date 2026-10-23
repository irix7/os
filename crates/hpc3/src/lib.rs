// SPDX-License-Identifier: GPL-3.0-only
//! HPC3 DMA descriptor layout and chain rules.
//!
//! The HPC3 fetches descriptors from main memory as 16-byte records: a
//! physical buffer address, a count-and-flags word, the physical address of
//! the next descriptor, and one quadword of padding.  The layout is shared by
//! the PBUS, Ethernet and SCSI engines; this crate currently models the SCSI
//! chain rules used by `kernel/src/scsi.rs`.
//!
//! The types and functions here are pure: no MMIO and no `unsafe`.  Host
//! tests pin the field order, the byte counts, the flags and the
//! zero-length EOX terminator that works around the HPC3 chain bug.
//!
//! Sources: Linux `arch/mips/include/asm/sgi/hpc3.h` (`struct hpc_dma_desc`
//! and `HPCDMA_*`), Linux `drivers/scsi/sgiwd93.c` (8192-byte cap and EOX
//! terminator), NetBSD `sys/arch/sgimips/hpc/hpcreg.h` (`struct
//! hpc_dma_desc`) and `hpc/hpcdma.c` (`hpcdma_sglist_create`).

#![no_std]
#![deny(unsafe_code)]

/// Last descriptor in the chain.  The SCSI engine stops here.
pub const DESC_EOX: u32 = 0x8000_0000;
/// Interrupt the CPU when this descriptor finishes.
pub const DESC_XIE: u32 = 0x2000_0000;
/// Payload byte count.
pub const DESC_COUNT_MASK: u32 = 0x0000_3FFF;

/// Descriptor size including the quadword pad.
pub const DESC_SIZE: usize = 16;

/// Largest payload used per descriptor.
///
/// The count field allows 16383 bytes, and Linux caps SCSI descriptors at
/// 8192 because of a documented HPC3 erratum ("even cntinfo could be up to
/// 16383, without magic only 8192 works correctly").  This crate goes one
/// step further and uses page-sized chunks: the HPC3 specification also says
/// a DMA buffer must not cross a page boundary, and 4096 bytes satisfies
/// both the erratum cap and a 4 KiB page.
pub const MAX_DESC_LEN: usize = 4096;

/// One HPC3 DMA descriptor.
///
/// `pad` keeps the next descriptor on a quadword boundary, as both Linux's
/// `struct hpc_chunk` and NetBSD's `struct hpc_dma_desc` do.
#[repr(C, align(16))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DmaDesc {
    /// Physical address of the data buffer.
    pub addr: u32,
    /// [`DESC_EOX`] | [`DESC_XIE`] | byte count.
    pub flags: u32,
    /// Physical address of the next descriptor, or zero after EOX.
    pub next: u32,
    /// Padding to the next quadword.
    pub pad: u32,
}

impl DmaDesc {
    /// An all-zero descriptor.
    pub const ZERO: Self = Self {
        addr: 0,
        flags: 0,
        next: 0,
        pad: 0,
    };

    /// Payload byte count carried by this descriptor.
    pub const fn count(&self) -> usize {
        (self.flags & DESC_COUNT_MASK) as usize
    }

    /// Whether this descriptor is marked end-of-chain.
    pub const fn is_eox(&self) -> bool {
        self.flags & DESC_EOX != 0
    }
}

/// Number of data descriptors needed for `len` bytes.
///
/// Returns `None` for an empty transfer; the engine must not be started with
/// a zero-length data phase.
pub const fn data_descriptors(len: usize) -> Option<usize> {
    if len == 0 {
        None
    } else {
        Some(len.div_ceil(MAX_DESC_LEN))
    }
}

/// Payload length of descriptor `index` in a `len`-byte transfer.
///
/// The final descriptor carries the remainder; every earlier descriptor is
/// [`MAX_DESC_LEN`] bytes.
pub const fn chunk_len(len: usize, index: usize) -> Option<usize> {
    let count = match data_descriptors(len) {
        Some(count) => count,
        None => return None,
    };
    if index >= count {
        return None;
    }
    if index + 1 == count {
        // The remainder is 1..=MAX_DESC_LEN for a non-empty transfer.
        Some(len - index * MAX_DESC_LEN)
    } else {
        Some(MAX_DESC_LEN)
    }
}

/// Build a descriptor chain for `len` bytes at `buf_phys`.
///
/// Descriptors are written from `descs[0]`; their `next` pointers are derived
/// from `desc_phys`, which must be the physical address of `descs[0]`.  The
/// chain ends in a zero-length descriptor with [`DESC_EOX`] set and a zero
/// `next` pointer, the workaround both Linux and NetBSD use for the HPC3
/// chain bug.
///
/// The final data descriptor carries [`DESC_XIE`]; earlier descriptors do
/// not.  Returns the number of descriptors written, including the
/// terminator, or `None` if `descs` is too small or an address overflows
/// 32 bits.
pub fn build_chain(
    descs: &mut [DmaDesc],
    buf_phys: u32,
    len: usize,
    desc_phys: u32,
) -> Option<usize> {
    let data = data_descriptors(len)?;
    if descs.len() < data + 1 || len > u32::MAX as usize {
        return None;
    }
    // The whole buffer, not just its start, must fit in the 32-bit physical
    // address space.
    buf_phys.checked_add(len as u32)?;

    let mut offset = 0usize;
    for (index, desc) in descs.iter_mut().enumerate().take(data) {
        let count = chunk_len(len, index)?;
        let addr = buf_phys.checked_add(offset as u32)?;
        let next_offset = ((index + 1) * DESC_SIZE) as u32;
        let next = desc_phys.checked_add(next_offset)?;
        let mut flags = count as u32;
        if index + 1 == data {
            flags |= DESC_XIE;
        }
        *desc = DmaDesc {
            addr,
            flags,
            next,
            pad: 0,
        };
        offset += count;
    }

    descs[data] = DmaDesc {
        addr: 0,
        flags: DESC_EOX,
        next: 0,
        pad: 0,
    };
    Some(data + 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn descriptor_is_a_quadword() {
        assert_eq!(core::mem::size_of::<DmaDesc>(), 16);
        assert_eq!(core::mem::align_of::<DmaDesc>(), 16);
    }

    #[test]
    fn one_sector_has_one_data_descriptor_and_terminator() {
        let mut descs = [DmaDesc::ZERO; 2];
        let written = build_chain(&mut descs, 0x08B0_0000, 512, 0x08B1_0000).unwrap();
        assert_eq!(written, 2);
        assert_eq!(descs[0].addr, 0x08B0_0000);
        assert_eq!(descs[0].count(), 512);
        assert_eq!(descs[0].flags & DESC_XIE, DESC_XIE);
        assert_eq!(descs[0].flags & DESC_EOX, 0);
        assert_eq!(descs[0].next, 0x08B1_0010);
        assert!(descs[1].is_eox());
        assert_eq!(descs[1].count(), 0);
        assert_eq!(descs[1].addr, 0);
        assert_eq!(descs[1].next, 0);
    }

    #[test]
    fn page_sized_chunks_split_large_reads() {
        assert_eq!(data_descriptors(20 * 1024), Some(5));
        assert_eq!(chunk_len(20 * 1024, 0), Some(4096));
        assert_eq!(chunk_len(20 * 1024, 3), Some(4096));
        assert_eq!(chunk_len(20 * 1024, 4), Some(20 * 1024 - 4 * 4096));
    }

    #[test]
    fn chain_addresses_advance_by_chunk() {
        let mut descs = [DmaDesc::ZERO; 6];
        let written = build_chain(&mut descs, 0x0900_0000, 20 * 1024, 0x08B1_0000).unwrap();
        assert_eq!(written, 6);
        assert_eq!(descs[0].addr, 0x0900_0000);
        assert_eq!(descs[1].addr, 0x0900_1000);
        assert_eq!(descs[2].addr, 0x0900_2000);
        assert_eq!(descs[0].next, 0x08B1_0010);
        assert_eq!(descs[1].next, 0x08B1_0020);
        assert_eq!(descs[2].next, 0x08B1_0030);
        assert_eq!(descs[4].flags & DESC_XIE, DESC_XIE);
        assert!(descs[5].is_eox());
    }

    #[test]
    fn only_the_last_data_descriptor_requests_an_interrupt() {
        let mut descs = [DmaDesc::ZERO; 5];
        build_chain(&mut descs, 0x0900_0000, 4 * 4096, 0x08B1_0000).unwrap();
        for desc in &descs[..3] {
            assert_eq!(desc.flags & DESC_XIE, 0);
        }
        assert_ne!(descs[3].flags & DESC_XIE, 0);
    }

    #[test]
    fn empty_and_oversized_requests_are_rejected() {
        let mut descs = [DmaDesc::ZERO; 4];
        assert_eq!(build_chain(&mut descs, 0x0900_0000, 0, 0x08B1_0000), None);
        assert_eq!(data_descriptors(0), None);
        // Four data descriptors plus the terminator do not fit in four
        // slots.
        assert_eq!(
            build_chain(&mut descs, 0x0900_0000, 4 * 4096, 0x08B1_0000),
            None
        );
        assert_eq!(chunk_len(512, 1), None);
    }

    #[test]
    fn address_overflow_is_rejected() {
        let mut descs = [DmaDesc::ZERO; 2];
        assert_eq!(
            build_chain(&mut descs, u32::MAX - 511, 512, 0x08B1_0000),
            None
        );
    }
}
