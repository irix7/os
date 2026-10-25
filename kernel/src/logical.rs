// SPDX-License-Identifier: GPL-3.0-only
//! M1 single-node policy in the kernel.
//!
//! This is the native fallback and the live policy for the pre-Wasm boot. A
//! committed slot-B Wasm policy can publish a read-only bridge; the kernel
//! adopts it for the userland `/etc/motd` path while retaining this native
//! namespace/CAS/actor/capability state as the fallback.

use crate::capability::{self, CapTable, Capability, Rights};
use agnostic_core::{ActorState, ActorTable, RunQueue};
use cas_core::ObjectKind;
use cas_store::Namespace;
use m1_policy_bridge::{bridge_address, Bridge, MAGIC, VERSION};
use mesh_core::{home_for, MembershipView};
use mesh_service::MembershipTable;
use sys_primitives::{ActorId, NodeId, ObjectId};

struct Local {
    namespace: Namespace,
    actors: ActorTable,
    queue: RunQueue,
    root: ObjectId,
    motd: ObjectId,
    actor: ActorId,
    caps: CapTable,
    cap: Capability,
    verified: bool,
}

static mut LOCAL: Option<Local> = None;
static mut WASM: Option<&'static Bridge> = None;

/// Initialise the native local M1 policy. Call after mesh and the heap are up.
pub fn init() {
    let node = crate::mesh::node_id();
    if node.is_zero() {
        crate::println("m1: no local node");
        return;
    }
    let now_ms = crate::idle::ticks() as u64 * 10;
    let Some(local) = Local::start(node, now_ms) else {
        crate::println("m1: policy start failed");
        return;
    };
    report(&local);
    unsafe {
        LOCAL = Some(local);
    }
}

fn local() -> Option<&'static Local> {
    // SAFETY: single-threaded kernel; LOCAL is initialised once at boot.
    unsafe { (&*core::ptr::addr_of!(LOCAL)).as_ref() }
}

fn local_mut() -> Option<&'static mut Local> {
    // SAFETY: single-threaded kernel; LOCAL is initialised once at boot.
    unsafe { (&mut *core::ptr::addr_of_mut!(LOCAL)).as_mut() }
}

/// Mark the single native userland actor as running and take it off the run
/// queue. This makes the M1 actor table a live userland path, not only a boot
/// self-test.
pub fn actor_enter() -> bool {
    let Some(local) = local_mut() else {
        return false;
    };
    if local.queue.pick() != Some(local.actor) {
        return false;
    }
    local
        .actors
        .set_state(local.actor, ActorState::Running)
        .is_ok()
}

/// Mark the native userland actor dead and reap it after the exit trap.
pub fn actor_leave() {
    let Some(local) = local_mut() else {
        return;
    };
    let _ = local.actors.set_state(local.actor, ActorState::Dead);
    let _ = local.actors.reap(local.actor);
}

fn wasm() -> Option<&'static Bridge> {
    // SAFETY: single-threaded kernel; the bridge is published before userland.
    unsafe { WASM }
}

/// Drop a previously adopted bridge before slot B is cleared or replaced.
pub fn forget_wasm() {
    // SAFETY: single-threaded kernel; the next userland read falls back to the
    // native policy until a new guest commits.
    unsafe { WASM = None };
}

/// Adopt the committed wasm policy bridge if the module published one.
pub fn adopt_wasm() {
    // SAFETY: the bridge record lives at the fixed end of the reserved slot B.
    let bridge = unsafe { &*(bridge_address() as *const Bridge) };
    if bridge.magic == MAGIC && bridge.version == VERSION {
        // SAFETY: single-threaded kernel; the bridge and Policy live for the
        // module's lifetime.
        unsafe { WASM = Some(bridge) };
        crate::console_println("m1-live: wasm-policy adopted");
    }
}

