// SPDX-License-Identifier: GPL-3.0-only
//! Trap and exception handling.
//!
//! MIPS III exception model:
//! - `Status.BEV` controls the exception vector base: 0 = kseg0 (0x8000_0000),
//!   1 = kseg1 (0xbfc0_0000, inside the PROM).
//! - Early trap handlers must account for `BEV` being set (PROM vectors).
//! - Exception causes are in `Cause.ExcCode`.
//! - CP0 Count/Compare is the only usable periodic timer (the 8254's
//!   interrupt output is broken on the Indy).
//!
//! Trap frame layout on the kernel stack:
//!   $at   $v0   $v1   $a0   $a1   $a2   $a3
//!   $t0..$t9  $s0..$s7  $t8   $t9
//!   $k0   $k1   $gp   $sp   $s8   $ra   $hi   $lo
//!   Cause   BadVAddr   Status   PC   CP0  ...

#![no_std]

use crate::print_hex8;

/// CP0 register selectors.
const C0_INDEX: u32 = 0;
const C0_RANDOM: u32 = 1;
const C0_ENTRYLO0: u32 = 2;
const C0_ENTRYLO1: u32 = 3;
const C0_CONTEXT: u32 = 4;
const C0_PAGEMASK: u32 = 5;
const C0_WIRED: u32 = 6;
const C0_BADVADDR: u32 = 8;
const C0_COUNT: u32 = 9;
const C0_ENTRYHI: u32 = 10;
const C0_COMPARE: u32 = 11;
const C0_STATUS: u32 = 12;
const C0_CAUSE: u32 = 13;
const C0_EPC: u32 = 14;
const C0_PRID: u32 = 15;
const C0_CONFIG: u32 = 16;
const C0_LLADDR: u32 = 17;
const C0_WATCHLO: u32 = 18;
const C0_WATCHHI: u32 = 19;
const C0_ECC: u32 = 26;
const C0_CACHEERR: u32 = 27;
const C0_TAGLO: u32 = 28;
const C0_TAGHI: u32 = 29;
const C0_ERROR_EPC: u32 = 30;

// Status register fields.
const STATUS_IE: u32 = 0x0000_0001; // Interrupt Enable
const STATUS_EXL: u32 = 0x0000_0002; // Exception Level
const STATUS_ERL: u32 = 0x0000_0004; // Error Level
const STATUS_KSU_MASK: u32 = 0x0000_0030; // KSU (mode) field
const STATUS_KSU_KERNEL: u32 = 0x0000_0000; // Kernel mode
const STATUS_KSU_SUPERVISOR: u32 = 0x0000_0010; // Supervisor mode
const STATUS_KSU_USER: u32 = 0x0000_0020; // User mode
const STATUS_BEV: u32 = 0x0000_0040; // Bootstrap Exception Vector
const STATUS_TS: u32 = 0x0000_0080; // TLB Shutdown
const STATUS_PE: u32 = 0x0000_0100; // Parity Enable
const STATUS_CM: u32 = 0x0000_0200; // Cache Parity Enable
const STATUS_CP0: u32 = 0x0000_1000; // Coprocessor 0 usable
const STATUS_CP1: u32 = 0x0000_2000; // Coprocessor 1 usable (FPU)
const STATUS_CP2: u32 = 0x0000_4000; // Coprocessor 2 usable
const STATUS_CP3: u32 = 0x0000_8000; // Coprocessor 3 usable
const STATUS_IM_MASK: u32 = 0xff00_0000; // Interrupt Mask (bits 8-15)

// Cause register fields.
const CAUSE_EXC_MASK: u32 = 0x0000_007c; // Exception Code (bits 2-6)
const CAUSE_IP_MASK: u32 = 0xff00_0000; // Interrupt Pending (bits 8-15)
const CAUSE_BD: u32 = 0x8000_0000; // Branch Delay slot

