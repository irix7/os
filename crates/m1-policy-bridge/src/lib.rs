// SPDX-License-Identifier: GPL-3.0-only
//! Fixed publication record for the M1 Wasm policy.
//!
//! The slot-B module writes this record after its guest has committed. The
//! kernel reads it once and adopts the committed `wasm_policy_host::Policy`
//! object into its own live state. The record lives in its own reserved page
//! below the module area, not at the end of slot B, so loading a runtime module
//! into any arena region cannot overwrite it.
//!
//! The record is only a publication marker. The live policy is the kernel's
//! adopted snapshot and the committed `Policy` object on the kernel heap; a
//! failed slot-B reload clears this marker without touching either.

#![no_std]
#![forbid(unsafe_code)]

/// Reserved bridge page in the low window, above the kernel stack top
/// (`0x883f_0000`) and below the module area (`0x8840_0000`).
pub const BRIDGE_BASE: usize = 0x883f_d000;

/// Magic identifying a published M1 policy bridge.
pub const MAGIC: u64 = 0x4d31_504f_4c49_4359; // "M1POLICY"
/// Bridge layout version.
///
/// Version 2 replaces the function-pointer surface with a committed-policy
/// context. Version 1 records are not adopted; there is no compatibility path.
pub const VERSION: u32 = 2;
/// Size of the publication record in bytes.
pub const BRIDGE_SIZE: usize = 24;

/// Publication record written by the slot-B Wasm module.
#[repr(C)]
pub struct Bridge {
    /// [`MAGIC`] when published.
    pub magic: u64,
    /// [`VERSION`] when published.
    pub version: u32,
    /// Reserved, must be zero.
    pub reserved: u32,
    /// Opaque pointer to the committed `wasm_policy_host::Policy`.
    pub context: u64,
}

impl Bridge {
    /// An unpublished record.
    pub const fn empty() -> Self {
        Self {
            magic: 0,
            version: 0,
            reserved: 0,
            context: 0,
        }
    }

    /// A record publishing `context`.
    pub const fn published(context: usize) -> Self {
        Self {
            magic: MAGIC,
            version: VERSION,
            reserved: 0,
            context: context as u64,
        }
    }

    /// The published context, or `None` when the record is empty or stale.
    pub const fn published_context(&self) -> Option<usize> {
        if self.magic == MAGIC && self.version == VERSION && self.context != 0 {
            Some(self.context as usize)
        } else {
            None
        }
    }

    /// Invalidate the record without touching the adopted live policy.
    pub fn unpublish(&mut self) {
        *self = Self::empty();
    }
}

const _: () = assert!(core::mem::size_of::<Bridge>() == BRIDGE_SIZE);

/// Address of the publication record.
pub const fn bridge_address() -> usize {
    BRIDGE_BASE
}
