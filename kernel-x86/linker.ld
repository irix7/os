/* SPDX-License-Identifier: GPL-3.0-only */
/*
 * The kernel image is linked at 1 MiB, the address QEMU's PVH loader uses for
 * a physical ELF.  The Xen note is its own PT_NOTE segment so the loader can
 * find it without depending on section headers.
 */

OUTPUT_ARCH(i386:x86-64)
ENTRY(_start32)

PHDRS
{
    note PT_NOTE;
    text PT_LOAD FLAGS(5);
    rodata PT_LOAD FLAGS(4);
    data PT_LOAD FLAGS(6);
}

SECTIONS
{
    . = 0x00100000;

    .note.Xen : ALIGN(4) {
        KEEP(*(.note.Xen))
    } :note

    .text : ALIGN(4096) {
        KEEP(*(.text.boot))
        *(.text .text.*)
    } :text

    .rodata : ALIGN(4096) {
        *(.rodata .rodata.*)
    } :rodata

    .data : ALIGN(4096) {
        *(.data .data.*)
    } :data

    .bss : ALIGN(4096) {
        __bss_start = .;
        *(.bss .bss.*)
        *(COMMON)
        . = ALIGN(16);
        __bss_end = .;
    } :data

    /* First byte after the image: bss, page tables and the boot stack. */
    __kernel_end = ALIGN(4096);

    /DISCARD/ : {
        *(.eh_frame)
        *(.comment)
    }
}
