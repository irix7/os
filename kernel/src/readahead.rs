// SPDX-License-Identifier: GPL-3.0-only
//! Read-ahead window arithmetic for the HTTP and HTTPS block device.
//!
//! One window is both the unit of the range request and the cache: a miss
//! fetches a whole aligned window and later reads are served from it.  The
//! module is pure logic with no kernel dependencies, so
//! `kernel/tests/readahead.rs` compiles it on the host.

/// The read-ahead window and cache size: 256 KiB.
pub const CACHE_SIZE: usize = 256 * 1024;

/// The first fetch covers only the superblock's block, before the image size
/// is known.
pub const PROBE_SIZE: usize = 4096;

/// A fetch window: an aligned start and a byte count.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Window {
    /// First byte offset of the window, aligned to [`CACHE_SIZE`].
    pub start: u64,
    /// Number of bytes to fetch.
    pub len: usize,
}

impl Window {
    /// Last byte offset of the window, inclusive.
    pub fn end(self) -> u64 {
        self.start + self.len as u64 - 1
    }
}

/// The window that covers `position`.
///
/// The window starts at the [`CACHE_SIZE`] boundary at or below `position` and
/// is clamped to the image end, so the final window can be shorter.  Once the
/// image size is known, a position at or past the end has no window; before
/// the superblock probe only [`PROBE_SIZE`] bytes are fetched.
pub fn window(position: u64, image_size: Option<u64>) -> Option<Window> {
    if image_size.is_some_and(|size| position >= size) {
        return None;
    }
    let start = position & !(CACHE_SIZE as u64 - 1);
    let available = match image_size {
        Some(size) => size.saturating_sub(start),
        None => PROBE_SIZE as u64,
    };
    if available == 0 {
        return None;
    }
    let len = (CACHE_SIZE as u64).min(available) as usize;
    Some(Window { start, len })
}