fn wasm_file(path: &[u8], offset: usize, out: &mut [u8]) -> Option<usize> {
    let bridge = wasm()?;
    let path = core::str::from_utf8(path).ok()?;
    let mut root = [0u8; 32];
    if unsafe { (bridge.root)(bridge.context, root.as_mut_ptr()) } != 0 {
        crate::console_println("m1-live: policy root failed");
        return None;
    }
    // The M1 guest issues one capability over the namespace root; traversal
    // through a path uses that root authority.
    if unsafe { (bridge.authorize)(bridge.context, root.as_ptr(), Rights::READ) } != 0 {
        crate::console_println("m1-live: policy authorize failed");
        return None;
    }
    let mut oid = [0u8; 32];
    if unsafe { (bridge.lookup)(bridge.context, path.as_ptr(), path.len(), oid.as_mut_ptr()) } != 0
    {
        crate::console_println("m1-live: motd lookup failed");
        return None;
    }
    let mut kind = 0i32;
    let mut data = [0u8; 512];
    let length = unsafe {
        (bridge.get)(
            bridge.context,
            oid.as_ptr(),
            &mut kind,
            data.as_mut_ptr(),
            data.len(),
        )
    };
    if length < 0 || length as usize > data.len() || offset > length as usize {
        return None;
    }
    let count = out.len().min(length as usize - offset);
    out[..count].copy_from_slice(&data[offset..offset + count]);
    Some(count)
}

fn native_file(path: &[u8], offset: usize, out: &mut [u8]) -> Option<usize> {
    let local = local()?;
    if !local
        .caps
        .permits(&local.cap.handle(), Rights(Rights::READ))
    {
        return None;
    }
    let path = core::str::from_utf8(path).ok()?;
    let oid = local.namespace.lookup(path).ok()?;
    let bytes = local.namespace.store().get(oid)?;
    if offset > bytes.len() {
        return None;
    }
    let count = out.len().min(bytes.len() - offset);
    out[..count].copy_from_slice(&bytes[offset..offset + count]);
    Some(count)
}

/// Read a policy file through the adopted Wasm policy, falling back to the
/// native kernel policy. `offset` is in object bytes.
pub fn read_file(path: &[u8], offset: usize, out: &mut [u8]) -> Option<usize> {
    if wasm().is_some() {
        wasm_file(path, offset, out)
    } else {
        native_file(path, offset, out)
    }
}

/// Size of a policy file, using the same Wasm/native selection as [`read_file`].
pub fn file_size(path: &[u8]) -> Option<usize> {
    let mut probe = [0u8; 512];
    let mut total = 0usize;
    loop {
        let count = read_file(path, total, &mut probe)?;
        if count == 0 {
            return Some(total);
        }
        total += count;
    }
}

/// Check whether the live policy exposes a readable file at `path`.
pub fn open_file(path: &[u8]) -> bool {
    file_size(path).is_some()
}

/// Check whether the live policy exposes a device object at `path`.
pub fn device(path: &[u8]) -> bool {
    if let Some(bridge) = wasm() {
        let mut root = [0u8; 32];
        if unsafe { (bridge.root)(bridge.context, root.as_mut_ptr()) } != 0 {
            return false;
        }
        if unsafe { (bridge.authorize)(bridge.context, root.as_ptr(), Rights::READ) } != 0 {
            return false;
        }
        let Ok(path) = core::str::from_utf8(path) else {
            return false;
        };
        let mut oid = [0u8; 32];
        return unsafe {
            (bridge.lookup)(bridge.context, path.as_ptr(), path.len(), oid.as_mut_ptr())
        } == 0;
    }
    let Some(local) = local() else {
        return false;
    };
    if !local
        .caps
        .permits(&local.cap.handle(), Rights(Rights::READ))
    {
        return false;
    }
    let Ok(path) = core::str::from_utf8(path) else {
        return false;
    };
    local.namespace.lookup(path).is_ok()
}

