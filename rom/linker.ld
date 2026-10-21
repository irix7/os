/*
 * Linker script for the ARCS PROM replacement (n32/MIPS III).
 *
 * The image executes from the PROM chip: text and read-only data are linked
 * at the kseg1 reset window 0xBFC00000 (physical 0x1FC00000), exactly where
 * the R4000 starts after reset.  They are never copied to RAM, so the RAM
 * test can run without overwriting the code that is doing the testing.
 *
 * Writable data lives in DRAM at 7 MiB, matching the original IP22 PROM's
 * BSSADDR 0xA8700000, with a 2 MiB heap.  The stack top is 10 MiB, leaving a
 * gap above the heap.  Staying well clear of the low 512 KiB matters: that
 * window is a special alias of 0x08000000..0x0807FFFF, and the original
 * firmware never places DMA buffers or its stack there.
 *   0xA870_0000 ..              .data/.bss/heap (2 MiB)
 *   0xA8A0_0000                 initial stack top (grows down)
 * The initial contents of .data are stored in the PROM image immediately
 * after .rodata and copied to RAM by the reset stub.
 */

OUTPUT_FORMAT("elf32-ntradbigmips")   /* n32 */
OUTPUT_ARCH(mips)
ENTRY(_start)

SECTIONS
{
  /* ── Code and read-only data: PROM (kseg1, uncached) ────────────────── */
  . = 0xBFC00000;

  .text : {
    KEEP(*(.text._start))
    KEEP(*(.reset))
    KEEP(*(.reset.*))
    *(.text)
    *(.text.*)
  }

  .rodata : {
    *(.rodata)
    *(.rodata.*)
  }

  /* ── MIPS ELF required sections ─────────────────────────────────────── */
  .MIPS.abiflags : {
    *(.MIPS.abiflags)
  }
  .reginfo : {
    *(.reginfo)
  }

  /* ── Read-write data: DRAM, with its initial image in the PROM ─────── */
  . = ALIGN(0x1000);
  __data_load = .;

  .data 0xA8700000 : AT(__data_load) {
    __data_start = .;
    *(.data)
    *(.data.*)
    *(.sdata)
    *(.sdata.*)
    *(.got)
    *(.got.*)
    . = ALIGN(8);
    __data_end = .;
  }
  __data_load_end = LOADADDR(.data) + SIZEOF(.data);

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

  /* Stack top: the stack grows down from 10 MiB, above the 2 MiB heap. */
  __stack_top = 0xA8A00000;

  /DISCARD/ : {
    *(.eh_frame)
    *(.MIPS.abiflags.note)
  }
}
