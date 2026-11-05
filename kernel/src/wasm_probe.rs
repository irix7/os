// SPDX-License-Identifier: GPL-3.0-only
//! The M1 Wasm policy probe, in the kernel (ADR M1-0003 step 4).
//!
//! This is the slot-B module's run flow with the module ABI removed. The guest
//! runs on a stack from contiguous RAM frames rather than a fixed window, so the
//! kernel layout does not change. The hooks and the bridge publication are the
//! module's, unchanged. The markers are the ones the Wasm gates check.

use alloc::boxed::Box;

use m1_policy_bridge::{bridge_address, Bridge};
use sys_primitives::{ActorId, NodeId};
use wasm_host::{Host, HostHooks, PolicyHooks};
use wasm_policy_host::Policy;

use crate::console_write;

/// The policy guest, generated from `modules/wasm/guest/`.
const GUEST: &[u8] = include_bytes!("../../modules/wasm/src/guest.wasm");

/// Guest stack size: 208 frames, 832 KiB.  A policy boot uses about 734 KiB (#272).
const GUEST_STACK_FRAMES: usize = 208;

/// Print through the kernel console (the guest's log lines are ASCII).
fn log(bytes: &[u8]) {
    console_write(core::str::from_utf8(bytes).unwrap_or("?"));
}

fn write_hex(value: usize) {
    log(b"0x");
    for shift in (0..8).rev() {
        let nibble = ((value >> (shift * 4)) & 0xf) as u8;
        let digit = if nibble < 10 {
            b'0' + nibble
        } else {
            b'a' + nibble - 10
        };
        log(&[digit]);
    }
}

fn write_dec(mut value: u64) {
    let mut buf = [0u8; 20];
    let mut len = 0;
    if value == 0 {
        log(b"0");
        return;
    }
    while value > 0 && len < buf.len() {
        buf[len] = b'0' + (value % 10) as u8;
        value /= 10;
        len += 1;
    }
    while len > 0 {
        len -= 1;
        log(&buf[len..len + 1]);
    }
}

fn kernel_clock_ms() -> u64 {
    crate::services::clock_ms()
}

fn kernel_console_write(bytes: &[u8]) {
    log(bytes);
}

fn policy(context: usize) -> Option<&'static mut Policy> {
    if context == 0 {
        return None;
    }
    // SAFETY: the module owns this allocation for its lifetime and all host
    // callbacks are synchronous on the single kernel thread.
    Some(unsafe { &mut *(context as *mut Policy) })
}

fn hook_node_id(context: usize, out: &mut [u8; 16]) -> i32 {
    match policy(context) {
        Some(policy) => {
            out.copy_from_slice(policy.node().as_bytes());
            0
        }
        None => -1,
    }
}

fn hook_begin(context: usize) -> i32 {
    match policy(context).map(Policy::begin) {
        Some(Ok(())) => 0,
        _ => -1,
    }
}

fn hook_abort(context: usize) {
    if let Some(policy) = policy(context) {
        policy.abort();
    }
}

fn hook_commit(context: usize) -> i32 {
    match policy(context).map(Policy::commit) {
        Some(Ok(_)) => 0,
        _ => -1,
    }
}

fn hook_mkdir(context: usize, path: &str, out: &mut [u8; 32]) -> i32 {
    let Some(policy) = policy(context) else {
        return -1;
    };
    match policy.mkdir(path) {
        Ok(oid) => {
            out.copy_from_slice(oid.as_bytes());
            0
        }
        Err(_) => -1,
    }
}

fn hook_put(context: usize, path: &str, kind: u8, bytes: &[u8], out: &mut [u8; 32]) -> i32 {
    let Some(policy) = policy(context) else {
        return -1;
    };
    match policy.put(path, kind, bytes) {
        Ok(oid) => {
            out.copy_from_slice(oid.as_bytes());
            0
        }
        Err(_) => -1,
    }
}

fn hook_lookup(context: usize, path: &str, out: &mut [u8; 32]) -> i32 {
    let Some(policy) = policy(context) else {
        return -1;
    };
    match policy.lookup(path) {
        Ok(oid) => {
            out.copy_from_slice(oid.as_bytes());
            0
        }
        Err(_) => -1,
    }
}

