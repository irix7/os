// SPDX-License-Identifier: GPL-3.0-only
//! M1 single-node policy in the kernel.
//!
//! The native namespace/CAS, actor and capability state is the pre-Wasm
//! bootstrap. When slot B publishes a committed Wasm policy, [`adopt_wasm`]
//! replaces the kernel's root, actor, root capability, rights and home with the
//! values the guest chose. File reads then go through the committed
//! `wasm_policy_host::Policy`, not through the native namespace.
//!
//! The adopted policy is kernel state. The module publication record lives in
//! its own reserved page, so clearing or failing to reload slot B invalidates
//! only the publication marker; the previous live policy stays authoritative
//! until a new guest commits and the kernel adopts it.

use crate::capability::{self, CapTable, Capability, Rights};
use agnostic_core::{ActorState, ActorTable, RunQueue};
use cas_core::ObjectKind;
use cas_store::Namespace;
use m1_policy_bridge::{bridge_address, Bridge};
use mesh_core::{home_for, MembershipView};
use mesh_service::MembershipTable;
use sys_primitives::{ActorId, NodeId, ObjectId};
use wasm_policy_host::{Identity, Policy};

struct Local {
    namespace: Namespace,
    actors: ActorTable,
    queue: RunQueue,
    root: ObjectId,
    motd: ObjectId,
    actor: ActorId,
    home: NodeId,
    objects: usize,
    caps: CapTable,
    cap: Capability,
    verified: bool,
    policy: Option<&'static Policy>,
}

static mut LOCAL: Option<Local> = None;

