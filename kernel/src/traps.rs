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
//! The general vector saves a full frame in `TRAP_FRAME`:
//!   `regs[0..32]` (`$0` is always zero), `lo`, `hi`, `epc`, `badvaddr`,
//!   `cause`, `status`.  The kernel is single-threaded while `EXL` is set, so
//!   one static frame is enough; nested exceptions are a bug.

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

/// The single kernel-mode trap frame used by the exception vector.
///
/// The kernel is single-threaded while an exception is being handled and
/// interrupts are disabled by `EXL`; a nested exception is a bug.
#[no_mangle]
static mut TRAP_FRAME: TrapFrame = TrapFrame {
    regs: [0; 32],
    lo: 0,
    hi: 0,
    epc: 0,
    badvaddr: 0,
    cause: 0,
    status: 0,
};

/// Number of self-test break exceptions handled.
static mut BREAKS: u32 = 0;
/// Number of self-test syscall exceptions handled.
static mut SYSCALLS: u32 = 0;

core::arch::global_asm!(
    r#"
    .set noreorder
    .set mips3
    .set noat

    /* General exception vector, copied to 0x8000_0180.  Timer interrupts
       stay on the fast path; every other cause enters the full frame save. */
    .section .text.trap_vector, "ax", @progbits
    .align 4
    .globl trap_vector
trap_vector:
    mfc0    $k0, $13
    andi    $k0, $k0, 0x7c
    bnez    $k0, 9f
    nop

    /* Timer: Compare = Count + 330000 (10 ms at the current iris rate). */
    mfc0    $k0, $9
    lui     $k1, 0x0005
    ori     $k1, $k1, 0x0910
    addu    $k0, $k0, $k1
    mtc0    $k0, $11

    lui     $k0, %hi(TICKS)
    addiu   $k0, $k0, %lo(TICKS)
    lw      $k1, 0($k0)
    addiu   $k1, $k1, 1
    sw      $k1, 0($k0)
    eret
    nop

9:  j       trap_entry
    nop
    .globl trap_vector_end
trap_vector_end:

    /* TLB refill vector, copied to 0x8000_0000.  It reports and parks until
       the software refill path lands. */
    .section .text.tlb_refill_vector, "ax", @progbits
    .align 4
    .globl tlb_refill_vector
tlb_refill_vector:
    j       trap_tlb_refill
    nop
    .globl tlb_refill_vector_end
tlb_refill_vector_end:

    /* Full register frame.  $k0/$k1 are scratch and are not restored. */
    .section .text.trap_entry, "ax", @progbits
    .align 4
    .globl trap_entry
trap_entry:
    lui     $k0, %hi(TRAP_FRAME)
    addiu   $k0, $k0, %lo(TRAP_FRAME)
    sw      $1, 4($k0)
    sw      $2, 8($k0)
    sw      $3, 12($k0)
    sw      $4, 16($k0)
    sw      $5, 20($k0)
    sw      $6, 24($k0)
    sw      $7, 28($k0)
    sw      $8, 32($k0)
    sw      $9, 36($k0)
    sw      $10, 40($k0)
    sw      $11, 44($k0)
    sw      $12, 48($k0)
    sw      $13, 52($k0)
    sw      $14, 56($k0)
    sw      $15, 60($k0)
    sw      $16, 64($k0)
    sw      $17, 68($k0)
    sw      $18, 72($k0)
    sw      $19, 76($k0)
    sw      $20, 80($k0)
    sw      $21, 84($k0)
    sw      $22, 88($k0)
    sw      $23, 92($k0)
    sw      $24, 96($k0)
    sw      $25, 100($k0)
    sw      $28, 112($k0)
    sw      $29, 116($k0)
    sw      $30, 120($k0)
    sw      $31, 124($k0)

    mfhi    $t0
    sw      $t0, 128($k0)
    mflo    $t0
    sw      $t0, 132($k0)
    mfc0    $t0, $14
    sw      $t0, 136($k0)
    mfc0    $t0, $8
    sw      $t0, 140($k0)
    mfc0    $t0, $13
    sw      $t0, 144($k0)
    mfc0    $t0, $12
    sw      $t0, 148($k0)

    move    $a0, $k0
    jal     trap_handler
    nop

    lui     $k0, %hi(TRAP_FRAME)
    addiu   $k0, $k0, %lo(TRAP_FRAME)
    lw      $1, 4($k0)
    lw      $2, 8($k0)
    lw      $3, 12($k0)
    lw      $4, 16($k0)
    lw      $5, 20($k0)
    lw      $6, 24($k0)
    lw      $7, 28($k0)
    lw      $8, 32($k0)
    lw      $9, 36($k0)
    lw      $10, 40($k0)
    lw      $11, 44($k0)
    lw      $12, 48($k0)
    lw      $13, 52($k0)
    lw      $14, 56($k0)
    lw      $15, 60($k0)
    lw      $16, 64($k0)
    lw      $17, 68($k0)
    lw      $18, 72($k0)
    lw      $19, 76($k0)
    lw      $20, 80($k0)
    lw      $21, 84($k0)
    lw      $22, 88($k0)
    lw      $23, 92($k0)
    lw      $24, 96($k0)
    lw      $25, 100($k0)
    lw      $28, 112($k0)
    lw      $29, 116($k0)
    lw      $30, 120($k0)
    lw      $31, 124($k0)

    lw      $t0, 128($k0)
    mthi    $t0
    lw      $t0, 132($k0)
    mtlo    $t0
    lw      $t0, 136($k0)
    mtc0    $t0, $14
    eret
    nop
"#
);

