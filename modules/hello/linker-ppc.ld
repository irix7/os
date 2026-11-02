/*
 * Flat loadable-module image for the PowerPC G3 machine, linked at the
 * physical module slot 0x03000000.  The kernel copies the body there and calls
 * offset 0, so the entry must stay at the start of the image.
 */

OUTPUT_FORMAT("elf32-powerpc")
OUTPUT_ARCH(powerpc)
ENTRY(_start)

SECTIONS
{
  . = 0x03000000;

  .text : ALIGN(16) {
    KEEP(*(.text._start))
    *(.text)
    *(.text.*)
  }

  .rodata : ALIGN(16) {
    *(.rodata)
    *(.rodata.*)
    *(.srodata)
    *(.srodata.*)
    *(.sdata2)
    *(.sdata2.*)
  }

  .data : ALIGN(16) {
    *(.data)
    *(.data.*)
    *(.sdata)
    *(.sdata.*)
    *(.got)
    *(.got.*)
    *(.toc)
    *(.toc.*)
  }

  .bss (NOLOAD) : ALIGN(16) {
    *(.bss)
    *(.bss.*)
    *(.sbss)
    *(.sbss.*)
    *(COMMON)
  }

  /DISCARD/ : {
    *(.comment)
    *(.eh_frame)
    *(.gnu.attributes)
    *(.note .note.*)
    *(.debug*)
  }

  ASSERT(. <= 0x03200000, "module image exceeds the 2 MiB PowerPC slot")
}
