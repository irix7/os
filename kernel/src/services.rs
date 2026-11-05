// SPDX-License-Identifier: GPL-3.0-only
//! The Indy's platform for the service table.
//!
//! The service table, its argument guards and its refusals are
//! `kernel-core::service`.  This file supplies the machine resources the Indy
//! owns: the mirrored serial/telnet console, the CP0-count network clock, the
//! kernel heap, the TCP and UDP socket slots, the DS1386 environment, the
//! status blocks, the REX3 shadow and SCSI.  The module entries keep their
//! refusals from `kernel-core`.

use core::alloc::Layout;

use kernel_core::service::Platform;
use module_block::SECTOR;

/// Milliseconds since boot from the network clock.
///
/// `Instant::total_millis` is signed; a clock that has not started yet reports
/// zero rather than wrapping into a huge positive value.
pub(crate) fn clock_ms() -> u64 {
    let ms = crate::net::clock_now().total_millis();
    if ms < 0 {
        0
    } else {
        ms as u64
    }
}

/// The Indy platform instance.
pub struct IndyPlatform;

static PLATFORM: IndyPlatform = IndyPlatform;

/// Install the platform.  Called once during boot, before the first service call.
pub fn install() {
    kernel_core::service::install(&PLATFORM);
}

/// The Indy's block service device: SCSI, with the caller's target passed
/// straight through.
struct ScsiBlock;

impl module_block::Block for ScsiBlock {
    fn read_sector(&mut self, target: u8, sector: u64, out: &mut [u8; SECTOR]) -> bool {
        u32::try_from(sector)
            .map(|lba| crate::scsi::read_sector(target, lba, out).is_ok())
            .unwrap_or(false)
    }

    fn write_sector(&mut self, target: u8, sector: u64, data: &[u8; SECTOR]) -> bool {
        u32::try_from(sector)
            .map(|lba| crate::scsi::write_sector(target, lba, data).is_ok())
            .unwrap_or(false)
    }
}

/// Every block target reaches the SCSI stack unchanged.
const BLOCK_STORE: module_block::Store = module_block::Store::direct();

impl Platform for IndyPlatform {
    fn write(&self, bytes: &[u8]) {
        crate::write_bytes(bytes);
    }

    fn clock_ms(&self) -> u64 {
        clock_ms()
    }

    fn alloc(&self, layout: Layout) -> *mut u8 {
        // SAFETY: the global allocator is the frame-backed bump heap.
        unsafe { alloc::alloc::alloc(layout) }
    }

    fn poll(&self) {
        crate::net::poll();
    }

    fn tcp_listen(&self, slot: u32, port: u16) -> i32 {
        crate::slots::tcp_listen(slot, port)
    }

    fn tcp_state(&self, slot: u32) -> u32 {
        crate::slots::tcp_state(slot)
    }

    fn tcp_recv(&self, slot: u32, out: &mut [u8]) -> isize {
        crate::slots::tcp_recv(slot, out)
    }

    fn tcp_send(&self, slot: u32, bytes: &[u8]) -> isize {
        crate::slots::tcp_send(slot, bytes)
    }

    fn tcp_close(&self, slot: u32) {
        crate::slots::tcp_close(slot);
    }

    fn tcp_abort(&self, slot: u32) {
        crate::slots::tcp_abort(slot);
    }

    fn tcp_connect(&self, slot: u32, address: u32, port: u16) -> i32 {
        crate::slots::tcp_connect(slot, address, port)
    }

    fn tcp_events(&self, slot: u32) -> u32 {
        crate::slots::tcp_events(slot)
    }

    fn tcp_slots(&self) -> u32 {
        crate::slots::tcp_slots()
    }

    fn console_read(&self, offset: u64, out: &mut [u8]) -> isize {
        crate::console_read(offset, out) as isize
    }

    fn console_total(&self) -> u64 {
        crate::console_total()
    }

    fn reset(&self) {
        crate::reboot();
    }

    fn poweroff(&self) {
        crate::poweroff();
    }

    fn status(&self, out: &mut [u8]) -> isize {
        crate::status::text(out) as isize
    }

    fn config_get(&self, key: &[u8], out: &mut [u8]) -> isize {
        let Ok(key) = core::str::from_utf8(key) else {
            return -1;
        };
        match crate::nvram::get(key, out) {
            Some(n) => n as isize,
            None => -1,
        }
    }

    fn info(&self, kind: u32, out: &mut [u8]) -> isize {
        let n = match kind {
            0 => crate::status::text(out),
            1 => crate::status::net(out),
            2 => crate::status::mem(out),
            3 => crate::status::time(out),
            _ => return -1,
        };
        n as isize
    }

    fn screen_read(&self, offset: u32, out: &mut [u8]) -> isize {
        crate::gfx::shadow_read(offset as usize, out) as isize
    }

    fn block_read(&self, target: u8, lba: u32, out: &mut [u8; SECTOR]) -> i32 {
        BLOCK_STORE.read_sector(&mut ScsiBlock, target, lba, out)
    }

    fn block_write(&self, target: u8, lba: u32, data: &[u8; SECTOR]) -> i32 {
        BLOCK_STORE.write_sector(&mut ScsiBlock, target, lba, data)
    }

    fn udp_open(&self, slot: u32, port: u16) -> i32 {
        crate::slots::udp_open(slot, port)
    }

    fn udp_send(&self, slot: u32, address: u32, port: u16, bytes: &[u8]) -> isize {
        crate::slots::udp_send(slot, address, port, bytes)
    }

    fn udp_recv(
        &self,
        slot: u32,
        out: &mut [u8],
        src_address: Option<&mut u32>,
        src_port: Option<&mut u16>,
    ) -> isize {
        crate::slots::udp_recv(slot, out, src_address, src_port)
    }

    fn udp_close(&self, slot: u32) {
        crate::slots::udp_close(slot);
    }

    fn udp_slots(&self) -> u32 {
        crate::slots::udp_slots()
    }

    fn udp_state(&self, slot: u32, address: Option<&mut u32>, port: Option<&mut u16>) -> i32 {
        crate::slots::udp_state(slot, address, port)
    }
}
