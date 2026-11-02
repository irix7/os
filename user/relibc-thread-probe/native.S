# SPDX-License-Identifier: GPL-3.0-only
#
# Raw native-gate stubs for the thread probe.
#
# The gate is `syscall 0x1F7`: the number goes in $2, arguments in $4-$7, and
# the kernel returns the CELL_E_* status in $2 and the value in $3.  These
# stubs are deliberately minimal: `thread_create` stores the status in the
# probe's `__irix7_thread_status` and returns the value, so the C caller can
# check both without a fifth argument (n32 passes arguments five and up on the
# stack).

    .set noreorder
    .set mips3
    .set noat

    .text
    .align 4

# long __irix7_thread_create(unsigned long entry, unsigned long arg,
#                            unsigned long stack_top, unsigned long tcb)
    .globl __irix7_thread_create
__irix7_thread_create:
    li      $2, 37
    syscall 0x1F7
    lui     $9, %hi(__irix7_thread_status)
    addiu   $9, $9, %lo(__irix7_thread_status)
    sw      $2, 0($9)
    move    $2, $3
    jr      $ra
    nop

# long __irix7_yield(void)
    .globl __irix7_yield
__irix7_yield:
    li      $2, 38
    syscall 0x1F7
    move    $2, $3
    jr      $ra
    nop

# void __irix7_thread_exit(long status)
#
# The kernel resumes another thread or returns to the console; it never
# returns here.  The loop only stops a stray return from executing garbage.
    .globl __irix7_thread_exit
__irix7_thread_exit:
    li      $2, 39
    syscall 0x1F7
1:
    b       1b
    nop
