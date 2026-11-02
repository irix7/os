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

/// The channel A register base of the ESCC.
pub fn escc(client: &Client) -> Option<u32> {
    let macio = client.finddevice(b"/pci/mac-io\0")?;
    let mut assigned = [0u8; 24];
    let length = client.getprop(macio, b"assigned-addresses\0", &mut assigned)?;
    if length < 12 {
        return None;
    }
    // One PCI address is [phys_hi, phys_mid, phys_lo, size_hi, size_lo].
    let macio_base = u32::from_be_bytes([assigned[8], assigned[9], assigned[10], assigned[11]]);

    let escc = client.finddevice(b"/pci/mac-io/escc\0")?;
    let mut reg = [0u8; 8];
    let length = client.getprop(escc, b"reg\0", &mut reg)?;
    if length < 4 {
        return None;
    }
    let offset = u32::from_be_bytes([reg[0], reg[1], reg[2], reg[3]]);
    Some(macio_base.wrapping_add(offset))
}

pub fn timebase(client: &Client) -> Option<u32> {
    let cpus = client.finddevice(b"/cpus\0")?;
    let cpu = client.child(cpus)?;
    client.getprop_u32(cpu, b"timebase-frequency\0")
}
