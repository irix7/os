/* SPDX-License-Identifier: GPL-3.0-only */
/*
 * Flat loadable-module image for the x86_64 machine.
 *
 * The kernel copies the body to MODULE_BASE and calls offset 0.  The address is
 * a reserved window in the machine's memory map (kernel-x86/src/memory.rs), so
 * the frame allocator never hands it out.  The image stays inside one 2 MiB
 * slot, with .bss zeroed by the loader over the whole slot.
 */

OUTPUT_FORMAT("elf64-x86-64")
OUTPUT_ARCH(i386:x86-64)
ENTRY(_start)

SECTIONS
{
  . = 0x02000000;

  .text : {
    KEEP(*(.text._start))
    *(.text)
    *(.text.*)
  }

  .rodata : ALIGN(16) {
    *(.rodata)
    *(.rodata.*)
  }

  .data : ALIGN(16) {
    *(.data)
    *(.data.*)
    *(.got)
    *(.got.*)
  }

  .bss (NOLOAD) : ALIGN(16) {
    *(.bss)
    *(.bss.*)
    *(COMMON)
  }

  __module_end = .;
  ASSERT(__module_end <= 0x02200000, "mcp x86 module image and .bss exceed its 2 MiB window")

  /DISCARD/ : {
    *(.eh_frame)
    *(.comment)
  }
}
