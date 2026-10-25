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
const STATUS_KSU_MASK: u32 = 0x0000_0018; // KSU (mode) field: bits 4:3
const STATUS_KSU_KERNEL: u32 = 0x0000_0000; // Kernel mode
const STATUS_KSU_SUPERVISOR: u32 = 0x0000_0008; // Supervisor mode
const STATUS_KSU_USER: u32 = 0x0000_0010; // User mode
const STATUS_UX: u32 = 0x0000_0020; // User 64-bit addressing
const STATUS_BEV: u32 = 0x0040_0000; // Bootstrap Exception Vector
const STATUS_TS: u32 = 0x0000_0080; // TLB Shutdown
const STATUS_PE: u32 = 0x0000_0100; // Parity Enable
const STATUS_CM: u32 = 0x0000_0200; // Cache Parity Enable
const STATUS_CP0: u32 = 0x0000_1000; // Coprocessor 0 usable
const STATUS_CP1: u32 = 0x0000_2000; // Coprocessor 1 usable (FPU)
const STATUS_CP2: u32 = 0x0000_4000; // Coprocessor 2 usable
const STATUS_CP3: u32 = 0x0000_8000; // Coprocessor 3 usable
const STATUS_IM_MASK: u32 = 0x0000_ff00; // Interrupt Mask (bits 8-15)

// Cause register fields.
const CAUSE_EXC_MASK: u32 = 0x0000_007c; // Exception Code (bits 2-6)
const CAUSE_IP_MASK: u32 = 0x0000_ff00; // Interrupt Pending (bits 8-15)
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
///
/// n32 has 64-bit GPRs even though pointers are 32-bit.  The frame is 320
/// bytes; `trap_entry` uses the same offsets and a compile-time size check
/// below keeps the two definitions in step.
#[repr(C)]
#[derive(Debug)]
pub struct TrapFrame {
    pub regs: [u64; 32], // $0..$31
    pub hi: u64,
    pub lo: u64,
    pub epc: u64,
    pub badvaddr: u64,
    pub cause: u64,
    pub status: u64,
    pub entryhi: u64,
    pub pagemask: u64,
}

const _: () = assert!(core::mem::size_of::<TrapFrame>() == 0x140);

impl TrapFrame {
    /// Low 32 bits of a saved GPR (the n32 syscall ABI uses 32-bit words).
    #[inline]
    pub fn arg(&self, index: usize) -> u32 {
        self.regs[index] as u32
    }

    /// Set the syscall result register, zero-extending into the 64-bit GPR.
    #[inline]
    pub fn set_result(&mut self, value: u32) {
        self.regs[2] = value as u64;
    }

    #[inline]
    pub fn advance_epc(&mut self) {
        self.epc = self.epc.wrapping_add(4);
    }
}

/// The single kernel-mode trap frame used by the exception vector.
///
/// The kernel is single-threaded while an exception is being handled and
/// interrupts are disabled by `EXL`; a nested exception is a bug.
#[no_mangle]
static mut TRAP_FRAME: TrapFrame = TrapFrame {
    regs: [0; 32],
    hi: 0,
    lo: 0,
    epc: 0,
    badvaddr: 0,
    cause: 0,
    status: 0,
    entryhi: 0,
    pagemask: 0,
};

/// A cacheable kernel word used to clear the per-processor LL bit on every
/// trap.  `sc` clears LL even when its store fails.
#[no_mangle]
static mut LL_SCRATCH: u64 = 0;

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
    pub fn user_from_console_elf();
    pub fn user_return_trampoline();
    fn user_eret(user_sp: u32, argc: u32, argv: u32, envp: u32) -> !;
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

    .section .text.user_from_console_elf, "ax", @progbits
    .align 4
    .globl user_from_console_elf
user_from_console_elf:
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
    jal     user_elf
    nop
    /* The loader returns only on a load failure; restore the console caller.
       A successful user image exits through user_return_trampoline instead. */
    lui     $t1, %hi(USER_RETURN_REGS)
    addiu   $t1, $t1, %lo(USER_RETURN_REGS)
    lw      $16, 0($t1)
    lw      $17, 4($t1)
    lw      $18, 8($t1)
    lw      $19, 12($t1)
    lw      $20, 16($t1)
    lw      $21, 20($t1)
    lw      $22, 24($t1)
    lw      $23, 28($t1)
    lw      $30, 32($t1)
    lui     $t1, %hi(USER_RETURN_RA)
    addiu   $t1, $t1, %lo(USER_RETURN_RA)
    lw      $ra, 0($t1)
    lui     $t1, %hi(USER_RETURN_SP)
    addiu   $t1, $t1, %lo(USER_RETURN_SP)
    lw      $sp, 0($t1)
    jr      $ra
    nop

    /* Enter user mode from C ABI arguments: $sp, $a0, $a1, $a2.  Keeping
       this in assembly avoids clobbering inline-asm inputs while loading
       the fixed argument registers. */
    .section .text.user_eret, "ax", @progbits
    .align 4
    .globl user_eret
