// SPDX-License-Identifier: GPL-3.0-only
//! A polled virtio-net driver over the legacy (virtio 0.9.5) PCI interface.
//!
//! The legacy interface is the I/O-port register block at BAR0.  QEMU exposes
//! it with `virtio-net-pci,disable-modern=on`, which keeps the driver to port
//! reads and writes and avoids walking PCI capabilities.  The driver is polled:
//! no device interrupt is enabled, so the receive path is `poll_rx` and transmit
//! waits for its own completion.
//!
//! Buffers are static, so they sit at identity-mapped physical addresses.  The
//! virtqueues are frames from the shared allocator.

use core::ptr::{addr_of, addr_of_mut, read_volatile, write_volatile};
use core::sync::atomic::{fence, Ordering};

use crate::memory;
use crate::pci;
use crate::port::{inb, inl, outb, outl, outw};

const VENDOR: u16 = 0x1AF4;
const DEVICE_NET_LEGACY: u16 = 0x1000;

// Legacy register block, offsets from BAR0.
const HOST_FEATURES: u16 = 0x00;
const GUEST_FEATURES: u16 = 0x04;
const QUEUE_PFN: u16 = 0x08;
const QUEUE_SIZE: u16 = 0x0C;
const QUEUE_SELECT: u16 = 0x0E;
const QUEUE_NOTIFY: u16 = 0x10;
const DEVICE_STATUS: u16 = 0x12;
const DEVICE_CONFIG: u16 = 0x14;

const STATUS_ACK: u8 = 1;
const STATUS_DRIVER: u8 = 2;
const STATUS_DRIVER_OK: u8 = 4;

/// VIRTIO_NET_F_MAC: the device has a MAC address in its configuration.
const FEATURE_MAC: u32 = 1 << 5;

const DESC_WRITE: u16 = 2;
const RING_ALIGN: usize = 4096;

/// The legacy net header prepended to every frame: no mergeable buffers and no
/// offload features are negotiated, so it is the base 10-byte header.
const NET_HEADER: usize = 10;
/// One receive buffer: the header and a full Ethernet frame.
const BUFFER: usize = 2048;
/// Ethernet frames the driver hands up, without the header.
pub const MAX_FRAME: usize = 1514;
/// Receive buffers the driver keeps posted.  The ring may be larger; the
/// unused slots stay empty.
const RX_SLOTS: usize = 16;
/// The minimum Ethernet frame without FCS; shorter frames are padded.
const MIN_FRAME: usize = 60;

static mut RX_BUFFERS: [[u8; BUFFER]; RX_SLOTS] = [[0; BUFFER]; RX_SLOTS];
static mut TX_BUFFER: [u8; BUFFER] = [0; BUFFER];

#[repr(C)]
#[derive(Clone, Copy)]
struct Descriptor {
    addr: u64,
    len: u32,
    flags: u16,
    next: u16,
}

/// One virtqueue in the legacy split layout: the descriptor table, the
/// available ring and the used ring, the last at a page boundary.
#[derive(Clone, Copy)]
struct Queue {
    base: usize,
    size: u16,
    used_offset: usize,
    avail_index: u16,
    last_used: u16,
}

impl Queue {
    fn avail_index_ptr(&self) -> *mut u16 {
        (self.base + self.size as usize * 16 + 2) as *mut u16
    }

    fn avail_ring(&self, slot: u16) -> *mut u16 {
        (self.base + self.size as usize * 16 + 4 + 2 * slot as usize) as *mut u16
    }

    fn used_index_ptr(&self) -> *const u16 {
        (self.base + self.used_offset + 2) as *const u16
    }

    fn used_element(&self, slot: u16) -> (u32, u32) {
        let address = (self.base + self.used_offset + 4 + 8 * slot as usize) as *const u32;
        // SAFETY: the used ring lies inside the queue's frames.
        unsafe { (read_volatile(address), read_volatile(address.add(1))) }
    }

    fn descriptor(&self, index: u16) -> *mut Descriptor {
        (self.base + 16 * index as usize) as *mut Descriptor
    }

    /// Post one descriptor index on the available ring and publish it.
    fn publish(&mut self, descriptor: u16) {
        let slot = self.avail_index % self.size;
        // SAFETY: the available ring lies inside the queue's frames.
        unsafe {
            write_volatile(self.avail_ring(slot), descriptor);
            fence(Ordering::SeqCst);
            self.avail_index = self.avail_index.wrapping_add(1);
            write_volatile(self.avail_index_ptr(), self.avail_index);
        }
    }
}

/// The driver state for one virtio-net device.
pub struct Nic {
    io: u16,
    mac: [u8; 6],
    rx: Queue,
    tx: Queue,
}

/// Allocate and zero one legacy virtqueue of `size` entries.
fn queue_for(size: u16) -> Option<Queue> {
    let size_usize = size as usize;
    let avail_end = size_usize * 16 + 6 + 2 * size_usize;
    let used_offset = avail_end.next_multiple_of(RING_ALIGN);
    let used_end = used_offset + 6 + 8 * size_usize;
    let frames = used_end.div_ceil(4096);

    let base = memory::alloc_frames(frames)? as usize;
    // SAFETY: the frames were just allocated, so nothing else uses them.
    unsafe { core::ptr::write_bytes(base as *mut u8, 0, frames * 4096) };
    Some(Queue {
        base,
        size,
        used_offset,
        avail_index: 0,
        last_used: 0,
    })
}

