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

    crate::print_str("mesh: single node ");
    for byte in node.as_bytes() {
        crate::print_hex8(*byte);
    }
    crate::println("");
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