/// Read CP0 Status.
pub fn status_read() -> u32 {
    let v: u32;
    unsafe {
        core::arch::asm!(".set noat", "mfc0 {0}, $12", out(reg) v);
    }
    v
}

/// Write CP0 Status.
pub fn status_write(val: u32) {
    unsafe {
        core::arch::asm!(".set noat", "mtc0 {0}, $12", in(reg) val);
    }
}

/// Read CP0 Cause.
pub fn cause_read() -> u32 {
    let v: u32;
    unsafe {
        core::arch::asm!(".set noat", "mfc0 {0}, $13", out(reg) v);
    }
    v
}

/// Write CP0 Cause.
pub fn cause_write(val: u32) {
    unsafe {
        core::arch::asm!(".set noat", "mtc0 {0}, $13", in(reg) val);
    }
}

/// Read CP0 EPC.
pub fn epc_read() -> u32 {
    let v: u32;
    unsafe {
        core::arch::asm!(".set noat", "mfc0 {0}, $14", out(reg) v);
    }
    v
}

/// Write CP0 EPC.
pub fn epc_write(val: u32) {
    unsafe {
        core::arch::asm!(".set noat", "mtc0 {0}, $14", in(reg) val);
    }
}

/// Read CP0 Count.
pub fn count_read() -> u32 {
    let v: u32;
    unsafe {
        core::arch::asm!(".set noat", "mfc0 {0}, $9", out(reg) v);
    }
    v
}

/// Write CP0 Count.
pub fn count_write(val: u32) {
    unsafe {
        core::arch::asm!(".set noat", "mtc0 {0}, $9", in(reg) val);
    }
}

/// Read CP0 Compare.
pub fn compare_read() -> u32 {
    let v: u32;
    unsafe {
        core::arch::asm!(".set noat", "mfc0 {0}, $11", out(reg) v);
    }
    v
}

/// Write CP0 Compare.
pub fn compare_write(val: u32) {
    unsafe {
        core::arch::asm!(".set noat", "mtc0 {0}, $11", in(reg) val);
    }
}

/// Read CP0 PRId (processor identifier).
pub fn prid_read() -> u32 {
    let v: u32;
    unsafe {
        core::arch::asm!(".set noat", "mfc0 {0}, $15", out(reg) v);
    }
    v
}

/// Read CP0 Config (cache and endianness configuration).
pub fn config_read() -> u32 {
    let v: u32;
    unsafe {
        core::arch::asm!(".set noat", "mfc0 {0}, $16", out(reg) v);
    }
    v
}

