// SPDX-License-Identifier: GPL-3.0-only
//! Physical memory for the x86_64 machine.
//!
//! The loader's memory map becomes the shared `frame-alloc` descriptor list.
//! The low megabyte and the whole kernel image, including `.bss` and the boot
//! stack and page tables, are reserved before anything can allocate, so the
//! allocator cannot hand out a frame the machine is running from.

use core::ptr::{addr_of, addr_of_mut};

use frame_alloc::{MemDesc, Store, KIND_RESERVED, KIND_USABLE_RAM};

use crate::pvh;

/// Descriptor slots: the reserved low image, the module windows and stack, and
/// the loader's entries.
const MAX_DESCS: usize = 32;

/// Frames the allocator tracks: 1 GiB of 4 KiB frames.  Frames above this are
/// never handed out.  The reference counts cost four bytes per frame of `.bss`.
const TRACKED_FRAMES: usize = 1024 * 1024 * 1024 / 4096;

/// The loader's memory map is at least this large in every boot we target.
const LOW_MEMORY: u64 = 0x10_0000;

// The first byte after the kernel image, set by `linker.ld`.
extern "C" {
    static __kernel_end: u8;
}

static mut STORE: Store<MAX_DESCS, TRACKED_FRAMES> = Store::new();

/// The machine's frame store.  The machine is single-threaded, so this is the
/// only mutable access in flight.
fn store() -> &'static mut Store<MAX_DESCS, TRACKED_FRAMES> {
    // SAFETY: single-threaded, and no caller holds the store across another call.
    unsafe { &mut *addr_of_mut!(STORE) }
}

/// Build the descriptor list from the loader's memory map and start the
/// allocator.  Called once, before the heap or any other allocation.
pub fn init(start_info: u64) {
    let kernel_end = (addr_of!(__kernel_end) as u64).max(LOW_MEMORY);
    let entries = pvh::memmap(start_info);

    let mut descs = [MemDesc {
        base: 0,
        size: 0,
        kind: KIND_RESERVED,
        flags: 0,
    }; MAX_DESCS];
    let mut count = 0;
    let mut reserve = |base: u64, size: u64| {
        descs[count] = MemDesc {
            base,
            size,
            kind: KIND_RESERVED,
            flags: 0,
        };
        count += 1;
    };
    // The low memory and kernel image come first.
    reserve(0, kernel_end);
    // Module windows and the large stack are reserved too: modules are linked
    // and run there, and the stack is not frame memory.
    for (base, size) in [
        (crate::module::MCP.base, crate::module::MCP.window),
        (crate::module::WASM.base, crate::module::WASM.window),
        (crate::stack::BASE, crate::stack::SIZE),
    ] {
        reserve(base as u64, size as u64);
    }
    for entry in entries {
        // The loader's type 1 is usable RAM; everything else is reserved.
        let kind = if entry.kind == 1 {
            KIND_USABLE_RAM
        } else {
            KIND_RESERVED
        };
        descs[count] = MemDesc {
            base: entry.addr,
            size: entry.size,
            kind,
            flags: 0,
        };
        count += 1;
    }
    assert!(count <= MAX_DESCS, "memory map has too many entries");
    store().init(&descs[..count]);
}

/// Allocate `count` physically contiguous frames.  The machine identity maps
/// the first 4 GiB, so the physical address is also the address to use.
pub fn alloc_frames(count: usize) -> Option<u64> {
    store().alloc_frames(count)
}

/// Allocate one frame.
pub fn alloc_frame() -> Option<u64> {
    alloc_frames(1)
}

/// Usable RAM described by the loader, in bytes.
pub fn total_ram() -> u64 {
    store().total_ram()
}

/// Frames the allocator can still hand out.
pub fn free_frames() -> usize {
    store().free_frames()
}
