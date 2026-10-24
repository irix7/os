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

/// Separate kernel stack for the exception handler.  A user exception may
/// arrive with an untrusted `$sp`; the handler must not run on it.
#[repr(align(16))]
struct TrapStack([u8; 16 * 1024]);

#[no_mangle]
static mut TRAP_STACK: TrapStack = TrapStack([0; 16 * 1024]);

/// Kernel `$sp`/`$ra` to restore when a user exit returns to the console.
#[no_mangle]
static mut USER_RETURN_SP: u32 = 0;
#[no_mangle]
static mut USER_RETURN_RA: u32 = 0;
/// Kernel callee-saved registers ($16-$23, $30) at the console entry.
#[no_mangle]
static mut USER_RETURN_REGS: [u32; 9] = [0; 9];

extern "C" {
    pub fn user_from_console();
    pub fn user_return_trampoline();
}

core::arch::global_asm!(
    r#"
    .set noreorder
    .set mips3
    .set noat

    /* Called from the telnet command.  Save the console caller's SP/RA, then
       enter the one-way user image.  A user exit erets to the trampoline,
       which returns here. */
    .section .text.user_from_console, "ax", @progbits
    .align 4
    .globl user_from_console
user_from_console:
    addiu   $sp, $sp, -16
    sw      $ra, 12($sp)
    addiu   $t0, $sp, 16
    lui     $t1, %hi(USER_RETURN_SP)
    addiu   $t1, $t1, %lo(USER_RETURN_SP)
    sw      $t0, 0($t1)
    lui     $t1, %hi(USER_RETURN_RA)
    addiu   $t1, $t1, %lo(USER_RETURN_RA)
    sw      $ra, 0($t1)
    lui     $t1, %hi(USER_RETURN_REGS)
    addiu   $t1, $t1, %lo(USER_RETURN_REGS)
    sw      $16, 0($t1)
    sw      $17, 4($t1)
    sw      $18, 8($t1)
    sw      $19, 12($t1)
    sw      $20, 16($t1)
    sw      $21, 20($t1)
    sw      $22, 24($t1)
    sw      $23, 28($t1)
    sw      $30, 32($t1)
    jal     user_smoke
    nop
9:  b       9b
    nop

    .section .text.user_return_trampoline, "ax", @progbits
    .align 4
    .globl user_return_trampoline
user_return_trampoline:
    jr      $ra
    nop
"#
);

/// Number of self-test break exceptions handled.
static mut BREAKS: u32 = 0;
/// Number of self-test syscall exceptions handled.
static mut SYSCALLS: u32 = 0;

/// One-entry software TLB refill mapping used by the kernel self-test.
#[no_mangle]
static mut TLB_TEST_VPN: u32 = 0;
#[no_mangle]
static mut TLB_TEST_ENTRYLO: u32 = 0;

/// Kernel `$gp` saved before entering user mode.
static mut USER_KERNEL_GP: u32 = 0;

#[repr(C, align(4096))]
struct TlbTestPage([u8; 4096]);

#[no_mangle]
static mut TLB_TEST_PAGE: TlbTestPage = TlbTestPage([0; 4096]);

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

    /* TLB refill vector, copied to 0x8000_0000.  This is a one-entry
       software refill used by the kernel self-test; unexpected faults fall
       through to the Rust reporter.  It must fit before 0x8000_0080. */
    .section .text.tlb_refill_vector, "ax", @progbits
    .align 4
    .globl tlb_refill_vector
tlb_refill_vector:
    mfc0    $k0, $8
    lui     $k1, 0xffff
    ori     $k1, $k1, 0xe000
    and     $k0, $k0, $k1
    lui     $k1, %hi(TLB_TEST_VPN)
    lw      $k1, %lo(TLB_TEST_VPN)($k1)
    bne     $k0, $k1, 9f
    nop
    mfc0    $k0, $8
    lui     $k1, 0xffff
    ori     $k1, $k1, 0xe000
    and     $k0, $k0, $k1
    mtc0    $k0, $10
    lui     $k1, %hi(TLB_TEST_ENTRYLO)
    lw      $k1, %lo(TLB_TEST_ENTRYLO)($k1)
    beqz    $k1, 9f
    nop
    mtc0    $k1, $2
    mtc0    $k1, $3
    tlbwr
    eret
    nop
