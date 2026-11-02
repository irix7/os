// SPDX-License-Identifier: GPL-3.0-only
//! Exception vectors and the deliberate-fault report.
//!
//! The two vectors the kernel owns are the data storage interrupt at 0x300
//! and the decrementer at 0x900.  Each vector holds a branch to a handler in
//! `boot.S`; the handlers save what they need and call back into Rust.

use core::arch::asm;
use core::ptr::write_volatile;

use crate::{halt, print_hex, print_line, serial};

pub const DSI_VECTOR: u32 = 0x300;
pub const DECREMENTER_VECTOR: u32 = 0x900;

extern "C" {
    fn dsi_handler();
    fn decrementer_handler();
}

/// Install both vectors while the firmware mapping is still active.
pub fn install() {
    write_vector(DSI_VECTOR, dsi_handler as *const () as u32);
    write_vector(DECREMENTER_VECTOR, decrementer_handler as *const () as u32);
}

fn write_vector(vector: u32, handler: u32) {
    let offset = handler.wrapping_sub(vector);
    assert!(
        offset < 0x0200_0000,
        "exception handler out of branch range"
    );
    // `b` (opcode 18) with the word offset in LI.
    let branch = 0x4800_0000 | (offset & 0x03ff_fffc);
    // SAFETY: the vector page is mapped by the firmware until the switch and
    // by the kernel's own IBAT1 afterwards.  The cache maintenance makes the
    // branch visible to instruction fetch.
    unsafe {
        write_volatile(vector as *mut u32, branch);
        asm!(
            "dcbst 0, {vector}",
            "sync",
            "icbi 0, {vector}",
            "sync",
            "isync",
            vector = in(reg) vector,
            options(nostack),
        );
    }
}

/// Report a data storage interrupt that was not supposed to be handled.
///
/// `boot.S` passes DSISR, DAR and SRR0 in r3, r4 and r5.
#[no_mangle]
pub extern "C" fn dsi_report(dsisr: u32, dar: u32, epc: u32) -> ! {
    serial::write(b"ppc: dsi dsisr=");
    print_hex(dsisr);
    serial::write(b" dar=");
    print_hex(dar);
    serial::write(b" epc=");
    print_hex(epc);
    serial::write_byte(b'\n');
    print_line(b"FAULT-OK");
    print_line(b"ppc: halted");
    halt()
}
