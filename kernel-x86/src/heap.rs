// SPDX-License-Identifier: GPL-3.0-only
//! The x86_64 kernel heap: a bump allocator over one contiguous run of frames.
//!
//! This is the first heap, enough for `alloc` collections in the machine's
//! own code.  It never reuses memory: `dealloc` is a no-op and the heap is
//! exhausted once its frames are used.  A free-list allocator replaces it when
//! the shared core needs reclamation; until then the limit is a fixed 1 MiB.

use core::alloc::{GlobalAlloc, Layout};
use core::sync::atomic::{AtomicUsize, Ordering};

use crate::memory;

/// Heap size in 4 KiB frames: 1 MiB.
const HEAP_FRAMES: usize = 256;
const HEAP_BYTES: usize = HEAP_FRAMES * 4096;

struct Bump {
    next: AtomicUsize,
    end: AtomicUsize,
}

#[global_allocator]
static HEAP: Bump = Bump {
    next: AtomicUsize::new(0),
    end: AtomicUsize::new(0),
};

// SAFETY: allocation is a compare-and-swap on `next`, so concurrent callers each
// receive a distinct range; `dealloc` never returns memory, so no range is ever
// handed out twice.
unsafe impl GlobalAlloc for Bump {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        // A zero-size request needs no memory, only a well-aligned non-null
        // pointer.
        if layout.size() == 0 {
            return layout.align() as *mut u8;
        }
        loop {
            let current = self.next.load(Ordering::Relaxed);
            let start = (current + layout.align() - 1) & !(layout.align() - 1);
            let Some(next) = start.checked_add(layout.size()) else {
                return core::ptr::null_mut();
            };
            if next > self.end.load(Ordering::Relaxed) {
                return core::ptr::null_mut();
            }
            if self
                .next
                .compare_exchange(current, next, Ordering::Relaxed, Ordering::Relaxed)
                .is_ok()
            {
                return start as *mut u8;
            }
        }
    }

    unsafe fn dealloc(&self, _ptr: *mut u8, _layout: Layout) {
        // No reclamation in this slice; see the module comment.
    }
}

/// Take the heap's frames from the allocator.  Must run after `memory::init`
/// and before the first `alloc` use.  Returns the heap's base and size.
pub fn init() -> (usize, usize) {
    let base = memory::alloc_frames(HEAP_FRAMES).expect("heap: no contiguous frames") as usize;
    HEAP.next.store(base, Ordering::Relaxed);
    HEAP.end.store(base + HEAP_BYTES, Ordering::Relaxed);
    (base, HEAP_BYTES)
}
