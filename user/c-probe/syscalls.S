# SPDX-License-Identifier: GPL-3.0-only
#
# Native syscall stubs for the n32 C probe.
#
# The kernel returns a CELL_E_* status in $2 and the result in $3.  On error
# these stubs map the status through status_errno[] into the TCB's errno slot
# and return -1, so callers see ordinary POSIX -1/errno without guessing.

    .set noreorder
    .set mips3
    .set noat

    .text
    .align 2
    .globl __irix7_syscall0
__irix7_syscall0:
    move    $2, $4
    syscall 0x1F7
    bnez    $2, 1f
    nop
    move    $2, $3
    jr      $ra
    nop
1:
    sw      $2, 12($28)
    lui     $8, %hi(status_errno)
    addiu   $8, $8, %lo(status_errno)
    addu    $8, $8, $2
    lbu     $9, 0($8)
    sw      $9, 8($28)
    addiu   $2, $zero, -1
    jr      $ra
    nop

    .align 2
    .globl __irix7_syscall1
__irix7_syscall1:
    move    $8, $4
    move    $4, $5
    move    $2, $8
    syscall 0x1F7
    bnez    $2, 1f
    nop
    move    $2, $3
    jr      $ra
    nop
1:
    sw      $2, 12($28)
    lui     $8, %hi(status_errno)
    addiu   $8, $8, %lo(status_errno)
    addu    $8, $8, $2
    lbu     $9, 0($8)
    sw      $9, 8($28)
    addiu   $2, $zero, -1
    jr      $ra
    nop

    .align 2
    .globl __irix7_syscall2
__irix7_syscall2:
    move    $8, $4
    move    $4, $5
    move    $5, $6
    move    $2, $8
    syscall 0x1F7
    bnez    $2, 1f
    nop
    move    $2, $3
    jr      $ra
    nop
1:
    sw      $2, 12($28)
    lui     $8, %hi(status_errno)
    addiu   $8, $8, %lo(status_errno)
    addu    $8, $8, $2
    lbu     $9, 0($8)
    sw      $9, 8($28)
    addiu   $2, $zero, -1
    jr      $ra
    nop

    .align 2
    .globl __irix7_syscall3
__irix7_syscall3:
    move    $8, $4
    move    $4, $5
    move    $5, $6
    move    $6, $7
    move    $2, $8
    syscall 0x1F7
    bnez    $2, 1f
    nop
    move    $2, $3
    jr      $ra
    nop
1:
    sw      $2, 12($28)
    lui     $8, %hi(status_errno)
    addiu   $8, $8, %lo(status_errno)
    addu    $8, $8, $2
    lbu     $9, 0($8)
    sw      $9, 8($28)
    addiu   $2, $zero, -1
    jr      $ra
    nop