/// Find and initialise the first legacy virtio-net device on bus 0.
pub fn probe() -> Option<Nic> {
    let function = pci::find(VENDOR, DEVICE_NET_LEGACY)?;
    pci::enable(function);
    let io = pci::io_bar0(function);

    // Reset, then announce the driver.
    outb(io + DEVICE_STATUS, 0);
    outb(io + DEVICE_STATUS, STATUS_ACK);
    outb(io + DEVICE_STATUS, STATUS_ACK | STATUS_DRIVER);

    let host = inl(io + HOST_FEATURES);
    outl(io + GUEST_FEATURES, host & FEATURE_MAC);

    let mut mac = [0u8; 6];
    for (i, byte) in mac.iter_mut().enumerate() {
        *byte = inb(io + DEVICE_CONFIG + i as u16);
    }

    outw(io + QUEUE_SELECT, 0);
    let rx_size = crate::port::inw(io + QUEUE_SIZE);
    outw(io + QUEUE_SELECT, 1);
    let tx_size = crate::port::inw(io + QUEUE_SIZE);
    if rx_size == 0 || tx_size == 0 || rx_size < RX_SLOTS as u16 {
        outb(io + DEVICE_STATUS, 0x80);
        return None;
    }

    let mut rx = queue_for(rx_size)?;
    let tx = queue_for(tx_size)?;
    outw(io + QUEUE_SELECT, 0);
    outl(io + QUEUE_PFN, (rx.base / 4096) as u32);
    outw(io + QUEUE_SELECT, 1);
    outl(io + QUEUE_PFN, (tx.base / 4096) as u32);

    // Post every receive buffer.  Descriptor i owns RX_BUFFERS[i].
    for i in 0..RX_SLOTS {
        let buffer = addr_of!(RX_BUFFERS) as usize + i * BUFFER;
        // SAFETY: the descriptor table is inside the receive queue's frames.
        unsafe {
            write_volatile(
                rx.descriptor(i as u16),
                Descriptor {
                    addr: buffer as u64,
                    len: BUFFER as u32,
                    flags: DESC_WRITE,
                    next: 0,
                },
            );
        }
    }
    for i in 0..RX_SLOTS {
        rx.publish(i as u16);
    }
    outb(
        io + DEVICE_STATUS,
        STATUS_ACK | STATUS_DRIVER | STATUS_DRIVER_OK,
    );
    outw(io + QUEUE_SELECT, 0);
    outw(io + QUEUE_NOTIFY, 0);

    Some(Nic { io, mac, rx, tx })
}

impl Nic {
    pub fn mac(&self) -> [u8; 6] {
        self.mac
    }

    /// Return the next received frame, copied into `out`, or `None` when the
    /// receive ring is empty.  Empty frames are skipped.  The buffer goes
    /// straight back on the ring.
    pub fn poll_rx(&mut self, out: &mut [u8; MAX_FRAME]) -> Option<usize> {
        loop {
            // SAFETY: the used index lies inside the receive queue's frames.
            let used = unsafe { read_volatile(self.rx.used_index_ptr()) };
            if used == self.rx.last_used {
                return None;
            }
            let slot = self.rx.last_used % self.rx.size;
            let (id, len) = self.rx.used_element(slot);
            self.rx.last_used = self.rx.last_used.wrapping_add(1);

            let id = id as usize;
            let payload = (len as usize).saturating_sub(NET_HEADER).min(MAX_FRAME);
            if id < RX_SLOTS && payload > 0 {
                let source = addr_of!(RX_BUFFERS) as usize + id * BUFFER + NET_HEADER;
                // SAFETY: the buffer is owned by the device until republished,
                // and the copy stays inside it.
                unsafe {
                    core::ptr::copy_nonoverlapping(source as *const u8, out.as_mut_ptr(), payload);
                }
            }
            // Give the buffer back to the device before anything else.
            self.rx.publish(id as u16);
            // SAFETY: the queue notify register is a plain port.
            outw_notify(self.io, 0);

            if id < RX_SLOTS && payload > 0 {
                return Some(payload);
            }
        }
    }

    /// Transmit one frame and wait until the device has used its buffer.  The
    /// frame is padded to the Ethernet minimum.
    pub fn send(&mut self, frame: &[u8]) -> bool {
        let len = frame.len().min(MAX_FRAME);
        let padded = len.max(MIN_FRAME);
        // SAFETY: the TX buffer is used by one descriptor at a time, and this
        // waits for the device to finish with it before returning.
        unsafe {
            let buffer = &mut *addr_of_mut!(TX_BUFFER);
            buffer[..NET_HEADER].fill(0);
            buffer[NET_HEADER..NET_HEADER + len].copy_from_slice(&frame[..len]);
            buffer[NET_HEADER + len..NET_HEADER + padded].fill(0);

            write_volatile(
                self.tx.descriptor(0),
                Descriptor {
                    addr: addr_of!(TX_BUFFER) as u64,
                    len: (NET_HEADER + padded) as u32,
                    flags: 0,
                    next: 0,
                },
            );
        }
        let before = self.tx.last_used;
        self.tx.publish(0);
        outw_notify(self.io, 1);

        // Completion: the used index moves past the entry we published.
        for _ in 0..50_000_000u32 {
            // SAFETY: the used index lies inside the transmit queue's frames.
            let used = unsafe { read_volatile(self.tx.used_index_ptr()) };
            if used != before {
                self.tx.last_used = used;
                return true;
            }
            core::hint::spin_loop();
        }
        false
    }
}

fn outw_notify(io: u16, queue: u16) {
    outw(io + QUEUE_NOTIFY, queue);
}
