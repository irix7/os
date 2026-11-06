// SPDX-License-Identifier: GPL-3.0-only
//! The M1 Wasm policy probe (ADR M1-0003 step 4).
//!
//! The embedded guest owns the policy: it chooses the local namespace layout,
//! object contents, actor identity, capability rights and the order of every
//! validation.  This crate supplies the native primitives behind the `i7host`
//! surface: a staged CAS/namespace, the actor directory and run queue, a
//! generation-checked capability table, the clock and the console.  It does not
//! let the guest read the kernel service table or an ambient pointer.
//!
//! A kernel calls [`run`] with a [`Platform`].  The iris kernel and the x86
//! machine both use it.  On success the guest prints `wasm: host=1` and
//! `m1-wasm: root=... actor=... cap=ok rights=35 home=...`; this crate prints
//! `wasm: run=42` and publishes the committed policy pointer at the reserved
//! bridge page.  A guest trap or a non-42 result aborts the stage, so no partial
//! object, actor or capability is published.  The kernel adopts the committed
//! root, actor, capability rights and home after `run` returns.

#![no_std]

extern crate alloc;

use alloc::boxed::Box;
use core::ptr::null_mut;
use core::sync::atomic::{AtomicPtr, Ordering};

use m1_policy_bridge::{bridge_address, Bridge};
use module_abi::Services;
use sys_primitives::{ActorId, NodeId};
use wasm_host::{Host, HostHooks, PolicyHooks};
use wasm_policy_host::Policy;

/// The policy guest is generated from `guest/wasm-policy/` and embedded here.
const GUEST: &[u8] = include_bytes!("../../../guest/wasm-policy/guest.wasm");

/// What a kernel supplies to the probe.
pub trait Platform {
    /// Write bytes to the console.  The guest's log lines are ASCII.
    fn console_write(&self, bytes: &[u8]);
    /// Milliseconds since boot.
    fn clock_ms(&self) -> u64;
    /// The local node identity, or `None` when the kernel has none.
    fn node_id(&self) -> Option<NodeId>;
    /// Run `entry(arg)` on a stack large enough for the Wasm runtime.  Returns
    /// `false` when no stack is available, and then `entry` does not run.
    fn on_guest_stack(&self, entry: extern "C" fn(usize), arg: usize) -> bool;
}

/// The platform in use for the current run.  Set once by [`run`], before any
/// host callback, on the single kernel thread.
static mut PLATFORM: Option<&'static dyn Platform> = None;

fn platform() -> Option<&'static dyn Platform> {
    // SAFETY: written once by `run` before any callback, and read only here.
    unsafe { *core::ptr::addr_of!(PLATFORM) }
}

fn log(bytes: &[u8]) {
    if let Some(platform) = platform() {
        platform.console_write(bytes);
    }
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
    let mut len = buf.len();
    loop {
        len -= 1;
        buf[len] = b'0' + (value % 10) as u8;
        value /= 10;
        if value == 0 {
            break;
        }
    }
    log(&buf[len..]);
}

fn kernel_clock_ms() -> u64 {
    platform().map_or(0, |platform| platform.clock_ms())
}

fn kernel_console_write(bytes: &[u8]) {
    log(bytes);
}

fn policy(context: usize) -> Option<&'static mut Policy> {
    if context == 0 {
        return None;
    }
    // SAFETY: the probe owns this allocation for its lifetime and all host
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

/// Outcome of the guest run, written by `guest_entry` on the guest stack.
static mut GUEST_RESULT: Result<i32, ()> = Err(());

