/*
 * Flat loadable-module image at kseg0 0x88400000.
 * The kernel copies the body there and calls offset 0.
 */

OUTPUT_FORMAT("elf32-ntradbigmips")
OUTPUT_ARCH(mips)
ENTRY(_start)

SECTIONS
{
  . = 0x88400000;

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
