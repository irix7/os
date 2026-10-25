// SPDX-License-Identifier: GPL-3.0-only
//! Slot-B Wasm core probe that runs the M1 local policy.
//!
//! The embedded guest owns the policy: it chooses the local namespace layout,
//! object contents, actor identity, capability rights and the order of every
//! validation.  This module supplies the native primitives behind the
//! `i7host` surface: a staged CAS/namespace, the actor directory and run
//! queue, a generation-checked capability table, the clock and the console.
//! It does not let the guest read the kernel service table or an ambient
//! pointer.
//!
//! On success the guest prints `wasm: host=1` and
//! `m1-wasm: root=... actor=... cap=ok rights=35 home=...`; this module prints
//! `wasm: run=42`.  A guest trap or a non-42 result aborts the stage, so no
//! partial object, actor or capability is published.  The kernel's native
//! `m1:` policy remains the fallback and is not modified by this module.

#![no_std]
#![no_main]

extern crate alloc;

use alloc::boxed::Box;
use core::alloc::{GlobalAlloc, Layout};
use core::ptr::null_mut;
use core::sync::atomic::{AtomicPtr, Ordering};

use module_abi::{ModuleVtable, Services};
use sys_primitives::{ActorId, NodeId};
use wasm_host::{Host, HostHooks, PolicyHooks};
use wasm_policy_host::Policy;

/// The policy guest is generated from `guest/` and embedded here.
const GUEST: &[u8] = include_bytes!("guest.wasm");

static SERVICES: AtomicPtr<Services> = AtomicPtr::new(null_mut());

struct ModuleAllocator;

unsafe impl GlobalAlloc for ModuleAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let table = SERVICES.load(Ordering::Acquire);
        if table.is_null() {
            return null_mut();
        }
        // SAFETY: the table is the kernel's v11 service table.
        let allocate = unsafe { (*table).alloc };
        unsafe { allocate(layout.size(), layout.align()) }
    }

    unsafe fn dealloc(&self, _ptr: *mut u8, _layout: Layout) {
        // The kernel heap is a bump allocator; memory is reclaimed by reboot.
    }
}

#[global_allocator]
static ALLOCATOR: ModuleAllocator = ModuleAllocator;

static OK: ModuleVtable = ModuleVtable {
    status: 0,
    tick: None,
};
static BAD: ModuleVtable = ModuleVtable {
    status: 1,
    tick: None,
};

fn log(services: &Services, text: &[u8]) {
    (services.write)(text.as_ptr(), text.len());
}

fn write_dec(services: &Services, mut value: u64) {
    let mut buf = [0u8; 20];
    let mut len = 0;
    if value == 0 {
        (services.write)(b"0".as_ptr(), 1);
        return;
    }
    while value > 0 && len < buf.len() {
        buf[len] = b'0' + (value % 10) as u8;
        value /= 10;
        len += 1;
    }
    while len > 0 {
        len -= 1;
        (services.write)(&buf[len], 1);
    }
}

fn services() -> Option<&'static Services> {
    let ptr = SERVICES.load(Ordering::Acquire);
    if ptr.is_null() {
        None
    } else {
        // SAFETY: the table lives for the kernel lifetime.
        Some(unsafe { &*ptr })
    }
}

fn kernel_clock_ms() -> u64 {
    services()
        .map(|services| (services.clock_ms)() as u64)
        .unwrap_or(0)
}