/// Initialise the native local M1 bootstrap policy. Call after mesh and the
/// heap are up. A committed slot-B guest replaces this state in [`adopt_wasm`].
pub fn init() {
    // The bridge page is reserved memory, not zeroed state. Clear a stale
    // publication before anything can read it.
    begin_wasm_load();
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

/// Mark the live userland actor as running and take it off the run queue.
/// Milliseconds between `logical: tick` lines.  Three lines end the demo.
const TICK_PERIOD_MS: u64 = 1_000;
const TICK_LIMIT: u32 = 3;
static mut TICK_LAST_MS: u64 = 0;
static mut TICK_COUNT: u32 = 0;

/// The logical-computer heartbeat, formerly the `logical` module's tick callback.
/// Called from the idle loop; prints one line per second for the first three.
pub fn tick() {
    // SAFETY: the kernel is single-threaded; the idle loop is the only caller.
    unsafe {
        if TICK_COUNT >= TICK_LIMIT {
            return;
        }
        let now = crate::services::clock_ms();
        if now.saturating_sub(TICK_LAST_MS) < TICK_PERIOD_MS {
            return;
        }
        TICK_LAST_MS = now;
        TICK_COUNT += 1;
        crate::console_println(&alloc::format!("logical: tick {}", TICK_COUNT));
    }
}

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

/// Mark the live userland actor dead and reap it after the exit trap.
pub fn actor_leave() {
    let Some(local) = local_mut() else {
        return;
    };
    let _ = local.actors.set_state(local.actor, ActorState::Dead);
    let _ = local.actors.reap(local.actor);
}

/// The live userland actor identity.
pub fn actor_id() -> Option<ActorId> {
    local().map(|local| local.actor)
}

/// Whether a guest policy has been adopted and is live.
pub fn has_policy() -> bool {
    local().is_some_and(|local| local.policy.is_some())
}

/// Report the outcome of a failed slot-B load without changing live policy.
pub fn report_reload_failure() {
    if has_policy() {
        crate::console_println("m1-reload: failed policy=retained");
    } else {
        crate::console_println("m1-reload: failed policy=none");
    }
}

impl Local {
    /// Replace the native actor with the guest-chosen actor while preserving
    /// the one-node home and the single run-queue entry. The replacement is
    /// installed before the bootstrap actor is retired, so a failed adoption
    /// leaves the old actor live.
    fn adopt_actor(&mut self, actor: ActorId) -> bool {
        if actor == self.actor {
            return true;
        }
        let node = crate::mesh::node_id();
        if self.actors.spawn(actor).is_err() {
            return false;
        }
        if self.actors.place(actor, node).is_err() {
            let _ = self.actors.set_state(actor, ActorState::Dead);
            let _ = self.actors.reap(actor);
            return false;
        }
        if self.queue.push_fair(actor, 0).is_err() {
            let _ = self.actors.set_state(actor, ActorState::Dead);
            let _ = self.actors.reap(actor);
            return false;
        }
        let _ = self.queue.remove(self.actor);
        let _ = self.actors.set_state(self.actor, ActorState::Dead);
        let _ = self.actors.reap(self.actor);
        self.actor = actor;
        true
    }

    /// Adopt the committed guest policy as this kernel's live state.
    fn adopt_wasm(&mut self, policy: &'static Policy, identity: Identity) -> bool {
        // Build the replacement capability table first. Nothing is published
        // until every value is valid.
        let root_bytes = identity.root.as_bytes();
        let root_id =
            u32::from_be_bytes([root_bytes[0], root_bytes[1], root_bytes[2], root_bytes[3]]);
        let mut caps = capability::init();
        let Some(cap) = caps.alloc(1, root_id, Rights(identity.rights)) else {
            return false;
        };
        let verified = caps.permits(&cap.handle(), Rights(Rights::READ))
            && !caps.permits(&cap.handle(), Rights(Rights::MANAGE))
            && caps.object(&cap.handle()) == Some((1, root_id));
        if !verified {
            return false;
        }
        let Ok(motd) = policy.lookup("/etc/motd") else {
            return false;
        };
        let Ok((motd_kind, _)) = policy.get(&motd) else {
            return false;
        };
        let Ok(dev_null) = policy.lookup("/dev/null") else {
            return false;
        };
        let Ok((dev_kind, _)) = policy.get(&dev_null) else {
            return false;
        };
        if motd_kind != ObjectKind::Blob || dev_kind != ObjectKind::Blob {
            return false;
        }
        let Ok(stats) = policy.stats() else {
            return false;
        };
        if !self.adopt_actor(identity.actor) {
            return false;
        }
        self.caps = caps;
        self.cap = cap;
        self.root = identity.root;
        self.motd = motd;
        self.home = identity.home;
        self.objects = stats.objects as usize;
        self.verified = verified;
        self.policy = Some(policy);
        true
    }
}

/// Invalidate the slot-B publication record before its image is cleared or
/// replaced. The adopted `Local` state is deliberately untouched.
pub fn begin_wasm_load() {
    // SAFETY: the bridge page is reserved for this record and the kernel is
    // single-threaded.
    unsafe {
        core::ptr::write_volatile(bridge_address() as *mut Bridge, Bridge::empty());
    }
}

/// Adopt the committed wasm policy bridge if the module published one.
///
/// Returns `true` only when the guest root, actor, capability rights and home
/// have been installed in the kernel's live `Local` state. A missing, stale or
/// malformed record leaves the previous policy in place.
pub fn adopt_wasm() -> bool {
    // SAFETY: the bridge record lives at the reserved publication address and
    // is written before this boot-path call.
    let record = unsafe { &*(bridge_address() as *const Bridge) };
    let Some(context) = record.published_context() else {
        return false;
    };
    // SAFETY: the module allocated this committed Policy on the kernel heap;
    // it is not in slot B and it outlives the module image.
    let policy = unsafe { &*(context as *const Policy) };
    let Some(identity) = policy.committed_identity() else {
        crate::console_println("m1-live: wasm-policy rejected identity");
        return false;
    };
    let node = crate::mesh::node_id();
    if identity.home != node {
        crate::console_println("m1-live: wasm-policy rejected home");
        return false;
    }
    let Some(local) = local_mut() else {
        return false;
    };
    if !local.adopt_wasm(policy, identity) {
        crate::console_println("m1-live: wasm-policy rejected policy");
        return false;
    }
    crate::console_write("m1-live: wasm-policy adopted actor=");
    for byte in identity.actor.0 {
        crate::print_hex8(byte);
    }
    crate::console_write(" root=");
    for byte in identity.root.as_bytes() {
        crate::print_hex8(*byte);
    }
    crate::console_write(" rights=");
    write_dec(identity.rights as usize);
    crate::console_write(" home=");
    for byte in identity.home.as_bytes() {
        crate::print_hex8(*byte);
    }
    crate::console_println("");
    report(local);
    true
}

fn live_file(path: &[u8], offset: usize, out: &mut [u8]) -> Option<usize> {
    let local = local()?;
    let policy = local.policy?;
    if !local
        .caps
        .permits(&local.cap.handle(), Rights(Rights::READ))
    {
        return None;
    }
    let root = policy.root().ok()?;
    if root != local.root || !policy.authorize(Rights::READ, &root) {
        return None;
    }
    let path = core::str::from_utf8(path).ok()?;
    let oid = policy.lookup(path).ok()?;
    let (_, bytes) = policy.get(&oid).ok()?;
    if offset > bytes.len() {
        return None;
    }
    let count = out.len().min(bytes.len() - offset);
    out[..count].copy_from_slice(&bytes[offset..offset + count]);
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

/// Read a policy file through the live policy. `offset` is in object bytes.
pub fn read_file(path: &[u8], offset: usize, out: &mut [u8]) -> Option<usize> {
    if local().is_some_and(|local| local.policy.is_some()) {
        live_file(path, offset, out)
    } else {
        native_file(path, offset, out)
    }
}

/// Size of a policy file, using the same live/native selection as [`read_file`].
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
    match local.policy {
        Some(policy) => {
            let Ok(root) = policy.root() else {
                return false;
            };
            root == local.root && policy.lookup(path).is_ok()
        }
        None => local.namespace.lookup(path).is_ok(),
    }
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
        // OID; the adopted Wasm capability uses the same object identity and
        // the guest's rights.
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
            objects: namespace.store().len(),
            namespace,
            actors,
            queue,
            root,
            motd,
            actor,
            home: node,
            caps,
            cap,
            verified,
            policy: None,
        })
    }
}

fn report(local: &Local) {
    let mut buf = [0u8; 512];
    let mut out = Out::new(&mut buf);
    out.str(b"m1: root=");
    out.hex(local.root.as_bytes());
    out.str(b" motd=");
    out.hex(local.motd.as_bytes());
    out.str(b" actor=");
    out.hex(&local.actor.0);
    out.str(b" objects=");
    out.dec(local.objects);
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
    out.hex(local.home.as_bytes());
    out.str(if local.policy.is_some() {
        b" policy=wasm\n"
    } else {
        b" policy=native\n"
    });
    crate::write_bytes(out.slice());
}

fn write_dec(value: usize) {
    irix7_fmt::decimal(value as u64, &mut |b| crate::write_byte(b));
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
    fn dec(&mut self, value: usize) {
        irix7_fmt::decimal(value as u64, &mut |b| self.push(b));
    }
    fn slice(&self) -> &[u8] {
        &self.buf[..self.len]
    }
}
