// SPDX-License-Identifier: GPL-3.0-only
//! Global Descriptor Table for x86_64.
//!
//! Long mode uses a flat model: segmentation is effectively disabled.  The
//! GDT still needs a valid code and data segment selector for far jumps and
//! for CPU compatibility, but all descriptors have base 0 and limit max.

const GDT_ENTRIES: usize = 6;

/// Segment descriptor access rights byte.
const AR_PRESENT: u64 = 1 << 7;
const AR_RW: u64 = 1 << 1; // data: writable; code: readable
const AR_EXECUTABLE: u64 = 1 << 3;
const AR_CODE_64BIT: u64 = 1 << 13; // L flag
const AR_DB: u64 = 1 << 14; // D/B flag

#[repr(C, align(8))]
struct GdtEntry {
    limit_low: u16,
    base_low: u16,
    base_mid: u8,
    access: u8,
    flags: u8,
    base_high: u8,
}

impl GdtEntry {
    const fn null() -> Self {
        GdtEntry {
            limit_low: 0,
            base_low: 0,
            base_mid: 0,
            access: 0,
            flags: 0,
            base_high: 0,
        }
    }

    const fn kernel_code() -> Self {
        // Executable, readable, present, ring 0, 64-bit code segment.
        GdtEntry {
            limit_low: 0xFFFF,
            base_low: 0,
            base_mid: 0,
            access: AR_PRESENT as u8 | AR_RW as u8 | AR_EXECUTABLE as u8,
            flags: AR_CODE_64BIT as u8,
            base_high: 0,
        }
    }

    const fn kernel_data() -> Self {
        // Readable/writable, present, ring 0 data segment.
        GdtEntry {
            limit_low: 0xFFFF,
            base_low: 0,
            base_mid: 0,
            access: AR_PRESENT as u8 | AR_RW as u8,
            flags: 0,
            base_high: 0,
        }
    }
}

#[repr(C, align(8))]
struct GdtPtr {
    limit: u16,
    base: u64,
}

static mut GDT: [GdtEntry; GDT_ENTRIES] = [
    GdtEntry::null(),        // 0x00: null
    GdtEntry::null(),        // 0x08: unused
    GdtEntry::kernel_code(), // 0x10: kernel code
    GdtEntry::kernel_data(), // 0x18: kernel data
    GdtEntry::null(),        // 0x20: unused
    GdtEntry::null(),        // 0x28: unused
];

/// Segment selectors.
pub const KERNEL_CODE_SEL: u16 = 0x10;
pub const KERNEL_DATA_SEL: u16 = 0x18;

/// Load the GDT and set the data segment registers.
pub unsafe fn init() {
    let gdt_ptr = GdtPtr {
        limit: (core::mem::size_of::<[GdtEntry; GDT_ENTRIES]>() - 1) as u16,
        base: GDT.as_ptr() as u64,
    };
    core::arch::asm!(
        "lgdt [{0}]",
        "mov ds, {1}",
        "mov es, {1}",
        "mov ss, {1}",
        in(reg) &gdt_ptr,
        const KERNEL_DATA_SEL,
        options(nostack)
    );
    // CS is set by a far jump in the boot entry.
}
