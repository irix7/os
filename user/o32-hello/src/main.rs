// SPDX-License-Identifier: GPL-3.0-only
//! Minimal synthetic IRIX o32 probe: classic IRIX write(4) and exit(1).

#![no_std]
#![no_main]
#![feature(asm_experimental_arch)]

use core::panic::PanicInfo;

core::arch::global_asm!(
    r#"
    .set noreorder
    .set mips3

    .section .text._start, "ax", @progbits
    .globl _start
    .ent _start
_start:
    li      $v0, 4
    li      $a0, 1
    lui     $a1, %hi(message)
    addiu   $a1, $a1, %lo(message)
    li      $a2, 15
    syscall

    # open("/hello.txt", O_RDONLY)
    li      $v0, 5
    lui     $a0, %hi(path)
    addiu   $a0, $a0, %lo(path)
    li      $a1, 0
    li      $a2, 0
    syscall
    move    $s0, $v0

    # read(fd, buffer, 64)
    li      $v0, 3
    move    $a0, $s0
    lui     $a1, %hi(buffer)
    addiu   $a1, $a1, %lo(buffer)
    li      $a2, 64
    syscall
    move    $s1, $v0

    # write(1, buffer, count)
    li      $v0, 4
    li      $a0, 1
    lui     $a1, %hi(buffer)
    addiu   $a1, $a1, %lo(buffer)
    move    $a2, $s1
    syscall

    # close(fd)
    li      $v0, 6
    move    $a0, $s0
    syscall

    li      $v0, 1
    li      $a0, 42
    syscall

1:  b       1b
    nop
    .end _start

    .section .rodata, "a", @progbits
message:
    .ascii "hello from o32\n"
path:
    .asciz "/hello.txt"

    .section .bss, "aw", @nobits
    .align 2
buffer:
    .space 64
"#
);

#[cfg(not(test))]
#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    loop {
        core::hint::spin_loop();
    }
}
