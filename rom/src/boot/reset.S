/*
 * reset.S -- Minimal SGI Indy ARCS PROM replacement (pure MIPS3 ASM).
 *
 * Linked at 0xBFC00000 (kseg1 PROM window).  Boot flow:
 *   1. CP0 init: interrupts off, BEV=1, TLB invalidated
 *   2. Map a probe window (VA 0x20000000..0x40000000 -> physical, uncached)
 *      so the DRAM banks can be sized and mapped
 *   3. Size and map the DRAM banks (memsize.S), then test every byte
 *      (memtest.S) -- still stackless, nothing of ours lives in DRAM yet
 *   4. Initialise the DUART1B serial console (0xBFBD9830)
 *   5. Network stage (Rust): DHCP, then download the kernel over HTTPS to
 *      physical 0x08040000 through its kseg1 alias 0xA8040000 (uncached, so the
 *      bytes are in memory before the kernel runs from kseg0 0x88040000)
 *   6. Flush the caches and jump to the kernel entry at 0x88040000; halt with
 *      a marker when the download fails
 *
 * Everything the machine does after step 6 lives in the kernel, including the
 * telnet console and networking.
 *
 * No stack exists during this phase; all state is in registers.
 */

.set noreorder
.set mips3

# I/O addresses (kseg1, uncached)
.set DUART_BASE,        0xBFBD9830

# Kernel image destinations
.set KERNEL_ENTRY,      0x88040000   /* kseg0 cached entry point */
.set KERNEL_STACK_TOP,  0x883f0000   /* matches the kernel's __stack_top; the 1 MiB
                                         below the module area at 0x88400000 belongs
                                         to the kernel stack (issue #271) */

.section .text._start, "ax", @progbits
.align 4

.globl _start
.ent _start
_start:
    # CP0: disable interrupts, kernel mode, BEV=1 for boot vectors
    mtc0    $zero, $13               /* Cause = 0 */
    li      $t1, 0x10000000          /* CU0, IE=0, KSU=0, BEV=1 */
    mtc0    $t1, $12                 /* Status */

    # Invalidate entire TLB (48 entries on R4000)
    li      $t1, 48
    move    $v0, $zero
1:
    mtc0    $v0, $0                  /* Index */
    mtc0    $zero, $10               /* EntryHi = 0 */
    mtc0    $zero, $2                /* EntryLo0 = 0 */
    mtc0    $zero, $3                /* EntryLo1 = 0 */
    tlbwi
    addiu   $v0, $v0, 1
    bne     $v0, $t1, 1b
    nop

    # Map VA 0x20000000..0x40000000 to the same physical range, uncached.
    # memsize.S probes each DRAM bank at physical 0x20000000, which is
    # outside the kseg windows and so needs a TLB entry of its own.
    li      $t1, 0x01FFE000           /* 32MB even/odd page pair */
    mtc0    $t1, $5                  /* PageMask */
    move    $t1, $zero
    li      $t2, 0x20000000
2:
    mtc0    $t1, $0                  /* Index */
    mtc0    $t2, $10                 /* EntryHi */
    srl     $t3, $t2, 6
    ori     $t3, $t3, 0x17           /* PFN | uncached | D | V | G */
    mtc0    $t3, $2                  /* EntryLo0 */
    li      $t4, 0x40000
    addu    $t4, $t3, $t4
    mtc0    $t4, $3                  /* EntryLo1 */
    tlbwi
    addiu   $t1, $t1, 1
    li      $t4, 0x2000000
    addu    $t2, $t2, $t4
    li      $t4, 16
    bne     $t1, $t4, 2b
    nop

    # Serial console (needed by memtest.S to report progress)
    jal     serial_init
    nop
    la      $a0, msg_booting
    jal     serial_puts
    nop

    # Size and map the DRAM banks; returns total RAM bytes in $v0.
    jal     mem_size_and_map
    nop
    move    $s4, $v0                  /* s4 = total RAM bytes */

    # Test every byte of DRAM.  Still stackless: nothing of ours is in DRAM
    # yet, so the test may scribble over the whole range.
    li      $a0, 0xA8000000          /* kseg1 alias of physical 0x08000000 */
    move    $a1, $s4
    jal     mem_test_all
    nop
    bne     $v0, $zero, ram_fail
    nop

    # Network stage (Rust): Seeq up, DHCP lease, kernel download.  It runs on a
    # stack at the top of the tested RAM and returns $v0 = 0 once the kernel is
    # in RAM.  The callee-saved $s4 (RAM size) survives the call.
    li      $sp, 0xA8A00000
    jal     rom_netstage
    nop
    bne     $v0, $zero, kernel_fail
    nop

    # The kernel was written through the uncached alias, but the RAM test may
    # have left stale lines in the data and instruction caches.  Start the
    # kernel with cold caches so its .data and text are read from memory.
    jal     cache_flush_all
    nop

    # Enter the kernel at its cached kseg0 address.
    #   $a0 = BootInfo physical address (0: none yet)
    #   $a1 = total RAM bytes
    #   $a2 = RAM base physical address
    move    $a0, $zero
    move    $a1, $s4                 /* total RAM bytes from mem_size_and_map */
    li      $a2, 0x08000000          /* RAM base (physical) */
    li      $sp, KERNEL_STACK_TOP    /* kernel stack */
    li      $t9, KERNEL_ENTRY
    jr      $t9
    nop

