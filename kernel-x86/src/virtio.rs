// SPDX-License-Identifier: GPL-3.0-only
//! The legacy (virtio 0.9.5) PCI transport shared by the x86 virtio drivers.
//!
//! The device's registers are an I/O-port block at BAR0.  A virtqueue is the
//! split layout: a descriptor table, an available ring and a used ring, the
//! last at a page boundary.  QEMU exposes this with `disable-modern=on`, which
//! keeps every driver to port I/O and avoids the PCI capability walk.
//!
//! Queue memory comes from the shared frame allocator, which the machine maps
//! one-to-one, so a frame address is also the device's DMA address.

use core::ptr::{read_volatile, write_volatile};
use core::sync::atomic::{fence, Ordering};

use crate::memory;
use crate::port::{inb, inl, outb, outl, outw};

// Register block, offsets from BAR0.
const HOST_FEATURES: u16 = 0x00;
const GUEST_FEATURES: u16 = 0x04;
const QUEUE_PFN: u16 = 0x08;
const QUEUE_SIZE: u16 = 0x0C;
const QUEUE_SELECT: u16 = 0x0E;
const QUEUE_NOTIFY: u16 = 0x10;
const DEVICE_STATUS: u16 = 0x12;
/// Device-specific configuration starts here.
pub const DEVICE_CONFIG: u16 = 0x14;

pub const STATUS_ACK: u8 = 1;
pub const STATUS_DRIVER: u8 = 2;
pub const STATUS_DRIVER_OK: u8 = 4;
pub const STATUS_FAILED: u8 = 0x80;

/// The descriptor continues through `next`.
pub const DESC_NEXT: u16 = 1;
/// The device writes this buffer.
pub const DESC_WRITE: u16 = 2;

const RING_ALIGN: usize = 4096;

#[repr(C)]
#[derive(Clone, Copy)]
pub struct Descriptor {
    pub addr: u64,
    pub len: u32,
    pub flags: u16,
    pub next: u16,
}

/// One virtqueue.  The driver publishes descriptor heads on the available
/// ring and reads completions from the used ring.
#[derive(Clone, Copy)]
pub struct Queue {
    base: usize,
    pub size: u16,
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

    /// The descriptor table entry `index`.
    pub fn descriptor(&self, index: u16) -> *mut Descriptor {
        (self.base + 16 * index as usize) as *mut Descriptor
    }

    /// Publish a chain whose first descriptor is `head`.  The chain itself must
    /// already be written.
    pub fn publish(&mut self, head: u16) {
        let slot = self.avail_index % self.size;
        // SAFETY: the available ring lies inside the queue's frames.
        unsafe {
            write_volatile(self.avail_ring(slot), head);
            fence(Ordering::SeqCst);
            self.avail_index = self.avail_index.wrapping_add(1);
            write_volatile(self.avail_index_ptr(), self.avail_index);
        }
    }

    /// The used index the device has reached.
    pub fn used(&self) -> u16 {
        // SAFETY: the used index lies inside the queue's frames.
        unsafe { read_volatile(self.used_index_ptr()) }
    }

    /// Take the next completion: `(descriptor head, bytes written)`, or `None`
    /// when the device has used nothing new.
    pub fn take_used(&mut self) -> Option<(u16, u32)> {
        if self.used() == self.last_used {
            return None;
        }
        let slot = self.last_used % self.size;
        let address = (self.base + self.used_offset + 4 + 8 * slot as usize) as *const u32;
        // SAFETY: the used ring lies inside the queue's frames.
        let (id, len) = unsafe { (read_volatile(address), read_volatile(address.add(1))) };
        self.last_used = self.last_used.wrapping_add(1);
        Some((id as u16, len))
    }
}

/// A device found on bus 0, with its register block.
pub struct Transport {
    pub io: u16,
}

impl Transport {
    /// Reset the device, announce the driver and negotiate features.  Returns the
    /// feature bits the device offers, after masking to `wanted`.
    pub fn begin(io: u16, wanted: u32) -> (Self, u32) {
        outb(io + DEVICE_STATUS, 0);
        outb(io + DEVICE_STATUS, STATUS_ACK);
        outb(io + DEVICE_STATUS, STATUS_ACK | STATUS_DRIVER);
        let offered = inl(io + HOST_FEATURES);
        let accepted = offered & wanted;
        outl(io + GUEST_FEATURES, accepted);
        (Self { io }, accepted)
    }

    /// Allocate and register virtqueue `index`.  Returns `None` if the device
    /// has no such queue or memory is short.
    pub fn queue(&self, index: u16) -> Option<Queue> {
        outw(self.io + QUEUE_SELECT, index);
        let size = crate::port::inw(self.io + QUEUE_SIZE);
        if size == 0 {
            return None;
        }
        let size_usize = size as usize;
        let avail_end = size_usize * 16 + 6 + 2 * size_usize;
        let used_offset = avail_end.next_multiple_of(RING_ALIGN);
        let used_end = used_offset + 6 + 8 * size_usize;
        let frames = used_end.div_ceil(4096);

        let base = memory::alloc_frames(frames)? as usize;
        // SAFETY: the frames were just allocated, so nothing else uses them.
        unsafe { core::ptr::write_bytes(base as *mut u8, 0, frames * 4096) };
        outl(self.io + QUEUE_PFN, (base / 4096) as u32);
        Some(Queue {
            base,
            size,
            used_offset,
            avail_index: 0,
            last_used: 0,
        })
    }

    /// Set the driver-ok bit: the device may now process queues.
    pub fn go(&self) {
        outb(
            self.io + DEVICE_STATUS,
            STATUS_ACK | STATUS_DRIVER | STATUS_DRIVER_OK,
        );
    }

    /// Ring queue `index`'s doorbell.
    pub fn notify(&self, index: u16) {
        outw(self.io + QUEUE_NOTIFY, index);
    }

    /// Read one byte of device-specific configuration.
    pub fn config_u8(&self, offset: u16) -> u8 {
        inb(self.io + DEVICE_CONFIG + offset)
    }

    /// Read a little-endian u32 of device configuration.
    pub fn config_u32(&self, offset: u16) -> u32 {
        inl(self.io + DEVICE_CONFIG + offset)
    }

    /// Mark the device failed; used when setup cannot complete.
    pub fn fail(&self) {
        outb(self.io + DEVICE_STATUS, STATUS_FAILED);
    }
}
