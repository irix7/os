/*
 * IRIX 7 x86_64 kernel image.
 *
 * Linked at physical 0x100000 (1 MiB) for QEMU `-kernel` / Multiboot2 boot.
 * The Multiboot2 header lives in .multiboot2.header within the first 8 KiB so
 * the firmware/boot loader can find it.  Text, rodata and data form one
 * contiguous load image; .bss follows and is zeroed before entering Rust.
 */

OUTPUT_FORMAT("elf64-x86-64")
OUTPUT_ARCH(i386:x86-64)
ENTRY(_start)

SECTIONS
{
  /* The Multiboot2 header must be within the first 8 KiB of the image. */
  . = 0x100000;

  .multiboot2.header : {
    *(.multiboot2.header)
  }

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
    . = ALIGN(8);
    __data_end = .;
  }

  .bss (NOLOAD) : {
    __bss_start = .;
    *(.bss)
    *(.bss.*)
    *(COMMON)
    . = ALIGN(8);
    __bss_end = .;
  }

  /* Boot stack, above the image. */
  __stack_top = 0x10_0000 + 0x200000; /* 2 MiB above the image, conservative */

  /DISCARD/ : {
    *(.eh_frame)
    *(.comment)
    *(.gnu.attributes)
  }
}
