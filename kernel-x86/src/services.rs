// SPDX-License-Identifier: GPL-3.0-only
//! The x86_64 machine's module service platform.
//!
//! The table itself, its argument guards and its refusals live in
//! `kernel-core`.  This file supplies only what the machine owns: the serial
//! console, the PIT clock, the heap, the smoltcp slots, the boot configuration,
//! the MAC information block and the reserved region of the virtio-blk disk.
//!
//! Entries the machine does not provide — console scrollback, screen, module
//! arenas, UDP — keep the shared refusal.  They are not missing pointers and
//! they do not differ from the Indy's answer for an absent resource.

use core::alloc::Layout;

use block_store::SECTOR;
use kernel_core::service::Platform;

use crate::{config, net, serial, tcp, timer, virtio_blk};

/// The x86_64 platform instance.
pub struct X86Platform;

static PLATFORM: X86Platform = X86Platform;

/// Install the platform.  Called once after the heap is ready and before the
/// first module load.
pub fn install() {
    kernel_core::service::install(&PLATFORM);
}

impl Platform for X86Platform {
    fn write(&self, bytes: &[u8]) {
        serial::write(bytes);
    }

    fn clock_ms(&self) -> u64 {
        u64::from(timer::ticks()) * 10
    }

    fn alloc(&self, layout: Layout) -> *mut u8 {
        // SAFETY: the global allocator is the machine's bump heap, which
        // accepts any valid layout.
        unsafe { alloc::alloc::alloc(layout) }
    }

    fn poll(&self) {
        net::poll();
    }

    fn tcp_listen(&self, slot: u32, port: u16) -> i32 {
        tcp::listen(slot, port)
    }

    fn tcp_state(&self, slot: u32) -> u32 {
        tcp::state(slot)
    }

    fn tcp_recv(&self, slot: u32, out: &mut [u8]) -> isize {
        tcp::recv(slot, out)
    }

    fn tcp_send(&self, slot: u32, bytes: &[u8]) -> isize {
        tcp::send(slot, bytes)
    }

    fn tcp_close(&self, slot: u32) {
        tcp::close(slot);
    }

    fn tcp_abort(&self, slot: u32) {
        tcp::abort(slot);
    }

    fn tcp_connect(&self, slot: u32, address: u32, port: u16) -> i32 {
        tcp::connect(slot, address, port)
    }

    fn tcp_events(&self, slot: u32) -> u32 {
        tcp::events(slot)
    }

    fn tcp_slots(&self) -> u32 {
        tcp::slots()
    }

    fn config_get(&self, key: &[u8], out: &mut [u8]) -> isize {
        config::get(key, out)
    }

    /// Machine information.  Query 1 is the node text the wasm policy module
    /// reads: `mac: aa:bb:cc:dd:ee:ff`.  Other queries are refused because the
    /// machine has no status block yet.
    fn info(&self, which: u32, out: &mut [u8]) -> isize {
        if which > 1 {
            return -1;
        }
        let Some(mac) = net::mac() else {
            return -1;
        };
        const DIGITS: &[u8; 16] = b"0123456789abcdef";
        let mut text = [0u8; 24];
        text[..5].copy_from_slice(b"mac: ");
        for (i, byte) in mac.iter().enumerate() {
            let at = 5 + i * 3;
            text[at] = DIGITS[(byte >> 4) as usize];
            text[at + 1] = DIGITS[(byte & 0xF) as usize];
            if i < 5 {
                text[at + 2] = b':';
            }
        }
        let count = text.len().min(out.len());
        out[..count].copy_from_slice(&text[..count]);
        count as isize
    }

    /// The block service exposes only the epoch store, at target 3, mapped to
    /// a reserved region of the machine's disk.
    fn block_read(&self, target: u8, lba: u32, out: &mut [u8; SECTOR]) -> i32 {
        const STORE: block_store::Store =
            block_store::Store::mapped(3, crate::virtio_blk::STORE_OFFSET);
        let Some(disk) = virtio_blk::disk() else {
            return -1;
        };
        STORE.read_sector(disk, target, lba, out)
    }

    fn block_write(&self, target: u8, lba: u32, data: &[u8; SECTOR]) -> i32 {
        const STORE: block_store::Store =
            block_store::Store::mapped(3, crate::virtio_blk::STORE_OFFSET);
        let Some(disk) = virtio_blk::disk() else {
            return -1;
        };
        STORE.write_sector(disk, target, lba, data)
    }
}
