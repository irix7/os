/*
 * memtest.S -- full RAM test for the SGI Indy POST.
 *
 * Must run BEFORE any stack is placed in DRAM (it overwrites every byte it
 * tests), i.e. from the same stackless phase as memsize.S.  Tests the whole
 * contiguous low-memory range one megabyte at a time, printing a dot per MB
 * on the ttyd1 serial console.
 *
 * mem_test_all($4 = base, $5 = total bytes) -> $v0 = 0 on success, else the
 * first failing address.  Clobbers $8..$15, $16..$18, $2, $4.
 */

.set noreorder
.set mips3

.equ SCC_CMD, 0xbfbd9830

    .text
    .globl mem_test_all
    .ent mem_test_all
mem_test_all:
    move    $23, $ra
    move    $16, $4                  /* s0 = base                    */
    addu    $17, $4, $5              /* s1 = end                     */
    move    $18, $4                  /* s2 = cursor                  */

    /* header */
    la      $4, mt_hdr
    jal     mt_puts
    nop

mt_mb:
    li      $9, 0x100000
    addu    $10, $18, $9             /* t2 = end of this MB          */
    sltu    $12, $17, $10            /* end < mb_end ?               */
    beq     $12, $0, 1f
    nop
    move    $10, $17                 /* cap at end                   */
1:
    /* pass 1: each word holds its own address */
    move    $11, $18
p1w:
    sw      $11, 0($11)
    addiu   $11, $11, 4
    bltu    $11, $10, p1w
    nop
    move    $11, $18
p1r:
    lw      $12, 0($11)
    bne     $12, $11, mt_fail
    nop
    addiu   $11, $11, 4
    bltu    $11, $10, p1r
    nop

    /* pass 2: each word holds the complement of its address */
    move    $11, $18
p2w:
    nor     $12, $11, $0
    sw      $12, 0($11)
    addiu   $11, $11, 4
    bltu    $11, $10, p2w
    nop
    move    $11, $18
p2r:
    lw      $12, 0($11)
    nor     $13, $11, $0
    bne     $12, $13, mt_fail
    nop
    addiu   $11, $11, 4
    bltu    $11, $10, p2r
    nop

    li      $4, '.'
    jal     mt_putc
    nop
    move    $18, $10
    bltu    $18, $17, mt_mb
    nop

    li      $4, '\n'
    jal     mt_putc
    nop
    move    $2, $0
    move    $ra, $23
    jr      $ra
    nop
mt_fail:
    move    $2, $11
    move    $ra, $23
    jr      $ra
    nop

mt_puts:
    move    $6, $ra
1:
    lb      $7, 0($4)
    beq     $7, $0, 2f
    nop
    move    $21, $4
    move    $4, $7
    jal     mt_putc
    nop
    move    $4, $21
    addiu   $4, $4, 1
    b       1b
    nop
2:
    move    $ra, $6
    jr      $ra
    nop

mt_putc:
    li      $14, SCC_CMD
1:
    sb      $0, 0($14)
    lb      $15, 0($14)
    andi    $15, $15, 0x04
    beq     $15, $0, 1b
    nop
    li      $15, 0x08
    sb      $15, 0($14)
    sb      $4, 0($14)
    jr      $ra
    nop
    .end mem_test_all

    .section .rodata
mt_hdr:
    .asciz "Testing RAM: "
