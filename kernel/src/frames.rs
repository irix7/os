// SPDX-License-Identifier: GPL-3.0-only
//! Frame allocator for the Indy kernel.
//!
//! The logic lives in the shared `frame-alloc` crate, which the x86_64 machine
//! also uses.  This module owns the static storage: a copy of the ARCS memory
//! descriptor list and one reference count per 4 KiB frame up to 256 MiB.
//! Frames above that bound are never handed out.

pub use frame_alloc::MemDesc;
use frame_alloc::Allocator;

/// Maximum number of memory descriptors from ARCS.
const MAX_MEM_DESCS: usize = 64;

/// Frames tracked by the allocator: 256 MiB of 4 KiB frames, 65536 in total.
const MAX_FRAMES: usize = 256 * 1024 * 1024 / 4096;

static mut MEM_DESCS: [MemDesc; MAX_MEM_DESCS] = [MemDesc {
    base: 0,
    size: 0,
    kind: 0,
    flags: 0,
}; MAX_MEM_DESCS];
static mut FRAME_REFS: [u32; MAX_FRAMES] = [0; MAX_FRAMES];
static mut ALLOC: Option<Allocator<'static>> = None;

/// Run `f` against the allocator, or return `None` before `init`.
fn with<R>(f: impl FnOnce(&mut Allocator<'static>) -> R) -> Option<R> {
    // SAFETY: single-threaded kernel; the allocator is only touched from the
    // kernel's one thread of control, and `init` runs before any caller.
    unsafe { (*core::ptr::addr_of_mut!(ALLOC)).as_mut().map(f) }
}

/// Initialise the frame allocator from the ARCS memory descriptor list.
pub fn init(descs: &[MemDesc], count: usize) {
    // SAFETY: called once during boot, before any allocation.
    unsafe {
        let stored = &mut *core::ptr::addr_of_mut!(MEM_DESCS);
        stored[..count].copy_from_slice(&descs[..count]);
        let refs = &mut *core::ptr::addr_of_mut!(FRAME_REFS);
        *core::ptr::addr_of_mut!(ALLOC) = Some(Allocator::new(&stored[..count], refs));
    }

    crate::print_str("frames: initialised from ");
    if count > 0 {
        // SAFETY: read-only access to the copy stored above.
        let first = unsafe { (*core::ptr::addr_of!(MEM_DESCS))[0] };
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
    with(|a| a.alloc_frame()).flatten()
}

/// Allocate `count` contiguous 4 KiB frames.
pub fn alloc_frames(count: usize) -> Option<u64> {
    with(|a| a.alloc_frames(count)).flatten()
}

/// Free a 4 KiB frame.
pub fn free_frame(addr: u64) {
    with(|a| a.free_frame(addr));
}

/// Get the reference count of a frame.
pub fn frame_ref_count(addr: u64) -> u32 {
    with(|a| a.ref_count(addr)).unwrap_or(0)
}

/// Get the total amount of usable RAM in bytes.
pub fn total_ram() -> u64 {
    with(|a| a.total_ram()).unwrap_or(0)
}

/// Number of free 4 KiB frames in the usable descriptors.
pub fn free_frames() -> usize {
    with(|a| a.free_frames()).unwrap_or(0)
}
