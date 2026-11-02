// SPDX-License-Identifier: GPL-3.0-only
//! Kernel side of the shared `BootInfo` contract.
//!
//! The layout, big-endian decoding, CRC32C, bounded memory-map walk and the
//! validation order all live in `boot-protocol-tests`. This module supplies
//! only the kseg0 physical reader and the kernel's console-service accessor,
//! so the live boot path and the host conformance tests execute the same
//! parser.

pub use boot_protocol_tests::{BootInfo, BootInfoError, LoadedBootInfo, MEM_KIND_USABLE_RAM};

/// kseg0 virtual base for the low 512 MiB physical window.
pub const KSEG0_BASE: u64 = 0x8000_0000;
/// First physical address outside the kseg0 window.
pub const KSEG0_PHYS_LIMIT: u64 = 0x2000_0000;

/// A physical reader for the kseg0 window.
pub struct Kseg0Memory;

impl boot_protocol_tests::PhysMemory for Kseg0Memory {
    fn read_phys(&mut self, phys: u64, out: &mut [u8]) -> bool {
        let end = match phys.checked_add(out.len() as u64) {
            Some(end) => end,
            None => return false,
        };
        if phys >= KSEG0_PHYS_LIMIT || end > KSEG0_PHYS_LIMIT {
            return false;
        }
        let virtual_base = (phys + KSEG0_BASE) as usize;
        for (index, byte) in out.iter_mut().enumerate() {
            // SAFETY: the range was checked against the kseg0 window, which is
            // mapped for the whole lifetime of the kernel.
            *byte = unsafe { core::ptr::read_volatile((virtual_base + index) as *const u8) };
        }
        true
    }
}

/// Read and validate the loader's `BootInfo` record.
///
/// A zero physical address is `Absent`; the caller falls back to the register
/// handoff only for that error. Every other error is a malformed record and
/// the caller halts.
pub fn load(bootinfo_phys: u32) -> Result<LoadedBootInfo, BootInfoError> {
    let mut memory = Kseg0Memory;
    boot_protocol_tests::load(bootinfo_phys as u64, &mut memory)
}

/// The resident console-poll routine published by the loader, if any.
pub fn console_poll(info: &BootInfo) -> Option<extern "C" fn(*mut ())> {
    if info.console_poll == 0 {
        return None;
    }
    // SAFETY: the loader stores a kseg1 virtual address of a routine that is
    // resident for the lifetime of the machine.
    Some(unsafe {
        core::mem::transmute::<usize, extern "C" fn(*mut ())>(info.console_poll as usize)
    })
}
