// SPDX-License-Identifier: GPL-3.0-only
//! Frame allocator for the Indy kernel.
//!
//! The logic and the storage live in the shared `frame-alloc` crate, which the
//! x86_64 machine also uses.  This module holds the one static store for the
//! Indy: up to 64 ARCS descriptors and one reference count per 4 KiB frame up
//! to 256 MiB.  Frames above that bound are never handed out.

pub use frame_alloc::MemDesc;
use frame_alloc::Store;

/// Maximum number of memory descriptors from ARCS.
const MAX_MEM_DESCS: usize = 64;

/// Frames tracked by the allocator: 256 MiB of 4 KiB frames, 65536 in total.
const MAX_FRAMES: usize = 256 * 1024 * 1024 / 4096;

static mut STORE: Store<MAX_MEM_DESCS, MAX_FRAMES> = Store::new();

/// The kernel's frame store.  The kernel is single-threaded, so this is the only
/// mutable access in flight.
fn store() -> &'static mut Store<MAX_MEM_DESCS, MAX_FRAMES> {
    // SAFETY: single-threaded, and no caller holds the store across another call.
    unsafe { &mut *core::ptr::addr_of_mut!(STORE) }
}

/// Initialise the frame allocator from the ARCS memory descriptor list.
pub fn init(descs: &[MemDesc], count: usize) {
    store().init(&descs[..count]);

    crate::print_str("frames: initialised from ");
    if count > 0 {
        let first = descs[0];
        crate::print_hex32(first.base as u32);
        crate::print_str(", ");
        crate::print_hex32(first.size as u32);
        crate::print_str(" bytes");
    } else {
        crate::print_str("no descriptors");
    }
    crate::println("");
}

/// Allocate a 4 KiB frame.
///
/// Returns the physical address of the allocated frame, or `None` if out of memory.
pub fn alloc_frame() -> Option<u64> {
    store().alloc_frame()
}

/// Allocate `count` contiguous 4 KiB frames.
pub fn alloc_frames(count: usize) -> Option<u64> {
    store().alloc_frames(count)
}

/// Free a 4 KiB frame.
pub fn free_frame(addr: u64) {
    store().free_frame(addr);
}

/// Get the reference count of a frame.
pub fn frame_ref_count(addr: u64) -> u32 {
    store().ref_count(addr)
}

/// Get the total amount of usable RAM in bytes.
pub fn total_ram() -> u64 {
    store().total_ram()
}

/// Number of free 4 KiB frames in the usable descriptors.
pub fn free_frames() -> usize {
    store().free_frames()
}