/// Read CP0 EntryHi.
pub fn entryhi_read() -> u32 {
    let v: u32;
    unsafe {
        core::arch::asm!(".set noat", "mfc0 {0}, $10", out(reg) v);
    }
    v
}

/// Write CP0 EntryHi.
pub fn entryhi_write(val: u32) {
    unsafe {
        core::arch::asm!(".set noat", "mtc0 {0}, $10", in(reg) val);
    }
}

/// Read CP0 PageMask.
pub fn pagemask_read() -> u32 {
    let v: u32;
    unsafe {
        core::arch::asm!(".set noat", "mfc0 {0}, $5", out(reg) v);
    }
    v
}

/// Write CP0 PageMask.
pub fn pagemask_write(val: u32) {
    unsafe {
        core::arch::asm!(".set noat", "mtc0 {0}, $5", in(reg) val);
    }
}

/// Read CP0 Index.
pub fn index_read() -> u32 {
    let v: u32;
    unsafe {
        core::arch::asm!(".set noat", "mfc0 {0}, $0", out(reg) v);
    }
    v
}

/// Write CP0 Index.
pub fn index_write(val: u32) {
    unsafe {
        core::arch::asm!(".set noat", "mtc0 {0}, $0", in(reg) val);
    }
}

/// Read CP0 EntryLo0.
pub fn entrylo0_read() -> u32 {
    let v: u32;
    unsafe {
        core::arch::asm!(".set noat", "mfc0 {0}, $2", out(reg) v);
    }
    v
}

/// Write CP0 EntryLo0.
pub fn entrylo0_write(val: u32) {
    unsafe {
        core::arch::asm!(".set noat", "mtc0 {0}, $2", in(reg) val);
    }
}

/// Read CP0 EntryLo1.
pub fn entrylo1_read() -> u32 {
    let v: u32;
    unsafe {
        core::arch::asm!(".set noat", "mfc0 {0}, $3", out(reg) v);
    }
    v
}

/// Write CP0 EntryLo1.
pub fn entrylo1_write(val: u32) {
    unsafe {
        core::arch::asm!(".set noat", "mtc0 {0}, $3", in(reg) val);
    }
}

/// Read CP0 BadVAddr.
pub fn badvaddr_read() -> u32 {
    let v: u32;
    unsafe {
        core::arch::asm!(".set noat", "mfc0 {0}, $8", out(reg) v);
    }
    v
}

/// Trap the current exception into the trap frame.
///
/// The assembly vector has already saved every general register and the CP0
/// state.  Break and syscall are currently used by the kernel self-test; every
/// other non-interrupt exception is fatal for now.
#[no_mangle]
pub extern "C" fn trap_handler(tf: &mut TrapFrame) {
    let cause = tf.cause;
    let exc_code = (cause & CAUSE_EXC_MASK) >> 2;

    match exc_code {
        EXC_BP => {
            // Self-test break: skip the break instruction and return.
            unsafe {
                BREAKS = BREAKS.wrapping_add(1);
            }
            tf.epc = tf.epc.wrapping_add(4);
        }
        EXC_SYS => {
            // Self-test syscall: skip the syscall instruction and return.
            unsafe {
                SYSCALLS = SYSCALLS.wrapping_add(1);
            }
            tf.epc = tf.epc.wrapping_add(4);
        }
        _ => {
            crate::println("trap:");
            crate::print_str("  cause=0x");
            print_hex8((cause >> 24) as u8);
            print_hex8((cause >> 16) as u8);
            print_hex8((cause >> 8) as u8);
            print_hex8(cause as u8);
            crate::print_str(" exc=");
            print_hex8(exc_code as u8);
            crate::print_str(" epc=0x");
            print_hex8((tf.epc >> 24) as u8);
            print_hex8((tf.epc >> 16) as u8);
            print_hex8((tf.epc >> 8) as u8);
            print_hex8(tf.epc as u8);
            crate::print_str(" badvaddr=0x");
            print_hex8((tf.badvaddr >> 24) as u8);
            print_hex8((tf.badvaddr >> 16) as u8);
            print_hex8((tf.badvaddr >> 8) as u8);
            print_hex8(tf.badvaddr as u8);
            crate::println("");
            loop {
                core::hint::spin_loop();
            }
        }
    }
}