// Exception codes.
const EXC_INT: u32 = 0; // Interrupt
const EXC_MOD: u32 = 1; // TLB modification
const EXC_TLBL: u32 = 2; // TLB miss (load/instr fetch)
const EXC_TLBS: u32 = 3; // TLB miss (store)
const EXC_ADEL: u32 = 4; // Address error (load/instr fetch)
const EXC_ADES: u32 = 5; // Address error (store)
const EXC_IBE: u32 = 6; // Bus error (instruction fetch)
const EXC_DBE: u32 = 7; // Bus error (data load/store)
const EXC_SYS: u32 = 8; // Syscall
const EXC_BP: u32 = 9; // Breakpoint
const EXC_RI: u32 = 10; // Reserved instruction
const EXC_CPU: u32 = 11; // Coprocessor unusable
const EXC_OVF: u32 = 12; // Arithmetic overflow
const EXC_TRAP: u32 = 13; // Trap instruction
const EXC_FPE: u32 = 15; // Floating point
const EXC_WATCH: u32 = 23; // Reference to watchpoint address

/// Saved register context on the trap stack.
#[repr(C)]
#[derive(Debug)]
pub struct TrapFrame {
    pub regs: [u32; 32], // $0..$31
    pub lo: u32,
    pub hi: u32,
    pub epc: u32,
    pub badvaddr: u32,
    pub cause: u32,
    pub status: u32,
}

/// Read CP0 Status.
pub fn status_read() -> u32 {
    let v: u32;
    unsafe {
        core::arch::asm!("mfc0 {0}, $12", out(reg) v);
    }
    v
}

/// Write CP0 Status.
pub fn status_write(val: u32) {
    unsafe {
        core::arch::asm!("mtc0 {0}, $12", in(reg) val);
    }
}

/// Read CP0 Cause.
pub fn cause_read() -> u32 {
    let v: u32;
    unsafe {
        core::arch::asm!("mfc0 {0}, $13", out(reg) v);
    }
    v
}

/// Write CP0 Cause.
pub fn cause_write(val: u32) {
    unsafe {
        core::arch::asm!("mtc0 {0}, $13", in(reg) val);
    }
}

/// Read CP0 EPC.
pub fn epc_read() -> u32 {
    let v: u32;
    unsafe {
        core::arch::asm!("mfc0 {0}, $14", out(reg) v);
    }
    v
}

/// Write CP0 EPC.
pub fn epc_write(val: u32) {
    unsafe {
        core::arch::asm!("mtc0 {0}, $14", in(reg) val);
    }
}

/// Read CP0 Count.
pub fn count_read() -> u32 {
    let v: u32;
    unsafe {
        core::arch::asm!("mfc0 {0}, $9", out(reg) v);
    }
    v
}

/// Write CP0 Count.
pub fn count_write(val: u32) {
    unsafe {
        core::arch::asm!("mtc0 {0}, $9", in(reg) val);
    }
}

/// Read CP0 Compare.
pub fn compare_read() -> u32 {
    let v: u32;
    unsafe {
        core::arch::asm!("mfc0 {0}, $11", out(reg) v);
    }
    v
}

/// Write CP0 Compare.
pub fn compare_write(val: u32) {
    unsafe {
        core::arch::asm!("mtc0 {0}, $11", in(reg) val);
    }
}

/// Read CP0 EntryHi.
pub fn entryhi_read() -> u32 {
    let v: u32;
    unsafe {
        core::arch::asm!("mfc0 {0}, $10", out(reg) v);
    }
    v
}

/// Write CP0 EntryHi.
pub fn entryhi_write(val: u32) {
    unsafe {
        core::arch::asm!("mtc0 {0}, $10", in(reg) val);
    }
}

/// Read CP0 PageMask.
pub fn pagemask_read() -> u32 {
    let v: u32;
    unsafe {
        core::arch::asm!("mfc0 {0}, $5", out(reg) v);
    }
    v
}

/// Write CP0 PageMask.
pub fn pagemask_write(val: u32) {
    unsafe {
        core::arch::asm!("mtc0 {0}, $5", in(reg) val);
    }
}

/// Read CP0 Index.
pub fn index_read() -> u32 {
    let v: u32;
    unsafe {
        core::arch::asm!("mfc0 {0}, $0", out(reg) v);
    }
    v
}

/// Write CP0 Index.
pub fn index_write(val: u32) {
    unsafe {
        core::arch::asm!("mtc0 {0}, $0", in(reg) val);
    }
}

