// SPDX-License-Identifier: GPL-3.0-only
//! The kernel's own address translation.
//!
//! OpenBIOS leaves a hashed page table and a one-to-one mapping behind.  The
//! kernel replaces both with block address translations: the image and its
//! stack, the low exception vectors, the Mac I/O register window and the
//! module slot.  SDR1 is then pointed at a zeroed table and the TLB is
//! invalidated, so an address outside those blocks has no translation at all.
//!
//! BAT block sizes on the 750 are a multiple of 128 KiB: the 11-bit BL field
//! at BATU bits 2..12 holds the size in 128 KiB units minus one.  The upper
//! register's BEPI field and the lower register's BRPN field both hold address
//! bits 0..14, so every block is identity-mapped.

use core::arch::asm;
use core::ptr::addr_of;

/// The smallest BAT block, and the granularity of the low vector mapping.
const BLOCK_128K: u32 = 0x0002_0000;
/// The largest block a 750 BAT can describe.
const BLOCK_MAX: u32 = 0x1000_0000;

const BAT_BEPI: u32 = 0xfffe_0000;
const BAT_BL_SHIFT: u32 = 2;
/// Supervisor-valid.  VP (user-valid) stays clear: there is no user mode.
const BAT_VS: u32 = 0x0000_0002;
const BATL_PP_RW: u32 = 0x0000_0002;

/// Guarded and cache-inhibited, for device registers.
pub const WIMG_IO: u32 = 0x0000_0028;

/// A BAT block: an aligned power-of-two range, mapped one-to-one.
#[derive(Clone, Copy)]
pub struct Bat {
    pub base: u32,
    pub size: u32,
}

impl Bat {
    fn upper(self) -> u32 {
        // The BL field is the block size in 128 KiB units, minus one.
        let bl = self.size / BLOCK_128K - 1;
        (self.base & BAT_BEPI) | ((bl & 0x7ff) << BAT_BL_SHIFT) | BAT_VS
    }

    fn lower(self, wimg: u32) -> u32 {
        (self.base & BAT_BEPI) | wimg | BATL_PP_RW
    }
}

/// The smallest aligned block that contains `[start, end)`.
pub fn cover(start: u32, end: u32) -> Bat {
    assert!(start < end, "empty BAT range");
    let mut size = BLOCK_128K;
    loop {
        let base = start & !(size - 1);
        if base.checked_add(size).is_some_and(|limit| limit >= end) {
            return Bat { base, size };
        }
        size <<= 1;
        assert!(size <= BLOCK_MAX, "address range exceeds one BAT block");
    }
}

/// The zeroed hash table SDR1 points at after the switch.
///
/// A zero table cannot produce a matching PTE, so a missing BAT is a fault
/// rather than an accidental translation through firmware state.
#[repr(align(65536))]
struct EmptyHash {
    /// Only the address is used; the hardware hash walker reads the zeros.
    #[allow(dead_code)]
    table: [u32; 16384],
}

static EMPTY_HASH: EmptyHash = EmptyHash { table: [0; 16384] };

/// Install the BAT entries that the kernel runs on.
///
/// IBAT0/DBAT0 cover the image, including `.bss` and the boot stack.  IBAT1
/// covers the 128 KiB vector page.  DBAT1 covers the Mac I/O window, guarded
/// and cache-inhibited.  IBAT2/DBAT2 cover the module slot, cacheable so the
/// loader can copy and hash into it directly.  The remaining slots are
/// cleared: a firmware BAT must not survive the switch.
pub fn install(image: Bat, io: Bat, module: Bat) {
    let vectors = Bat {
        base: 0,
        size: BLOCK_128K,
    };
    unsafe {
        write_ibat(0, image.upper(), image.lower(0));
        write_ibat(1, vectors.upper(), vectors.lower(0));
        write_ibat(2, module.upper(), module.lower(0));
        write_ibat(3, 0, 0);
        write_dbat(0, image.upper(), image.lower(0));
        write_dbat(1, io.upper(), io.lower(WIMG_IO));
        write_dbat(2, module.upper(), module.lower(0));
        write_dbat(3, 0, 0);
        asm!("isync", options(nostack));
    }
}

/// Point SDR1 at the empty table and drop every cached page translation for
/// the ranges the kernel owns.
///
/// The 750 provides `tlbia` for the all-entry case, but QEMU's 750 model does
/// not include it in the instruction set (it has `tlbie` but not
/// `PPC_MEM_TLBIA`).  Invalidate the three ranges page by page instead; that
/// covers every firmware entry the kernel could otherwise keep using.
///
/// Returns the table address, for the boot log.
pub fn activate(image: Bat, io: Bat, module: Bat) -> u32 {
    let table = addr_of!(EMPTY_HASH) as u32;
    // SAFETY: SDR1 and tlbie are privileged; the image BAT and the vector BAT
    // are already installed, so this code keeps executing through them.
    unsafe {
        asm!("mtspr 25, {table}", table = in(reg) table, options(nostack));
    }
    invalidate(image);
    invalidate(io);
    invalidate(module);
    invalidate(Bat {
        base: 0,
        size: BLOCK_128K,
    });
    unsafe {
        // isync is a context synchronising event: it completes the deferred
        // local flushes on a 32-bit MMU.
        asm!("sync", "isync", options(nostack));
    }
    table
}

fn invalidate(range: Bat) {
    let end = range.base + range.size;
    let mut address = range.base;
    while address < end {
        // SAFETY: tlbie is privileged; the range is page aligned.
        unsafe {
            asm!("tlbie {address}", address = in(reg) address, options(nostack));
        }
        address += 0x1000;
    }
}

unsafe fn write_ibat(index: u32, upper: u32, lower: u32) {
    match index {
        0 => asm!("mtspr 528, {upper}", "mtspr 529, {lower}",
            upper = in(reg) upper, lower = in(reg) lower, options(nostack)),
        1 => asm!("mtspr 530, {upper}", "mtspr 531, {lower}",
            upper = in(reg) upper, lower = in(reg) lower, options(nostack)),
        2 => asm!("mtspr 532, {upper}", "mtspr 533, {lower}",
            upper = in(reg) upper, lower = in(reg) lower, options(nostack)),
        3 => asm!("mtspr 534, {upper}", "mtspr 535, {lower}",
            upper = in(reg) upper, lower = in(reg) lower, options(nostack)),
        _ => unreachable!(),
    }
}

unsafe fn write_dbat(index: u32, upper: u32, lower: u32) {
    match index {
        0 => asm!("mtspr 536, {upper}", "mtspr 537, {lower}",
            upper = in(reg) upper, lower = in(reg) lower, options(nostack)),
        1 => asm!("mtspr 538, {upper}", "mtspr 539, {lower}",
            upper = in(reg) upper, lower = in(reg) lower, options(nostack)),
        2 => asm!("mtspr 540, {upper}", "mtspr 541, {lower}",
            upper = in(reg) upper, lower = in(reg) lower, options(nostack)),
        3 => asm!("mtspr 542, {upper}", "mtspr 543, {lower}",
            upper = in(reg) upper, lower = in(reg) lower, options(nostack)),
        _ => unreachable!(),
    }
}