/*
 * cache_flush_all -- write back and invalidate the whole L1 data cache and
 * invalidate the whole L1 instruction cache.
 *
 * The R4400 has 16 KiB caches with 16-byte lines.  Index operations select a
 * line from the address bits, so walking 0x80000000..0x80004000 covers every
 * set (and both ways when the way bit is part of the index).
 *
 * Clobbers $8 and $9.
 */
cache_flush_all:
    li      $8, 0x80000000
    li      $9, 0x80004000
1:
    cache   0x08, 0($8)             /* Index Writeback Invalidate (D) */
    addu    $8, $8, 16
    blt     $8, $9, 1b
    nop
    li      $8, 0x80000000
    li      $9, 0x80004000
2:
    cache   0x00, 0($8)             /* Index Invalidate (I) */
    addu    $8, $8, 16
    blt     $8, $9, 2b
    nop
    sync
    jr      $ra
    nop

ram_fail:
    la      $a0, msg_ramfail
    jal     serial_puts
    nop
1:
    b       1b
    nop

kernel_fail:
    la      $a0, msg_kernelfail
    jal     serial_puts
    nop
1:
    b       1b
    nop

.end _start

# ------------------------------------------------------------
# serial_init -- configure DUART1B for 9600 8N1
# Preserves $ra in $s0 because duart_wr (called via jal) clobbers it.
# ------------------------------------------------------------
.ent serial_init
serial_init:
    move    $s0, $ra

    li      $t0, 0xC0                /* WR9: hardware reset */
    li      $t1, 9
    jal     duart_wr
    nop

    li      $t0, 0x44                /* WR4: x16 clock, 1 stop, no parity */
    li      $t1, 4
    jal     duart_wr
    nop

    li      $t0, 0xC0                /* WR3: 8 bits/char, RX disabled */
    li      $t1, 3
    jal     duart_wr
    nop

    li      $t0, 0x28                /* WR5: 8 bits/char, TX disabled */
    li      $t1, 5
    jal     duart_wr
    nop

    li      $t0, 0x50                /* WR11: Rx/Tx clock <- BRG */
    li      $t1, 11
    jal     duart_wr
    nop

    li      $t0, 10                  /* WR12: 9600 BRG low */
    li      $t1, 12
    jal     duart_wr
    nop

    li      $t0, 0                  /* WR13: BRG high */
    li      $t1, 13
    jal     duart_wr
    nop

    li      $t0, 1                  /* WR14: BRG enable */
    li      $t1, 14
    jal     duart_wr
    nop

    li      $t0, 0xC1               /* WR3: Rx enable */
    li      $t1, 3
    jal     duart_wr
    nop

    li      $t0, 0x68               /* WR5: Tx enable */
    li      $t1, 5
    jal     duart_wr
    nop

    jr      $s0
    nop

.end serial_init

# duart_wr -- write register $t1 with value $t0 (Z85C30 indirect access)
.ent duart_wr
duart_wr:
    li      $t3, DUART_BASE
    sb      $t1, 0($t3)             /* select register */
    sb      $t0, 0($t3)             /* write data */
    jr      $ra
    nop

.end duart_wr

# serial_puts -- print null-terminated string in $a0
# Preserves $ra in $s0 because serial_putc (called via jal) clobbers it.
.ent serial_puts
serial_puts:
    move    $s0, $ra
    move    $t0, $a0
1:
    lbu     $t1, 0($t0)
    beqz    $t1, 2f
    nop
    move    $a0, $t1
    jal     serial_putc
    nop
    addiu   $t0, $t0, 1
    b       1b
    nop
2:
    jr      $s0
    nop

.end serial_puts

# serial_putc -- send byte in $a0
.globl serial_putc
.ent serial_putc
serial_putc:
    li      $t1, DUART_BASE
1:
    li      $t3, 0
    sb      $t3, 0($t1)             /* select RR0 */
    lbu     $t2, 0($t1)
    andi    $t2, $t2, 0x04          /* TX buffer empty? */
    beqz    $t2, 1b
    nop
    li      $t3, 0x08
    sb      $t3, 0($t1)             /* select data register */
    sb      $a0, 0($t1)
    jr      $ra
    nop

.end serial_putc


# ------------------------------------------------------------
# Data
# ------------------------------------------------------------

msg_booting:
    .asciz "\r\nROM: ARCS PROM replacement booting\r\n"

msg_ramfail:
    .asciz "ROM: RAM TEST FAILED\r\n"

msg_kernelfail:
    .asciz "ROM: BOOT-NOT-READY: no kernel in RAM, halted\r\n"
