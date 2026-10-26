/*
 * Flat loadable-module image at arena region 3 (0x88580000).
 * The kernel copies the body there and the manager enters the manifest offset.
 */

OUTPUT_FORMAT("elf32-ntradbigmips")
OUTPUT_ARCH(mips)
ENTRY(_start)

SECTIONS
{
  . = 0x88580000;

  .text : {
    KEEP(*(.text._start))
    *(.text)
    *(.text.*)
  }

  .rodata : {
    *(.rodata)
    *(.rodata.*)
  }

  .data : {
    *(.data)
    *(.data.*)
    *(.got)
    *(.got.*)
  }

  .bss (NOLOAD) : {
    *(.bss)
    *(.bss.*)
    *(COMMON)
  }

  /DISCARD/ : {
    *(.MIPS.abiflags)
    *(.reginfo)
    *(.pdr)
    *(.comment)
    *(.gnu.attributes)
    *(.mdebug*)
    *(.options*)
    *(.eh_frame)
  }
}
