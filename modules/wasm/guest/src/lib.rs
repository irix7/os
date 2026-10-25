// SPDX-License-Identifier: GPL-3.0-only
//! M1 local-policy guest for the slot-B Wasm core.
//!
//! The guest owns the policy decision.  It chooses the local namespace layout,
//! the object kinds and bytes, the actor identity, the capability rights and
//! the order of every check.  The `i7host` callbacks supply bulk storage,
//! object hashing, the actor and capability tables and the clock.  The guest
//! never receives a native pointer or a kernel service table.
//!
//! It prints `wasm: host=1` for the host-ABI probe, then
//! `wasm-policy: root=... actor=... cap=ok home=...` once the policy is
//! committed.  Any failure aborts the stage and returns a non-42 status.

#![no_std]

const HOST_ABI_VERSION: i32 = 1;
const KIND_BLOB: i32 = 1;
const RIGHT_READ: i32 = 1;
const RIGHT_WRITE: i32 = 1 << 1;
const RIGHT_INVOKE: i32 = 1 << 5;
const RIGHT_MANAGE: i32 = 1 << 9;
const RIGHTS: i32 = RIGHT_READ | RIGHT_WRITE | RIGHT_INVOKE;

const MOTD: &[u8] = b"Welcome to IRIX 7.\n";
const ETC: &[u8] = b"/etc";
const MOTD_PATH: &[u8] = b"/etc/motd";

#[link(wasm_import_module = "i7host")]
extern "C" {
    fn host_version() -> i32;
    fn host_now() -> i64;
    fn host_submit(op: i32, a0: i64, a1: i64, a2: i64, a3: i64) -> i64;
    fn host_poll(out: i32, max: i32) -> i32;
    fn host_cancel(token: i64) -> i32;
    fn host_console_write(ptr: i32, len: i32);
    fn host_yield();
    fn host_node_id(out: i32) -> i32;
    fn host_policy_begin() -> i32;
    fn host_policy_abort();
    fn host_policy_commit() -> i32;
    fn host_policy_stats(out: i32) -> i32;
    fn host_ns_mkdir(path: i32, len: i32, out: i32) -> i32;
    fn host_ns_put(path: i32, path_len: i32, kind: i32, data: i32, data_len: i32, out: i32) -> i32;
    fn host_ns_lookup(path: i32, len: i32, out: i32) -> i32;
    fn host_ns_get(oid: i32, kind_out: i32, out: i32, capacity: i32) -> i32;
    fn host_ns_root(out: i32) -> i32;
    fn host_home(oid: i32, out: i32) -> i32;
    fn host_actor_spawn(id: i32) -> i32;
    fn host_actor_place(id: i32, host: i32) -> i32;
    fn host_actor_queue(id: i32) -> i32;
    fn host_cap_issue(object: i32, rights: i32, subject: i32, home: i32) -> i64;
    fn host_cap_check(handle: i64, rights: i32, object: i32) -> i32;
}

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    loop {
        core::hint::spin_loop();
    }
}

fn address<T>(value: &T) -> i32 {
    value as *const T as usize as i32
}

fn address_mut<T>(value: &mut T) -> i32 {
    value as *mut T as usize as i32
}

fn write(bytes: &[u8]) {
    unsafe { host_console_write(address(&bytes[0]), bytes.len() as i32) };
}

fn fail(step: i32) -> i32 {
    unsafe { host_policy_abort() };
    let mut buffer = [0u8; 48];
    let mut out = Out::new(&mut buffer);
    out.text(b"wasm-policy: FAILED step=");
    out.dec(step as usize);
    out.text(b"\n");
    write(out.slice());
    step
}

