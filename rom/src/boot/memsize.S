/*
 * memsize.S -- SGI Indy (IP24) memory sizing and mapping.
 *
 * A port of stand/arcs/IP22prom/lmem_conf.s (the upstream PROM source's
 * IP22 name).  Runs with NO stack: RAM is unmapped until MEMCFG0/1 are
 * programmed, so there is nowhere to put one.
 * Only registers are used, exactly as the original firmware does.
 *
 * Each bank is probed through the KSEG1 alias of physical HIGH_MEM_BASE
 * (0x20000000 -> 0xa0000000), so no TLB entries are needed.
 *
 * Returns total bytes of RAM in $v0.
 *
 * Register conventions for the whole file:
 *   $s0 mc_rev   $s1 unitshift   $s2 maxsize   $s3 unitsize   $s4 probe_memcfg
 *   $s5 current bank   $s7 saved $ra
 * Helper calls preserve $s*, and all of $t0..$t9 except $24/$25 where noted.
 * set_bank_conf / get_bank_conf only clobber $1,$2,$3,$6,$7.
 */

.set noreorder
.set mips3

.equ MEMCFG0, 0xbfa000c4
.equ MEMCFG1, 0xbfa000cc
.equ MC_SYSID, 0xbfa0001c
.equ MC_CPUCTRL0, 0xbfa00004

.equ MEMCFG_VLD, 0x2000
.equ MEMCFG_BNK, 0x4000
.equ MEMCFG_ADDR_MASK, 0x00ff
.equ MEMCFG_DRAM_MASK, 0x1f00

.equ HIGH_MEM_ALIAS, 0x20000000
.equ PHYS_RAMBASE, 0x08000000
.equ HIGH_MEM_BASE, 0x20000000
.equ LOW_MEM_TOP, 0x18000000
.equ MAXUNITS, 32

    .text
    .globl mem_size_and_map
    .ent mem_size_and_map
mem_size_and_map:
    move    $23, $ra                 /* s7 = ra                       */

    li      $8, MC_SYSID
    lw      $16, 0($8)
    andi    $16, $16, 0x0f           /* s0 = mc_rev                   */
    li      $17, 22
    li      $18, 0x08000000
    li      $19, 0x00400000
    li      $20, 0x3f80
    li      $8, 5
    blt     $16, $8, setup_done
    nop
    li      $17, 24
    li      $18, 0x20000000
    li      $19, 0x01000000
    li      $20, 0x3f20
setup_done:
    li      $8, MEMCFG0
    sw      $0, 0($8)
    li      $8, MEMCFG1
    sw      $0, 0($8)

    move    $21, $0                  /* s5 = bank 0                   */
bank_loop:
    move    $4, $21
    move    $5, $20
    jal     set_bank_conf            /* map bank with probe config    */
    nop

    move    $4, $21
    jal     size_bank                /* v0 = units, v1 = badsimm      */
    nop
    move    $24, $2                  /* t8 = size units               */

    move    $4, $21
    move    $5, $24
    jal     set_bank_size            /* encode size field             */
    nop

    addiu   $21, $21, 1
    li      $8, 4
    bne     $21, $8, bank_loop
    nop

    jal     set_bank_addrs
    nop
    jal     szmem
    nop

    move    $ra, $23
    jr      $ra
    nop
    .end mem_size_and_map


/* set_bank_conf(bank=$a0, value=$a1). clobbers $1,$2,$3,$6,$7 */
    .globl set_bank_conf
    .ent set_bank_conf
set_bank_conf:
    li      $2, MEMCFG0
    li      $3, 2
    blt     $4, $3, 1f
    nop
    li      $2, MEMCFG1
1:
    lw      $8, 0($2)
    andi    $3, $4, 1
    bne     $3, $0, 2f
    nop
    andi    $8, $8, 0xffff
    sll     $6, $5, 16
    or      $8, $8, $6
    b       3f
    nop
2:
    lui     $7, 0xffff
    and     $8, $8, $7
    andi    $6, $5, 0xffff
    or      $8, $8, $6
3:
    sw      $8, 0($2)
    li      $7, MC_CPUCTRL0
    lw      $7, 0($7)
    jr      $ra
    nop
    .end set_bank_conf


/* get_bank_conf(bank=$a0) -> $v0. clobbers $2,$3 */
    .globl get_bank_conf
    .ent get_bank_conf
get_bank_conf:
    li      $2, MEMCFG0
    li      $3, 2
    blt     $4, $3, 1f
    nop
    li      $2, MEMCFG1
