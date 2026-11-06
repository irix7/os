// SPDX-License-Identifier: GPL-3.0-only
//! The hello demo on the PowerPC G3 machine (ADR M1-0003).
//!
//! The demo writes a line, reads the machine information and registers one
//! tick.  It runs in the kernel image through the shared service table.

use kernel_core::service::SERVICES;

/// What the demo did.
pub struct Report {
    /// The demo registered its tick.
    pub ticked: bool,
}

fn write(bytes: &[u8]) {
    // SAFETY: the kernel service table is installed before the demo runs, and
    // `write` copies `bytes` before it returns.
    unsafe { (SERVICES.write)(bytes.as_ptr(), bytes.len()) };
}

/// Run the demo once.
pub fn run() -> Report {
    write(b"ppc: hello run\n");

    // Exercise the information service.  A short buffer proves the kernel bounds
    // the copy to the caller's length.
    let mut info = [0u8; 64];
    // SAFETY: `info` is valid for `info.len()` bytes.
    let count = unsafe { (SERVICES.info)(0, info.as_mut_ptr(), info.len()) };
    if count > 0 {
        let count = count as usize;
        write(b"hello info: ");
        write(&info[..count]);
        if info[count - 1] != b'\n' {
            write(b"\n");
        }
    }

    tick();
    Report { ticked: true }
}

fn tick() {
    write(b"hello tick\n");
}
