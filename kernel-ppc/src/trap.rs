// SPDX-License-Identifier: GPL-3.0-only
//! Exception vectors and the common fault report.
//!
//! The kernel owns six vectors: the data and instruction storage interrupts,
//! alignment, program, the decrementer and the system call.  Each vector holds
//! a branch to a stub in `boot.S`; the stubs share one entry that saves the
//! interrupted context and calls [`trap_dispatch`].
//!
//! A faulting instruction is stepped over by advancing SRR0 by four bytes and
//! returning through `rfi`.  An instruction fetch has no instruction at SRR0
//! to step over, so the probe enters the unmapped range with `bctrl` and the
//! handler resumes at the interrupted LR.  The system call is already past the
//! `sc` instruction when the vector is taken, so its SRR0 is used unchanged.

use core::arch::asm;
use core::ptr::addr_of;

use crate::{console, halt, print_hex, print_line};

pub const DSI_VECTOR: u32 = 0x300;
pub const ISI_VECTOR: u32 = 0x400;
pub const EXTERNAL_VECTOR: u32 = 0x500;
pub const ALIGNMENT_VECTOR: u32 = 0x600;
pub const PROGRAM_VECTOR: u32 = 0x700;
pub const DECREMENTER_VECTOR: u32 = 0x900;
pub const SYSTEM_CALL_VECTOR: u32 = 0xc00;

/// The call number the boot probe places in r3.
pub const SYS_CALL_PROBE: u32 = 0x2a;
/// The value the system-call handler returns in r3.
pub const SYS_CALL_RESULT: u32 = SYS_CALL_PROBE + 1;

extern "C" {
    fn dsi_handler();
    fn isi_handler();
    fn external_handler();
    fn alignment_handler();
    fn program_handler();
    fn system_call_handler();
    fn decrementer_handler();
    static __kernel_start: u8;
    static __kernel_end: u8;
}

/// Install every vector while the firmware mapping is still active.
pub fn install() {
    write_vector(DSI_VECTOR, dsi_handler as *const () as u32);
    write_vector(ISI_VECTOR, isi_handler as *const () as u32);
    write_vector(EXTERNAL_VECTOR, external_handler as *const () as u32);
    write_vector(ALIGNMENT_VECTOR, alignment_handler as *const () as u32);
    write_vector(PROGRAM_VECTOR, program_handler as *const () as u32);
    write_vector(DECREMENTER_VECTOR, decrementer_handler as *const () as u32);
    write_vector(SYSTEM_CALL_VECTOR, system_call_handler as *const () as u32);
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
        core::ptr::write_volatile(vector as *mut u32, branch);
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

/// Report the exception, set the resume address and return the value for r3.
///
/// `boot.S` passes the vector number, the interrupted r3 and the interrupted
/// LR.  Every other field comes from the exception registers, which the
/// handler reads before anything can overwrite them.
#[no_mangle]
pub extern "C" fn trap_dispatch(vector: u32, call: u32, link: u32) -> u32 {
    let srr0 = read_srr0();
    let srr1 = read_srr1();

    // An external interrupt is taken at an instruction boundary: SRR0 is the
    // instruction to resume at, so it is written back unchanged.  The handler
    // acknowledges the controller before returning.
    if vector == EXTERNAL_VECTOR {
        write_srr0(srr0);
        return crate::irq::dispatch(srr0, srr1, call);
    }

    report(vector, srr0, srr1);

    let resume = match vector {
        // The system call exception saves the instruction after `sc`.
        SYSTEM_CALL_VECTOR => srr0,
        // There is no instruction at an unmapped fetch address to step over.
        // The probe enters the range with bctrl, so LR holds the only valid
        // continuation; refuse to return to a stray address.
        ISI_VECTOR => {
            let start = addr_of!(__kernel_start) as u32;
            let end = addr_of!(__kernel_end) as u32;
            if link < start || link >= end || link & 3 != 0 {
                print_line(b"ppc: isi without a resume address");
                print_line(b"ppc: halted");
                halt()
            }
            link
        }
        _ => srr0.wrapping_add(4),
    };
    write_srr0(resume);

    if vector == SYSTEM_CALL_VECTOR {
        let result = call.wrapping_add(1);
        console::write(b"ppc: syscall call=");
        print_hex(call);
        console::write(b" result=");
        print_hex(result);
        console::write_byte(b'\n');
        result
    } else {
        call
    }
}

fn report(vector: u32, srr0: u32, srr1: u32) {
    console::write(b"ppc: trap vector=");
    print_hex(vector);
    console::write(b" srr0=");
    print_hex(srr0);
    console::write(b" srr1=");
    print_hex(srr1);
    if vector == DSI_VECTOR || vector == ALIGNMENT_VECTOR {
        console::write(b" dsisr=");
        print_hex(read_dsisr());
        console::write(b" dar=");
        print_hex(read_dar());
    }
    console::write_byte(b'\n');
}

fn read_srr0() -> u32 {
    let value: u32;
    // SAFETY: SRR0 is privileged and holds the faulting instruction address.
    unsafe {
        asm!("mfspr {value}, 26", value = out(reg) value, options(nomem, nostack));
    }
    value
}

fn read_srr1() -> u32 {
    let value: u32;
    // SAFETY: SRR1 is privileged and holds the saved MSR and cause bits.
    unsafe {
        asm!("mfspr {value}, 27", value = out(reg) value, options(nomem, nostack));
    }
    value
}

fn read_dsisr() -> u32 {
    let value: u32;
    // SAFETY: DSISR is privileged and describes a data or alignment fault.
    unsafe {
        asm!("mfspr {value}, 18", value = out(reg) value, options(nomem, nostack));
    }
    value
}

fn read_dar() -> u32 {
    let value: u32;
    // SAFETY: DAR is privileged and holds the faulting data address.
    unsafe {
        asm!("mfspr {value}, 19", value = out(reg) value, options(nomem, nostack));
    }
    value
}

fn write_srr0(value: u32) {
    // SAFETY: SRR0 is privileged; rfi returns through the address written here.
    unsafe {
        asm!("mtspr 26, {value}", value = in(reg) value, options(nostack));
    }
}
