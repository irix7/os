// SPDX-License-Identifier: GPL-3.0-only
//! Single-node logical-computer bootstrap.
//!
//! M1 is deliberately local: one membership replica, every home is this node,
//! and no mesh frame is constructed. The mesh policy crates are the same ones
//! the host tests and the future multi-node kernel use.

use mesh_core::{actor_home, home_for, MembershipView};
use mesh_service::MembershipTable;
use sys_primitives::{ActorId, NodeId, ObjectId};

static mut NODE: NodeId = NodeId::ZERO;
/// The local membership table.  M1 has one confirmed replica; the table is the
/// same state machine the join path installs when a mesh is confirmed.
static mut TABLE: Option<MembershipTable> = None;
/// This boot's incarnation.  It rises across a restart and is the value the
/// membership view records when the node heartbeats.
static mut INCARNATION: u64 = 0;
/// M1 instrumentation: a byte would not be sent by the single-node path.
static mut FRAMES: u32 = 0;
static mut RPCS: u32 = 0;
static mut ENVELOPES: u32 = 0;

/// Initialise the one-node membership view from the DS1386 MAC address.
pub fn init() {
    let mac = crate::net::mac_address();
    let mut bytes = [0u8; 16];
    bytes[..mac.len()].copy_from_slice(&mac);
    let node = NodeId(bytes);
    let now_ms = crate::idle::uptime_ms();
    let table = MembershipTable::single(node, now_ms.saturating_mul(1_000_000));
    let view = table.snapshot();
    debug_assert!(view.is_single());
    debug_assert_eq!(home_for(&ObjectId([0x5a; 32]), &view), Some(node));

    // The RTC keeps a real wall clock across a power cycle; mixing in CP0 Count
    // keeps two RTC-less boots distinct.  The value is a boot incarnation, not a
    // monotonic global clock.
    let clock = crate::rtc::read_unix().unwrap_or(0) as u64;
    let count = crate::traps::count_read() as u64;
    let incarnation = (clock.rotate_left(17) ^ count ^ 0x4952_4958_3700_0001).max(1);
    unsafe {
        NODE = node;
        TABLE = Some(table);
        INCARNATION = incarnation;
    }

    self_test();
    crate::print_str("mesh: single node ");
    for byte in node.as_bytes() {
        crate::print_hex8(*byte);
    }
    crate::println("");
}

/// Count a mesh frame.  No transport calls this in M1; it exists so a future
/// transport cannot silently turn the single-node boot into a mesh boot.
pub fn note_frame() {
    // SAFETY: single-threaded kernel.
    unsafe { FRAMES = FRAMES.saturating_add(1) };
}

/// Count a mesh RPC.
pub fn note_rpc() {
    // SAFETY: single-threaded kernel.
    unsafe { RPCS = RPCS.saturating_add(1) };
}

/// Count a mesh envelope construction.
pub fn note_envelope() {
    // SAFETY: single-threaded kernel.
    unsafe { ENVELOPES = ENVELOPES.saturating_add(1) };
}

/// Prove the three counters are wired, then restore the zero baseline.
fn self_test() {
    note_frame();
    note_rpc();
    note_envelope();
    assert_eq!(frames(), 1);
    assert_eq!(rpcs(), 1);
    assert_eq!(envelopes(), 1);
    // SAFETY: single-threaded boot path.
    unsafe {
        FRAMES = 0;
        RPCS = 0;
        ENVELOPES = 0;
    }
}

/// Mesh frames observed since boot.
pub fn frames() -> u32 {
    // SAFETY: single-threaded kernel; reads are atomic with respect to it.
    unsafe { FRAMES }
}

/// Mesh RPCs observed since boot.
pub fn rpcs() -> u32 {
    // SAFETY: single-threaded kernel.
    unsafe { RPCS }
}

/// Mesh envelopes constructed since boot.
pub fn envelopes() -> u32 {
    // SAFETY: single-threaded kernel.
    unsafe { ENVELOPES }
}

/// This node's routing identity. Zero means the mesh layer was not initialised.
pub fn node_id() -> NodeId {
    unsafe { NODE }
}

/// Whether the local node is the complete logical computer.
pub fn is_single() -> bool {
    !node_id().is_zero()
}

/// The confirmed membership epoch this replica is routing at.
pub fn epoch() -> u32 {
    // SAFETY: single-threaded kernel; the table is installed once at boot.
    unsafe {
        (*core::ptr::addr_of!(TABLE))
            .as_ref()
            .map(|table| table.epoch())
    }
    .unwrap_or(0)
}

/// This boot's incarnation.  A stale handle that names an older incarnation of
/// the same node is refused by the home.
pub fn incarnation() -> u64 {
    // SAFETY: single-threaded kernel.
    unsafe { INCARNATION }
}

/// Resolve an object home in the current local view.
pub fn home(object: ObjectId) -> Option<NodeId> {
    let node = node_id();
    if node.is_zero() {
        return None;
    }
    home_for(&object, &MembershipView::single(node))
}

/// Resolve an actor's rendezvous home in the confirmed local view.
pub fn actor_home_for(actor: &ActorId) -> Option<NodeId> {
    // SAFETY: single-threaded kernel; the table is installed once at boot and
    // a snapshot allocates only for the duration of this call.
    let table = unsafe { (*core::ptr::addr_of!(TABLE)).as_ref()? };
    actor_home(actor, &table.snapshot())
}