/// TLB refill vector target.  The software refill path is not implemented yet.
#[no_mangle]
pub extern "C" fn trap_tlb_refill() -> ! {
    let badvaddr = badvaddr_read();
    crate::print_str("trap: TLB refill at badvaddr=0x");
    print_hex8((badvaddr >> 24) as u8);
    print_hex8((badvaddr >> 16) as u8);
    print_hex8((badvaddr >> 8) as u8);
    print_hex8(badvaddr as u8);
    crate::println(" (software refill not implemented)");
    loop {
        core::hint::spin_loop();
    }
}

/// Exercise the general exception vector, frame save/restore and `eret`.
///
/// This deliberately executes a break and a syscall in kernel mode.  It does
/// not test user mode or a TLB refill.
pub fn self_test() -> bool {
    let before_breaks = unsafe { BREAKS };
    let before_syscalls = unsafe { SYSCALLS };

    // SAFETY: the exception vector is installed before this is callable.
    unsafe {
        core::arch::asm!("break 7");
        core::arch::asm!("syscall");
    }

    let after_breaks = unsafe { BREAKS };
    let after_syscalls = unsafe { SYSCALLS };
    crate::print_str("trap: self-test break=");
    crate::print_hex32(after_breaks.wrapping_sub(before_breaks));
    crate::print_str(" syscall=");
    crate::print_hex32(after_syscalls.wrapping_sub(before_syscalls));
    crate::println("");

    after_breaks == before_breaks.wrapping_add(1)
        && after_syscalls == before_syscalls.wrapping_add(1)
}

/// Initialise traps and exception vectors.
///
/// The ROM already sets up CP0 Count/Compare, Status (IE=1, BEV=0), and the
/// exception vectors at kseg0 0x80000000..0x80000100. Nothing to do here.
pub fn init() {
    // ROM already configured CP0 and exception vectors; nothing to initialise.
}

/// Install the general and TLB-refill vectors into kseg0.
pub fn install_vectors() {
    extern "C" {
        static trap_vector: u8;
        static trap_vector_end: u8;
        static tlb_refill_vector: u8;
        static tlb_refill_vector_end: u8;
    }

    const GENERAL_VECTOR: *mut u8 = 0x8000_0180 as *mut u8;
    const TLB_VECTOR: *mut u8 = 0x8000_0000 as *mut u8;

    let (general_src, general_len) = unsafe {
        let src = &trap_vector as *const u8;
        let len = (&trap_vector_end as *const u8 as usize) - (src as usize);
        (src, len)
    };
    let (tlb_src, tlb_len) = unsafe {
        let src = &tlb_refill_vector as *const u8;
        let len = (&tlb_refill_vector_end as *const u8 as usize) - (src as usize);
        (src, len)
    };

    for i in 0..general_len {
        // SAFETY: both ranges are in the kernel's direct-mapped window.
        unsafe {
            core::ptr::write_volatile(
                GENERAL_VECTOR.add(i),
                core::ptr::read_volatile(general_src.add(i)),
            );
        }
    }
    for i in 0..tlb_len {
        // SAFETY: both ranges are in the kernel's direct-mapped window.
        unsafe {
            core::ptr::write_volatile(TLB_VECTOR.add(i), core::ptr::read_volatile(tlb_src.add(i)));
        }
    }

    crate::cache::dcache_writeback_invalidate(GENERAL_VECTOR as usize, general_len);
    crate::cache::icache_invalidate(GENERAL_VECTOR as usize, general_len);
    crate::cache::dcache_writeback_invalidate(TLB_VECTOR as usize, tlb_len);
    crate::cache::icache_invalidate(TLB_VECTOR as usize, tlb_len);
    crate::cache::sync();

    // The vectors above are the BEV=0 table.
    let status = status_read();
    status_write(status & !STATUS_BEV);
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
