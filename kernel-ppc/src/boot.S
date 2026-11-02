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
 * Exception vector targets.
 *
 * The 0x300, 0x400, 0x500, 0x600, 0x700 and 0xC00 vectors each hold a branch
 * to one of these stubs.  A stub parks the interrupted r0 in SPRG0, puts its
 * vector number in r0 and falls into the common entry.  SPRG0 is kernel
 * scratch and is safe because the hardware clears MSR[EE] on exception entry,
 * so a stub cannot be interrupted by another exception.
 *
 * The common entry saves every volatile register and the special registers,
 * calls trap_dispatch with the vector, the interrupted r3 and LR, writes the
 * returned r3 into the frame, restores the context and returns with rfi.
 * r13-r31 are callee-saved by trap_dispatch, so they need no frame slot.
 */
.section .text
.align 4
.global trap_entry
.type trap_entry, @function
trap_entry:
    stwu    1, -128(1)
    stw     0, 4(1)                 /* vector number */
    mfspr   0, 272                  /* SPRG0: interrupted r0 */
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

    lwz     3, 4(1)                 /* vector */
    lwz     4, 16(1)                /* interrupted r3 */
    lwz     5, 60(1)                /* interrupted LR */
    bl      trap_dispatch
    stw     3, 16(1)

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
    addi    1, 1, 128
    rfi
.size trap_entry, . - trap_entry

.section .text
.align 4
.global dsi_handler
.type dsi_handler, @function
dsi_handler:
    mtspr   272, 0
    li      0, 0x300
    b       trap_entry
.size dsi_handler, . - dsi_handler

.section .text
.align 4
.global isi_handler
.type isi_handler, @function
isi_handler:
    mtspr   272, 0
    li      0, 0x400
    b       trap_entry
.size isi_handler, . - isi_handler

/*
 * External interrupt vector target.  The controller output is level
 * sensitive, so the Rust handler clears the source and acknowledges the
 * controller before returning through the common entry.
 */
.section .text
.align 4
.global external_handler
.type external_handler, @function
external_handler:
    mtspr   272, 0
    li      0, 0x500
    b       trap_entry
.size external_handler, . - external_handler

.section .text
.align 4
.global alignment_handler
.type alignment_handler, @function
alignment_handler:
    mtspr   272, 0
    li      0, 0x600
    b       trap_entry
.size alignment_handler, . - alignment_handler

.section .text
.align 4
.global program_handler
.type program_handler, @function
program_handler:
    mtspr   272, 0
    li      0, 0x700
    b       trap_entry
.size program_handler, . - program_handler

.section .text
.align 4
.global system_call_handler
.type system_call_handler, @function
system_call_handler:
    mtspr   272, 0
    li      0, 0xc00
    b       trap_entry
.size system_call_handler, . - system_call_handler

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
