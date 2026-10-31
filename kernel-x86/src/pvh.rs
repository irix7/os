// SPDX-License-Identifier: GPL-3.0-only
//! The Xen PVH start information QEMU passes on entry.
//!
//! QEMU's PVH path starts the 32-bit stub with `%ebx` holding the physical
//! address of [`HvmStartInfo`].  The structure carries the memory map, so the
//! machine reads its DRAM geometry from the loader rather than probing it the
//! way the ROM does on the Indy.

/// `hvm_start_info.magic`.
pub const PVH_MAGIC: u32 = 0x336e_c578;

#[repr(C)]
#[derive(Clone, Copy)]
pub struct HvmMemmapEntry {
    pub addr: u64,
    pub size: u64,
    /// 1 = usable RAM, 2 = reserved, 3 = ACPI reclaimable, 4 = ACPI NVS,
    /// 5 = unusable.
    pub kind: u32,
    pub reserved: u32,
}

#[repr(C)]
pub struct HvmStartInfo {
    pub magic: u32,
    pub version: u32,
    pub flags: u32,
    pub nr_modules: u32,
    pub modlist_paddr: u64,
    pub cmdline_paddr: u64,
    pub rsdp_paddr: u64,
    pub memmap_paddr: u64,
    pub memmap_entries: u32,
    pub reserved: u32,
}

fn info(start_info: u64) -> Option<&'static HvmStartInfo> {
    let info = unsafe { &*(start_info as *const HvmStartInfo) };
    (info.magic == PVH_MAGIC).then_some(info)
}

/// Print the loader's memory map and return the usable-RAM total in bytes.
pub fn describe(start_info: u64) -> u64 {
    let Some(info) = info(start_info) else {
        crate::print_line(&[b"x86: pvh: bad start_info"]);
        return 0;
    };
    crate::serial::write(b"x86: pvh magic=");
    crate::print_hex(info.magic as u64);
    crate::serial::write(b" version=");
    crate::print_dec(info.version as u64);
    crate::serial::write(b" entries=");
    crate::print_dec(info.memmap_entries as u64);
    crate::serial::write_byte(b'\n');

    let mut ram = 0u64;
    for index in 0..info.memmap_entries as usize {
        let entry = unsafe { &*((info.memmap_paddr as usize as *const HvmMemmapEntry).add(index)) };
        crate::serial::write(b"x86: memmap ");
        crate::print_hex(entry.addr);
        crate::serial::write(b"+");
        crate::print_hex(entry.size);
        crate::serial::write(b" type=");
        crate::print_dec(entry.kind as u64);
        crate::serial::write_byte(b'\n');
        if entry.kind == 1 {
            ram = ram.saturating_add(entry.size);
        }
    }
    ram
}
