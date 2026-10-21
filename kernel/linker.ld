/*
 * IRIX 7 kernel image.
 *
 * The ROM copies the raw binary to KSEG0 0x88040000 (physical 0x08040000).
 * Text, rodata and data are one contiguous load image so a single memcpy of
 * the binary places everything correctly.  .bss follows in RAM and is zeroed
 * by the boot entry before entering Rust.
 */

OUTPUT_FORMAT("elf32-ntradbigmips")
OUTPUT_ARCH(mips)
ENTRY(_start)

SECTIONS
{
  . = 0x88040000;

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
    *(.sdata)
    *(.sdata.*)
    *(.got)
    *(.got.*)
    . = ALIGN(8);
    __data_end = .;
  }

  .bss (NOLOAD) : {
    __bss_start = .;
    *(.bss)
    *(.bss.*)
    *(.sbss)
    *(.sbss.*)
    *(COMMON)
    . = ALIGN(8);
    __bss_end = .;
  }

  /* Kernel entry stack, above the image and below the ROM's BootInfo. */
  __stack_top = 0x88200000;

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
