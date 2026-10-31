// SPDX-License-Identifier: GPL-3.0-only
//! Physical memory for the x86_64 machine.
//!
//! The loader's memory map becomes the shared `frame-alloc` descriptor list.
//! The low megabyte and the whole kernel image, including `.bss` and the boot
//! stack and page tables, are reserved before anything can allocate, so the
//! allocator cannot hand out a frame the machine is running from.

use core::ptr::{addr_of, addr_of_mut};

use frame_alloc::{Allocator, MemDesc, KIND_RESERVED, KIND_USABLE_RAM};

use crate::pvh;

/// Descriptor slots: the reserved low image plus the loader's entries.
const MAX_DESCS: usize = 32;

/// Frames the allocator tracks: 1 GiB of 4 KiB frames.  Frames above this are
/// never handed out, so a larger guest keeps its extra RAM unused until the
/// array grows.  The array costs four bytes per frame of `.bss`.
const TRACKED_FRAMES: usize = 1024 * 1024 * 1024 / 4096;

/// The loader's memory map is at least this large in every boot we target.
const LOW_MEMORY: u64 = 0x10_0000;

// The first byte after the kernel image, set by `linker.ld`.
extern "C" {
    static __kernel_end: u8;
}

static mut DESCS: [MemDesc; MAX_DESCS] = [MemDesc {
    base: 0,
    size: 0,
    kind: KIND_RESERVED,
    flags: 0,
}; MAX_DESCS];
static mut REFS: [u32; TRACKED_FRAMES] = [0; TRACKED_FRAMES];
static mut ALLOC: Option<Allocator<'static>> = None;

/// Build the descriptor list from the loader's memory map and start the
/// allocator.  Called once, before the heap or any other allocation.
pub fn init(start_info: u64) {
    let kernel_end = addr_of!(__kernel_end) as u64;
    let kernel_end = kernel_end.max(LOW_MEMORY);
    let entries = pvh::memmap(start_info);
    assert!(entries.len() < MAX_DESCS, "memory map has too many entries");

    // SAFETY: called once during boot, before any other access to these
    // statics; no interrupt handler reads them.
    unsafe {
        let descs = &mut *addr_of_mut!(DESCS);
        descs[0] = MemDesc {
            base: 0,
            size: kernel_end,
            kind: KIND_RESERVED,
            flags: 0,
        };
        for (slot, entry) in descs[1..].iter_mut().zip(entries) {
            // The loader's type 1 is usable RAM; everything else is reserved.
            *slot = MemDesc {
                base: entry.addr,
                size: entry.size,
                kind: if entry.kind == 1 {
                    KIND_USABLE_RAM
                } else {
                    KIND_RESERVED
                },
                flags: 0,
            };
        }
        let count = 1 + entries.len();
        let all: &'static [MemDesc; MAX_DESCS] = &*addr_of!(DESCS);
        let descs = &all[..count];
        let refs = &mut *addr_of_mut!(REFS);
        *addr_of_mut!(ALLOC) = Some(Allocator::new(descs, refs));
    }
}

fn with<R>(f: impl FnOnce(&mut Allocator<'static>) -> R) -> Option<R> {
    // SAFETY: the machine is single-threaded and `init` has run before use.
    unsafe { (*addr_of_mut!(ALLOC)).as_mut().map(f) }
}

/// Allocate `count` physically contiguous frames.  The machine identity maps
/// the first 4 GiB, so the physical address is also the address to use.
pub fn alloc_frames(count: usize) -> Option<u64> {
    with(|a| a.alloc_frames(count)).flatten()
}

/// Allocate one frame.
pub fn alloc_frame() -> Option<u64> {
    alloc_frames(1)
}

/// Usable RAM described by the loader, in bytes.
pub fn total_ram() -> u64 {
    with(|a| a.total_ram()).unwrap_or(0)
}

/// Frames the allocator can still hand out.
pub fn free_frames() -> usize {
    with(|a| a.free_frames()).unwrap_or(0)
}
