// SPDX-License-Identifier: GPL-3.0-only
//! The M1 Wasm policy probe in the iris kernel (ADR M1-0003 step 4).
//!
//! The probe itself is the `wasm-probe` crate, shared with the x86 machine.  This
//! file supplies the iris kernel's platform: the console, the clock, the mesh
//! node identity, and a guest stack from contiguous RAM frames, so the kernel
//! layout does not change.

use sys_primitives::NodeId;

use crate::console_write;

/// Guest stack size: 208 frames, 832 KiB.  A policy boot uses about 734 KiB (#272).
const GUEST_STACK_FRAMES: usize = 208;

#[cfg(any(target_arch = "mips", target_arch = "mips64"))]
core::arch::global_asm!(
    ".set noreorder",
    ".globl wasm_probe_call_on_stack",
    ".ent wasm_probe_call_on_stack",
    "wasm_probe_call_on_stack:",
    "addiu $sp, $sp, -32",
    "sw $ra, 28($sp)",
    "sw $s0, 24($sp)",
    "move $s0, $sp",
    "move $sp, $a0",
    "move $t9, $a1",
    "jalr $t9",
    "move $a0, $a2",
    "move $sp, $s0",
    "lw $s0, 24($sp)",
    "lw $ra, 28($sp)",
    "jr $ra",
    "addiu $sp, $sp, 32",
    ".end wasm_probe_call_on_stack",
    ".set reorder",
);

#[cfg(any(target_arch = "mips", target_arch = "mips64"))]
extern "C" {
    fn wasm_probe_call_on_stack(stack_top: usize, entry: extern "C" fn(usize), arg: usize);
}

/// Non-MIPS builds run the guest on the current stack.
#[cfg(not(any(target_arch = "mips", target_arch = "mips64")))]
unsafe fn wasm_probe_call_on_stack(_stack_top: usize, entry: extern "C" fn(usize), arg: usize) {
    entry(arg);
}

struct KernelPlatform;

static KERNEL_PLATFORM: KernelPlatform = KernelPlatform;

impl ::wasm_probe::Platform for KernelPlatform {
    fn console_write(&self, bytes: &[u8]) {
        console_write(core::str::from_utf8(bytes).unwrap_or("?"));
    }

    fn clock_ms(&self) -> u64 {
        crate::services::clock_ms()
    }

    /// Node identity for the policy: the kernel's mesh node.
    fn node_id(&self) -> Option<NodeId> {
        let node = crate::mesh::node_id();
        if node.is_zero() {
            None
        } else {
            Some(node)
        }
    }

    fn on_guest_stack(&self, entry: extern "C" fn(usize), arg: usize) -> bool {
        let Some(stack_base) = crate::frames::alloc_frames(GUEST_STACK_FRAMES) else {
            return false;
        };
        let stack_top =
            crate::KSEG0_OFFSET as usize + stack_base as usize + GUEST_STACK_FRAMES * 4096;
        // SAFETY: the frames are reserved for this call and freed after it.
        unsafe {
            wasm_probe_call_on_stack(stack_top, entry, arg);
        }
        for index in 0..GUEST_STACK_FRAMES {
            crate::frames::free_frame(stack_base + (index as u64) * 4096);
        }
        true
    }
}

/// Run the guest policy and publish its committed root.  Called once at boot,
/// before `logical::adopt_wasm`.  Every failure reports a failed reload.
pub fn run() {
    crate::logical::begin_wasm_load();
    if !::wasm_probe::run(&KERNEL_PLATFORM) {
        crate::logical::report_reload_failure();
    }
}
