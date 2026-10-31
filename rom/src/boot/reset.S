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
 *   5. Copy the kernel payload from PROM to physical 0x08040000 via its
 *      kseg1 alias 0xA8040000 (uncached, so the bytes are in memory before
 *      the kernel starts executing from the cached kseg0 alias 0x88040000)
 *   6. Jump to the kernel entry at 0x88040000
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
.set KERNEL_PHYS_KSEG1, 0xA8040000   /* physical 0x08040000, uncached */
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

    # Decompress the kernel payload from PROM to physical 0x08040000 via kseg1.
    la      $a0, rom_payload_start
    lw      $a1, ROM_PAYLOAD_LEN
    la      $a2, KERNEL_PHYS_KSEG1
    jal     lz4_decompress
    nop
    bne     $v0, $zero, lz4_fail
    nop

    la      $a0, msg_done
    jal     serial_puts
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

lz4_fail:
    la      $a0, msg_lz4fail
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
# lz4_decompress -- decompress an LZ4 block payload
#
# Payload layout: [4 bytes magic 0x184D2204][4 bytes size, big-endian][LZ4 block]
#
# Input:  $a0 = src (payload), $a1 = payload length, $a2 = dst
# Output: $v0 = 0 on success, 1 on a bad payload
#
# The LZ4 block itself is RFC 7857: a run of sequences, each a token byte
# (high nibble literal length, low nibble match length minus 4), the
# literals, a two-byte little-endian distance, and the match.  Lengths of 15
# or more continue in extra bytes, each worth 255 until one is smaller.
# The block ends when the uncompressed size has been produced; the last
# sequence is literals only.
#
# Register use: the assembler renumbers $t0-$t3, so the temporaries live in
# $a0-$a3, $v0 and $s5-$s7 instead.  $s4 belongs to the caller.
# ------------------------------------------------------------
.ent lz4_decompress
lz4_decompress:
    move    $s0, $a0                 # src
    move    $s2, $a2                 # dst

    # Magic, read a byte at a time so the payload need not be aligned.
    # Big-endian: the first byte is the most significant.
    lbu     $a0, 0($s0)
    lbu     $a1, 1($s0)
    lbu     $a2, 2($s0)
    lbu     $a3, 3($s0)
    sll     $a0, $a0, 24
    sll     $a1, $a1, 16
    sll     $a2, $a2, 8
    or      $a0, $a0, $a1
    or      $a0, $a0, $a2
    or      $a0, $a0, $a3
    li      $v0, 0x184D2204
    bne     $a0, $v0, lz4_bad
    nop
    addiu   $s0, $s0, 4

    # Uncompressed size, big-endian.
    lbu     $a0, 0($s0)
    lbu     $a1, 1($s0)
    lbu     $a2, 2($s0)
    lbu     $a3, 3($s0)
    sll     $a0, $a0, 24
    sll     $a1, $a1, 16
    sll     $a2, $a2, 8
    or      $a0, $a0, $a1
    or      $a0, $a0, $a2
    or      $a0, $a0, $a3
    move    $s3, $a0                 # bytes still to produce
    addiu   $s0, $s0, 4

lz4_loop:
    beqz    $s3, lz4_done
    nop
    lbu     $s5, 0($s0)             # token
    addiu   $s0, $s0, 1

    # Literal run length: the token's high nibble, 15 meaning "more follows".
    srl     $s6, $s5, 4
    li      $v0, 15
    bne     $s6, $v0, lz4_lit_copy
    nop
lz4_lit_len:
    lbu     $a0, 0($s0)
    addiu   $s0, $s0, 1
    addu    $s6, $s6, $a0
    li      $v0, 255
    beq     $a0, $v0, lz4_lit_len
    nop
lz4_lit_copy:
    beqz    $s6, lz4_lit_done
    nop
lz4_lit_byte:
    lbu     $a0, 0($s0)
    addiu   $s0, $s0, 1
    sb      $a0, 0($s2)
    addiu   $s2, $s2, 1
    addiu   $s3, $s3, -1
    addiu   $s6, $s6, -1
    bnez    $s6, lz4_lit_byte
    nop
lz4_lit_done:
    beqz    $s3, lz4_done           # the last sequence is literals only
    nop

    # Match distance: two bytes, little-endian.  It comes before the
    # match-length extension, so it has to be read first.
    lbu     $a0, 0($s0)
    lbu     $a1, 1($s0)
    addiu   $s0, $s0, 2
    sll     $a1, $a1, 8
    or      $a0, $a0, $a1            # $a0 = distance back

    # Match length: the token's low nibble, 15 meaning "more follows".
    andi    $s6, $s5, 0x0F
    li      $v0, 15
    bne     $s6, $v0, lz4_match_have
    nop
lz4_match_len:
    lbu     $a1, 0($s0)
    addiu   $s0, $s0, 1
    addu    $s6, $s6, $a1
    li      $v0, 255
    beq     $a1, $v0, lz4_match_len
    nop
lz4_match_have:
    addiu   $s6, $s6, 4             # total match length

    # Copy the match: the source may overlap the destination, so copy a
    # byte at a time.
    subu    $a1, $s2, $a0            # source = dst - distance
lz4_match_byte:
    lbu     $a0, 0($a1)
    sb      $a0, 0($s2)
    addiu   $a1, $a1, 1
    addiu   $s2, $s2, 1
    addiu   $s3, $s3, -1
    addiu   $s6, $s6, -1
    bnez    $s6, lz4_match_byte
    nop

    j       lz4_loop
    nop

lz4_done:
    li      $v0, 0
    jr      $ra
    nop

lz4_bad:
    li      $v0, 1
    jr      $ra
    nop

.end lz4_decompress

# ------------------------------------------------------------
# Data
# ------------------------------------------------------------

msg_booting:
    .asciz "\r\nROM: ARCS PROM replacement booting\r\n"

msg_ramfail:
    .asciz "ROM: RAM TEST FAILED\r\n"

msg_done:
    .asciz "ROM: decompressed kernel to 0x88040000, jumping...\r\n"

msg_lz4fail:
    .asciz "ROM: KERNEL IMAGE DECOMPRESSION FAILED\r\n"

# Length of the kernel payload in PROM (resolved at link time).
.align 2
.globl ROM_PAYLOAD_LEN
ROM_PAYLOAD_LEN:
    .word rom_payload_end - rom_payload_start