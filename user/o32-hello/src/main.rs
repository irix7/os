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

    li      $v0, 1
    li      $a0, 42
    syscall

1:  b       1b
    nop
    .end _start

    .section .rodata, "a", @progbits
message:
    .ascii "hello from o32\n"
"#
);

#[cfg(not(test))]
#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    loop {
        core::hint::spin_loop();
    }
}
