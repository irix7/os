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

  /* Boot slot A is 512 KiB at 0x8840_0000; slot B starts at 0x8848_0000.
     The flat image does not contain .bss, but the loader maps the whole slot,
     so image plus .bss must stay below the next slot.  A silent overflow would
     overwrite the second boot module. */
  __module_end = .;
  ASSERT(__module_end <= 0x88480000, "mcp module image and .bss exceed slot A")

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