user_eret:
    move    $sp, $4
    move    $a0, $5
    move    $a1, $6
    move    $a2, $7
    move    $ra, $zero
    eret
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

    /* A timer trap also clears LL; a preempted CAS must retry. */
    lui     $k0, %hi(LL_SCRATCH)
    addiu   $k0, $k0, %lo(LL_SCRATCH)
    sc      $zero, 0($k0)
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
    sd      $1, 8($k0)
    sd      $2, 16($k0)
    sd      $3, 24($k0)
    sd      $4, 32($k0)
    sd      $5, 40($k0)
    sd      $6, 48($k0)
    sd      $7, 56($k0)
    sd      $8, 64($k0)
    sd      $9, 72($k0)
    sd      $10, 80($k0)
    sd      $11, 88($k0)
    sd      $12, 96($k0)
    sd      $13, 104($k0)
    sd      $14, 112($k0)
    sd      $15, 120($k0)
    sd      $16, 128($k0)
    sd      $17, 136($k0)
    sd      $18, 144($k0)
    sd      $19, 152($k0)
    sd      $20, 160($k0)
    sd      $21, 168($k0)
    sd      $22, 176($k0)
    sd      $23, 184($k0)
    sd      $24, 192($k0)
    sd      $25, 200($k0)
    sd      $28, 224($k0)
    sd      $29, 232($k0)
    sd      $30, 240($k0)
    sd      $31, 248($k0)

    mfhi    $t0
    sd      $t0, 256($k0)
    mflo    $t0
    sd      $t0, 264($k0)
    mfc0    $t0, $14
    sd      $t0, 272($k0)
    mfc0    $t0, $8
    sd      $t0, 280($k0)
    mfc0    $t0, $13
    sd      $t0, 288($k0)
    mfc0    $t0, $12
    sd      $t0, 296($k0)
    mfc0    $t0, $10
    sd      $t0, 304($k0)
    mfc0    $t0, $5
    sd      $t0, 312($k0)

    /* Clear the per-processor LL bit.  SC clears it whether the store
       succeeds or fails, so a preempted CAS retries instead of falsely
       succeeding. */
    lui     $k1, %hi(LL_SCRATCH)
    addiu   $k1, $k1, %lo(LL_SCRATCH)
    sc      $zero, 0($k1)

    /* Switch to the dedicated trap stack.  The saved user/kernel $sp is in
       the frame and is restored below. */
    lui     $sp, %hi(TRAP_STACK)
    addiu   $sp, $sp, %lo(TRAP_STACK)
    addiu   $sp, $sp, 16384

    move    $a0, $k0
    jal     trap_handler
    nop

    lui     $k0, %hi(TRAP_FRAME)
    addiu   $k0, $k0, %lo(TRAP_FRAME)
    ld      $t0, 256($k0)
    mthi    $t0
    ld      $t0, 264($k0)
    mtlo    $t0
    ld      $t0, 304($k0)
    mtc0    $t0, $10
    ld      $t0, 312($k0)
    mtc0    $t0, $5
    ld      $t0, 272($k0)
    mtc0    $t0, $14

    /* Restore the GPRs after CP0 state.  $k0/$k1 are scratch and are not
       part of the user context. */
    ld      $1, 8($k0)
    ld      $2, 16($k0)
    ld      $3, 24($k0)
    ld      $4, 32($k0)
    ld      $5, 40($k0)
    ld      $6, 48($k0)
    ld      $7, 56($k0)
    ld      $8, 64($k0)
    ld      $9, 72($k0)
    ld      $10, 80($k0)
    ld      $11, 88($k0)
    ld      $12, 96($k0)
    ld      $13, 104($k0)
    ld      $14, 112($k0)
    ld      $15, 120($k0)
    ld      $16, 128($k0)
    ld      $17, 136($k0)
    ld      $18, 144($k0)
    ld      $19, 152($k0)
    ld      $20, 160($k0)
    ld      $21, 168($k0)
    ld      $22, 176($k0)
    ld      $23, 184($k0)
    ld      $24, 192($k0)
    ld      $25, 200($k0)
    ld      $28, 224($k0)
    ld      $29, 232($k0)
    ld      $30, 240($k0)
    ld      $31, 248($k0)

    lui     $k0, %hi(TRAP_FRAME)
    addiu   $k0, $k0, %lo(TRAP_FRAME)
    ld      $k0, 296($k0)
    ori     $k0, $k0, 0x2
    mtc0    $k0, $12
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
    let cause = tf.cause as u32;
    let exc_code = (cause & CAUSE_EXC_MASK) >> 2;

    // User-mode syscall smoke test.  Code 1 is a console write and returns to
    // user mode; code 2 exits to a kernel continuation.  This is deliberately
    // a fixed ABI for the smoke test, not the native syscall ABI.
    if exc_code == EXC_SYS && tf.status as u32 & STATUS_KSU_MASK == STATUS_KSU_USER {
        match tf.arg(2) {
            1 => {
                tf.set_result(crate::user::write_fd(tf.arg(4), tf.arg(5), tf.arg(6)) as u32);
                tf.advance_epc();
            }
            3 => {
                // Provisional anonymous mmap(addr, length, prot, flags):
                // the address, protection and flags are ignored in v0.
                tf.set_result(crate::user::mmap(tf.arg(5)) as u32);
                tf.advance_epc();
            }
            4 => {
                // munmap: the v0 heap is one-way; mappings are released at exit.
                tf.set_result(0);
                tf.advance_epc();
            }
            5 => {
                // mprotect: every user page is already read/write.
                tf.set_result(0);
                tf.advance_epc();
            }
            6 => {
                tf.set_result(crate::user::open_path(tf.arg(4), tf.arg(5)) as u32);
                tf.advance_epc();
            }
            7 => {
                tf.set_result(crate::fd::close(tf.arg(4) as i32) as u32);
                tf.advance_epc();
            }
            8 => {
                tf.set_result(0);
                tf.advance_epc();
            }
            9 => {
                tf.set_result(crate::fd::dup(tf.arg(4) as i32) as u32);
                tf.advance_epc();
            }
            10 => {
                tf.set_result(crate::user::getcwd_text(tf.arg(4), tf.arg(5)) as u32);
                tf.advance_epc();
            }
            11 => {
                tf.set_result(crate::user::poll_user(tf.arg(4), tf.arg(5)) as u32);
                tf.advance_epc();
            }
            12 => {
                // signal(SIGPIPE, SIG_IGN): no signal machinery yet.
                tf.set_result(0);
                tf.advance_epc();
            }
            13 => {
                tf.set_result(crate::user::entropy_user(tf.arg(4), tf.arg(5)) as u32);
                tf.advance_epc();
            }
            14 => {
                tf.set_result(crate::user::clock_user(tf.arg(4), tf.arg(5)) as u32);
                tf.advance_epc();
            }
            15 => {
                tf.set_result(1);
                tf.advance_epc();
            }
            16 => {
                tf.set_result(crate::user::read_fd(tf.arg(4), tf.arg(5), tf.arg(6)) as u32);
                tf.advance_epc();
            }
            17 => {
                tf.set_result(crate::user::fstat_user(tf.arg(4), tf.arg(5)) as u32);
                tf.advance_epc();
            }
            18 => {
                tf.set_result(crate::user::stat_user(tf.arg(4), tf.arg(5)) as u32);
                tf.advance_epc();
            }
            19 => {
                tf.set_result(
                    crate::user::readdir_user(tf.arg(4), tf.arg(5), tf.arg(6), tf.arg(7)) as u32,
                );
                tf.advance_epc();
            }
            20 => {
                tf.set_result(crate::user::sleep_user(tf.arg(4)) as u32);
                tf.advance_epc();
            }
            21 => {
                tf.set_result(crate::user::chdir_user(tf.arg(4)) as u32);
                tf.advance_epc();
            }
            22 => {
                tf.set_result(crate::user::pipe_user(tf.arg(4)) as u32);
                tf.advance_epc();
            }
            2 => {
                let code = tf.arg(4);
                crate::user::cleanup();
                crate::print_str("user: exit syscall code=0x");
                crate::print_hex32(code);
                crate::println("");
                tf.epc = user_return_trampoline as usize as u64;
                tf.regs[4] = code as u64;
                let saved =
                    unsafe { core::ptr::read_volatile(core::ptr::addr_of!(USER_RETURN_REGS)) };
                for (index, value) in saved[..8].iter().enumerate() {
                    tf.regs[16 + index] = *value as u64;
                }
                tf.regs[30] = saved[8] as u64;
                tf.regs[28] = unsafe { USER_KERNEL_GP } as u64;
                tf.regs[29] = unsafe { USER_RETURN_SP } as u64;
                tf.regs[31] = unsafe { USER_RETURN_RA } as u64;
                tf.status &= !((STATUS_KSU_MASK | STATUS_UX) as u64); // return to kernel mode
            }
            _ => fatal_trap(tf, exc_code),
        }
        return;
    }

    match exc_code {
        EXC_BP => {
            // Only the console's `break 7` self-test is handled.  Every other
            // break is a real fault and must not be swallowed.
            if self_test_instruction(tf.epc as u32, 7, 16) {
                unsafe {
                    BREAKS = BREAKS.wrapping_add(1);
                }
                tf.advance_epc();
            } else {
                fatal_trap(tf, exc_code);
            }
        }
        EXC_SYS => {
            // Only the console's `syscall 7` self-test is handled here.  A
            // user syscall has no ABI yet and must not be silently skipped.
            if self_test_instruction(tf.epc as u32, 7, 6) {
                unsafe {
                    SYSCALLS = SYSCALLS.wrapping_add(1);
                }
                tf.advance_epc();
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
    let epc: u32;
    let cause: u32;
    unsafe {
        core::arch::asm!(".set noat", "mfc0 {0}, $14", out(reg) epc);
        core::arch::asm!(".set noat", "mfc0 {0}, $13", out(reg) cause);
    }
    crate::print_str("trap: TLB refill epc=0x");
    print_hex8((epc >> 24) as u8);
    print_hex8((epc >> 16) as u8);
    print_hex8((epc >> 8) as u8);
    print_hex8(epc as u8);
    crate::print_str(" cause=0x");
    print_hex8((cause >> 24) as u8);
    print_hex8((cause >> 16) as u8);
    print_hex8((cause >> 8) as u8);
    print_hex8(cause as u8);
    crate::print_str(" badvaddr=0x");
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
    print_hex8(((tf.cause as u32) >> 24) as u8);
    print_hex8(((tf.cause as u32) >> 16) as u8);
    print_hex8(((tf.cause as u32) >> 8) as u8);
    print_hex8(tf.cause as u8);
    crate::print_str(" exc=");
    print_hex8(exc_code as u8);
    crate::print_str(" epc=0x");
    print_hex8(((tf.epc as u32) >> 24) as u8);
    print_hex8(((tf.epc as u32) >> 16) as u8);
    print_hex8(((tf.epc as u32) >> 8) as u8);
    print_hex8(tf.epc as u8);
    crate::print_str(" badvaddr=0x");
    print_hex8(((tf.badvaddr as u32) >> 24) as u8);
    print_hex8(((tf.badvaddr as u32) >> 16) as u8);
    print_hex8(((tf.badvaddr as u32) >> 8) as u8);
    print_hex8(tf.badvaddr as u8);
    crate::println("");
    loop {
        core::hint::spin_loop();
    }
}

/// Exercise the software TLB-refill vector with one private mapping.
///
/// The test writes a word through the uncached kseg1 alias, invalidates the
/// non-wired TLB, then reads the private virtual page through kuseg.  The
/// refill vector maps it uncached and returns through `eret`.
pub fn tlb_self_test() -> bool {
    // Keep the test VPN disjoint from the user image (0x1000_0000), the
    // anonymous heap (0x2000_0000) and the user stack (0x7FF0_0000).
    let virt = 0x3000_0000u32;
    let page = core::ptr::addr_of_mut!(TLB_TEST_PAGE) as u32;
    let phys32 = page.wrapping_sub(0x8000_0000);
    // Write through kseg1 (uncached) so this test does not depend on a
    // writeback from the cached kseg0 alias.
    let alias = (phys32 | 0xA000_0000) as *mut u32;
    let entrylo = ((phys32 >> 12) << 6) | 0x7; // D | V | G, uncached
    let value = 0x5a17_c0deu32;

    // Start from a clean non-wired TLB and a 4 KiB page mask.  A previous
    // user image or an earlier test must not satisfy the read without the
    // refill vector running.
    crate::tlb::invalidate_all();
    crate::traps::pagemask_write(0);
    unsafe {
        core::ptr::write_volatile(core::ptr::addr_of_mut!(TLB_TEST_VPN), virt);
        core::ptr::write_volatile(core::ptr::addr_of_mut!(TLB_TEST_ENTRYLO), entrylo);
    }
    crate::cache::sync();

    unsafe {
        core::ptr::write_volatile(alias, value);
    }
    crate::cache::sync();

    let observed = unsafe { core::ptr::read_volatile(virt as *const u32) };

    // Drop the test mapping and any stale entry it created.
    unsafe {
        core::ptr::write_volatile(core::ptr::addr_of_mut!(TLB_TEST_VPN), 0);
        core::ptr::write_volatile(core::ptr::addr_of_mut!(TLB_TEST_ENTRYLO), 0);
    }
    crate::tlb::invalidate_all();
    crate::cache::sync();

    crate::print_str("tlb: self-test value=");
    crate::print_hex32(observed);
    crate::println("");
    let ok = observed == value;
    crate::print_str("tlb: self-test ");
    crate::println(if ok { "ok" } else { "FAILED" });
    ok
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

    run_user(0x1000_0000, 0x1000_2000, 0, 0, 0)
}

/// Enter user mode at `entry` with `user_sp`, `argc`, `argv` and `envp`.
pub fn run_user(entry: u32, user_sp: u32, argc: u32, argv: u32, envp: u32) -> ! {
    let kernel_gp: u32;
    unsafe {
        core::arch::asm!("move {0}, $gp", out(reg) kernel_gp);
        USER_KERNEL_GP = kernel_gp;
    }

    // Set up the user entry with interrupts masked by EXL.  If IE is enabled
    // before EPC is set, a timer interrupt can overwrite EPC; the following
    // `eret` then returns to the interrupted kernel PC instead of userland.
    let mut status = status_read();
    status &= !(STATUS_KSU_MASK | STATUS_ERL | STATUS_IE);
    status_write(status);
    epc_write(entry);
    status |= STATUS_KSU_USER | STATUS_UX | STATUS_IE | (1 << 15) | STATUS_EXL;
    status_write(status);

    crate::println("user: entering user mode");
    crate::graphics_console::flush();
    unsafe { user_eret(user_sp, argc, argv, envp) }
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

    let ok = after_breaks == before_breaks.wrapping_add(1)
        && after_syscalls == before_syscalls.wrapping_add(1);
    crate::print_str("trap: self-test ");
    crate::println(if ok { "ok" } else { "FAILED" });
    ok
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
    const XTLB_VECTOR: *mut u8 = 0x8000_0080 as *mut u8;

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
            // With UX=1 the CPU uses the 64-bit XTLB refill vector at 0x80.
            core::ptr::write_volatile(XTLB_VECTOR.add(i), core::ptr::read_volatile(tlb_src.add(i)));
        }
    }

    crate::cache::dcache_writeback_invalidate(GENERAL_VECTOR as usize, general_len);
    crate::cache::icache_invalidate(GENERAL_VECTOR as usize, general_len);
    crate::cache::dcache_writeback_invalidate(TLB_VECTOR as usize, tlb_len);
    crate::cache::icache_invalidate(TLB_VECTOR as usize, tlb_len);
    crate::cache::dcache_writeback_invalidate(XTLB_VECTOR as usize, tlb_len);
    crate::cache::icache_invalidate(XTLB_VECTOR as usize, tlb_len);
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

#[cfg(test)]
mod tests {
    use super::*;
    use core::mem::{offset_of, size_of};

    #[test]
    fn trap_frame_layout() {
        assert_eq!(size_of::<TrapFrame>(), 0x140);
        assert_eq!(offset_of!(TrapFrame, regs), 0x000);
        assert_eq!(offset_of!(TrapFrame, hi), 0x100);
        assert_eq!(offset_of!(TrapFrame, lo), 0x108);
        assert_eq!(offset_of!(TrapFrame, epc), 0x110);
        assert_eq!(offset_of!(TrapFrame, badvaddr), 0x118);
        assert_eq!(offset_of!(TrapFrame, cause), 0x120);
        assert_eq!(offset_of!(TrapFrame, status), 0x128);
        assert_eq!(offset_of!(TrapFrame, entryhi), 0x130);
        assert_eq!(offset_of!(TrapFrame, pagemask), 0x138);
    }

    #[test]
    fn status_mode_bits() {
        assert_eq!(STATUS_KSU_MASK, 0x18);
        assert_eq!(STATUS_KSU_USER, 0x10);
        assert_eq!(STATUS_UX, 0x20);
        assert_eq!(STATUS_BEV, 0x0040_0000);
        assert_eq!(STATUS_IM_MASK, 0x0000_ff00);
    }
}
