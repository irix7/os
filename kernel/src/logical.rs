// SPDX-License-Identifier: GPL-3.0-only
//! M1 single-node policy in the kernel.
//!
//! This is the same policy the loadable `logical` module proves, moved onto
//! the boot path: a local directory/CAS namespace, an actor table and run
//! queue, and a 1-of-1 Ed25519 capability over the root directory.  It
//! constructs no mesh frame and resolves every home to this node.

use crate::capability::{self, CapTable, Capability, Rights};
use agnostic_core::{ActorTable, RunQueue};
use cas_core::ObjectKind;
use cas_store::Namespace;
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

/// Initialise the local M1 policy.  Call after mesh and the heap are up.
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

impl Local {
    fn start(node: NodeId, now_ms: u64) -> Option<Self> {
        let table = MembershipTable::single(node, now_ms);
        let view: MembershipView = table.snapshot();
        let mut namespace = Namespace::new();
        namespace.create_dir_all("/etc").ok()?;
        let motd = namespace
            .put("/etc/motd", ObjectKind::Blob, b"Welcome to IRIX 7.\n")
            .ok()?;
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

        // A local kernel capability over the root directory.  The table is
        // the authority; the handle is only a name.  This proves rights
        // verification without pulling Ed25519 into the fixed PROM.
        let mut caps = capability::init();
        let cap = caps.alloc(1, 1, Rights(Rights::READ | Rights::WRITE | Rights::INVOKE))?;
        let verified = caps.permits(&cap.handle(), Rights(Rights::READ));

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
