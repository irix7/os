/* SPDX-License-Identifier: GPL-3.0-only */
/*
 * The G3 Beige firmware loads the raw kernel image at 0x01000000 and enters
 * it there with the MMU already on.  The image is position-dependent: every
 * address below is the physical address the CPU executes at.
 */

OUTPUT_ARCH(powerpc)
ENTRY(_start)

SECTIONS
{
    . = 0x01000000;
    __kernel_start = .;

    .text : ALIGN(16) {
        KEEP(*(.text.boot))
        *(.text .text.*)
    }

    .rodata : ALIGN(16) {
        *(.rodata .rodata.*)
    }

    .data : ALIGN(16) {
        *(.data .data.*)
    }

    .bss : ALIGN(16) {
        __bss_start = .;
        *(.bss .bss.*)
        *(COMMON)
        . = ALIGN(16);
        __bss_end = .;
    }

    /* The boot stack is outside the file-backed image; the loader leaves the
       rest of RAM zeroed. */
    .stack (NOLOAD) : ALIGN(16) {
        __stack_bottom = .;
        . += 0x10000;
        __stack_top = .;
    }

    __kernel_end = .;

    /* The image, .bss and the boot stack share one BAT block. */
    ASSERT(__kernel_end <= 0x02000000, "kernel image exceeds the first 16 MiB")

    /DISCARD/ : {
        *(.eh_frame)
        *(.comment)
        *(.note .note.*)
    }
}
