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
