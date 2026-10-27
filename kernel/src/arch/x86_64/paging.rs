// SPDX-License-Identifier: GPL-3.0-only
//! 4-level x86_64 paging for the early kernel.
//!
//! The kernel is linked at a fixed physical address and identity-mapped: the
//! first 4 GiB of physical memory are mapped at the same virtual address.
//! This keeps the bring-up simple while giving the core a normal paged,
//! protected address space.  A later slice can switch to a high kernel map
//! and per-process page tables.
//!
//! Page table entries:
//!   bit 0   Present
//!   bit 1   Read/Write
//!   bit 2   User/Supervisor
//!   bit 3   Page-level Write-Through
//!   bit 4   Page-level Cache Disable
//!   bit 7   Page Size (huge pages, only in PD/PDPT)
//!   bits 11-51 physical address (4 KiB aligned)
//!   bit 63  No-Execute

use crate::arch::interface::Arch;

const PAGE_SIZE: usize = 4096;
const ENTRY_PRESENT: u64 = 1 << 0;
const ENTRY_WRITABLE: u64 = 1 << 1;
const ENTRY_USER: u64 = 1 << 2;
const ENTRY_PWT: u64 = 1 << 3;
const ENTRY_PCD: u64 = 1 << 4;
const ENTRY_HUGE: u64 = 1 << 7;
const ENTRY_ADDR_MASK: u64 = 0x000F_FFFF_FFFF_F000;

// Table sizes.  Each table is one page holding 512 entries of 8 bytes.
const TABLE_ENTRIES: usize = 512;

// PML4 index: bits 47:39, PDPT: 38:30, PD: 29:21, PT: 20:12, offset: 11:0.
const PML4_SHIFT: usize = 39;
const PDPT_SHIFT: usize = 30;
const PD_SHIFT: usize = 21;
const PT_SHIFT: usize = 12;

/// Index into a table for a virtual address.
#[inline]
fn pml4_index(vaddr: usize) -> usize {
    (vaddr >> PML4_SHIFT) & 0x1FF
}
#[inline]
fn pdpt_index(vaddr: usize) -> usize {
    (vaddr >> PDPT_SHIFT) & 0x1FF
}
#[inline]
fn pd_index(vaddr: usize) -> usize {
    (vaddr >> PD_SHIFT) & 0x1FF
}
#[inline]
fn pt_index(vaddr: usize) -> usize {
    (vaddr >> PT_SHIFT) & 0x1FF
}

/// A page-aligned physical frame allocated for a page table.
struct PageTableFrame {
    phys: usize,
}

/// One level of a page table: 512 8-byte entries in a single page.
#[repr(C, align(4096))]
struct PageTable {
    entries: [u64; TABLE_ENTRIES],
}

impl PageTable {
    fn zeroed() -> Self {
        PageTable {
            entries: [0; TABLE_ENTRIES],
        }
    }

    fn entry_mut(&mut self, index: usize) -> &mut u64 {
        &mut self.entries[index]
    }

    fn as_phys(&self) -> usize {
        self as *const PageTable as usize
    }
}

/// Early page-table state.  Allocated statically so the boot path needs no
/// allocator.
static mut PML4: Option<PageTableFrame> = None;

/// Map a 4 KiB page at `vaddr` to `phys` with the given flags.  Walks (and
/// allocates) intermediate tables on demand.
///
/// # Safety
/// Caller must ensure `vaddr` and `phys` are page-aligned and that any
/// intermediate table pages are reserved (not handed to the frame allocator).
unsafe fn map_page(pml4: &mut PageTable, vaddr: usize, phys: usize, flags: u64) {
    let p4i = pml4_index(vaddr);
    let pdpt = get_or_create_table(pml4.entry_mut(p4i));
    let pdpti = pdpt_index(vaddr);
    let pd = get_or_create_table(pdpt.entry_mut(pdpti));
    let pdi = pd_index(vaddr);
    let pt = get_or_create_table(pd.entry_mut(pdi));
    let pti = pt_index(vaddr);
    pt.entry_mut(pti)
        .write_volatile((phys as u64 & ENTRY_ADDR_MASK) | flags | ENTRY_PRESENT | ENTRY_WRITABLE);
}

/// Follow or create the next-level table pointed at by `entry`.
unsafe fn get_or_create_table(entry: &mut u64) -> &'static mut PageTable {
    if *entry & ENTRY_PRESENT == 0 {
        // Allocate a fresh page for the table.  Bump from a reserved area.
        static mut NEXT_TABLE: usize = 0;
        let phys = NEXT_TABLE;
        NEXT_TABLE += PAGE_SIZE;
        *entry = (phys as u64 & ENTRY_ADDR_MASK) | ENTRY_PRESENT | ENTRY_WRITABLE;
        &mut *(phys as *mut PageTable)
    } else {
        &mut *((*entry & ENTRY_ADDR_MASK) as *mut PageTable)
    }
}