9:  j       trap_tlb_refill
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

    /* Switch to the dedicated trap stack; the saved user/kernel $sp is in
       the frame and is restored below. */
    lui     $sp, %hi(TRAP_STACK)
    addiu   $sp, $sp, %lo(TRAP_STACK)
    addiu   $sp, $sp, 16384

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
    /* Return status: keep EXL set until eret clears it.  The handler may
       switch KSU for a user exit. */
    lw      $t0, 148($k0)
    ori     $t0, $t0, 0x2
    mtc0    $t0, $12
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

    // User-mode syscall smoke test.  Code 1 is a console write and returns to
    // user mode; code 2 exits to a kernel continuation.  This is deliberately
    // a fixed ABI for the smoke test, not the native syscall ABI.
    if exc_code == EXC_SYS && tf.status & STATUS_KSU_MASK == STATUS_KSU_USER {
        match tf.regs[2] {
            1 => {
                crate::println("user: write syscall (returning to user)");
                tf.epc = tf.epc.wrapping_add(4);
            }
            2 => {
                let code = tf.regs[4];
                crate::print_str("user: exit syscall code=0x");
                crate::print_hex32(code);
                crate::println("");
                tf.epc = user_return_trampoline as usize as u32;
                tf.regs[4] = code;
                let saved =
                    unsafe { core::ptr::read_volatile(core::ptr::addr_of!(USER_RETURN_REGS)) };
                for (index, value) in saved[..8].iter().enumerate() {
                    tf.regs[16 + index] = *value;
                }
                tf.regs[30] = saved[8];
                tf.regs[28] = unsafe { USER_KERNEL_GP };
                tf.regs[29] = unsafe { USER_RETURN_SP };
                tf.regs[31] = unsafe { USER_RETURN_RA };
                tf.status &= !STATUS_KSU_MASK; // return to kernel mode
            }
            _ => fatal_trap(tf, exc_code),
        }
        return;
    }

    match exc_code {
        EXC_BP => {
            // Only the console's `break 7` self-test is handled.  Every other
            // break is a real fault and must not be swallowed.
            if self_test_instruction(tf.epc, 7, 16) {
                unsafe {
                    BREAKS = BREAKS.wrapping_add(1);
                }
                tf.epc = tf.epc.wrapping_add(4);
            } else {
                fatal_trap(tf, exc_code);
            }
        }
        EXC_SYS => {
            // Only the console's `syscall 7` self-test is handled here.  A
            // user syscall has no ABI yet and must not be silently skipped.
            if self_test_instruction(tf.epc, 7, 6) {
                unsafe {
                    SYSCALLS = SYSCALLS.wrapping_add(1);
                }
                tf.epc = tf.epc.wrapping_add(4);
            } else {
                fatal_trap(tf, exc_code);
            }
        }
        _ => fatal_trap(tf, exc_code),
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

/// Whether the instruction at `epc` is the self-test `break`/`syscall` with
/// the expected code.  Only canonical kseg0/kseg1 instruction addresses are
/// read; a user address is not safe to dereference from the handler.
fn self_test_instruction(epc: u32, code: u32, shift: u32) -> bool {
    if !(0x8000_0000..0x9000_0000).contains(&epc) || epc % 4 != 0 {
        return false;
    }
    // SAFETY: the range check keeps this in the kernel's direct map; the
    // self-test instruction itself is known to be present.
    let instruction = unsafe { core::ptr::read_volatile(epc as *const u32) };
    (instruction >> shift) & 0x3ff == code
}

/// Print a fatal trap and park.  The console is the only safe sink while the
/// exception level is set.
fn fatal_trap(tf: &TrapFrame, exc_code: u32) -> ! {
    crate::println("trap:");
    crate::print_str("  cause=0x");
    print_hex8((tf.cause >> 24) as u8);
    print_hex8((tf.cause >> 16) as u8);
    print_hex8((tf.cause >> 8) as u8);
    print_hex8(tf.cause as u8);
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

/// Exercise the software TLB-refill vector with one private mapping.
///
/// The test allocates a frame, writes a word through the kseg0 alias, makes
/// the test virtual page absent, then reads it through kuseg.  The refill
/// vector maps it uncached and returns through `eret`.
pub fn tlb_self_test() -> bool {
    let virt = 0x1000_0000u32;
    let page = core::ptr::addr_of_mut!(TLB_TEST_PAGE) as u32;
    let phys32 = page.wrapping_sub(0x8000_0000);
    let alias = page;
    let entrylo = ((phys32 >> 12) << 6) | 0x7; // D | V | G, uncached
    let value = 0x5a17_c0deu32;

    // Publish the one-entry mapping before touching the page.
    unsafe {
        core::ptr::write_volatile(core::ptr::addr_of_mut!(TLB_TEST_VPN), virt);
        core::ptr::write_volatile(core::ptr::addr_of_mut!(TLB_TEST_ENTRYLO), entrylo);
    }
    crate::cache::sync();

    // Write through the cached kseg0 alias, then flush before the uncached
    // virtual read.  The test page is private to this self-test.
    unsafe {
        core::ptr::write_volatile(alias as *mut u32, value);
    }
    crate::cache::dcache_writeback_invalidate(alias as usize, 4);
    crate::cache::sync();

    let observed = unsafe { core::ptr::read_volatile(virt as *const u32) };

    // Drop the test mapping so a later unexpected access is still fatal.
    unsafe {
        core::ptr::write_volatile(core::ptr::addr_of_mut!(TLB_TEST_VPN), 0);
        core::ptr::write_volatile(core::ptr::addr_of_mut!(TLB_TEST_ENTRYLO), 0);
    }
    crate::cache::sync();

    crate::print_str("tlb: self-test value=");
    crate::print_hex32(observed);
    crate::println("");
    observed == value
}

/// Enter a tiny user-mode image and take one syscall.
///
/// This is a one-way smoke test: the user syscall handler reports the trap and
/// parks, because there is no process/return path yet.  It proves the TLB
/// user mappings, Status.KSU, exception entry, trap-stack switch and CP0
/// frame handling.  The telnet `user` command enters through
/// [`user_from_console`] and returns there after the exit syscall.
#[no_mangle]
pub extern "C" fn user_smoke() -> ! {
    let code_phys = crate::frames::alloc_frame().expect("user smoke: code frame");
    let stack_phys = crate::frames::alloc_frame().expect("user smoke: stack frame");
    let code_phys = code_phys as u32;
    let stack_phys = stack_phys as u32;

    // MIPS III: write syscall; exit(42); loop.  Code 1 returns to user, code
    // 2 switches to the console return trampoline.
    let code: [u32; 7] = [
        0x2402_0001, // li $v0, 1
        0x0000_000c, // syscall
        0x2402_0002, // li $v0, 2
        0x2404_002a, // li $a0, 42
        0x0000_000c, // syscall
        0x1000_ffff, // b .
        0x0000_0000, // nop
    ];
    let code_alias = (code_phys | 0x8000_0000) as *mut u32;
    for (i, word) in code.iter().enumerate() {
        unsafe {
            core::ptr::write_volatile(code_alias.add(i), *word);
        }
    }
    crate::cache::dcache_writeback_invalidate(code_alias as usize, code.len() * 4);
    crate::cache::sync();

    crate::tlb::map_page_at(0x1000_2000, stack_phys, 0, 41, 0, true);
    crate::tlb::map_page_at(0x1000_0000, code_phys, 0, 40, 0, true);

    let kernel_gp: u32;
    unsafe {
        core::arch::asm!("move {0}, $gp", out(reg) kernel_gp);
        USER_KERNEL_GP = kernel_gp;
    }

    let mut status = status_read();
    status &= !(STATUS_KSU_MASK | STATUS_EXL | STATUS_ERL);
    status |= STATUS_KSU_USER | STATUS_IE | (1 << 15);
    epc_write(0x1000_0000);
    status_write(status);

    crate::println("user: entering user mode");
    unsafe {
        core::arch::asm!(
            "move $sp, {0}",
            "move $ra, $zero",
            "eret",
            in(reg) 0x1000_2000u32,
            options(noreturn)
        );
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
        core::arch::asm!("syscall 7");
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

    crate::print_str("vectors: src=0x");
    crate::print_hex32(general_src as u32);
    crate::print_str(" len=0x");
    crate::print_hex32(general_len as u32);
    crate::println("");

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
