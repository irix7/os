// SPDX-License-Identifier: GPL-3.0-only
//! M1 single-node policy probe as a loadable module.
//!
//! This is the first runtime policy that ships outside the compressed PROM.
//! It builds a local namespace/CAS, an actor directory and run queue, and a
//! 1-of-1 Ed25519 capability over the root directory. Everything resolves to
//! the local MAC-derived node id and no fabric frame is constructed.

#![no_std]
#![no_main]

extern crate alloc;

use core::alloc::{GlobalAlloc, Layout};
use core::ptr::{self, null_mut};
use core::sync::atomic::{AtomicPtr, Ordering};

use agnostic_core::{ActorTable, RunQueue};
use cap_engine::{node_key, sign_capability, verify_capability, TrustStore};
use cas_core::ObjectKind;
use cas_store::Namespace;
use ed25519_dalek::SigningKey;
use mesh_core::{home_for, MembershipView};
use mesh_service::MembershipTable;
use module_abi::{ModuleVtable, Services};
use sys_primitives::{ActorId, Capability, NodeId, ObjectId, PrincipalId, Rights};

/// The service table, published before the first allocation.
static SERVICES: AtomicPtr<Services> = AtomicPtr::new(ptr::null_mut());

struct ModuleAllocator;

unsafe impl GlobalAlloc for ModuleAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let table = SERVICES.load(Ordering::Acquire);
        if table.is_null() {
            return null_mut();
        }
        // SAFETY: the table came from `_start` and stays valid for the module.
        let allocate = unsafe { (*table).alloc };
        unsafe { allocate(layout.size(), layout.align()) }
    }

    unsafe fn dealloc(&self, _ptr: *mut u8, _layout: Layout) {
        // The kernel heap is a bump allocator; memory is reclaimed by reboot.
    }
}

#[global_allocator]
static ALLOCATOR: ModuleAllocator = ModuleAllocator;

struct Local {
    namespace: Namespace,
    actors: ActorTable,
    queue: RunQueue,
    root: ObjectId,
    motd: ObjectId,
    actor: ActorId,
    token: Capability,
    issued: bool,
}

static mut LOCAL: Option<Local> = None;
static mut LAST_TICK: u64 = 0;
static mut TICKS: u32 = 0;

static VTABLE: ModuleVtable = ModuleVtable {
    status: 0,
    tick: Some(tick),
};
static ERROR_VTABLE: ModuleVtable = ModuleVtable {
    status: 1,
    tick: None,
};

/// Module entry.
///
/// # Safety
/// `services` must point to a valid service table supplied by the kernel.
#[no_mangle]
pub unsafe extern "C" fn _start(services: *const Services) -> *const ModuleVtable {
    let Some(services) = (unsafe { services.as_ref() }) else {
        return &ERROR_VTABLE;
    };
    if services.version < 10 {
        return &ERROR_VTABLE;
    }
    SERVICES.store(
        services as *const Services as *mut Services,
        Ordering::Release,
    );
    let node = node_id(services);
    let Some(local) = Local::start(services, node) else {
        write(services, b"logical: policy start failed\n");
        return &ERROR_VTABLE;
    };
    report(services, &local);
    unsafe {
        LAST_TICK = (services.clock_ms)();
        TICKS = 0;
        LOCAL = Some(local);
    }
    &VTABLE
}

impl Local {
    fn start(services: &Services, node: NodeId) -> Option<Self> {
        let table = MembershipTable::single(node, (services.clock_ms)());
        let view: MembershipView = table.snapshot();
        let mut namespace = Namespace::new();
        namespace
            .create_dir_all("/etc")
            .expect("local mkdir in bootstrap module");
        let motd = namespace
            .put("/etc/motd", ObjectKind::Blob, b"Welcome to IRIX 7.\n")
            .expect("local motd in bootstrap module");
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

        // A demo 1-of-1 authority. The seed is derived from the local node id
        // and the boot clock; persistent key custody is a later native-shim
        // responsibility, not a module one.
        let mut seed = [0u8; 32];
        let mut hasher = blake3::Hasher::new();
        hasher.update(b"IRIX7-M1-LOCAL-AUTHORITY-v1");
        hasher.update(node.as_bytes());
        hasher.update(&(services.clock_ms)().to_be_bytes());
        seed.copy_from_slice(hasher.finalize().as_bytes());
        let signing = SigningKey::from_bytes(&seed);
        let issuer = node_key(&signing);
        let mut trust = TrustStore::new();
        trust.insert(issuer);
        let mut token = Capability {
            magic: Capability::MAGIC,
            version: Capability::VERSION,
            rights: Rights::READ,
            flags: 0,
            object: root,
            issuer,
            subject: PrincipalId([0x33; 32]),
            home: node,
            not_before: 0,
            not_after: u64::MAX,
            object_epoch: 0,
            home_epoch: view.epoch,
            root_epoch: 0,
            reserved0: 0,
            nonce: [0x44; 16],
            parent: ObjectId::ZERO,
            delegation: ObjectId::ZERO,
            reserved1: [0; 24],
            signature: [0; 64],
        };
        sign_capability(&signing, &mut token);
        let issued = verify_capability(&trust, &token).is_ok()
            && token
                .verify(
                    &trust,
                    (services.clock_ms)().saturating_mul(1_000_000),
                    Rights::READ,
                    root,
                )
                .is_ok();

        Some(Self {
            namespace,
            actors,
            queue,
            root,
            motd,
            actor,
            token,
            issued,
        })
    }

