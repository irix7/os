// SPDX-License-Identifier: GPL-3.0-only
//! Boot configuration on the x86_64 machine.
//!
//! Four disk sectors hold `key=value` lines.  The machine has no NVRAM or
//! signed boot record yet; this is the version-1 source the module services
//! read.  A key's value ends at the next newline or NUL.

use crate::virtio_blk::{Disk, SECTOR};

/// First sector of the configuration block.
const CONFIG_SECTOR: u64 = 100;
/// Sectors in the block: 2 KiB.
const CONFIG_SECTORS: usize = 4;
const CAPACITY: usize = CONFIG_SECTORS * SECTOR as usize;

static mut CONFIG: [u8; CAPACITY] = [0; CAPACITY];
static mut READY: bool = false;

/// Read the configuration block into memory.
pub fn init(disk: &mut Disk) {
    let mut sector = [0u8; SECTOR];
    // SAFETY: called once during boot, before any reader.
    let config = unsafe { &mut *core::ptr::addr_of_mut!(CONFIG) };
    for index in 0..CONFIG_SECTORS {
        if !disk.read(CONFIG_SECTOR + index as u64, &mut sector) {
            return;
        }
        config[index * SECTOR as usize..][..SECTOR as usize].copy_from_slice(&sector);
    }
    // SAFETY: as above.
    unsafe { *core::ptr::addr_of_mut!(READY) = true };
}

/// Copy the value of `key` into `out`.  Returns the value length, 0 when the
/// key is absent, or -1 for a bad argument.
pub fn get(key: &[u8], out: &mut [u8]) -> isize {
    // SAFETY: read-only access on the single thread, after `init`.
    let ready = unsafe { *core::ptr::addr_of!(READY) };
    if !ready {
        return -1;
    }
    let config = unsafe { &*core::ptr::addr_of!(CONFIG) };
    irix7_config::get(config, key, out)
}
