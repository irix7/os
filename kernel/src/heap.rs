// SPDX-License-Identifier: GPL-3.0-only
//! Frame-backed bump allocator.
//!
//! The kernel needs `alloc` for embedded-tls record buffers.  There is no
//! free path: each new allocation that does not fit the current frame takes
//! fresh pages from the frame allocator and the remainder of the previous
//! frame is abandoned.  That is enough for a bounded number of console
//! operations and keeps the allocator trivial.
//!
//! The heap is CPU-only, so the cacheable kseg0 mapping needs no maintenance
//! for it.  DMA-visible buffers live in the Seeq device's uncached alias.

use core::alloc::{GlobalAlloc, Layout};
use core::ptr::null_mut;
use core::sync::atomic::{AtomicUsize, Ordering};

const PAGE: usize = 4096;

struct BumpAllocator;

static NEXT: AtomicUsize = AtomicUsize::new(0);
static END: AtomicUsize = AtomicUsize::new(0);

unsafe impl GlobalAlloc for BumpAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let align = layout.align().max(8);
        let size = layout.size().max(1);
        if align > PAGE {
            return null_mut();
        }

        loop {
            let next = NEXT.load(Ordering::Relaxed);
            let end = END.load(Ordering::Relaxed);
            let aligned = (next + align - 1) & !(align - 1);
            if aligned + size <= end {
                let end_new = aligned + size;
                if NEXT
                    .compare_exchange_weak(next, end_new, Ordering::SeqCst, Ordering::Relaxed)
                    .is_ok()
                {
                    return aligned as *mut u8;
                }
                continue;
            }

            // Take enough contiguous frames for the request.
            let pages = size.div_ceil(PAGE).max(1);
            let Some(phys) = crate::frames::alloc_frames(pages) else {
                return null_mut();
            };
            let base = (phys as usize) | 0x8000_0000;
            NEXT.store(base, Ordering::SeqCst);
            END.store(base + pages * PAGE, Ordering::SeqCst);
        }
    }

    unsafe fn dealloc(&self, _ptr: *mut u8, _layout: Layout) {
        // Bump allocator: memory is reclaimed only by reboot.
    }
}

#[global_allocator]
static ALLOCATOR: BumpAllocator = BumpAllocator;
