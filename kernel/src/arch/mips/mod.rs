// SPDX-License-Identifier: GPL-3.0-only
//! MIPS implementation of the [`Arch`](crate::arch::interface::Arch) boundary.
//!
//! This is a thin delegation layer over the existing MIPS bring-up code
//! (`traps`, `tlb`, `idle`, `cache`, `console`, `platform`).  The MIPS machine
//! keeps its CP0/TLB/exception-vector model unchanged; the arch trait is the
//! only thing the shared core sees.

use crate::arch::interface::{Arch, ArchCaps, ArchError, TrapFrame};
use crate::frames::MemDesc;
use platform_api::PlatformShim;

/// MIPS arch state.  A unit struct: all mutable state lives in the existing
/// `static mut` cells in `traps`, `tlb`, `idle`, `cache`, `platform`.
#[derive(Clone, Copy, Debug, Default)]
pub struct MipsArch;

impl MipsArch {
    pub const fn new() -> Self {
        MipsArch
    }
}

impl Arch for MipsArch {
    fn caps(&self) -> ArchCaps {
        ArchCaps(ArchCaps::MMU.0 | ArchCaps::BIG_ENDIAN.0 | ArchCaps::RTC.0)
    }

    fn init(&mut self) -> Result<(), ArchError> {
        // The ROM already programmed CP0, the serial console and DRAM.  The
        // Indy shim validates NVRAM and seeds entropy.
        crate::platform::init();
        Ok(())
    }

    fn install_vectors(&mut self) {
        crate::traps::install_vectors();
    }

    fn init_timer(&mut self) {
        crate::idle::init();
    }

    fn ticks(&self) -> u32 {
        crate::idle::ticks()
    }

    fn wait(&mut self) {
        crate::idle::wait();
    }

    fn console_write(&mut self, bytes: &[u8]) {
        crate::write_bytes(bytes);
    }

    fn console_read(&mut self, out: &mut [u8]) -> usize {
        // The Indy shim reads from the keyboard polled through the platform
        // shim; expose it here via the existing console read path.
        let mut shim = crate::platform::IndyShim::new();
        shim.console_read(out)
    }

    fn trap(&mut self, _tf: &mut TrapFrame) -> bool {
        // The MIPS general exception vector saves the full CP0 frame into the
        // static TRAP_FRAME and dispatches to traps::trap_handler directly.
        // The arch-level TrapFrame is a thin shared view; MIPS keeps its own
        // frame layout, so a full arch-level dispatch is not used on MIPS.
        false
    }

    fn fatal_trap(&mut self, tf: &TrapFrame) -> ! {
        crate::println("trap:");
        crate::print_str("  cause=0x");
        crate::print_hex8(((tf.cause as u32) >> 24) as u8);
        crate::print_hex8(((tf.cause as u32) >> 16) as u8);
        crate::print_hex8(((tf.cause as u32) >> 8) as u8);
        crate::print_hex8(tf.cause as u8);
        crate::print_str(" epc=0x");
        crate::print_hex8(((tf.epc as u32) >> 24) as u8);
        crate::print_hex8(((tf.epc as u32) >> 16) as u8);
        crate::print_hex8(((tf.epc as u32) >> 8) as u8);
        crate::print_hex8(tf.epc as u8);
        crate::print_str(" badvaddr=0x");
        crate::print_hex8(((tf.badvaddr as u32) >> 24) as u8);
        crate::print_hex8(((tf.badvaddr as u32) >> 16) as u8);
        crate::print_hex8(((tf.badvaddr as u32) >> 8) as u8);
        crate::print_hex8(tf.badvaddr as u8);
        crate::println("");
        loop {
            core::hint::spin_loop();
        }
    }

    fn reboot(&mut self) -> ! {
        crate::reboot()
    }

    fn poweroff(&mut self) -> ! {
        crate::poweroff()
    }

    fn memory_map(&mut self, _descs: &mut [MemDesc]) -> usize {
        // The MIPS boot path reads BootInfo in main.rs and passes descriptors
        // to the frame allocator directly.  No arch-level map needed here.
        0
    }

    fn dcache_writeback_invalidate(&mut self, addr: usize, len: usize) {
        crate::cache::dcache_writeback_invalidate(addr, len);
    }

    fn icache_invalidate(&mut self, addr: usize, len: usize) {
        crate::cache::icache_invalidate(addr, len);
    }

    fn sync(&mut self) {
        crate::cache::sync();
    }

    fn direct_map_offset(&self) -> u64 {
        0x8000_0000
    }

    fn rtc_read_unix(&mut self) -> Option<u64> {
        crate::rtc::read_unix()
    }
}