    fn summary(&self) -> Summary {
        Summary {
            root: self.root,
            motd: self.motd,
            actor: self.actor,
            objects: self.namespace.store().len(),
            actors: self.actors.len(),
            queued: self.queue.len(),
            issued: self.issued,
            token_home: self.token.home,
        }
    }
}

struct Summary {
    root: ObjectId,
    motd: ObjectId,
    actor: ActorId,
    objects: usize,
    actors: usize,
    queued: usize,
    issued: bool,
    token_home: NodeId,
}

fn report(services: &Services, local: &Local) {
    let summary = local.summary();
    let mut buf = [0u8; 384];
    let mut out = Out::new(&mut buf);
    out.str(b"logical: root=");
    out.hex(summary.root.as_bytes());
    out.str(b" motd=");
    out.hex(summary.motd.as_bytes());
    out.str(b" actor=");
    out.hex(&summary.actor.0);
    out.str(b" objects=");
    out.dec(summary.objects);
    out.str(b" actors=");
    out.dec(summary.actors);
    out.str(b" queued=");
    out.dec(summary.queued);
    out.str(b" cap=");
    out.str(if summary.issued { b"ok" } else { b"FAILED" });
    out.str(b" home=");
    out.hex(summary.token_home.as_bytes());
    out.str(b"\n");
    write(services, out.slice());
}

unsafe extern "C" fn tick(services: *const Services) -> i32 {
    let Some(services) = (unsafe { services.as_ref() }) else {
        return 1;
    };
    let now = (services.clock_ms)();
    let last = unsafe { LAST_TICK };
    if now.saturating_sub(last) < 1000 {
        return 0;
    }
    unsafe {
        LAST_TICK = now;
        TICKS += 1;
    }
    let count = unsafe { TICKS };
    let mut buf = [0u8; 48];
    let mut out = Out::new(&mut buf);
    out.str(b"logical: tick ");
    out.dec(count as usize);
    out.str(b"\n");
    write(services, out.slice());
    if count >= 3 {
        1
    } else {
        0
    }
}

fn node_id(services: &Services) -> NodeId {
    let mut text = [0u8; 256];
    let n = unsafe { (services.info)(1, text.as_mut_ptr(), text.len()) };
    if n > 0 {
        let text = core::str::from_utf8(&text[..n as usize]).unwrap_or("");
        if let Some(mac) = text.find("mac: ") {
            let mac = &text[mac + 5..];
            let bytes = mac.as_bytes();
            if bytes.len() >= 17 {
                let mut node = [0u8; 16];
                let mut valid = true;
                for i in 0..6 {
                    let hi = hex_nibble(bytes[i * 3]);
                    let lo = hex_nibble(bytes[i * 3 + 1]);
                    match (hi, lo) {
                        (Some(hi), Some(lo)) => node[i] = (hi << 4) | lo,
                        _ => {
                            valid = false;
                            break;
                        }
                    }
                }
                if valid {
                    return NodeId(node);
                }
            }
        }
    }
    NodeId([
        0x02, 0x00, 0x69, 0x12, 0x34, 0x56, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    ])
}

fn hex_nibble(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
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

fn write(services: &Services, bytes: &[u8]) {
    (services.write)(bytes.as_ptr(), bytes.len());
}

#[cfg(not(test))]
#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    loop {
        core::hint::spin_loop();
    }
}
