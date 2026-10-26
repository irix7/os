/*
 * Flat loadable-module image at arena region 2 (0x88540000).
 * The kernel copies the body there and the manager enters the manifest offset.
 */

OUTPUT_FORMAT("elf32-ntradbigmips")
OUTPUT_ARCH(mips)
ENTRY(_start)

SECTIONS
{
  . = 0x88540000;

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

  . = ALIGN(8);
  __m2_mesh_exports = .;
  .m2_exports : {
    KEEP(*(.m2_exports))
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
