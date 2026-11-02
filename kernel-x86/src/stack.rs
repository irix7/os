// SPDX-License-Identifier: GPL-3.0-only
//! A large kernel stack for code that runs deep.
//!
//! The boot stack is 32 KiB, which is too small for a Wasm runtime: the Indy
//! kernel's interpreter needed about 530 KiB of native stack (issue #272).  The
//! machine reserves a 2 MiB window for a second stack and runs deep work on it
//! through `on_stack`.  The window is reserved in the memory map, so the frame
//! allocator never hands it out.

use core::arch::asm;
use core::ffi::c_void;

/// Base of the large stack window.  The stack grows down from `TOP`.
pub const BASE: usize = 0x0600_0000;
/// Size of the window: 2 MiB.
pub const SIZE: usize = 0x0020_0000;
/// The initial stack pointer: the top of the window, 16-byte aligned.
pub const TOP: usize = BASE + SIZE;

/// Call `entry(arg)` with the stack pointer at `top`, then restore the caller's
/// stack.
///
/// # Safety
///
/// `top` must be 16-byte aligned and must lie at the top of a writable region
/// with at least the stack space `entry` needs.  Nothing else may use that
/// region while the call runs.
unsafe fn call_on_stack(top: usize, entry: extern "C" fn(*mut c_void), arg: *mut c_void) {
    // SAFETY: the caller guarantees the region; `r12` is callee-saved in the C
    // ABI, so it keeps the old stack pointer across the call.
    unsafe {
        asm!(
            "mov r12, rsp",
            "mov rsp, {top}",
            "call {entry}",
            "mov rsp, r12",
            top = in(reg) top,
            entry = in(reg) entry,
            in("rdi") arg,
            out("r12") _,
            clobber_abi("C"),
        );
    }
}

extern "C" fn trampoline(arg: *mut c_void) {
    // SAFETY: `on_stack` passes a pointer to a live `&mut dyn FnMut()` for the
    // duration of the call.
    let closure = unsafe { &mut *(arg as *mut &mut dyn FnMut()) };
    closure();
}

/// Run `work` on the large stack.
pub fn on_stack(work: &mut dyn FnMut()) {
    let mut work = work;
    // SAFETY: the window is reserved and unused, its top is aligned, and the
    // trampoline reads `work` only while this call is live.
    unsafe {
        call_on_stack(
            TOP,
            trampoline,
            &mut work as *mut &mut dyn FnMut() as *mut c_void,
        );
    }
}
