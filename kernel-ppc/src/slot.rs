// SPDX-License-Identifier: GPL-3.0-only
//! The reserved 2 MiB region above the PowerPC kernel image.
//!
//! The region holds no loadable code.  One IBAT2/DBAT2 pair maps it, so the
//! memory map keeps it out of the frame allocator.  The module-area change
//! removes it (ADR M1-0003).

/// Base of the reserved region: physical `0x0180_0000`.
///
/// It sits above the kernel image, which ends below `0x0200_0000`.
pub const SLOT_BASE: u32 = 0x0180_0000;

/// Size of the reserved region in bytes.
pub const SLOT_SIZE: usize = 0x0020_0000;