1:
    lw      $2, 0($2)
    andi    $3, $4, 1
    bne     $3, $0, 2f
    nop
    srl     $2, $2, 16
2:
    andi    $2, $2, 0xffff
    jr      $ra
    nop
    .end get_bank_conf


/* set_bank_size(bank=$a0, size units=$a1) -> $v0 encoded half */
    .globl set_bank_size
    .ent set_bank_size
set_bank_size:
    move    $25, $ra
    blez    $5, sbs_zero
    nop
    andi    $14, $5, 0x2a
    move    $15, $0
    beq     $14, $0, 1f
    nop
    li      $15, MEMCFG_BNK
1:
    addiu   $24, $5, -1
    sll     $24, $24, 8
    or      $24, $24, $15
    or      $5, $24, 0xff
    jal     set_bank_conf
    nop
    move    $2, $24
    move    $ra, $25
    jr      $ra
    nop
sbs_zero:
    move    $5, $0
    jal     set_bank_conf
    nop
    move    $2, $0
    move    $ra, $25
    jr      $ra
    nop
    .end set_bank_size


/* size_bank(bank=$a0) -> $v0 size units, $v1 bad SIMM code */
    .globl size_bank
    .ent size_bank
size_bank:
    move    $25, $ra
    li      $8, HIGH_MEM_ALIAS
    li      $9, 0x11111111
    sw      $9, 0($8)
    sll     $10, $9, 1
    sw      $10, 4($8)
    sll     $10, $10, 1
    sw      $10, 8($8)
    sll     $10, $10, 1
    sw      $10, 12($8)

    move    $3, $0
    sw      $0, 16($8)
    lw      $11, 0($8)
    li      $12, 0x11111111
    beq     $11, $12, sb_c1
    nop
    ori     $3, $3, 0x8000
sb_c1:
    sw      $0, 20($8)
    lw      $11, 4($8)
    li      $12, 0x22222222
    beq     $11, $12, sb_c2
    nop
    ori     $3, $3, 0x4000
sb_c2:
    sw      $0, 24($8)
    lw      $11, 8($8)
    li      $12, 0x44444444
    beq     $11, $12, sb_c3
    nop
    ori     $3, $3, 0x2000
sb_c3:
    sw      $0, 28($8)
    lw      $11, 12($8)
    li      $12, 0x88888888
    beq     $11, $12, sb_c4
    nop
    ori     $3, $3, 0x1000
sb_c4:
    beq     $3, $0, sb_all
    nop
    move    $2, $0
    move    $ra, $25
    jr      $ra
    nop

sb_all:
    srl     $14, $18, 1              /* t6 = maxsize/2                */
    move    $2, $19
    sw      $0, 0($8)
    sw      $0, 4($8)
    sw      $0, 8($8)
    sw      $0, 12($8)
sb_loop:
    addu    $15, $8, $2
    li      $9, 0x11111111
    sw      $9, 0($15)
    sll     $10, $9, 1
    sw      $10, 4($15)
    sll     $10, $10, 1
    sw      $10, 8($15)
    sll     $10, $10, 1
    sw      $10, 12($15)

    move    $3, $0
    lw      $11, 0($8)
    beq     $11, $0, sb_z1
    nop
    ori     $3, $3, 0x8000
sb_z1:
    lw      $11, 4($8)
    beq     $11, $0, sb_z2
    nop
    ori     $3, $3, 0x4000
sb_z2:
    lw      $11, 8($8)
    beq     $11, $0, sb_z3
    nop
    ori     $3, $3, 0x2000
sb_z3:
    lw      $11, 12($8)
    beq     $11, $0, sb_z4
    nop
    ori     $3, $3, 0x1000
sb_z4:
    bne     $3, $0, sb_alias
    nop
    sll     $2, $2, 2
    bne     $2, $14, sb_loop
    nop
    b       sb_subbank
    nop

sb_alias:
    li      $12, 0xf000
    beq     $3, $12, sb_subbank
    nop
    srlv    $2, $2, $17
    move    $ra, $25
    jr      $ra
    nop

sb_subbank:
    move    $14, $2
    move    $24, $20
    ori     $24, $24, MEMCFG_BNK
    move    $4, $21
    move    $5, $24
    jal     set_bank_conf
    nop
    li      $8, HIGH_MEM_ALIAS
    srl     $9, $18, 1
    addu    $8, $8, $9
    li      $9, 0x11111111
    sw      $9, 0($8)
    sll     $10, $9, 1
    sw      $10, 4($8)
    sll     $10, $10, 1
    sw      $10, 8($8)
    sll     $10, $10, 1
    sw      $10, 12($8)
    move    $3, $0
    sw      $0, 16($8)
    lw      $11, 0($8)
    li      $12, 0x11111111
    beq     $11, $12, sb_d1
    nop
    ori     $3, $3, 0x8000
