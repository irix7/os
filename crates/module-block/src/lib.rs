// SPDX-License-Identifier: GPL-3.0-only
//! The module block service, shared by every IRIX 7 machine.
//!
//! A module sees `block_read(target, lba, buf)` and `block_write(...)`, one
//! 512-byte sector at a time.  The machine supplies a [`Block`] implementation
//! over its own device and a [`Store`] policy that says which module-visible
//! target maps to that device and where its LBA zero sits on the physical
//! medium.
//!
//! The Indy passes targets straight through to SCSI.  The x86 machine has one
//! virtio-blk disk shared with the boot header and configuration, so target 3
//! maps to a reserved region of that disk.  The PowerBook will use the same
//! policy shape over ATA.

#![no_std]

/// Bytes in one block-service sector.
pub const SECTOR: usize = 512;

/// A machine block device.  `target` is the module-visible target, so a device
/// can pass it to its own stack (SCSI) or ignore it (a single disk).
pub trait Block {
    fn read_sector(&mut self, target: u8, sector: u64, out: &mut [u8; SECTOR]) -> bool;
    fn write_sector(&mut self, target: u8, sector: u64, data: &[u8; SECTOR]) -> bool;
}

/// How the module block service maps onto a machine device.
#[derive(Clone, Copy, Debug)]
pub struct Store {
    /// The module-visible target this device serves, or `None` to pass every
    /// target through to the device.
    pub target: Option<u8>,
    /// Physical sector that the module sees as LBA 0.
    pub offset: u64,
}

impl Store {
    /// Pass every target through, with no offset.
    pub const fn direct() -> Self {
        Self {
            target: None,
            offset: 0,
        }
    }

    /// Serve one module target at `offset` sectors into the device.
    pub const fn mapped(target: u8, offset: u64) -> Self {
        Self {
            target: Some(target),
            offset,
        }
    }

    fn accepts(&self, target: u8) -> bool {
        self.target.is_none_or(|served| served == target)
    }

    /// Read one sector for a module.
    ///
    /// # Safety
    ///
    /// `ptr` must be valid for `SECTOR` writes for the duration of the call.
    pub unsafe fn read(&self, block: &mut impl Block, target: u8, lba: u32, ptr: *mut u8) -> i32 {
        if ptr.is_null() || !self.accepts(target) {
            return -1;
        }
        let mut sector = [0u8; SECTOR];
        if !block.read_sector(target, self.offset + u64::from(lba), &mut sector) {
            return -1;
        }
        // SAFETY: the caller guarantees the destination.
        unsafe { core::ptr::copy_nonoverlapping(sector.as_ptr(), ptr, SECTOR) };
        0
    }

    /// Write one sector for a module.
    ///
    /// # Safety
    ///
    /// `ptr` must be valid for `SECTOR` reads for the duration of the call.
    pub unsafe fn write(
        &self,
        block: &mut impl Block,
        target: u8,
        lba: u32,
        ptr: *const u8,
    ) -> i32 {
        if ptr.is_null() || !self.accepts(target) {
            return -1;
        }
        let mut sector = [0u8; SECTOR];
        // SAFETY: the caller guarantees the source.
        unsafe { core::ptr::copy_nonoverlapping(ptr, sector.as_mut_ptr(), SECTOR) };
        if block.write_sector(target, self.offset + u64::from(lba), &sector) {
            0
        } else {
            -1
        }
    }
}

#[cfg(test)]
extern crate std;

#[cfg(test)]
mod tests {
    use super::*;

    /// Eight sectors of memory, addressed by the store's offset.
    struct Memory([u8; SECTOR * 8]);

    impl Block for Memory {
        fn read_sector(&mut self, _: u8, sector: u64, out: &mut [u8; SECTOR]) -> bool {
            let at = sector as usize * SECTOR;
            if at + SECTOR > self.0.len() {
                return false;
            }
            out.copy_from_slice(&self.0[at..at + SECTOR]);
            true
        }

        fn write_sector(&mut self, _: u8, sector: u64, data: &[u8; SECTOR]) -> bool {
            let at = sector as usize * SECTOR;
            if at + SECTOR > self.0.len() {
                return false;
            }
            self.0[at..at + SECTOR].copy_from_slice(data);
            true
        }
    }

    #[test]
    fn mapped_store_offsets_and_rejects_other_targets() {
        let mut memory = Memory([0; SECTOR * 8]);
        let store = Store::mapped(3, 4);
        let mut data = [0u8; SECTOR];
        data[0] = 0xAB;
        // SAFETY: the buffers are valid for one sector.
        assert_eq!(unsafe { store.write(&mut memory, 3, 1, data.as_ptr()) }, 0);
        assert_eq!(memory.0[5 * SECTOR], 0xAB);
        let mut out = [0u8; SECTOR];
        assert_eq!(
            unsafe { store.read(&mut memory, 3, 1, out.as_mut_ptr()) },
            0
        );
        assert_eq!(out[0], 0xAB);
        // A different module target is refused before the device is touched.
        assert_eq!(
            unsafe { store.read(&mut memory, 2, 1, out.as_mut_ptr()) },
            -1
        );
        // So is a null buffer.
        assert_eq!(
            unsafe { store.read(&mut memory, 3, 1, core::ptr::null_mut()) },
            -1
        );
    }

    #[test]
    fn direct_store_passes_targets_and_checks_bounds() {
        let mut memory = Memory([0; SECTOR * 8]);
        let store = Store::direct();
        let mut out = [0u8; SECTOR];
        assert_eq!(
            unsafe { store.read(&mut memory, 9, 0, out.as_mut_ptr()) },
            0
        );
        assert_eq!(
            unsafe { store.read(&mut memory, 9, 8, out.as_mut_ptr()) },
            -1
        );
    }
}
