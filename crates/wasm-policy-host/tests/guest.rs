// SPDX-License-Identifier: GPL-3.0-only
//! Run the committed policy guest against the native policy primitives.
//!
//! This is the host-side reproduction of the iris slot-B boot: the guest is
//! the same binary that `modules/wasm` embeds, and the hooks are the same
//! `wasm-policy-host` primitives the module supplies.

use std::sync::Mutex;

use cas_core::ObjectKind;
use cas_store::Namespace;
use sys_primitives::NodeId;
use wasm_host::{Host, HostHooks, PolicyHooks};
use wasm_policy_host::Policy;

static EVIDENCE: Mutex<Vec<u8>> = Mutex::new(Vec::new());

fn clock_ms() -> u64 {
    0
}

fn console_write(bytes: &[u8]) {
    EVIDENCE
        .lock()
        .expect("evidence lock")
        .extend_from_slice(bytes);
}

fn policy(context: usize) -> &'static mut Policy {
    // SAFETY: the test leaks the allocation for the duration of the run and
    // the hooks are called synchronously.
    unsafe { &mut *(context as *mut Policy) }
}

fn node_id(context: usize, out: &mut [u8; 16]) -> i32 {
    out.copy_from_slice(policy(context).node().as_bytes());
    0
}

fn begin(context: usize) -> i32 {
    match policy(context).begin() {
        Ok(()) => 0,
        Err(_) => -1,
    }
}

fn abort(context: usize) {
    policy(context).abort();
}

fn commit(context: usize) -> i32 {
    match policy(context).commit() {
        Ok(_) => 0,
        Err(_) => -1,
    }
}

fn mkdir(context: usize, path: &str, out: &mut [u8; 32]) -> i32 {
    match policy(context).mkdir(path) {
        Ok(oid) => {
            out.copy_from_slice(oid.as_bytes());
            0
        }
        Err(_) => -1,
    }
}

fn put(context: usize, path: &str, kind: u8, bytes: &[u8], out: &mut [u8; 32]) -> i32 {
    match policy(context).put(path, kind, bytes) {
        Ok(oid) => {
            out.copy_from_slice(oid.as_bytes());
            0
        }
        Err(_) => -1,
    }
}

fn lookup(context: usize, path: &str, out: &mut [u8; 32]) -> i32 {
    match policy(context).lookup(path) {
        Ok(oid) => {
            out.copy_from_slice(oid.as_bytes());
            0
        }
        Err(_) => -1,
    }
}