fn kernel_console_write(bytes: &[u8]) {
    if let Some(services) = services() {
        (services.write)(bytes.as_ptr(), bytes.len());
    }
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

/// # Safety
/// `services` must point to a valid v11 kernel service table.
#[no_mangle]
pub unsafe extern "C" fn _start(services: *const Services) -> *const ModuleVtable {
    let Some(services) = (unsafe { services.as_ref() }) else {
        return &BAD;
    };
    if services.version < 11 {
        log(services, b"wasm: ABI too old\n");
        return &BAD;
    }
    SERVICES.store(
        services as *const Services as *mut Services,
        Ordering::Release,
    );
    let Some(node) = node_id(services) else {
        log(services, b"wasm: no local node\n");
        return &BAD;
    };

    let policy = Box::into_raw(Box::new(Policy::new(node, (services.clock_ms)())));
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
            // SAFETY: the allocation is owned by this module and not published.
            unsafe { (*policy).abort() };
            log(services, b"wasm: host setup failed\n");
            return &BAD;
        }
    };
    let started = (services.clock_ms)();
    let result = host.run_i32(GUEST, "run", 10_000_000);
    let elapsed = (services.clock_ms)().saturating_sub(started);
    log(services, b"wasm: runtime=");
    write_dec(services, elapsed);
    log(services, b" ms\n");
    match result {
        Ok(42) => {
            // SAFETY: the module owns this allocation; guest callbacks are done.
            if !unsafe { (*policy).is_committed() } {
                // SAFETY: the module owns this allocation and the guest has returned.
                unsafe { (*policy).abort() };
                log(services, b"wasm: policy not committed\n");
                return &BAD;
            }
            log(services, b"wasm: run=42\n");
            &OK
        }
        Ok(_) => {
            // SAFETY: the module owns this allocation and the guest has returned.
            unsafe { (*policy).abort() };
            log(services, b"wasm: wrong result\n");
            &BAD
        }
        Err(_) => {
            // SAFETY: the module owns this allocation and the guest has returned.
            unsafe { (*policy).abort() };
            log(services, b"wasm: run failed\n");
            &BAD
        }
    }
}

fn node_id(services: &Services) -> Option<NodeId> {
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

#[cfg(not(test))]
#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    loop {
        core::hint::spin_loop();
    }
}

// The module links only core/alloc and compiler_builtins.
//
/// # Safety
/// `dst` must be valid for writes of `n` bytes.
#[no_mangle]
pub unsafe extern "C" fn memset(
    dst: *mut core::ffi::c_void,
    c: i32,
    n: usize,
) -> *mut core::ffi::c_void {
    let dst = dst as *mut u8;
    let mut i = 0;
    while i < n {
        unsafe {
            *dst.add(i) = c as u8;
        }
        i += 1;
    }
    dst as *mut core::ffi::c_void
}

/// # Safety
/// `dst` must be valid for writes of `n` bytes and `src` for reads of `n` bytes.
#[no_mangle]
pub unsafe extern "C" fn memcpy(
    dst: *mut core::ffi::c_void,
    src: *const core::ffi::c_void,
    n: usize,
) -> *mut core::ffi::c_void {
    let d = dst as *mut u8;
    let s = src as *const u8;
    let mut i = 0;
    while i < n {
        unsafe {
            *d.add(i) = *s.add(i);
        }
        i += 1;
    }
    dst
}

/// # Safety
/// `a` and `b` must be valid for reads of `n` bytes.
#[no_mangle]
pub unsafe extern "C" fn memcmp(
    a: *const core::ffi::c_void,
    b: *const core::ffi::c_void,
    n: usize,
) -> i32 {
    let a = a as *const u8;
    let b = b as *const u8;
    let mut i = 0;
    while i < n {
        let av = unsafe { *a.add(i) };
        let bv = unsafe { *b.add(i) };
        if av != bv {
            return av as i32 - bv as i32;
        }
        i += 1;
    }
    0
}

/// # Safety
/// `dst` and `src` must be valid for `n` bytes; regions may overlap.
#[no_mangle]
pub unsafe extern "C" fn memmove(
    dst: *mut core::ffi::c_void,
    src: *const core::ffi::c_void,
    n: usize,
) -> *mut core::ffi::c_void {
    let d = dst as *mut u8;
    let s = src as *const u8;
    if (d as usize) < (s as usize) {
        let mut i = 0;
        while i < n {
            unsafe {
                *d.add(i) = *s.add(i);
            }
            i += 1;
        }
    } else {
        let mut i = n;
        while i > 0 {
            i -= 1;
            unsafe {
                *d.add(i) = *s.add(i);
            }
        }
    }
    dst
}
