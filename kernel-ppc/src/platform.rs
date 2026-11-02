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
    let macio = find_node(client, b"/pci/mac-io\0", b"/pci\0", b"mac-io")?;
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

/// An ESCC channel.  The register layout is fixed by the chip: channel A
/// control is at `+0x20`, channel B control at `+0x00`, and each data port is
/// 16 bytes above its control port.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Channel {
    A,
    B,
}

impl Channel {
    pub const fn index(self) -> usize {
        match self {
            Self::A => 0,
            Self::B => 1,
        }
    }

    pub const fn letter(self) -> u8 {
        match self {
            Self::A => b'a',
            Self::B => b'b',
        }
    }
}

/// An Open Firmware string property, truncated to the buffer we keep for the
/// boot log.  The buffer is deliberately fixed-size: the device tree is read
/// before the heap exists.
pub struct Property {
    value: [u8; 64],
    length: usize,
    present: bool,
}

impl Property {
    const fn empty() -> Self {
        Self {
            value: [0; 64],
            length: 0,
            present: false,
        }
    }

    fn from_buffer(buffer: [u8; 64], length: usize) -> Self {
        Self {
            value: buffer,
            length: length.min(buffer.len()),
            present: true,
        }
    }

    pub fn present(&self) -> bool {
        self.present
    }

    pub fn bytes(&self) -> &[u8] {
        let end = self.value[..self.length]
            .iter()
            .position(|&byte| byte == 0)
            .unwrap_or(self.length);
        &self.value[..end]
    }
}

/// The ESCC register window and its two serial channels.
///
/// `channels[0]` is channel A and `channels[1]` is channel B.  A machine where
/// only one channel is described leaves the other entry empty; the serial
/// driver then drives whatever is present.
pub struct Escc {
    pub base: u32,
    pub channels: [Option<u32>; 2],
    /// The channel named by the firmware's console property, if it names one.
    pub selected: Option<Channel>,
    /// `/options:output-device`, the Open Firmware console selection.
    pub output_device: Property,
    /// `/chosen:stdout-path`, the standard console path when the firmware
    /// publishes it.
    pub stdout_path: Property,
}

/// The two ESCC channels, read from the children of the `escc` node.
///
/// A CHRP tree puts `ch-a` and `ch-b` under `/pci/mac-io/escc`; OpenBIOS and
/// Apple's Old World firmware both build that shape.  The child `reg` values
/// are offsets in the Mac I/O window on both, but a relative `reg` is also
/// accepted.  If the children are missing, the chip's fixed channel layout
/// inside the ESCC window is used.
pub fn escc(client: &Client, macio: &MacIo) -> Option<Escc> {
    let node = find_node(client, b"/pci/mac-io/escc\0", b"/pci/mac-io\0", b"escc")?;
    let mut reg = [0u8; 8];
    let length = client.getprop(node, b"reg\0", &mut reg)?;
    if length < 8 {
        return None;
    }
    let offset = u32::from_be_bytes([reg[0], reg[1], reg[2], reg[3]]);
    let size = u32::from_be_bytes([reg[4], reg[5], reg[6], reg[7]]);
    if size < 0x40 {
        return None;
    }
    let base = macio.base.checked_add(offset)?;
    let end = base.checked_add(size)?;
    let window_end = macio.base.checked_add(macio.size)?;
    if base < macio.base || end > window_end {
        return None;
    }

    let mut channels = [None; 2];
    let mut child = client.child(node);
    while let Some(child_node) = child {
        if let Some((channel, address)) = channel_node(client, macio, base, child_node) {
            channels[channel.index()] = Some(address);
        }
        child = client.peer(child_node);
    }
    // The chip always has both channels even when the firmware omits a child
    // node.  Both fallbacks stay inside the ESCC window checked above.
    channels[Channel::A.index()].get_or_insert(base + 0x20);
    channels[Channel::B.index()].get_or_insert(base);

    let output_device = property(client, b"/options\0", b"output-device\0");
    let stdout_path = property(client, b"/chosen\0", b"stdout-path\0");
    let selected =
        channel_from_property(&output_device).or_else(|| channel_from_property(&stdout_path));

    Some(Escc {
        base,
        channels,
        selected,
        output_device,
        stdout_path,
    })
}

fn find_node(client: &Client, path: &[u8], parent: &[u8], name: &[u8]) -> Option<u32> {
    if let Some(node) = client.finddevice(path) {
        return Some(node);
    }
    let parent = client.finddevice(parent)?;
    child_named(client, parent, name)
}

fn child_named(client: &Client, parent: u32, name: &[u8]) -> Option<u32> {
    let mut child = client.child(parent);
    while let Some(node) = child {
        let mut buffer = [0u8; 32];
        if let Some(length) = client.getprop(node, b"name\0", &mut buffer) {
            if string_equals(&buffer, length, name) {
                return Some(node);
            }
        }
        child = client.peer(node);
    }
    None
}

fn property(client: &Client, node: &[u8], name: &[u8]) -> Property {
    let Some(node) = client.finddevice(node) else {
        return Property::empty();
    };
    let mut buffer = [0u8; 64];
    match client.getprop(node, name, &mut buffer) {
        Some(length) if length > 0 => Property::from_buffer(buffer, length),
        _ => Property::empty(),
    }
}

fn channel_node(client: &Client, macio: &MacIo, base: u32, node: u32) -> Option<(Channel, u32)> {
    let mut name = [0u8; 32];
    let name_length = client
        .getprop(node, b"name\0", &mut name)
        .unwrap_or_default();
    let named = if string_equals(&name, name_length, b"ch-a") {
        Some(Channel::A)
    } else if string_equals(&name, name_length, b"ch-b") {
        Some(Channel::B)
    } else {
        None
    };

    let address = match client.getprop_u32(node, b"AAPL,address\0") {
        Some(address) => address,
        None => {
            let mut reg = [0u8; 8];
            let length = client.getprop(node, b"reg\0", &mut reg).unwrap_or(0);
            if length < 8 {
                return None;
            }
            let offset = u32::from_be_bytes([reg[0], reg[1], reg[2], reg[3]]);
            if offset < 0x1000 {
                base.checked_add(offset)?
            } else {
                macio.base.checked_add(offset)?
            }
        }
    };

    let channel = named.or(match address & 0x30 {
        0x20 => Some(Channel::A),
        0x00 => Some(Channel::B),
        _ => None,
    })?;

    let end = macio.base.checked_add(macio.size)?;
    let block_end = address.checked_add(0x20)?;
    if address < macio.base || block_end > end {
        return None;
    }
    Some((channel, address))
}

fn channel_from_property(property: &Property) -> Option<Channel> {
    if !property.present() {
        return None;
    }
    channel_from_path(property.bytes())
}

fn channel_from_path(value: &[u8]) -> Option<Channel> {
    let end = value
        .iter()
        .position(|&byte| byte == 0 || byte == b':' || byte == b',')
        .unwrap_or(value.len());
    let value = &value[..end];
    if contains(value, b"ch-a") || value == b"scca" || value == b"ttya" {
        Some(Channel::A)
    } else if contains(value, b"ch-b") || value == b"sccb" || value == b"ttyb" {
        Some(Channel::B)
    } else {
        None
    }
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack
        .windows(needle.len())
        .any(|window| window == needle)
}

fn string_equals(buffer: &[u8], length: usize, value: &[u8]) -> bool {
    let length = length.min(buffer.len());
    let end = buffer[..length]
        .iter()
        .position(|&byte| byte == 0)
        .unwrap_or(length);
    &buffer[..end] == value
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
