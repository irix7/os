/* Linker script for the relibc-linked C probe.
 *
 * The kernel's ELF loader maps PT_LOAD segments at the fixed 0x1000_0000 user
 * base and caps the image at 256 KiB.  --gc-sections is mandatory: librelibc.a
 * is 5.6 MiB unstripped and a full link does not fit the slot.
 */
OUTPUT_FORMAT("elf32-ntradbigmips")
OUTPUT_ARCH(mips)
ENTRY(_start)

SECTIONS
{
  . = 0x10000000;

  .text : {
    *(.text._start)
    *(.text)
    *(.text.*)
  }

  .rodata : {
    *(.rodata)
    *(.rodata.*)
  }

  .preinit_array : {
    __preinit_array_start = .;
    KEEP(*(.preinit_array))
    __preinit_array_end = .;
  }

  .init_array : {
    __init_array_start = .;
    KEEP(*(SORT_BY_INIT_PRIORITY(.init_array.*)))
    KEEP(*(.init_array))
    __init_array_end = .;
  }

  .fini_array : {
    __fini_array_start = .;
    KEEP(*(SORT_BY_INIT_PRIORITY(.fini_array.*)))
    KEEP(*(.fini_array))
    __fini_array_end = .;
  }

  .data : {
    *(.data)
    *(.data.*)
    *(.sdata)
    *(.sdata.*)
    *(.got)
    *(.got.*)
  }

  .bss (NOLOAD) : {
    *(.bss)
    *(.bss.*)
    *(.sbss)
    *(.sbss.*)
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