fn get(context: usize, oid: &[u8; 32], kind: &mut i32, out: &mut [u8; 512]) -> i32 {
    match policy(context).get(&sys_primitives::ObjectId(*oid)) {
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

fn root(context: usize, out: &mut [u8; 32]) -> i32 {
    match policy(context).root() {
        Ok(oid) => {
            out.copy_from_slice(oid.as_bytes());
            0
        }
        Err(_) => -1,
    }
}

fn home(context: usize, oid: &[u8; 32], out: &mut [u8; 16]) -> i32 {
    match policy(context).home(&sys_primitives::ObjectId(*oid)) {
        Some(node) => {
            out.copy_from_slice(node.as_bytes());
            0
        }
        None => -1,
    }
}

fn actor_spawn(context: usize, id: &[u8; 16]) -> i32 {
    match policy(context).actor_spawn(sys_primitives::ActorId(*id)) {
        Ok(()) => 0,
        Err(_) => -1,
    }
}

fn actor_place(context: usize, id: &[u8; 16], host: &[u8; 16]) -> i32 {
    match policy(context).actor_place(sys_primitives::ActorId(*id), NodeId(*host)) {
        Ok(()) => 0,
        Err(_) => -1,
    }
}

fn actor_queue(context: usize, id: &[u8; 16]) -> i32 {
    match policy(context).actor_queue(sys_primitives::ActorId(*id)) {
        Ok(()) => 0,
        Err(_) => -1,
    }
}

fn cap_issue(
    context: usize,
    object: &[u8; 32],
    rights: u32,
    subject: &[u8; 32],
    home: &[u8; 16],
) -> i64 {
    match policy(context).cap_issue(
        sys_primitives::ObjectId(*object),
        rights,
        *subject,
        NodeId(*home),
    ) {
        Ok(handle) => handle as i64,
        Err(_) => 0,
    }
}

fn cap_check(context: usize, handle: u64, rights: u32, object: &[u8; 32]) -> i32 {
    if policy(context).cap_check(handle, rights, &sys_primitives::ObjectId(*object)) {
        0
    } else {
        -1
    }
}

fn cap_rights(context: usize, handle: u64, object: &[u8; 32]) -> i32 {
    match policy(context).cap_rights(handle, &sys_primitives::ObjectId(*object)) {
        Some(rights) => rights as i32,
        None => -1,
    }
}

fn stats(context: usize, out: &mut [u32; 4]) -> i32 {
    match policy(context).stats() {
        Ok(summary) => {
            out[0] = summary.objects;
            out[1] = summary.actors;
            out[2] = summary.queued;
            out[3] = summary.caps;
            0
        }
        Err(_) => -1,
    }
}

#[test]
fn guest_drives_the_local_policy_and_prints_reproducible_evidence() {
    // wasmi's debug-build interpreter uses a deep host stack; give this test
    // its own thread rather than depending on the harness default.
    std::thread::Builder::new()
        .stack_size(16 * 1024 * 1024)
        .spawn(run_policy_guest)
        .expect("test thread")
        .join()
        .expect("policy guest");
}

fn run_policy_guest() {
    EVIDENCE.lock().expect("evidence lock").clear();
    let node = NodeId([
        0x02, 0x11, 0x22, 0x33, 0x44, 0x55, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    ]);
    let policy = Box::into_raw(Box::new(Policy::new(node, 0)));
    let hooks = HostHooks {
        clock_ms,
        console_write,
        policy: PolicyHooks {
            context: policy as usize,
            node_id,
            begin,
            abort,
            commit,
            mkdir,
            put,
            lookup,
            get,
            root,
            home,
            actor_spawn,
            actor_place,
            actor_queue,
            cap_issue,
            cap_check,
            cap_rights,
            stats,
        },
    };
    let host = Host::with_hooks(hooks).expect("host");
    let result = host
        .run_i32(
            include_bytes!("../../../modules/wasm/src/guest.wasm"),
            "run",
            10_000_000,
        )
        .expect("guest run");
    assert_eq!(result, 42);
    // SAFETY: the test owns the leaked policy and the guest has returned.
    assert!(unsafe { (*policy).is_committed() });

    let evidence =
        String::from_utf8(EVIDENCE.lock().expect("evidence lock").clone()).expect("utf-8 evidence");
    assert!(evidence.contains("wasm: host=1\n"));
    assert!(evidence.contains("m1-wasm: root="));
    assert!(evidence.contains(" cap=ok rights=35 home="));
    assert!(evidence.contains("objects=5 actors=1 queued=1 caps=1\n"));

    let mut namespace = Namespace::new();
    namespace.create_dir_all("/etc").expect("etc");
    namespace
        .put("/etc/motd", ObjectKind::Blob, b"Welcome to IRIX 7.\n")
        .expect("motd");
    let expected = hex(namespace.root().as_bytes());
    assert!(
        evidence.contains(&format!("root={expected}")),
        "evidence did not match the native root: {evidence}"
    );
    assert!(evidence.contains(&format!(
        "actor={}",
        hex(&{
            let mut actor = node.0;
            actor[15] = 1;
            actor
        })
    )));
    assert!(evidence.contains(&format!("home={}", hex(node.as_bytes()))));
}

fn hex(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push_str(&format!("{byte:02x}"));
    }
    out
}