fn hook_get(context: usize, oid: &[u8; 32], kind: &mut i32, out: &mut [u8; 512]) -> i32 {
    let Some(policy) = policy(context) else {
        return -1;
    };
    let object = sys_primitives::ObjectId(*oid);
    match policy.get(&object) {
        Ok((object_kind, bytes)) => {
            if bytes.len() > out.len() {
                return -1;
            }
            out[..bytes.len()].copy_from_slice(bytes);
            *kind = object_kind as i32;
            bytes.len() as i32
        }
        Err(_) => -1,
    }
}

fn hook_root(context: usize, out: &mut [u8; 32]) -> i32 {
    let Some(policy) = policy(context) else {
        return -1;
    };
    match policy.root() {
        Ok(oid) => {
            out.copy_from_slice(oid.as_bytes());
            0
        }
        Err(_) => -1,
    }
}

fn hook_home(context: usize, oid: &[u8; 32], out: &mut [u8; 16]) -> i32 {
    let Some(policy) = policy(context) else {
        return -1;
    };
    match policy.home(&sys_primitives::ObjectId(*oid)) {
        Some(node) => {
            out.copy_from_slice(node.as_bytes());
            0
        }
        None => -1,
    }
}

fn hook_actor_spawn(context: usize, id: &[u8; 16]) -> i32 {
    match policy(context).map(|policy| policy.actor_spawn(ActorId(*id))) {
        Some(Ok(())) => 0,
        _ => -1,
    }
}

fn hook_actor_place(context: usize, id: &[u8; 16], host: &[u8; 16]) -> i32 {
    match policy(context).map(|policy| policy.actor_place(ActorId(*id), NodeId(*host))) {
        Some(Ok(())) => 0,
        _ => -1,
    }
}

fn hook_actor_queue(context: usize, id: &[u8; 16]) -> i32 {
    match policy(context).map(|policy| policy.actor_queue(ActorId(*id))) {
        Some(Ok(())) => 0,
        _ => -1,
    }
}

fn hook_cap_issue(
    context: usize,
    object: &[u8; 32],
    rights: u32,
    subject: &[u8; 32],
    home: &[u8; 16],
) -> i64 {
    let Some(policy) = policy(context) else {
        return 0;
    };
    match policy.cap_issue(
        sys_primitives::ObjectId(*object),
        rights,
        *subject,
        NodeId(*home),
    ) {
        Ok(handle) => handle as i64,
        Err(_) => 0,
    }
}

fn hook_cap_check(context: usize, handle: u64, rights: u32, object: &[u8; 32]) -> i32 {
    let Some(policy) = policy(context) else {
        return -1;
    };
    if policy.cap_check(handle, rights, &sys_primitives::ObjectId(*object)) {
        0
    } else {
        -1
    }
}

fn hook_cap_rights(context: usize, handle: u64, object: &[u8; 32]) -> i32 {
    let Some(policy) = policy(context) else {
        return -1;
    };
    match policy.cap_rights(handle, &sys_primitives::ObjectId(*object)) {
        Some(rights) => rights as i32,
        None => -1,
    }
}

fn hook_stats(context: usize, out: &mut [u32; 4]) -> i32 {
    let Some(policy) = policy(context) else {
        return -1;
    };
    match policy.stats() {
        Ok(stats) => {
            out[0] = stats.objects;
            out[1] = stats.actors;
            out[2] = stats.queued;
            out[3] = stats.caps;
            0
        }
        Err(_) => -1,
    }
}

unsafe fn publish_bridge(policy: *mut Policy) -> bool {
    if policy.is_null() {
        return false;
    }
    let bridge = Bridge::published(policy as usize);
    unsafe { core::ptr::write_volatile(bridge_address() as *mut Bridge, bridge) };
    true
}

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

/// Outcome of the guest run, written by `guest_entry` on the guest stack.
static mut GUEST_RESULT: Result<i32, ()> = Err(());

