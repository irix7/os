/* SPDX-License-Identifier: GPL-3.0-only */
/*
 * x86_64 entry, from the Xen PVH 32-bit entry to long mode.
 *
 * QEMU loads this ELF through its PVH path because of the `.note.Xen`
 * descriptor below: a 64-bit ELF whose note names a 32-bit physical entry
 * point.  The CPU arrives in 32-bit protected mode, paging off, with `%ebx`
 * holding the physical address of `hvm_start_info`.  This stub zeroes .bss,
 * builds an identity map of the first 4 GiB, enables long mode and jumps to
 * `kernel_main`.
 *
 * The identity map is deliberately the whole first 4 GiB rather than just the
 * kernel image: the 16550, the IO-APIC/PIC and later the PCI BARs all sit in
 * that window, and the frame allocator can hand out any page below it until
 * the machine layer narrows the mapping.
 */

.section .text.boot, "ax"
.global _start32
.code32
_start32:
    cli
    cld

    /* The loader does not promise a zeroed .bss. */
    lea __bss_start, %eax
    lea __bss_end, %ecx
    subl %eax, %ecx
    xorl %edx, %edx
1:
    testl %ecx, %ecx
    jz 2f
    movb %dl, (%eax)
    incl %eax
    decl %ecx
    jmp 1b
2:

    /* Keep the PVH start_info pointer for the 64-bit side. */
    movl %ebx, boot_start_info

    movl $boot_stack_top, %esp

    /* PML4[0] -> PDPT. */
    movl $boot_pdpt, %eax
    orl $0x3, %eax
    movl %eax, boot_pml4
    movl $0, boot_pml4 + 4

    /* PDPT[i] -> boot_pds + i * 4096, 1 GiB per entry. */
    xorl %ecx, %ecx
3:
    movl $boot_pds, %eax
    movl %ecx, %edx
    shll $12, %edx
    addl %edx, %eax
    orl $0x3, %eax
    movl %eax, boot_pdpt(,%ecx,8)
    movl $0, boot_pdpt + 4(,%ecx,8)
    incl %ecx
    cmpl $4, %ecx
    jne 3b

    /* PD[i][j] = i * 1 GiB + j * 2 MiB, present|writable|2 MiB. */
    xorl %ebx, %ebx
4:
    movl %ebx, %eax
    shll $30, %eax
    orl $0x83, %eax
    movl %ebx, %edx
    shll $12, %edx
    addl $boot_pds, %edx
    xorl %ecx, %ecx
5:
    movl %eax, (%edx)
    movl $0, 4(%edx)
    addl $0x200000, %eax
    addl $8, %edx
    incl %ecx
    cmpl $512, %ecx
    jne 5b
    incl %ebx
    cmpl $4, %ebx
    jne 4b

    /* PAE, then the PML4, then long mode, then paging. */
    movl %cr4, %eax
    orl $0x20, %eax
    movl %eax, %cr4

    movl $boot_pml4, %eax
    movl %eax, %cr3

    movl $0xC0000080, %ecx
    rdmsr
    orl $0x100, %eax
    wrmsr

    lgdt gdt64_pointer

    movl %cr0, %eax
    orl $0x80000001, %eax
    movl %eax, %cr0

    ljmp $0x08, $start64

.code64
start64:
    movw $0x10, %ax
    movw %ax, %ds
    movw %ax, %es
    movw %ax, %ss
    movw %ax, %fs
    movw %ax, %gs
    movq $boot_stack_top, %rsp
    movl boot_start_info(%rip), %edi
    call kernel_main
6:
    cli
    hlt
    jmp 6b

/*
 * Exception and IRQ entries.
 *
 * Only the vectors this slice installs need a stub: a breakpoint, #GP, #PF
 * and the timer.  Every other IDT entry points at `isr_unexpected`, which
 * reports a sentinel vector rather than jumping through a null handler.
 */
.macro isr_no_error name, vector
.global \name
\name:
    pushq $0
    pushq $\vector
    jmp isr_common
.endm

.macro isr_with_error name, vector
.global \name
\name:
    pushq $\vector
    jmp isr_common
.endm

isr_no_error isr3, 3
isr_with_error isr13, 13
isr_with_error isr14, 14

.global isr_unexpected
isr_unexpected:
    pushq $0
    pushq $0xFF
    jmp isr_common

isr_common:
    movq %rsp, %rdi
    call exception_dispatch
7:
    cli
    hlt
    jmp 7b

.global irq0
irq0:
    pushq $0
    pushq $32
    pushq %rax
    pushq %rcx
    pushq %rdx
    pushq %rsi
    pushq %rdi
    pushq %r8
    pushq %r9
    pushq %r10
    pushq %r11
    call timer_tick
    popq %r11
    popq %r10
    popq %r9
    popq %r8
    popq %rdi
    popq %rsi
    popq %rdx
    popq %rcx
    popq %rax
    addq $16, %rsp
    iretq

/*
 * The handler addresses are read from this table rather than declared one by
 * one in Rust: one row per installed vector, so the IDT code stays data-driven.
 */
.section .rodata
.align 8
.global isr_table
isr_table:
    .quad isr3
    .quad isr13
    .quad isr14
    .quad isr_unexpected
    .quad irq0

.align 8
gdt64:
    .quad 0
    .quad 0x00AF9A000000FFFF      /* 64-bit code, DPL0 */
    .quad 0x00CF92000000FFFF      /* flat data */
gdt64_end:
gdt64_pointer:
    .word gdt64_end - gdt64 - 1
    .long gdt64

/*
 * The PVH entry note.  Type 18 is XEN_ELFNOTE_PHYS32_ENTRY; QEMU uses it to
 * decide this ELF is a PVH kernel and to pick the 32-bit entry address.
 */
.section .note.Xen, "a", @note
.align 4
    .long 4                       /* namesz, including the NUL */
    .long 4                       /* descsz */
    .long 18                      /* XEN_ELFNOTE_PHYS32_ENTRY */
    .asciz "Xen"
    .align 4
    .long _start32

.section .bss, "aw", @nobits
.align 8
boot_start_info:
    .skip 8

.align 4096
boot_pml4:
    .skip 4096
boot_pdpt:
    .skip 4096
boot_pds:
    .skip 16384

.align 16
boot_stack:
    .skip 32768
boot_stack_top:
