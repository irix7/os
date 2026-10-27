/*
 * x86_64 boot entry (Multiboot2).
 *
 * QEMU loads this with `-kernel` and, on recognising the Multiboot2 header,
 * starts the CPU in protected mode with a flat GDT and identity paging, then
 * jumps to the entry address in the header (_start).  _start reloads our own
 * GDT, sets up a stack, enables 4-level paging and enters Rust.
 */

.section .multiboot2.header, "a"
.align 8
.header_start:
    .long 0xe85250d6                 // magic
    .long 0                          // architecture 0 = i386
    .long header_end - header_start  // header length
    .long 0x100000000 - (0xe85250d6 + 0 + (header_end - header_start)) // checksum

    // Required Multiboot2 tags.
    .align 8
    .long 0, 0                       // end of tag list (type 0, size 0)
header_end:

.section .text._start, "ax"
.align 16
.globl _start
.type _start, @function
_start:
    cli

    // Load our GDT and reload CS with a far jump so the code segment
    // selector is our kernel code selector.
    lgdt (gdt_ptr)
    movq $0x10, %rax
    movq %rax, %ds
    movq %rax, %es
    movq %rax, %ss
    ljmp $0x10, $1f
1:
    // Set up a stack below the module area / above the image.
    movq $stack_top, %rsp

    // Enter Rust: initialise IDT, paging, then the shared kernel main.
    call rust_main

    hlt
    jmp _start
.size _start, .-_start

// ---------------------------------------------------------------------------
// GDT (loaded by _start).  Flat kernel model.
// ---------------------------------------------------------------------------
.section .rodata
.align 8
gdt:
    .quad 0x0000000000000000          // null descriptor
    .quad 0x0000000000000000          // reserved
    .quad 0x00af9a000000ffff          // kernel code: present, ring0, executable, readable, 64-bit
    .quad 0x00cf92000000ffff          // kernel data: present, ring0, writable
    .quad 0x0000000000000000
    .quad 0x0000000000000000
gdt_end:
gdt_ptr:
    .word gdt_end - gdt - 1           // limit
    .quad gdt                         // base

// ---------------------------------------------------------------------------
// Stack
// ---------------------------------------------------------------------------
.section .bss.noinit, "aw", @nobits
.align 16
stack_bottom:
    .skip 32768                       // 32 KiB boot stack
stack_top:

// ---------------------------------------------------------------------------
// ISR stubs.  Each saves caller-saved registers, loads the vector number and
// a zero error code into rdi/rsi, calls isr_handler, restores registers,
// removes the CPU-pushed error code for error-code vectors, and iretqs.
// ---------------------------------------------------------------------------
.section .text.isrs, "ax"
.align 16

.macro isr_stub num, has_err
.globl isr_\num
.type isr_\num, @function
isr_\num:
    cli
    push %rax
    push %rcx
    push %rdx
    push %rsi
    push %rdi
    push %r8
    push %r9
    push %r10
    push %r11
    movq $\num, %rdi
    movq $0, %rsi
    call isr_handler
    pop %r11
    pop %r10
    pop %r9
    pop %r8
    pop %rdi
    pop %rsi
    pop %rdx
    pop %rcx
    pop %rax
    iretq
    .if \has_err
    addq $8, %rsp                     // remove CPU-pushed error code after iretq
    .endif
.size isr_\num, .-isr_\num
.endm

// Vectors without error codes.
isr_stub 0, 0
isr_stub 1, 0
isr_stub 2, 0
isr_stub 3, 0
isr_stub 4, 0
isr_stub 5, 0
isr_stub 6, 0
isr_stub 7, 0
isr_stub 9, 0
isr_stub 15, 0
isr_stub 16, 0
isr_stub 18, 0
isr_stub 19, 0
isr_stub 20, 0
isr_stub 21, 0
isr_stub 22, 0
isr_stub 23, 0
isr_stub 24, 0
isr_stub 25, 0
isr_stub 26, 0
isr_stub 27, 0
isr_stub 28, 0
isr_stub 29, 0
isr_stub 30, 0
isr_stub 31, 0
// Vectors with error codes.
isr_stub 8, 1
isr_stub 10, 1
isr_stub 11, 1
isr_stub 12, 1
isr_stub 13, 1
isr_stub 14, 1
isr_stub 17, 1

// Timer IRQ0 -> vector 32 (no error code).
isr_stub 32, 0
.globl isr_timer
.type isr_timer, @function
isr_timer:
    jmp isr_32
.size isr_timer, .-isr_timer

// Catch-all unexpected interrupt (reports vector 255).
.globl isr_unexpected
.type isr_unexpected, @function
isr_unexpected:
    cli
    push %rax
    push %rcx
    push %rdx
    push %rsi
    push %rdi
    push %r8
    push %r9
    push %r10
    push %r11
    movq $255, %rdi
    movq $0, %rsi
    call isr_handler
    pop %r11
    pop %r10
    pop %r9
    pop %r8
    pop %rdi
    pop %rsi
    pop %rdx
    pop %rcx
    pop %rax
    iretq
.size isr_unexpected, .-isr_unexpected
