/*
 * Kernel boot entry.
 *
 * Entered from the ROM at KSEG0 0x88040000 with:
 *   $a0 = physical address of BootInfo
 *   $sp = valid stack
 *   caches on, interrupts off
 *
 * Clear .bss, preserve BootInfo in $s0, and enter Rust.
 */

.set noreorder
.set mips3
.set noat

/* ------------------------------------------------------------
 * Exception vectors at kseg0 0x80000000 (BEV = 0)
 * ------------------------------------------------------------
.section .text.vectors, "ax", @progbits
.align 4

.globl reset_vector
reset_vector:
    j       _start
    nop

. = 0x80000080
.globl tlb_refill_entry
tlb_refill_entry:
    j       general_vector
    nop

. = 0x80000100
.globl general_vector
general_vector:
    /* Timer interrupt: advance CP0 Compare and return */
    mfc0    k0, $9           /* Count */
    lui     k1, 0x000F
    ori     k1, k1, 0x4240   /* 1000000 */
    addu    k0, k0, k1
    mtc0    k0, $11          /* Compare = Count + interval */
    eret
    nop

/* ------------------------------------------------------------
 * Kernel boot entry at 0x88040000
 * ------------------------------------------------------------
.section .text._start, "ax", @progbits
.globl _start
.ent _start
_start:
    move    $s0, $a0

    la      $sp, __stack_top

    la      $t0, __bss_start
    la      $t1, __bss_end
1:
    sltu    $t2, $t0, $t1
    beqz    $t2, 2f
    nop
    sw      $zero, 0($t0)
    addiu   $t0, $t0, 4
    b       1b
    nop
2:
    move    $a0, $s0
    jal     kernel_main
    nop
3:
    b       3b
    nop
.end _start