#[no_mangle]
pub extern "C" fn run() -> i32 {
    if unsafe { host_version() } != HOST_ABI_VERSION {
        return fail(1);
    }
    let now = unsafe { host_now() };

    // The host-ABI probe from T1: version, clock, submit, poll and cancel.
    write(b"wasm: host=1\n");
    let token = unsafe { host_submit(1, now, 0, 0, 0) };
    if token != 1 {
        return fail(2);
    }
    let mut completion = [0u8; 128];
    if unsafe { host_poll(address_mut(&mut completion), 1) } != 1 {
        return fail(3);
    }
    if unsafe { host_cancel(1) } != 0 {
        return fail(4);
    }

    // Identity comes from the machine, never from an ambient guest capability.
    let mut node = [0u8; 16];
    if unsafe { host_node_id(address_mut(&mut node)) } != 0 {
        return fail(5);
    }
    if node.iter().all(|&byte| byte == 0) {
        return fail(6);
    }

    if unsafe { host_policy_begin() } != 0 {
        return fail(7);
    }

    // The guest chooses the local namespace layout and object content.
    let mut after_mkdir = [0u8; 32];
    if unsafe {
        host_ns_mkdir(
            ETC.as_ptr() as i32,
            ETC.len() as i32,
            address_mut(&mut after_mkdir),
        )
    } != 0
    {
        return fail(8);
    }
    let mut motd = [0u8; 32];
    if unsafe {
        host_ns_put(
            MOTD_PATH.as_ptr() as i32,
            MOTD_PATH.len() as i32,
            KIND_BLOB,
            MOTD.as_ptr() as i32,
            MOTD.len() as i32,
            address_mut(&mut motd),
        )
    } != 0
    {
        return fail(9);
    }

    // Validate the path and fetch the object by OID, not by ambient handle.
    let mut looked_up = [0u8; 32];
    if unsafe {
        host_ns_lookup(
            MOTD_PATH.as_ptr() as i32,
            MOTD_PATH.len() as i32,
            address_mut(&mut looked_up),
        )
    } != 0
        || looked_up != motd
    {
        return fail(10);
    }
    let mut kind = 0i32;
    let mut bytes = [0u8; 64];
    let length = unsafe {
        host_ns_get(
            address(&motd),
            address_mut(&mut kind),
            address_mut(&mut bytes),
            bytes.len() as i32,
        )
    };
    if length != MOTD.len() as i32 || kind != KIND_BLOB || bytes[..length as usize] != *MOTD {
        return fail(11);
    }

    let mut root = [0u8; 32];
    if unsafe { host_ns_root(address_mut(&mut root)) } != 0 {
        return fail(12);
    }
    let mut home = [0u8; 16];
    if unsafe { host_home(address(&root), address_mut(&mut home)) } != 0 || home != node {
        return fail(13);
    }

    // The actor id is derived from the node id, and the guest checks the home
    // before asking the host to place or queue it.
    let mut actor = node;
    actor[15] = 1;
    if unsafe { host_actor_spawn(address(&actor)) } != 0 {
        return fail(14);
    }
    if unsafe { host_actor_place(address(&actor), address(&node)) } != 0 {
        return fail(15);
    }
    if unsafe { host_actor_queue(address(&actor)) } != 0 {
        return fail(16);
    }

    // The guest chooses the rights and validates allow and deny cases before
    // anything is published.
    let mut subject = [0u8; 32];
    subject[..16].copy_from_slice(&actor);
    let handle =
        unsafe { host_cap_issue(address(&root), RIGHTS, address(&subject), address(&node)) };
    if handle == 0 {
        return fail(17);
    }
    if unsafe { host_cap_check(handle, RIGHT_READ, address(&root)) } != 0 {
        return fail(18);
    }
    if unsafe { host_cap_check(handle, RIGHT_MANAGE, address(&root)) } >= 0 {
        return fail(19);
    }
    let other = [0xffu8; 32];
    if unsafe { host_cap_check(handle, RIGHT_READ, address(&other)) } >= 0 {
        return fail(20);
    }
    if unsafe { host_cap_check(handle ^ (1 << 32), RIGHT_READ, address(&root)) } >= 0 {
        return fail(21);
    }

    let mut stats = [0u32; 4];
    if unsafe { host_policy_stats(address_mut(&mut stats)) } != 0 {
        return fail(22);
    }
    if stats != [5, 1, 1, 1] {
        return fail(23);
    }

    // Only now is the stage published, as one atomic transition.
    if unsafe { host_policy_commit() } != 0 {
        return fail(24);
    }
    if unsafe { host_cap_check(handle, RIGHT_READ, address(&root)) } != 0 {
        return fail(25);
    }
    if unsafe { host_cap_check(handle, RIGHT_MANAGE, address(&root)) } >= 0 {
        return fail(26);
    }
    let mut committed_root = [0u8; 32];
    if unsafe { host_ns_root(address_mut(&mut committed_root)) } != 0 || committed_root != root {
        return fail(27);
    }

    let mut evidence = [0u8; 256];
    let mut out = Out::new(&mut evidence);
    out.text(b"wasm-policy: root=");
    out.hex(&root);
    out.text(b" actor=");
    out.hex(&actor);
    out.text(b" cap=ok home=");
    out.hex(&node);
    out.text(b" objects=");
    out.dec(stats[0] as usize);
    out.text(b" actors=");
    out.dec(stats[1] as usize);
    out.text(b" queued=");
    out.dec(stats[2] as usize);
    out.text(b" caps=");
    out.dec(stats[3] as usize);
    out.text(b"\n");
    write(out.slice());
    unsafe { host_yield() };
    42
}

struct Out<'a> {
    buffer: &'a mut [u8],
    length: usize,
}

impl<'a> Out<'a> {
    fn new(buffer: &'a mut [u8]) -> Self {
        Self { buffer, length: 0 }
    }

    fn push(&mut self, byte: u8) {
        if self.length < self.buffer.len() {
            self.buffer[self.length] = byte;
            self.length += 1;
        }
    }

    fn text(&mut self, bytes: &[u8]) {
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
        &self.buffer[..self.length]
    }
}
