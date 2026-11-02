// SPDX-License-Identifier: GPL-3.0-only
//! The machine description the firmware hands over.
//!
//! Every address here is read from the Open Firmware device tree.  The Mac I/O
//! PCI BAR and the ESCC offset are assigned by the firmware, so a compiled-in
//! address would be wrong as soon as the firmware or the set of PCI devices
//! changes.
//!
//! A PowerBook G3 M4753 has two Mac I/O controllers under `/pci`: the primary
//! Heathrow controller and a second Gatwick controller for the communication
//! slot and media bay.  Both can carry an ESCC and ATA children, so the first
//! node in tree order is not necessarily the one with the boot console and
//! disk.  [`macio`] walks the `/pci` children, scores every Mac I/O node by
//! its children and selects the node with the ESCC console and the boot ATA
//! disk.  Every other lookup is relative to that node.

use crate::of::Client;

pub struct Memory {
    pub base: u32,
    pub size: u32,
}

/// A device-tree path captured for the boot log.  The buffer is fixed-size
/// because the tree is read before the heap exists.
#[derive(Clone, Copy)]
pub struct Path {
    value: [u8; 96],
    length: usize,
}

impl Path {
    pub fn bytes(&self) -> &[u8] {
        &self.value[..self.length]
    }

    fn from_buffer(buffer: [u8; 96], length: usize) -> Self {
        let length = length.min(buffer.len());
        let end = buffer[..length]
            .iter()
            .position(|&byte| byte == 0)
            .unwrap_or(length);
        Self {
            value: buffer,
            length: end,
        }
    }
}

/// The selected Mac I/O register window and the children that selected it.
pub struct MacIo {
    pub base: u32,
    pub size: u32,
    /// The Open Firmware phandle.  Every child lookup starts here.
    pub node: u32,
    /// The selected node's path, for the boot log.
    pub path: Path,
    /// ESCC children found below the selected node.
    pub escc: u32,
    /// ATA channels found below the selected node.
    pub ata: u32,
    /// ATA channels that the firmware gave a `disk` child.
    pub disks: u32,
}

/// The deepest child level the score scans.  A Mac I/O node reaches its ATA
/// channel through `media-bay` on the PowerBook, so three levels are needed;
/// one spare level keeps the scan tolerant of a wrapping node.
const MAX_DEPTH: u32 = 4;

/// One candidate Mac I/O node before selection.
struct Candidate {
    node: u32,
    base: u32,
    size: u32,
    escc: u32,
    ata: u32,
    disks: u32,
}

impl Candidate {
    /// The ESCC is the boot console and dominates the score.  A channel with
    /// a disk child is the boot disk and outranks an empty channel; the ATA
    /// count only breaks ties between otherwise equal controllers.
    fn score(&self) -> u32 {
        self.escc * 100 + self.disks * 50 + self.ata * 10
    }
}

