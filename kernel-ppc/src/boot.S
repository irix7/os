/* SPDX-License-Identifier: GPL-3.0-only */
/*
 * PowerPC G3 Beige entry.
 *
 * OpenBIOS's preloaded-kernel path enters a raw image at 0x01000000 with the
 * MMU already enabled and RAM mapped one-to-one.  The only register the
 * client may rely on is r5, which holds the Open Firmware client interface
 * handler.  r3/r4 carry the initrd address and size (unused by this slice).
 *
 * The entry point establishes its own stack, clears .bss, and calls the Rust
 * kernel with the client handler.
 *
 * LLVM's PowerPC integrated assembler uses numeric register names; the
 * `rN` spelling is a GNU as extension and is rejected for inline assembly.
 */

.section .text.boot, "ax"
.global _start
.type _start, @function
_start:
    /* Clear .bss before any Rust code touches it. */
    lis     3, __bss_start@ha
    addi    3, 3, __bss_start@l
    lis     4, __bss_end@ha
    addi    4, 4, __bss_end@l
    li      0, 0
1:
    cmpw    3, 4
    bge     2f
    stw     0, 0(3)
    addi    3, 3, 4
    b       1b
2:
    lis     1, __stack_top@ha
    addi    1, 1, __stack_top@l

    mr      3, 5
    bl      kernel_main
3:
    b       3b
.size _start, . - _start

/*
 * Call the Open Firmware client interface handler.
 *
 * r3 = handler address, r4 = pointer to the argument block.  The handler
 * returns 0 (true) or -1 (false) in r3.
 */
.section .text
.global of_call
.type of_call, @function
of_call:
    mflr    0
    stw     0, 4(1)
    stwu    1, -16(1)
    mtctr   3
    mr      3, 4
    bctrl
    addi    1, 1, 16
    lwz     0, 4(1)
    mtlr    0
    blr
.size of_call, . - of_call

/*
 * Decrementer exception vector target.  The hardware clears MSR[EE] on entry,
 * so the handler is not re-entered.  It counts the tick, re-arms the
 * decrementer, and returns through rfi.
 */
.section .text
.align 4
.global decrementer_handler
.type decrementer_handler, @function
decrementer_handler:
    stwu    1, -96(1)
    stw     0, 8(1)
    stw     2, 12(1)
    stw     3, 16(1)
    stw     4, 20(1)
    stw     5, 24(1)
    stw     6, 28(1)
    stw     7, 32(1)
    stw     8, 36(1)
    stw     9, 40(1)
    stw     10, 44(1)
    stw     11, 48(1)
    stw     12, 52(1)
    mfcr    0
    stw     0, 56(1)
    mflr    0
    stw     0, 60(1)
    mfctr   0
    stw     0, 64(1)
    mfxer   0
    stw     0, 68(1)

    lis     4, TICKS@ha
    lwz     5, TICKS@l(4)
    addi    5, 5, 1
    stw     5, TICKS@l(4)

    lis     4, TICK_RELOAD@ha
    lwz     5, TICK_RELOAD@l(4)
    mtspr   22, 5

    lwz     0, 68(1)
    mtxer   0
    lwz     0, 64(1)
    mtctr   0
    lwz     0, 60(1)
    mtlr    0
    lwz     0, 56(1)
    mtcr    0
    lwz     2, 12(1)
    lwz     3, 16(1)
    lwz     4, 20(1)
    lwz     5, 24(1)
    lwz     6, 28(1)
    lwz     7, 32(1)
    lwz     8, 36(1)
    lwz     9, 40(1)
    lwz     10, 44(1)
    lwz     11, 48(1)
    lwz     12, 52(1)
    lwz     0, 8(1)
    addi    1, 1, 96
    rfi
.size decrementer_handler, . - decrementer_handler