impl Local {
    fn start(node: NodeId, now_ms: u64) -> Option<Self> {
        let table = MembershipTable::single(node, now_ms);
        let view: MembershipView = table.snapshot();
        let mut namespace = Namespace::new();
        namespace.create_dir_all("/etc").ok()?;
        let motd = namespace
            .put("/etc/motd", ObjectKind::Blob, b"Welcome to IRIX 7.\n")
            .ok()?;
        // The device namespace is part of the same local policy, so M1 can
        // exercise a capability-backed /dev object rather than only a static
        // fd special case.
        namespace.create_dir_all("/dev").ok()?;
        let dev_null = namespace.put("/dev/null", ObjectKind::Blob, b"").ok()?;
        // Fetch the stored objects back by OID; a path lookup alone is not a
        // CAS read.
        if namespace.store().get(motd)? != b"Welcome to IRIX 7.\n"
            || namespace.store().kind(dev_null) != Some(ObjectKind::Blob)
        {
            return None;
        }
        let root = namespace.root();
        if home_for(&root, &view) != Some(node) {
            return None;
        }

        let mut actors = ActorTable::new(view.clone());
        let mut actor_bytes = node.0;
        actor_bytes[15] = 1;
        let actor = ActorId(actor_bytes);
        actors.spawn(actor).ok()?;
        actors.place(actor, node).ok()?;
        let mut queue = RunQueue::new();
        queue.push_fair(actor, 0).ok()?;

        // Bind the native bring-up capability to the first word of the root
        // OID; the Wasm bridge keeps the full OID in `wasm-policy-host`.
        let mut caps = capability::init();
        let root_bytes = root.as_bytes();
        let root_id =
            u32::from_be_bytes([root_bytes[0], root_bytes[1], root_bytes[2], root_bytes[3]]);
        let cap = caps.alloc(
            1,
            root_id,
            Rights(Rights::READ | Rights::WRITE | Rights::INVOKE),
        )?;
        let verified = caps.permits(&cap.handle(), Rights(Rights::READ))
            && !caps.permits(&cap.handle(), Rights(Rights::MANAGE))
            && caps.object(&cap.handle()) == Some((1, root_id));

        Some(Self {
            namespace,
            actors,
            queue,
            root,
            motd,
            actor,
            caps,
            cap,
            verified,
        })
    }
}

fn report(local: &Local) {
    let mut buf = [0u8; 384];
    let mut out = Out::new(&mut buf);
    out.str(b"m1: root=");
    out.hex(local.root.as_bytes());
    out.str(b" motd=");
    out.hex(local.motd.as_bytes());
    out.str(b" actor=");
    out.hex(&local.actor.0);
    out.str(b" objects=");
    out.dec(local.namespace.store().len());
    out.str(b" actors=");
    out.dec(local.actors.len());
    out.str(b" queued=");
    out.dec(local.queue.len());
    out.str(b" cap=");
    out.str(if local.verified { b"ok" } else { b"FAILED" });
    out.str(b" rights=");
    out.dec(local.cap.rights().0 as usize);
    out.str(b" caps=");
    out.dec(local.caps.count());
    out.str(b" mesh=");
    out.dec(crate::mesh::frames() as usize);
    out.str(b"/");
    out.dec(crate::mesh::rpcs() as usize);
    out.str(b"/");
    out.dec(crate::mesh::envelopes() as usize);
    out.str(b" home=");
    out.hex(crate::mesh::node_id().as_bytes());
    out.str(b"\n");
    crate::write_bytes(out.slice());
}

struct Out<'a> {
    buf: &'a mut [u8],
    len: usize,
}

impl<'a> Out<'a> {
    fn new(buf: &'a mut [u8]) -> Self {
        Self { buf, len: 0 }
    }
    fn push(&mut self, byte: u8) {
        if self.len < self.buf.len() {
            self.buf[self.len] = byte;
            self.len += 1;
        }
    }
    fn str(&mut self, bytes: &[u8]) {
        for &byte in bytes {
            self.push(byte);
        }
    }
    fn hex(&mut self, bytes: &[u8]) {
        const HEX: &[u8; 16] = b"0123456789abcdef";
        for &byte in bytes {
            self.push(HEX[(byte >> 4) as usize]);
            self.push(HEX[(byte & 0x0f) as usize]);
        }
    }
    fn dec(&mut self, mut value: usize) {
        let mut digits = [0u8; 20];
        let mut count = 0;
        if value == 0 {
            self.push(b'0');
            return;
        }
        while value > 0 && count < digits.len() {
            digits[count] = b'0' + (value % 10) as u8;
            value /= 10;
            count += 1;
        }
        while count > 0 {
            count -= 1;
            self.push(digits[count]);
        }
    }
    fn slice(&self) -> &[u8] {
        &self.buf[..self.len]
    }
}
