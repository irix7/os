// SPDX-License-Identifier: GPL-3.0-only
//! ROM boot entry point (minimal Rust stub).
//! All real work is in assembly:
//!   boot/reset.S   -- CP0 init, DRAM sizing, RAM test, serial, kernel handoff
//!   boot/memsize.S -- DRAM bank sizing and MEMCFG programming
//!   boot/memtest.S -- full RAM test
//!   netstage.rs    -- Seeq, DHCP and the HTTPS kernel download (M1-ADR-0002)

#![cfg_attr(not(test), no_main)]
#![no_std]
#![feature(asm_experimental_arch)]

extern crate alloc;

core::arch::global_asm!(include_str!("boot/reset.S"));
core::arch::global_asm!(include_str!("boot/memsize.S"));
core::arch::global_asm!(include_str!("boot/memtest.S"));

mod fetch;
mod netstage;

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    loop {
        core::hint::spin_loop();
    }
}
