// SPDX-License-Identifier: GPL-3.0-only
//! A polled virtio-net driver over the legacy PCI transport in `virtio.rs`.
//!
//! No device interrupt is enabled.  The receive path is `poll_rx`, and transmit
//! waits for its own completion.  Buffers are static, so they sit at
//! identity-mapped physical addresses.

use core::ptr::{addr_of, addr_of_mut, write_volatile};

use crate::pci;
use crate::virtio::{Descriptor, Queue, Transport, DESC_WRITE};

const VENDOR: u16 = 0x1AF4;
const DEVICE_NET_LEGACY: u16 = 0x1000;

/// VIRTIO_NET_F_MAC: the device has a MAC address in its configuration.
const FEATURE_MAC: u32 = 1 << 5;

/// The legacy net header prepended to every frame: no mergeable buffers and no
/// offload features are negotiated, so it is the base 10-byte header.
const NET_HEADER: usize = 10;
/// One receive buffer: the header and a full Ethernet frame.
const BUFFER: usize = 2048;
/// Ethernet frames the driver hands up, without the header.
pub const MAX_FRAME: usize = 1514;
/// Receive buffers the driver keeps posted.
const RX_SLOTS: usize = 16;
/// The minimum Ethernet frame without FCS; shorter frames are padded.
const MIN_FRAME: usize = 60;

static mut RX_BUFFERS: [[u8; BUFFER]; RX_SLOTS] = [[0; BUFFER]; RX_SLOTS];
static mut TX_BUFFER: [u8; BUFFER] = [0; BUFFER];

/// The driver state for one virtio-net device.
pub struct Nic {
    transport: Transport,
    mac: [u8; 6],
    rx: Queue,
    tx: Queue,
}

/// Find and initialise the first legacy virtio-net device on bus 0.
pub fn probe() -> Option<Nic> {
    let function = pci::find(VENDOR, DEVICE_NET_LEGACY)?;
    pci::enable(function);
    let io = pci::io_bar0(function);

    let (transport, _) = Transport::begin(io, FEATURE_MAC);
    let mut mac = [0u8; 6];
    for (i, byte) in mac.iter_mut().enumerate() {
        *byte = transport.config_u8(i as u16);
    }

    let (Some(mut rx), Some(tx)) = (transport.queue(0), transport.queue(1)) else {
        transport.fail();
        return None;
    };
    if (rx.size as usize) < RX_SLOTS {
        transport.fail();
        return None;
    }

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
        rx.publish(i as u16);
    }

    transport.go();
    transport.notify(0);

    Some(Nic {
        transport,
        mac,
        rx,
        tx,
    })
}

impl Nic {
    pub fn mac(&self) -> [u8; 6] {
        self.mac
    }

    /// Return the next received frame, copied into `out`, or `None` when the
    /// receive ring is empty.  Empty frames are skipped.  Each buffer goes back
    /// on the ring before the next one is read.
    pub fn poll_rx(&mut self, out: &mut [u8; MAX_FRAME]) -> Option<usize> {
        while let Some((id, len)) = self.rx.take_used() {
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
            self.rx.publish(id as u16);
            self.transport.notify(0);
            if id < RX_SLOTS && payload > 0 {
                return Some(payload);
            }
        }
        None
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
        self.tx.publish(0);
        self.transport.notify(1);

        for _ in 0..50_000_000u32 {
            if self.tx.take_used().is_some() {
                return true;
            }
            core::hint::spin_loop();
        }
        false
    }
}