/// Read CP0 EntryLo0.
pub fn entrylo0_read() -> u32 {
    let v: u32;
    unsafe {
        core::arch::asm!("mfc0 {0}, $2", out(reg) v);
    }
    v
}

/// Write CP0 EntryLo0.
pub fn entrylo0_write(val: u32) {
    unsafe {
        core::arch::asm!("mtc0 {0}, $2", in(reg) val);
    }
}

/// Read CP0 EntryLo1.
pub fn entrylo1_read() -> u32 {
    let v: u32;
    unsafe {
        core::arch::asm!("mfc0 {0}, $3", out(reg) v);
    }
    v
}

/// Write CP0 EntryLo1.
pub fn entrylo1_write(val: u32) {
    unsafe {
        core::arch::asm!("mtc0 {0}, $3", in(reg) val);
    }
}

/// Read CP0 BadVAddr.
pub fn badvaddr_read() -> u32 {
    let v: u32;
    unsafe {
        core::arch::asm!("mfc0 {0}, $8", out(reg) v);
    }
    v
}

/// Trap the current exception into the trap frame.
///
/// Called from the assembly top-half in traps.S after saving registers.
#[no_mangle]
pub extern "C" fn trap_handler(tf: &mut TrapFrame) {
    let cause = tf.cause;
    let exc_code = (cause & CAUSE_EXC_MASK) >> 2;

    crate::println("trap:");
    crate::print_str("  cause=0x");
    print_hex8((cause >> 24) as u8);
    print_hex8((cause >> 16) as u8);
    print_hex8((cause >> 8) as u8);
    print_hex8(cause as u8);
    crate::println("");

    crate::print_str("  exc_code=");
    print_hex8(exc_code as u8);
    crate::println("");

    crate::print_str("  epc=0x");
    print_hex8((tf.epc >> 24) as u8);
    print_hex8((tf.epc >> 16) as u8);
    print_hex8((tf.epc >> 8) as u8);
    print_hex8(tf.epc as u8);
    crate::println("");

    crate::print_str("  badvaddr=0x");
    print_hex8((tf.badvaddr >> 24) as u8);
    print_hex8((tf.badvaddr >> 16) as u8);
    print_hex8((tf.badvaddr >> 8) as u8);
    print_hex8(tf.badvaddr as u8);
    crate::println("");

    match exc_code {
        EXC_INT => {
            crate::println("  exception: interrupt");
        }
        EXC_TLBL => {
            crate::println("  exception: TLB miss (load/inst fetch)");
        }
        EXC_TLBS => {
            crate::println("  exception: TLB miss (store)");
        }
        EXC_SYS => {
            crate::println("  exception: syscall");
        }
        EXC_RI => {
            crate::println("  exception: reserved instruction");
        }
        _ => {
            crate::println("  exception: unknown");
        }
    }

    // Halt on trap for now — the kernel will be extended to handle each case.
    loop {
        core::hint::spin_loop();
    }
}

/// Initialise traps and exception vectors.
///
/// The ROM already sets up CP0 Count/Compare, Status (IE=1, BEV=0), and the
/// exception vectors at kseg0 0x80000000..0x80000100. Nothing to do here.
pub fn init() {
    // ROM already configured CP0 and exception vectors; nothing to initialise.
}

/// Install exception vectors into kseg0.
pub fn install_vectors() {
    // The exception vector is at 0x8000_0000 (BEV = 0).
    // The assembly stub in traps.S is linked at this address.
}

/// Initialise the CP0 timer (Count/Compare).
pub fn init_timer() {
    // Set an initial compare value (will be calibrated later).
    let count = count_read();
    compare_write(count.wrapping_add(1_000_000)); // ~10ms at 100MHz

    // Enable timer interrupt (IP7 = bit 15 of Cause.IM).
    let mut status = status_read();
    status |= STATUS_IM_MASK & (1 << 15);
    status_write(status);
}

/// Calibrate the CP0 timer against the 8254 reference.
pub fn calibrate_timer() {
    // Program 8254 counter 2 in mode 0, then count CP0 ticks during a known
    // interval. The exact method is in the hardware report §14.2.
    // For now, assume 100 MHz.
    crate::println("timer: assuming 100 MHz (calibration pending)");
}
