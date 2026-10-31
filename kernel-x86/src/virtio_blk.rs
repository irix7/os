// SPDX-License-Identifier: GPL-3.0-only
//! A polled virtio-blk driver over the legacy PCI transport in `virtio.rs`.
//!
//! One request at a time: a header, one 512-byte data buffer and a status byte,
//! chained as three descriptors.  The driver waits for the device to complete
//! each request.  No feature is negotiated, so the device uses the base
//! request format and a single queue.

use core::ptr::{addr_of, addr_of_mut, write_volatile};

use crate::pci;
use crate::virtio::{Descriptor, Queue, Transport, DESC_NEXT, DESC_WRITE};

const VENDOR: u16 = 0x1AF4;
const DEVICE_BLK_LEGACY: u16 = 0x1001;

pub const SECTOR: usize = 512;

const REQUEST_IN: u32 = 0; // read from the device
const REQUEST_OUT: u32 = 1; // write to the device

#[repr(C)]
#[derive(Clone, Copy)]
struct Header {
    kind: u32,
    reserved: u32,
    sector: u64,
}

static mut HEADER: Header = Header {
    kind: 0,
    reserved: 0,
    sector: 0,
};
static mut DATA: [u8; SECTOR] = [0; SECTOR];
static mut STATUS: u8 = 0xFF;

pub struct Disk {
    transport: Transport,
    queue: Queue,
    sectors: u64,
}

/// Find and initialise the first legacy virtio-blk device on bus 0.
pub fn probe() -> Option<Disk> {
    let function = pci::find(VENDOR, DEVICE_BLK_LEGACY)?;
    pci::enable(function);
    let io = pci::io_bar0(function);

    let (transport, _) = Transport::begin(io, 0);
    let low = u64::from(transport.config_u32(0));
    let high = u64::from(transport.config_u32(4));
    let sectors = low | (high << 32);

    let Some(queue) = transport.queue(0) else {
        transport.fail();
        return None;
    };
    if queue.size < 3 {
        transport.fail();
        return None;
    }
    transport.go();
    Some(Disk {
        transport,
        queue,
        sectors,
    })
}

impl Disk {
    /// The disk's capacity in 512-byte sectors.
    pub fn sectors(&self) -> u64 {
        self.sectors
    }

    /// Read sector `sector` into `out`.  Returns `false` on a device error or a
    /// timeout.
    pub fn read(&mut self, sector: u64, out: &mut [u8; SECTOR]) -> bool {
        if !self.request(REQUEST_IN, sector, None) {
            return false;
        }
        // SAFETY: the request has completed, so the device has released DATA.
        unsafe { out.copy_from_slice(&*addr_of!(DATA)) };
        true
    }

    /// Write `data` to sector `sector`.  Returns `false` on a device error or a
    /// timeout.
    pub fn write(&mut self, sector: u64, data: &[u8; SECTOR]) -> bool {
        self.request(REQUEST_OUT, sector, Some(data))
    }

    fn request(&mut self, kind: u32, sector: u64, data: Option<&[u8; SECTOR]>) -> bool {
        // SAFETY: one request is in flight at a time, so the static buffers
        // belong to the device only between `publish` and completion.
        unsafe {
            if let Some(data) = data {
                (*addr_of_mut!(DATA)).copy_from_slice(data);
            }
            write_volatile(
                addr_of_mut!(HEADER),
                Header {
                    kind,
                    reserved: 0,
                    sector,
                },
            );
            write_volatile(addr_of_mut!(STATUS), 0xFF);

            // Header, data, status: the data buffer is device-written on a read.
            let data_flags = if kind == REQUEST_IN {
                DESC_NEXT | DESC_WRITE
            } else {
                DESC_NEXT
            };
            write_volatile(
                self.queue.descriptor(0),
                Descriptor {
                    addr: addr_of!(HEADER) as u64,
                    len: core::mem::size_of::<Header>() as u32,
                    flags: DESC_NEXT,
                    next: 1,
                },
            );
            write_volatile(
                self.queue.descriptor(1),
                Descriptor {
                    addr: addr_of!(DATA) as u64,
                    len: SECTOR as u32,
                    flags: data_flags,
                    next: 2,
                },
            );
            write_volatile(
                self.queue.descriptor(2),
                Descriptor {
                    addr: addr_of!(STATUS) as u64,
                    len: 1,
                    flags: DESC_WRITE,
                    next: 0,
                },
            );
        }

        self.queue.publish(0);
        self.transport.notify(0);

        for _ in 0..50_000_000u32 {
            if self.queue.take_used().is_some() {
                // SAFETY: the device has finished; the status byte is its last write.
                return unsafe { read_status() } == 0;
            }
            core::hint::spin_loop();
        }
        false
    }
}

/// Read the status byte the device wrote.
///
/// # Safety
///
/// Call only after the request has completed.
unsafe fn read_status() -> u8 {
    core::ptr::read_volatile(addr_of!(STATUS))
}

/// Boot-time proof of the disk: read a marker the gate placed in sector 1,
/// then write a pattern to sector 2 and read it back.  A missing device or a
/// wrong byte stops the machine with a failing exit status.
pub fn selftest() {
    let Some(mut disk) = probe() else {
        crate::print_line(&[b"x86: blk: no virtio-blk device"]);
        crate::qemu_exit(1);
    };
    crate::serial::write(b"x86: blk capacity=");
    crate::print_dec(disk.sectors());
    crate::serial::write(b" sectors\n");

    let mut sector = [0u8; SECTOR];
    let marker = b"IRIX7-X86-DISK-OK";
    if !disk.read(1, &mut sector) || &sector[..marker.len()] != marker {
        crate::print_line(&[b"x86: blk read sector 1 failed"]);
        crate::qemu_exit(1);
    }
    crate::print_line(&[b"x86: blk read sector 1=IRIX7-X86-DISK-OK"]);

    let mut pattern = [0u8; SECTOR];
    for (i, byte) in pattern.iter_mut().enumerate() {
        *byte = b"IRIX7-WRITE"[i % 11];
    }
    let mut readback = [0u8; SECTOR];
    if !disk.write(2, &pattern) || !disk.read(2, &mut readback) || readback != pattern {
        crate::print_line(&[b"x86: blk write readback failed"]);
        crate::qemu_exit(1);
    }
    crate::print_line(&[b"x86: blk write readback ok"]);
}
