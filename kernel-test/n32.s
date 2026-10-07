	.section	.mdebug.abiN32,"",@progbits
	.nan	legacy
	.module	softfloat
	.text
	.file	"no_core.4ef6f5f479e3d344-cgu.0"
	.section	.text.load,"ax",@progbits
	.globl	load
	.p2align	3
	.type	load,@function
	.set	nomicromips
	.set	nomips16
	.ent	load
load:
	.frame	$sp,0,$ra
	.mask 	0x00000000,0
	.fmask	0x00000000,0
	.set	noreorder
	.set	nomacro
	.set	noat
	sll	$1, $4, 0
	ld	$2, 0($1)
	jr	$ra
	nop
	.set	at
	.set	macro
	.set	reorder
	.end	load
.Lfunc_end0:
	.size	load, .Lfunc_end0-load

	.section	.text.store,"ax",@progbits
	.globl	store
	.p2align	3
	.type	store,@function
	.set	nomicromips
	.set	nomips16
	.ent	store
store:
	.frame	$sp,0,$ra
	.mask 	0x00000000,0
	.fmask	0x00000000,0
	.set	noreorder
	.set	nomacro
	.set	noat
	sll	$1, $4, 0
	jr	$ra
	sw	$5, 0($1)
	.set	at
	.set	macro
	.set	reorder
	.end	store
.Lfunc_end1:
	.size	store, .Lfunc_end1-store

	.ident	"rustc version 1.101.0-nightly (db8f076d2 2026-10-03)"
	.section	".note.GNU-stack","",@progbits
	.text
