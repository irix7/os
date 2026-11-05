// SPDX-License-Identifier: GPL-3.0-only
//! Large zeroed buffers, allocated on first use from the kernel heap.
//!
//! A `static mut [u8; N]` counts toward `.bss`, and the kernel's `.bss` has to
//! stay below the boot stack.  A buffer that is only needed sometimes is kept as
//! an `Option` in `.bss` (a `None` takes no space) and allocated the first time
//! it is asked for.  It is never freed: the users run once per boot or per command.

use alloc::vec;

/// The buffer in `slot`, allocated zeroed on first use.
pub fn get(slot: &'static mut Option<&'static mut [u8]>, len: usize) -> &'static mut [u8] {
    slot.get_or_insert_with(|| vec![0u8; len].leak())
}