extern "C" fn guest_entry(host: usize) {
    let marker: usize = 0;
    let here = core::hint::black_box(&marker) as *const usize as usize;
    log(b"wasm: guest sp=");
    write_hex(here);
    log(b"\n");
    // SAFETY: `host` is the `Host` on the caller's frame, alive for the call.
    let host = unsafe { &*(host as *const Host) };
    let result = host.run_i32(GUEST, "run", 10_000_000).map_err(|_| ());
    // SAFETY: single-threaded kernel; only this call writes the result.
    unsafe { GUEST_RESULT = result };
}

/// Node identity for the policy: the kernel's mesh node.
fn node_id() -> Option<NodeId> {
    let node = crate::mesh::node_id();
    if node.is_zero() {
        None
    } else {
        Some(node)
    }
}

/// Run the guest policy and publish its committed root.  Called once at boot,
/// before `logical::adopt_wasm`.
pub fn run() {
    crate::logical::begin_wasm_load();
    let Some(node) = node_id() else {
        log(b"wasm: no local node\n");
        crate::logical::report_reload_failure();
        return;
    };

    let policy = Box::into_raw(Box::new(Policy::new(node, kernel_clock_ms())));
    let hooks = HostHooks {
        clock_ms: kernel_clock_ms,
        console_write: kernel_console_write,
        policy: PolicyHooks {
            context: policy as usize,
            node_id: hook_node_id,
            begin: hook_begin,
            abort: hook_abort,
            commit: hook_commit,
            mkdir: hook_mkdir,
            put: hook_put,
            lookup: hook_lookup,
            get: hook_get,
            root: hook_root,
            home: hook_home,
            actor_spawn: hook_actor_spawn,
            actor_place: hook_actor_place,
            actor_queue: hook_actor_queue,
            cap_issue: hook_cap_issue,
            cap_check: hook_cap_check,
            cap_rights: hook_cap_rights,
            stats: hook_stats,
        },
    };
    let host = match Host::with_hooks(hooks) {
        Ok(host) => host,
        Err(_) => {
            // SAFETY: the allocation is owned here and not published.
            unsafe { (*policy).abort() };
            log(b"wasm: host setup failed\n");
            crate::logical::report_reload_failure();
            return;
        }
    };

    let Some(stack_base) = crate::frames::alloc_frames(GUEST_STACK_FRAMES) else {
        log(b"wasm: no guest stack\n");
        // SAFETY: the allocation is owned here and not published.
        unsafe { (*policy).abort() };
        crate::logical::report_reload_failure();
        return;
    };
    let stack_top = crate::KSEG0_OFFSET as usize + stack_base as usize + GUEST_STACK_FRAMES * 4096;
    let started = kernel_clock_ms();
    // SAFETY: the stack frames are reserved for this call and freed after it.
    unsafe {
        wasm_probe_call_on_stack(stack_top, guest_entry, &host as *const Host as usize);
    }
    for index in 0..GUEST_STACK_FRAMES {
        crate::frames::free_frame(stack_base + (index as u64) * 4096);
    }
    // SAFETY: `guest_entry` has returned, so the result is complete.
    let result = unsafe { GUEST_RESULT };
    let elapsed = kernel_clock_ms().saturating_sub(started);
    log(b"wasm: runtime=");
    write_dec(elapsed);
    log(b" ms\n");
    match result {
        Ok(42) => {
            // SAFETY: the allocation is owned here; guest callbacks are done.
            if !unsafe { (*policy).is_committed() } {
                // SAFETY: as above.
                unsafe { (*policy).abort() };
                log(b"wasm: policy not committed\n");
                crate::logical::report_reload_failure();
                return;
            }
            // SAFETY: as above.
            if !unsafe { publish_bridge(policy) } {
                log(b"wasm: bridge failed\n");
                crate::logical::report_reload_failure();
                return;
            }
            log(b"wasm: run=42\n");
        }
        Ok(_) => {
            // SAFETY: as above.
            unsafe { (*policy).abort() };
            log(b"wasm: wrong result\n");
            crate::logical::report_reload_failure();
        }
        Err(_) => {
            // SAFETY: as above.
            unsafe { (*policy).abort() };
            log(b"wasm: run failed\n");
            crate::logical::report_reload_failure();
        }
    }
}