/// Identity-map `[start, end)` (page-aligned) with `flags`.
unsafe fn identity_map_range(pml4: &mut PageTable, start: usize, end: usize, flags: u64) {
    let mut vaddr = start;
    while vaddr < end {
        map_page(pml4, vaddr, vaddr, flags);
        vaddr += PAGE_SIZE;
    }
}

/// Initialise the early page tables and return the physical address of the
/// PML4.  Maps the low 4 GiB identity, plus the kernel image region.
///
/// # Safety
/// Called once at boot before paging is enabled.  The table pages must not be
/// reclaimed by the frame allocator.
pub unsafe fn init_paging() -> usize {
    let pml4_phys = reserve_table_frame();
    PML4 = Some(PageTableFrame { phys: pml4_phys });
    let pml4 = &mut *(pml4_phys as *mut PageTable);

    // Identity-map the first 4 GiB (read/write, kernel).
    identity_map_range(pml4, 0, 0x1_0000_0000, 0);

    PML4.as_ref().unwrap().phys
}

/// Reserve a page-aligned frame for a page table from the early reserved
/// area.  The kernel image and stack are below 0x10_0000; start tables at
/// 0x20_0000 and bump upwards.
unsafe fn reserve_table_frame() -> usize {
    static mut NEXT_TABLE: usize = 0x20_0000;
    let phys = NEXT_TABLE;
    NEXT_TABLE += PAGE_SIZE;
    phys
}

/// Read the current CR3 (page-table base).
pub fn cr3_read() -> usize {
    let cr3: usize;
    unsafe { core::arch::asm!("mov {0}, cr3", out(reg) cr3) }
    cr3
}

/// Write CR3 to activate a page table.
pub fn cr3_write(value: usize) {
    unsafe { core::arch::asm!("mov cr3, {0}", in(reg) value, options(nostack)) }
}

/// Read CR0.
pub fn cr0_read() -> usize {
    let cr0: usize;
    unsafe { core::arch::asm!("mov {0}, cr0", out(reg) cr0) }
    cr0
}

/// Write CR0.
pub fn cr0_write(value: usize) {
    unsafe { core::arch::asm!("mov cr0, {0}", in(reg) value, options(nostack)) }
}

/// Read CR4.
pub fn cr4_read() -> usize {
    let cr4: usize;
    unsafe { core::arch::asm!("mov {0}, cr4", out(reg) cr4) }
    cr4
}

/// Write CR4.
pub fn cr4_write(value: usize) {
    unsafe { core::arch::asm!("mov cr4, {0}", in(reg) value, options(nostack)) }
}

/// Read the EFER MSR.
pub fn efer_read() -> u64 {
    let v: u64;
    unsafe { core::arch::asm!("rdmsr", out(rax) v, in(rcx) 0xC0000080u32, options(nostack)) }
    v
}

/// Write the EFER MSR.
pub fn efer_write(value: u64) {
    unsafe { core::arch::asm!("wrmsr", in(rcx) 0xC0000080u32, in(rax) value, options(nostack)) }
}

/// Enable paging: set CR0.PG and CR4.PAE, and EFER.LME (long mode enable).
/// Long mode must already be selected by the boot loader (we run in long mode
/// already under a 64-bit kernel), so this just flips PG.
pub unsafe fn enable_paging() {
    let mut cr4 = cr4_read();
    cr4 |= 1 << 5; // PAE
    cr4_write(cr4);

    let mut efer = efer_read();
    efer |= 1 << 8; // LME
    efer_write(efer);

    cr3_write(PML4.as_ref().unwrap().phys);

    let mut cr0 = cr0_read();
    cr0 |= 1 << 31; // PG
    cr0_write(cr0);
}

/// Invalidate a single TLB entry for `vaddr`.
pub fn tlb_flush(vaddr: usize) {
    unsafe { core::arch::asm!("invlpg {0}", in(mem) *(vaddr as *mut u8), options(nostack)) }
}

/// Flush the entire TLB by reloading CR3.
pub fn tlb_flush_all() {
    cr3_write(cr3_read());
}

/// Cache maintenance is a no-op on x86_64: the CPU is cache-coherent and the
/// kernel uses a normal writeback mapping.  Code written to RAM and executed
/// does not need explicit DCache/ICache maintenance on x86.
pub fn dcache_writeback_invalidate(_addr: usize, _len: usize) {}
pub fn icache_invalidate(_addr: usize, _len: usize) {}
pub fn sync() {
    unsafe { core::arch::asm!("mfence", options(nostack)) }
}