sb_d1:
    sw      $0, 20($8)
    lw      $11, 4($8)
    li      $12, 0x22222222
    beq     $11, $12, sb_d2
    nop
    ori     $3, $3, 0x4000
sb_d2:
    sw      $0, 24($8)
    lw      $11, 8($8)
    li      $12, 0x44444444
    beq     $11, $12, sb_d3
    nop
    ori     $3, $3, 0x2000
sb_d3:
    sw      $0, 28($8)
    lw      $11, 12($8)
    li      $12, 0x88888888
    beq     $11, $12, sb_d4
    nop
    ori     $3, $3, 0x1000
sb_d4:
    bne     $3, $0, sb_onesub
    nop
    addiu   $15, $17, -1
    srlv    $2, $14, $15
    move    $ra, $25
    jr      $ra
    nop
sb_onesub:
    move    $4, $21
    move    $5, $20
    jal     set_bank_conf
    nop
    srlv    $2, $14, $17
    move    $ra, $25
    jr      $ra
    nop
    .end size_bank


/* set_bank_addrs -- assign bases largest-first, low memory then high. */
    .globl set_bank_addrs
    .ent set_bank_addrs
set_bank_addrs:
    move    $25, $ra
    li      $14, 4                   /* t6 = banks remaining          */
    li      $9, PHYS_RAMBASE
    srlv    $9, $9, $17              /* t1 = low base (units)         */
    li      $12, HIGH_MEM_BASE
    srlv    $12, $12, $17            /* t4 = high base (units)        */
    li      $13, LOW_MEM_TOP
    srlv    $13, $13, $17            /* t5 = low top (units)          */
    li      $24, MAXUNITS            /* t8 = current size (units)     */
sba_size:
    addiu   $10, $24, -1             /* t2 = size-1                   */
    move    $11, $0                  /* t3 = bank                     */
sba_bank:
    move    $4, $11
    jal     get_bank_conf
    nop
    beq     $2, $0, sba_next
    nop
    and     $15, $2, MEMCFG_DRAM_MASK
    srl     $15, $15, 8              /* t7 = size field               */
    bne     $15, $10, sba_next
    nop
    ori     $2, $2, MEMCFG_VLD
    li      $8, MEMCFG_ADDR_MASK
    not     $8, $8
    and     $5, $2, $8               /* a1 = v0 & ~ADDR_MASK          */
    li      $8, 5
    blt     $16, $8, sba_low         /* mc_rev < 5 -> low             */
    nop
    li      $8, 0xf
    blt     $15, $8, sba_low
    nop
    or      $5, $5, $12
    move    $4, $11
    jal     set_bank_conf
    nop
    addu    $12, $12, $24
    b       sba_dec
    nop
sba_low:
    or      $5, $5, $9
    move    $4, $11
    jal     set_bank_conf
    nop
    addu    $9, $9, $24
    bne     $9, $13, sba_dec
    nop
    move    $9, $12
sba_dec:
    subu    $14, $14, 1
    beq     $14, $0, sba_done
    nop
sba_next:
    addiu   $11, $11, 1
    li      $8, 4
    bne     $11, $8, sba_bank
    nop
    srl     $24, $24, 1
    bne     $24, $0, sba_size
    nop
sba_done:
    move    $ra, $25
    jr      $ra
    nop
    .end set_bank_addrs


/* szmem -> $v0 total bytes */
    .globl szmem
    .ent szmem
szmem:
    move    $25, $ra
    move    $2, $0
    li      $8, MEMCFG0
    li      $9, MEMCFG1
    lw      $10, 0($8)
    lw      $11, 0($9)
    srl     $12, $10, 16
    jal     sz_add
    nop
    andi    $12, $10, 0xffff
    jal     sz_add
    nop
    srl     $12, $11, 16
    jal     sz_add
    nop
    andi    $12, $11, 0xffff
    jal     sz_add
    nop
    move    $ra, $25
    jr      $ra
    nop

sz_add:
    andi    $13, $12, MEMCFG_VLD
    beq     $13, $0, sz_ret
    nop
    andi    $12, $12, MEMCFG_DRAM_MASK
    srl     $12, $12, 8
    addiu   $12, $12, 1
    sllv    $12, $12, $17
    addu    $2, $2, $12
sz_ret:
    jr      $ra
    nop
    .end szmem
