// SPDX-License-Identifier: GPL-3.0-only
//! Deterministic exception probes.
//!
//! Each probe faults once and returns through the shared exception entry.  The
//! marker is printed after the faulting instruction, so a handler that does
//! not set a valid resume address leaves the boot stuck before the marker
//! rather than reporting a pass.

use core::arch::asm;

use crate::trap::{SYS_CALL_PROBE, SYS_CALL_RESULT};
use crate::{halt, print_hex, print_line, serial};

/// Outside every BAT: 512 MiB, above the largest RAM size the gate uses and
/// below the PCI hole.
const UNMAPPED: u32 = 0x2000_0000;

#[repr(align(16))]
struct Aligned(u32);

static ALIGN_PROBE: Aligned = Aligned(0);

pub fn run() {
    data_fault();
    instruction_fault();
    alignment();
    program();
    system_call();
}

fn data_fault() {
    print_line(b"ppc: deliberate unmapped load");
    // SAFETY: the access is the point of the test.  No BAT covers
    // 0x2000_0000, so the load raises a DSI and the handler steps over it.
    unsafe {
        core::ptr::read_volatile(UNMAPPED as *const u32);
    }
    print_line(b"FAULT-OK");
}

fn instruction_fault() {
    print_line(b"ppc: deliberate unmapped fetch");
    // SAFETY: the branch target is unmapped.  bctrl leaves the continuation
    // in LR, which is the only address the ISI handler can resume at: an
    // unmapped fetch address has no instruction to step over.
    unsafe {
        asm!(
            "mtctr 11",
            "bctrl",
            in("r11") UNMAPPED,
            out("ctr") _,
            out("lr") _,
            options(nostack),
        );
    }
    print_line(b"ISI-OK");
}

fn alignment() {
    print_line(b"ppc: deliberate misaligned load");
    let address = core::ptr::addr_of!(ALIGN_PROBE.0) as u32 + 1;
    let value: u32;
    // SAFETY: the address is one byte past an aligned static.  lwarx is the
    // integer load the 750 and QEMU both require to be word aligned; a plain
    // lwz is not checked by QEMU's 750 model, so it cannot prove the vector.
    unsafe {
        asm!(
            "lwarx 3, 0, 11",
            in("r11") address,
            out("r3") value,
            options(nostack),
        );
    }
    let _ = value;
    print_line(b"ALIGN-OK");
}

fn program() {
    print_line(b"ppc: deliberate illegal instruction");
    // SAFETY: 0x00000000 is not a valid PowerPC instruction; the program
    // exception handler steps over the four-byte word.
    unsafe {
        asm!(".long 0", options(nostack));
    }
    print_line(b"PROGRAM-OK");
}

fn system_call() {
    let result: u32;
    // SAFETY: the system-call handler reads r3 as the call number, returns the
    // next number in r3 and resumes at the instruction after sc.
    unsafe {
        asm!(
            "li 3, {call}",
            "sc",
            call = const SYS_CALL_PROBE,
            out("r3") result,
            options(nostack),
        );
    }
    if result == SYS_CALL_RESULT {
        print_line(b"SYSCALL-OK");
    } else {
        serial::write(b"ppc: syscall returned=");
        print_hex(result);
        serial::write_byte(b'\n');
        print_line(b"ppc: halted");
        halt();
    }
}
