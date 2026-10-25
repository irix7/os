// SPDX-License-Identifier: GPL-3.0-only
//! Single-node logical-computer bootstrap.
//!
//! M1 is deliberately local: one membership replica, every home is this node,
//! and no mesh frame is constructed. The mesh policy crates are the same ones
//! the host tests and the future multi-node kernel use.

use mesh_core::{home_for, MembershipView};
use mesh_service::MembershipTable;
use sys_primitives::{NodeId, ObjectId};

static mut NODE: NodeId = NodeId::ZERO;
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
    unsafe {
        NODE = node;
    }

    let table = MembershipTable::single(node, 0);
    let view = table.snapshot();
    debug_assert!(view.is_single());
    debug_assert_eq!(home_for(&ObjectId([0x5a; 32]), &view), Some(node));

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

/// Resolve an object home in the current local view.
pub fn home(object: ObjectId) -> Option<NodeId> {
    let node = node_id();
    if node.is_zero() {
        return None;
    }
    home_for(&object, &MembershipView::single(node))
}
