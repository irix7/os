// SPDX-License-Identifier: GPL-3.0-only
//! Large zeroed buffers, allocated on first use from the kernel heap.
//!
//! A `static mut [u8; N]` counts toward `.bss`, and the kernel's `.bss` has to
//! stay below the boot stack.  A buffer that is only needed sometimes is kept as
//! a pointer in `.bss` and allocated the first time it is asked for.  It is never
//! freed: the users run once per boot or per command.

use alloc::vec;

/// The buffer behind `slot`, allocated zeroed on first use.  `slot` is a
/// pointer in `.bss` that starts null.
pub fn get(slot: *mut *mut u8, len: usize) -> &'static mut [u8] {
    // SAFETY: the kernel is single-threaded, and `slot` is one `static mut`
    // pointer owned by one caller.
    unsafe {
        if (*slot).is_null() {
            let buffer: &'static mut [u8] = vec![0u8; len].leak();
            *slot = buffer.as_mut_ptr();
        }
        core::slice::from_raw_parts_mut(*slot, len)
    }
}