extern "C" fn guest_entry(host: usize) {
    // Diagnostic: the guest's stack pointer, so a boot log shows the switch.
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

/// Run the policy guest and publish the committed policy.  Returns `false` when
/// the policy did not run or commit; the kernel then reports a failed reload.
pub fn run(platform_impl: &'static dyn Platform) -> bool {
    // SAFETY: the single kernel thread sets the platform before any callback.
    unsafe { *core::ptr::addr_of_mut!(PLATFORM) = Some(platform_impl) };
    let Some(node) = platform_impl.node_id() else {
        log(b"wasm: no local node\n");
        return false;
    };

    let policy = Box::into_raw(Box::new(Policy::new(node, platform_impl.clock_ms())));
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
            return false;
        }
    };
    let started = platform_impl.clock_ms();
    if !platform_impl.on_guest_stack(guest_entry, &host as *const Host as usize) {
        // SAFETY: the allocation is owned here and not published.
        unsafe { (*policy).abort() };
        log(b"wasm: no guest stack\n");
        return false;
    }
    // SAFETY: `guest_entry` has returned, so the result is complete.
    let result = unsafe { GUEST_RESULT };
    let elapsed = platform_impl.clock_ms().saturating_sub(started);
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
                return false;
            }
            // SAFETY: as above.
            if !unsafe { publish_bridge(policy) } {
                log(b"wasm: bridge failed\n");
                return false;
            }
            log(b"wasm: run=42\n");
            true
        }
        Ok(_) => {
            // SAFETY: as above.
            unsafe { (*policy).abort() };
            log(b"wasm: wrong result\n");
            false
        }
        Err(_) => {
            // SAFETY: as above.
            unsafe { (*policy).abort() };
            log(b"wasm: run failed\n");
            false
        }
    }
}

/// The service table of a kernel that exposes one, such as the x86 machine.
/// The table is installed by [`start`] before the run.
static SERVICES: AtomicPtr<Services> = AtomicPtr::new(null_mut());

/// Adapter from the shared service table.  Its methods read [`SERVICES`].
pub struct ServicePlatform;

/// The service-table platform.
pub static SERVICE_PLATFORM: ServicePlatform = ServicePlatform;

fn services() -> Option<&'static Services> {
    let ptr = SERVICES.load(Ordering::Acquire);
    if ptr.is_null() {
        None
    } else {
        // SAFETY: the table lives for the kernel lifetime.
        Some(unsafe { &*ptr })
    }
}

impl Platform for ServicePlatform {
    fn console_write(&self, bytes: &[u8]) {
        if let Some(services) = services() {
            (services.write)(bytes.as_ptr(), bytes.len());
        }
    }

    fn clock_ms(&self) -> u64 {
        services().map_or(0, |services| (services.clock_ms)())
    }

    fn node_id(&self) -> Option<NodeId> {
        node_id_from_info(services()?)
    }

    fn on_guest_stack(&self, entry: extern "C" fn(usize), arg: usize) -> bool {
        // The caller runs the probe on its large stack already.
        entry(arg);
        true
    }
}

/// Run the probe on a kernel service table.  The table must be version 11 or
/// later.  The caller runs this on a stack large enough for the Wasm runtime.
pub fn start(services: &'static Services) -> bool {
    if services.version < 11 {
        (services.write)(b"wasm: ABI too old\n".as_ptr(), 18);
        return false;
    }
    SERVICES.store(
        services as *const Services as *mut Services,
        Ordering::Release,
    );
    run(&SERVICE_PLATFORM)
}

fn node_id_from_info(services: &Services) -> Option<NodeId> {
    let mut text = [0u8; 256];
    // SAFETY: the service writes at most `text.len()` bytes into the buffer and
    // returns the number of bytes written.
    let n = unsafe { (services.info)(1, text.as_mut_ptr(), text.len()) };
    if n <= 0 {
        return None;
    }
    let text = core::str::from_utf8(&text[..n as usize]).ok()?;
    let mac = &text[text.find("mac: ")? + 5..];
    let bytes = mac.as_bytes();
    if bytes.len() < 17 {
        return None;
    }
    let mut node = [0u8; 16];
    for i in 0..6 {
        let hi = hex_nibble(bytes[i * 3])?;
        let lo = hex_nibble(bytes[i * 3 + 1])?;
        node[i] = (hi << 4) | lo;
    }
    Some(NodeId(node))
}

fn hex_nibble(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}
