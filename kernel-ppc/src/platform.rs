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

/// The interrupt controller built into the Mac I/O node.
///
/// The beige G3 Mac I/O controller is Heathrow, and the Heathrow interrupt
/// controller is the first register block of the Mac I/O window.  It is not
/// an OpenPIC: OpenPIC is the New World controller in KeyLargo, and neither
/// this machine nor QEMU's `g3beige` model has one.  The `compatible`
/// property selects the controller, so a machine with a different Mac I/O
/// device fails here instead of writing Heathrow registers into an OpenPIC.
pub struct Pic {
    pub base: u32,
}

pub fn pic(client: &Client, macio: &MacIo) -> Option<Pic> {
    let node = client.finddevice(b"/pci/mac-io\0")?;
    let mut compatible = [0u8; 32];
    let length = client.getprop(node, b"compatible\0", &mut compatible)?;
    if length < 8 || &compatible[..8] != b"heathrow" {
        return None;
    }
    Some(Pic { base: macio.base })
}

/// The CUDA, which carries the MOS 6522 VIA and its programmable timer.
///
/// `reg` holds the offset of the CUDA window inside Mac I/O.
/// `AAPL,interrupts` holds the interrupt controller line the CUDA asserts.
pub struct Cuda {
    pub base: u32,
    pub line: u32,
}

pub fn cuda(client: &Client, macio: &MacIo) -> Option<Cuda> {
    let node = client.finddevice(b"/pci/mac-io/via-cuda\0")?;
    let mut reg = [0u8; 8];
    let length = client.getprop(node, b"reg\0", &mut reg)?;
    if length < 4 {
        return None;
    }
    let offset = u32::from_be_bytes([reg[0], reg[1], reg[2], reg[3]]);
    let line = client.getprop_u32(node, b"AAPL,interrupts\0")?;
    Some(Cuda {
        base: macio.base.wrapping_add(offset),
        line,
    })
}

pub fn timebase(client: &Client) -> Option<u32> {
    let cpus = client.finddevice(b"/cpus\0")?;
    let cpu = client.child(cpus)?;
    client.getprop_u32(cpu, b"timebase-frequency\0")
}

/// The ATA channel that carries the boot disk.
///
/// The `ata-3` node publishes two `reg` ranges: the task-file registers and
/// the DBDMA channel.  Only the first range is used by a polled PIO driver.
/// The firmware numbers the channels and assigns the Mac I/O offsets, so
/// both the `reg` offset and the `AAPL,interrupts` line are read here rather
/// than compiled in.  A channel whose tree has a `disk` child wins over an
/// empty one.
pub struct Ata {
    pub base: u32,
    pub size: u32,
    pub line: u32,
}

pub fn ata(client: &Client, macio: &MacIo) -> Option<Ata> {
    let macio_node = client.finddevice(b"/pci/mac-io\0")?;
    let mut fallback = None;
    let mut child = client.child(macio_node);
    while let Some(node) = child {
        if let Some(ata) = ata_node(client, macio, node) {
            if has_disk(client, node) {
                return Some(ata);
            }
            fallback.get_or_insert(ata);
        }
        child = client.peer(node);
    }
    fallback
}

fn ata_node(client: &Client, macio: &MacIo, node: u32) -> Option<Ata> {
    let mut kind = [0u8; 8];
    let length = client.getprop(node, b"device_type\0", &mut kind)?;
    if length < 4 || &kind[..4] != b"ata\0" {
        return None;
    }
    let mut reg = [0u8; 16];
    let length = client.getprop(node, b"reg\0", &mut reg)?;
    if length < 8 {
        return None;
    }
    let offset = u32::from_be_bytes([reg[0], reg[1], reg[2], reg[3]]);
    let size = u32::from_be_bytes([reg[4], reg[5], reg[6], reg[7]]);
    if size < 0x200 {
        return None;
    }
    let line = client.getprop_u32(node, b"AAPL,interrupts\0")?;
    Some(Ata {
        base: macio.base.wrapping_add(offset),
        size,
        line,
    })
}

fn has_disk(client: &Client, node: u32) -> bool {
    let mut child = client.child(node);
    while let Some(child_node) = child {
        let mut name = [0u8; 8];
        if let Some(length) = client.getprop(child_node, b"name\0", &mut name) {
            if length >= 5 && &name[..5] == b"disk\0" {
                return true;
            }
        }
        child = client.peer(child_node);
    }
    false
}
