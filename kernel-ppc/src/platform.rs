// SPDX-License-Identifier: GPL-3.0-only
//! The machine description the firmware hands over.
//!
//! Every address here is read from the Open Firmware device tree.  The
//! Mac I/O PCI BAR and the ESCC offset are assigned by OpenBIOS, so a
//! compiled-in address would be wrong as soon as the firmware or the set of
//! PCI devices changes.

use crate::of::Client;

pub struct Memory {
    pub base: u32,
    pub size: u32,
}

/// The Mac I/O register window assigned by the firmware.
pub struct MacIo {
    pub base: u32,
    pub size: u32,
}

pub fn memory(client: &Client) -> Option<Memory> {
    let phandle = client.finddevice(b"/memory\0")?;
    let mut reg = [0u8; 8];
    let length = client.getprop(phandle, b"reg\0", &mut reg)?;
    if length < 8 {
        return None;
    }
    Some(Memory {
        base: u32::from_be_bytes([reg[0], reg[1], reg[2], reg[3]]),
        size: u32::from_be_bytes([reg[4], reg[5], reg[6], reg[7]]),
    })
}

/// The 32-bit Mac I/O BAR and its window size.
///
/// `assigned-addresses` holds one PCI address per 20 bytes:
/// `[phys_hi, phys_mid, phys_lo, size_hi, size_lo]`.  Mac I/O has a single
/// BAR, so the first entry describes the whole register window.
pub fn macio(client: &Client) -> Option<MacIo> {
    let macio = client.finddevice(b"/pci/mac-io\0")?;
    let mut assigned = [0u8; 20];
    let length = client.getprop(macio, b"assigned-addresses\0", &mut assigned)?;
    if length < assigned.len() {
        return None;
    }
    let base = u32::from_be_bytes([assigned[8], assigned[9], assigned[10], assigned[11]]);
    let size = u32::from_be_bytes([assigned[16], assigned[17], assigned[18], assigned[19]]);
    if size == 0 {
        return None;
    }
    Some(MacIo { base, size })
}

/// The channel A register base of the ESCC, inside `macio`.
pub fn escc(client: &Client, macio: &MacIo) -> Option<u32> {
    let escc = client.finddevice(b"/pci/mac-io/escc\0")?;
    let mut reg = [0u8; 8];
    let length = client.getprop(escc, b"reg\0", &mut reg)?;
    if length < 4 {
        return None;
    }
    let offset = u32::from_be_bytes([reg[0], reg[1], reg[2], reg[3]]);
    Some(macio.base.wrapping_add(offset))
}

pub fn timebase(client: &Client) -> Option<u32> {
    let cpus = client.finddevice(b"/cpus\0")?;
    let cpu = client.child(cpus)?;
    client.getprop_u32(cpu, b"timebase-frequency\0")
}
