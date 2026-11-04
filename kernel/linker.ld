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
  /* Main kernel image at kseg0 0x88040000 */
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

  /* Kernel entry stack, above the image and below the module area. */
  /* Boot stack top.
   *
   * The stack grows down from here into the reserved low window; `__bss_end`
   * is its floor.  The gap is load-bearing: a Wasm policy boot reaches roughly
   * 530 KiB of native stack inside wasmi's handlers, so a 0x8830_0000 top left
   * only a few kilobytes of margin once the module ABI grew, and the stack
   * crossed into .bss and overwrote live frames (see issue #271 and
   * docs/adr/m3-0006-udp-transport.md).  The 1 MiB between 0x8830_0000 and the
   * module area at 0x8840_0000 is reserved and otherwise unused, so the top
   * sits just below the module area and the stack gets the whole window. */
  /* The Wasm policy guest runs on its own 832 KiB stack directly above .bss
     (ADR M1-0003).  Its stack grows down from __guest_stack_top, so the
     guest's floor is __guest_stack_base.  The kernel boot stack sits above it
     and gets 1 MiB, with its own use peaking at about 37 KiB (issue #272). */
  .guest_stack (NOLOAD) : {
    . = ALIGN(0x1000);
    __guest_stack_base = .;
    . = . + 0x000D0000;
    . = ALIGN(0x1000);
    __guest_stack_top = .;
  }

  __stack_top = __guest_stack_top + 0x00100000;

  /* A boot stack that grows into .bss corrupts statics.  Keep a real gap. */
  ASSERT(__bss_end < __guest_stack_base, "kernel .bss overlaps the guest stack")
  ASSERT(__stack_top <= 0x88600000, "kernel stack overlaps the Seeq DMA area")

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
