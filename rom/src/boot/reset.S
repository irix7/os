/*
 * reset.S -- stackless reset path for the Indy ROM.
 *
 * Execution starts at 0xBFC00000 (kseg1 alias of the PROM at physical
 * 0x1FC00000).  DRAM is not mapped yet, so this phase uses registers only:
 *   1. establish the reset CP0 state and an uncached TLB window at
 *      0x20000000 for the MC's bank-probe address;
 *   2. size and map the DRAM banks (memsize.S);
 *   3. test every byte of DRAM (memtest.S) -- nothing of ours lives there yet;
 *   4. only now create the writable world: stack, .data copy, .bss clear;
 *   5. enter Rust with RAM known good.
 */

.set noreorder
.set mips3

.section .text._start, "ax", @progbits
.globl _start
.ent _start
_start:
    /* CP0: interrupts off, kernel mode, BEV on for the boot vectors. */
    mtc0    $0, $13                  /* Cause = 0 */
    li      $8, 0x10000000           /* CU0, IE off, KSU=0 */
    mtc0    $8, $12                  /* Status */
    mtc0    $0, $5                   /* PageMask = 0 */
    nop

    /* Invalidate the whole TLB (48 entries on R4000). */
    li      $8, 48
    move    $9, $0
1:
    mtc0    $9, $0                   /* Index */
    mtc0    $0, $10                  /* EntryHi = 0 */
    mtc0    $0, $2                   /* EntryLo0 = 0 */
    mtc0    $0, $3                   /* EntryLo1 = 0 */
    tlbwi
    addiu   $9, $9, 1
    bne     $9, $8, 1b
    nop

    /*
     * Map VA 0x20000000..0x40000000 to the same physical range, uncached.
     * memsize.S places each bank at physical 0x20000000 while probing it, and
     * physical 0x20000000 is outside the kseg windows, so it needs a TLB.
     */
    li      $8, 0x01ffe000           /* 32MB entry (2 x 16MB pages) */
    mtc0    $8, $5                   /* PageMask */
    move    $9, $0
    li      $10, 0x20000000
2:
    mtc0    $9, $0                   /* Index */
    mtc0    $10, $10                 /* EntryHi */
    srl     $11, $10, 6
    ori     $11, $11, 0x17           /* PFN | uncached | D | V | G */
    mtc0    $11, $2                  /* EntryLo0 */
    li      $12, 0x40000
    addu    $12, $11, $12
    mtc0    $12, $3                  /* EntryLo1 */
    tlbwi
    addiu   $9, $9, 1
    li      $12, 0x2000000
    addu    $10, $10, $12
    li      $12, 16
    bne     $9, $12, 2b
    nop

    /* Early SCC console: DUART1B at 0xbfbd9830 (iris tty1 / port 8881). */
    li      $8, 0xbfbd9830
    li      $9, 0xc0                 /* WR9: hardware reset */
    sb      $9, 0($8)
    li      $9, 4
    sb      $9, 0($8)
    li      $9, 0x44                 /* WR4: x16 clock, 1 stop */
    sb      $9, 0($8)
    li      $9, 3
    sb      $9, 0($8)
    li      $9, 0xc0                 /* WR3: 8-bit Rx */
    sb      $9, 0($8)
    li      $9, 5
    sb      $9, 0($8)
    li      $9, 0x28                 /* WR5: 8-bit Tx, RTS+DTR */
    sb      $9, 0($8)
    li      $9, 11
    sb      $9, 0($8)
    li      $9, 0x50                 /* WR11: Rx/Tx clock <- BRG */
    sb      $9, 0($8)
    li      $9, 12
    sb      $9, 0($8)
    li      $9, 0x0a                 /* WR12: 9600 BRG low */
    sb      $9, 0($8)
    li      $9, 13
    sb      $9, 0($8)
    li      $9, 0x00                 /* WR13: BRG high */
    sb      $9, 0($8)
    li      $9, 14
    sb      $9, 0($8)
    li      $9, 0x01                 /* WR14: BRG enable */
    sb      $9, 0($8)
    li      $9, 3
    sb      $9, 0($8)
    li      $9, 0xc1                 /* WR3: Rx enable */
    sb      $9, 0($8)
    li      $9, 5
    sb      $9, 0($8)
    li      $9, 0x68                 /* WR5: Tx enable */
    sb      $9, 0($8)

    /* Size/map DRAM (no stack), then test every byte of it (no stack). */
    jal     mem_size_and_map
    nop
    move    $20, $2                  /* s4 = total RAM bytes */

    li      $4, 0xa8000000           /* kseg1 alias of physical 0x08000000 */
    move    $5, $20
    jal     mem_test_all
    nop
    li      $8, 0xa8000400
    sw      $2, 0($8)                /* first failure (0 = pass) */
    sw      $20, 4($8)               /* total bytes */

    /*
     * RAM is now known good.  Build the writable world at 7 MiB, matching the
     * original PROM (BSSADDR 0xA8700000, PROM_STACK 8 MiB).  The low 512 KiB
     * is a special alias window and is deliberately left alone.
     */
    li      $29, 0xa8800000

    la      $8, __data_load
    la      $9, __data_start
    la      $10, __data_end
3:
    sltu    $11, $9, $10
    beq     $11, $0, 4f
    nop
    lw      $12, 0($8)
    sw      $12, 0($9)
    addiu   $8, $8, 4
    addiu   $9, $9, 4
    b       3b
    nop
4:
    la      $8, __bss_start
    la      $9, __bss_end
5:
    sltu    $11, $8, $9
    beq     $11, $0, 6f
    nop
    sw      $0, 0($8)
    addiu   $8, $8, 4
    b       5b
    nop
6:
    jal     rom_main
    nop
7:
    b       7b
    nop
.end _start