#[derive(Default)]
struct Children {
    escc: u32,
    ata: u32,
    disks: u32,
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

/// The Mac I/O window that carries the boot console and disk.
///
/// The M4753 has two Mac I/O nodes under `/pci`.  Their tree order is a
/// firmware detail, so each node is scored by the children it has: the ESCC
/// console, ATA channels, and ATA channels with a `disk` child.  Ties stay
/// with the first node in tree order.
///
/// `assigned-addresses` holds one PCI address per 20 bytes:
/// `[phys_hi, phys_mid, phys_lo, size_hi, size_lo]`.  Mac I/O has a single
/// BAR, so the first entry describes the whole register window.
pub fn macio(client: &Client) -> Option<MacIo> {
    let pci = pci_node(client)?;
    let mut best: Option<Candidate> = None;
    let mut child = client.child(pci);
    while let Some(node) = child {
        if let Some(candidate) = macio_candidate(client, node) {
            if best
                .as_ref()
                .is_none_or(|best| candidate.score() > best.score())
            {
                best = Some(candidate);
            }
        }
        child = client.peer(node);
    }
    let best = best?;
    Some(MacIo {
        base: best.base,
        size: best.size,
        node: best.node,
        path: path(client, best.node),
        escc: best.escc,
        ata: best.ata,
        disks: best.disks,
    })
}

/// The PCI host node.  `/pci` is the OpenBIOS name; Apple's Old World tree
/// calls the node `pci@80000000`, so fall back to a child of the root named
/// `pci` when the short path does not resolve.
fn pci_node(client: &Client) -> Option<u32> {
    if let Some(node) = client.finddevice(b"/pci\0") {
        return Some(node);
    }
    let root = client.finddevice(b"/\0")?;
    find_child(client, root, b"pci", b"pci")
}

fn macio_candidate(client: &Client, node: u32) -> Option<Candidate> {
    if !is_macio(client, node) {
        return None;
    }
    let mut assigned = [0u8; 20];
    let length = client.getprop(node, b"assigned-addresses\0", &mut assigned)?;
    if length < assigned.len() {
        return None;
    }
    let base = u32::from_be_bytes([assigned[8], assigned[9], assigned[10], assigned[11]]);
    let size = u32::from_be_bytes([assigned[16], assigned[17], assigned[18], assigned[19]]);
    if size == 0 {
        return None;
    }
    let mut children = Children::default();
    scan_children(client, node, 0, &mut children);
    // Without an ESCC there is no console on which to report a failure, so a
    // disk-heavy controller that lacks one must not win the score.
    if children.escc == 0 {
        return None;
    }
    Some(Candidate {
        node,
        base,
        size,
        escc: children.escc,
        ata: children.ata,
        disks: children.disks,
    })
}

fn scan_children(client: &Client, node: u32, depth: u32, children: &mut Children) {
    let mut child = client.child(node);
    while let Some(child_node) = child {
        if is_escc(client, child_node) {
            children.escc += 1;
        }
        if is_ata(client, child_node) {
            children.ata += 1;
            if has_disk(client, child_node) {
                children.disks += 1;
            }
        }
        if depth < MAX_DEPTH {
            scan_children(client, child_node, depth + 1, children);
        }
        child = client.peer(child_node);
    }
}

fn is_macio(client: &Client, node: u32) -> bool {
    matches_property(client, node, b"device_type\0", b"mac-io")
        || matches_property(client, node, b"name\0", b"mac-io")
}

fn is_escc(client: &Client, node: u32) -> bool {
    matches_property(client, node, b"device_type\0", b"escc")
        || matches_property(client, node, b"name\0", b"escc")
}

fn is_ata(client: &Client, node: u32) -> bool {
    if matches_property(client, node, b"device_type\0", b"ata") {
        return true;
    }
    let mut name = [0u8; 16];
    match client.getprop(node, b"name\0", &mut name) {
        Some(length) => starts_with(&name, length, b"ata"),
        None => false,
    }
}

/// The full path of `node`, from `package-to-path` when the firmware has it.
/// The name walk is the fallback; it omits unit addresses, so the boot log
/// also records the Mac I/O base.
fn path(client: &Client, node: u32) -> Path {
    let mut buffer = [0u8; 96];
    if let Some(length) = client.package_to_path(node, &mut buffer) {
        if length > 0 {
            return Path::from_buffer(buffer, length);
        }
    }
    path_from_names(client, node)
}

fn path_from_names(client: &Client, node: u32) -> Path {
    let mut parts = [[0u8; 24]; 6];
    let mut lengths = [0usize; 6];
    let mut count = 0;
    let mut current = Some(node);
    while let Some(current_node) = current {
        if count == parts.len() {
            break;
        }
        let mut name = [0u8; 24];
        let length = client
            .getprop(current_node, b"name\0", &mut name)
            .unwrap_or(0);
        let length = trim_string(&name, length);
        if length > 0 && !(length == 1 && name[0] == b'/') {
            parts[count][..length].copy_from_slice(&name[..length]);
            lengths[count] = length;
            count += 1;
        }
        current = client.parent(current_node);
    }

    let mut buffer = [0u8; 96];
    let mut length = 0;
    for index in (0..count).rev() {
        if length + 1 + lengths[index] > buffer.len() {
            break;
        }
        buffer[length] = b'/';
        length += 1;
        buffer[length..length + lengths[index]].copy_from_slice(&parts[index][..lengths[index]]);
        length += lengths[index];
    }
    if length == 0 {
        buffer[0] = b'/';
        length = 1;
    }
    Path {
        value: buffer,
        length,
    }
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

/// The two ESCC channels, read from the children of the `escc` node below the
/// selected Mac I/O node.
///
/// A CHRP tree puts `ch-a` and `ch-b` under `escc`; OpenBIOS and Apple's Old
/// World firmware both build that shape.  The child `reg` values are offsets
/// in the Mac I/O window on both, but a relative `reg` is also accepted.  If
/// the children are missing, the chip's fixed channel layout inside the ESCC
/// window is used.
pub fn escc(client: &Client, macio: &MacIo) -> Option<Escc> {
    let node = find_child(client, macio.node, b"escc", b"escc")?;
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

fn find_child(client: &Client, parent: u32, device_type: &[u8], name: &[u8]) -> Option<u32> {
    let mut child = client.child(parent);
    while let Some(node) = child {
        if matches_property(client, node, b"device_type\0", device_type)
            || matches_property(client, node, b"name\0", name)
        {
            return Some(node);
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

fn starts_with(buffer: &[u8], length: usize, value: &[u8]) -> bool {
    let length = length.min(buffer.len());
    length >= value.len() && &buffer[..value.len()] == value
}

fn string_equals(buffer: &[u8], length: usize, value: &[u8]) -> bool {
    let length = length.min(buffer.len());
    let end = buffer[..length]
        .iter()
        .position(|&byte| byte == 0)
        .unwrap_or(length);
    &buffer[..end] == value
}

fn trim_string(buffer: &[u8], length: usize) -> usize {
    let length = length.min(buffer.len());
    buffer[..length]
        .iter()
        .position(|&byte| byte == 0)
        .unwrap_or(length)
}

fn matches_property(client: &Client, node: u32, property: &[u8], value: &[u8]) -> bool {
    let mut buffer = [0u8; 32];
    match client.getprop(node, property, &mut buffer) {
        Some(length) => string_equals(&buffer, length, value),
        None => false,
    }
}

/// The interrupt controller built into the selected Mac I/O node.
///
/// The beige G3 and the PowerBook's primary Mac I/O controller are Heathrow;
/// the PowerBook's second controller is Gatwick.  Both put the same interrupt
/// controller block at the start of the Mac I/O window.  It is not an OpenPIC:
/// OpenPIC is the New World controller in KeyLargo, and neither this machine
/// nor QEMU's `g3beige` model has one.  The `compatible` property selects the
/// controller, so a machine with a different Mac I/O device fails here instead
/// of writing Heathrow registers into an OpenPIC.
pub struct Pic {
    pub base: u32,
}

pub fn pic(client: &Client, macio: &MacIo) -> Option<Pic> {
    let mut compatible = [0u8; 32];
    let length = client.getprop(macio.node, b"compatible\0", &mut compatible)?;
    let first = first_string(&compatible, length)?;
    if first == b"heathrow" || first == b"gatwick" {
        Some(Pic { base: macio.base })
    } else {
        None
    }
}

fn first_string(buffer: &[u8], length: usize) -> Option<&[u8]> {
    let end = trim_string(buffer, length);
    if end == 0 {
        None
    } else {
        Some(&buffer[..end])
    }
}

/// The CUDA, which carries the MOS 6522 VIA and its programmable timer.
///
/// `reg` holds the offset of the CUDA window inside Mac I/O.
/// `AAPL,interrupts` holds the interrupt controller line the CUDA asserts.
/// The lookup is relative to the selected Mac I/O node: a second controller's
/// VIA is not the machine's timer.
pub struct Cuda {
    pub base: u32,
    pub line: u32,
}

pub fn cuda(client: &Client, macio: &MacIo) -> Option<Cuda> {
    let node = find_child(client, macio.node, b"via-cuda", b"via-cuda")?;
    let base = via_base(client, macio, node)?;
    let line = client.getprop_u32(node, b"AAPL,interrupts\0")?;
    Some(Cuda { base, line })
}

/// The ADB host and the keyboard's bus address.
///
/// QEMU's `g3beige` and a beige G3 route ADB through the CUDA's 6522 shift
/// register.  The PowerBook G3 M4753 has no CUDA: its 6522 sits in the PMU
/// node (`via-pmu`) and the PMU microcontroller drives the same ADB pins.
/// Both trees put an `adb` child under the VIA node and the keyboard below
/// that, so the transport is selected by the node that exists, and the
/// keyboard address comes from its `reg` rather than a constant.
pub enum AdbHost {
    Cuda { base: u32, keyboard: u8 },
    Pmu { base: u32, keyboard: u8 },
}

pub fn adb(client: &Client, macio: &MacIo) -> Option<AdbHost> {
    if let Some(node) = find_child(client, macio.node, b"via-cuda", b"via-cuda") {
        if let Some(host) = adb_from_via(client, macio, node, false) {
            return Some(host);
        }
    }
    if let Some(node) = find_child(client, macio.node, b"via-pmu", b"via-pmu") {
        if let Some(host) = adb_from_via(client, macio, node, true) {
            return Some(host);
        }
    }
    None
}

fn adb_from_via(client: &Client, macio: &MacIo, node: u32, pmu: bool) -> Option<AdbHost> {
    let base = via_base(client, macio, node)?;
    let keyboard = adb_keyboard(client, node)?;
    Some(if pmu {
        AdbHost::Pmu { base, keyboard }
    } else {
        AdbHost::Cuda { base, keyboard }
    })
}

fn via_base(client: &Client, macio: &MacIo, node: u32) -> Option<u32> {
    let mut reg = [0u8; 8];
    let length = client.getprop(node, b"reg\0", &mut reg)?;
    if length < 4 {
        return None;
    }
    let offset = u32::from_be_bytes([reg[0], reg[1], reg[2], reg[3]]);
    let size = if length >= 8 {
        u32::from_be_bytes([reg[4], reg[5], reg[6], reg[7]])
    } else {
        0
    };
    let base = macio.base.checked_add(offset)?;
    let window_end = macio.base.checked_add(macio.size)?;
    if base < macio.base || base.checked_add(size)? > window_end {
        return None;
    }
    Some(base)
}

/// The keyboard's ADB address from the `adb` child of the VIA node.
///
/// A tree with an `adb` node but no keyboard child is treated as having the
/// standard address; the gate's tree names `keyboard@8` explicitly, and a
/// hardware tree that omits the child still gets a working default.
fn adb_keyboard(client: &Client, via_node: u32) -> Option<u8> {
    let adb = find_child(client, via_node, b"adb", b"adb")?;
    let mut child = client.child(adb);
    while let Some(node) = child {
        if matches_property(client, node, b"device_type\0", b"keyboard")
            || matches_property(client, node, b"name\0", b"keyboard")
        {
            return Some(adb_address(client, node).unwrap_or(crate::adb::DEFAULT_KEYBOARD_ADDRESS));
        }
        child = client.peer(node);
    }
    Some(crate::adb::DEFAULT_KEYBOARD_ADDRESS)
}

fn adb_address(client: &Client, node: u32) -> Option<u8> {
    let mut reg = [0u8; 4];
    let length = client.getprop(node, b"reg\0", &mut reg)?;
    match length {
        0 => None,
        1..=3 => Some(reg[0]),
        _ => Some(reg[3]),
    }
}

pub fn timebase(client: &Client) -> Option<u32> {
    let cpus = client.finddevice(b"/cpus\0")?;
    let cpu = client.child(cpus)?;
    client.getprop_u32(cpu, b"timebase-frequency\0")
}

/// The ATA channel that carries the boot disk.
///
/// The node publishes two `reg` ranges: the task-file registers and the DBDMA
/// channel.  Only the first range is used by a polled PIO driver.  The
/// firmware numbers the channels and assigns the Mac I/O offsets, so both the
/// `reg` offset and the `AAPL,interrupts` line are read here rather than
/// compiled in.  A channel whose tree has a `disk` child wins over an empty
/// one.  The scan follows the selected node's children through `media-bay`,
/// where the PowerBook puts the media-bay channel.
pub struct Ata {
    pub base: u32,
    pub size: u32,
    pub line: u32,
}

pub fn ata(client: &Client, macio: &MacIo) -> Option<Ata> {
    let mut fallback = None;
    scan_ata(client, macio, macio.node, 0, &mut fallback).or(fallback)
}

fn scan_ata(
    client: &Client,
    macio: &MacIo,
    node: u32,
    depth: u32,
    fallback: &mut Option<Ata>,
) -> Option<Ata> {
    let mut child = client.child(node);
    while let Some(child_node) = child {
        if let Some(ata) = ata_node(client, macio, child_node) {
            if has_disk(client, child_node) {
                return Some(ata);
            }
            fallback.get_or_insert(ata);
        }
        if depth < MAX_DEPTH {
            if let Some(ata) = scan_ata(client, macio, child_node, depth + 1, fallback) {
                return Some(ata);
            }
        }
        child = client.peer(child_node);
    }
    None
}

fn ata_node(client: &Client, macio: &MacIo, node: u32) -> Option<Ata> {
    if !is_ata(client, node) {
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
    let base = macio.base.checked_add(offset)?;
    let window_end = macio.base.checked_add(macio.size)?;
    if base < macio.base || base.checked_add(size)? > window_end {
        return None;
    }
    Some(Ata { base, size, line })
}

fn has_disk(client: &Client, node: u32) -> bool {
    let mut child = client.child(node);
    while let Some(child_node) = child {
        let mut name = [0u8; 8];
        if let Some(length) = client.getprop(child_node, b"name\0", &mut name) {
            if starts_with(&name, length, b"disk") {
                return true;
            }
        }
        child = client.peer(child_node);
    }
    false
}
