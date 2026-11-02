# SPDX-License-Identifier: GPL-3.0-only
#
# Probe entry.  The kernel enters with $sp at the System V stack, $a0 = argc,
# $a1 = argv, $a2 = envp, $a3 = auxv, $28 = 0.  Establish the main-thread TCB
# at $28 before any C code runs; MIPS III has no rdhwr for a thread pointer.

    .set noreorder
    .set mips3
    .set noat

    .text
    .align 2
    .globl _start
    .type _start, @function
_start:
    lui     $8, %hi(main_tcb)
    addiu   $8, $8, %lo(main_tcb)
    sw      $8, 0($8)           # self
    sw      $zero, 4($8)        # dtv: no ELF TLS in v0
    sw      $zero, 8($8)        # errno
    sw      $zero, 12($8)       # reserved
    move    $28, $8
    move    $4, $sp
    jal     probe_main
    nop

    # probe_main must not return.  If it does, exit 43.
    addiu   $4, $zero, 43
    addiu   $2, $zero, 2        # SYS_EXIT
    addiu   $3, $zero, 0
    syscall 0x1F7
1:
    b       1b
    nop
    .size _start, .-_start
