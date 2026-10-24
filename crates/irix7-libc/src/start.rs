// SPDX-License-Identifier: GPL-3.0-only
//! C runtime entry for the static bring-up image.

use core::ffi::{c_char, c_int};

#[no_mangle]
pub static mut environ: *mut *mut c_char = core::ptr::null_mut();

#[allow(dead_code)]
extern "C" {
    fn main(argc: c_int, argv: *mut *mut c_char, envp: *mut *mut c_char) -> c_int;
    fn exit(status: c_int) -> !;
}

core::arch::global_asm!(
    r#"
    .set noreorder
    .set mips3

    .section .text.__start, "ax", @progbits
    .align 4
    .globl __start
    .globl _start
    .ent __start
__start:
_start:
    move    $s0, $sp
    lw      $a0, 0($sp)          /* argc */
    addiu   $a1, $sp, 4          /* argv */
    sll     $t0, $a0, 2
    addu    $a2, $a1, $t0
    addiu   $a2, $a2, 4          /* envp */
    and     $sp, $sp, -16

    la      $t1, environ
    sw      $a2, 0($t1)

    la      $gp, _gp
    jal     main
    nop
    move    $a0, $v0
    jal     exit
    nop
1:  b       1b
    nop
.end __start

    .globl _gp
_gp:
"#,
);

#[no_mangle]
pub unsafe extern "C" fn __irix7_environ() -> *mut *mut c_char {
    environ
}
