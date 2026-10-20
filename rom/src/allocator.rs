//! A minimal bump allocator for the ROM.
//!
//! The ROM runs very early, before a frame allocator exists, so smoltcp's
//! socket set and the HTTP client's string/vec allocations need a global
//! allocator. A bump allocator is sufficient here: allocations are made once
//! at boot and never freed.

use core::alloc::{GlobalAlloc, Layout};
use core::ptr::null_mut;

/// Size of the bump-allocated heap (64 KiB).
const HEAP_SIZE: usize = 64 * 1024;

static mut HEAP: [u8; HEAP_SIZE] = [0; HEAP_SIZE];
static mut HEAP_PTR: usize = 0;

/// A bump-pointer allocator: allocations move the pointer forward, deallocation
/// is a no-op.
pub struct BumpAllocator;

unsafe impl GlobalAlloc for BumpAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let size = layout.size().max(1);
        let align = layout.align().max(1);
        let ptr = HEAP_PTR;
        let aligned = (ptr + align - 1) & !(align - 1);
        let end = aligned.checked_add(size).unwrap_or(HEAP_SIZE + 1);
        if end <= HEAP_SIZE {
            HEAP_PTR = end;
            HEAP.as_mut_ptr().add(aligned)
        } else {
            null_mut()
        }
    }

    unsafe fn dealloc(&self, _ptr: *mut u8, _layout: Layout) {
        // Bump allocators never free; memory is reclaimed on reboot.
    }
}

#[global_allocator]
static ALLOCATOR: BumpAllocator = BumpAllocator;
