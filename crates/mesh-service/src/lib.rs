// SPDX-License-Identifier: GPL-3.0-only
//! Membership and failure-detection state for the IRIX 7 logical computer.
//!
//! The service is deliberately split from the routing policy: this crate owns
//! the mutable local view of node liveness and the membership epoch; `mesh-core`
//! turns a snapshot into deterministic homes and placement. The single-node
//! table is complete and routable without a peer.

#![no_std]
#![forbid(unsafe_code)]
#![deny(missing_docs)]

extern crate alloc;

use alloc::collections::BTreeMap;
use core::fmt;
use mesh_core::{MembershipView, MAX_NODES};
use sys_primitives::NodeId;

/// Current liveness state of a node in the local view.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NodeState {
    /// Heartbeats are current and the node is routable.
    Alive,
    /// A probe was missed; the node remains routable until failure is confirmed.
    Suspect,
    /// The failure detector confirmed the node is unavailable.
    Failed,
}

/// One member's local liveness record.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NodeRecord {
    /// Current liveness state.
    pub state: NodeState,
    /// Incarnation supplied by the node. Stale heartbeats are ignored.
    pub incarnation: u64,
    /// Local monotonic time of the last accepted heartbeat.
    pub last_seen_ns: u64,
}

/// Membership mutation failure.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MembershipError {
    /// The 128-replica hard limit would be exceeded.
    Capacity,
    /// The membership epoch cannot advance further.
    EpochOverflow,
    /// The node is not present in the local view.
    UnknownNode,
    /// A confirmed view cannot be empty; every kernel replica includes itself.
    #[cfg(feature = "confirmed")]
    EmptyView,
    /// A node ID is the all-zero sentinel.
    #[cfg(feature = "confirmed")]
    ZeroNode,
    /// The same node appears more than once in a confirmed view.
    #[cfg(feature = "confirmed")]
    DuplicateNode,
    /// A confirmed epoch is older than the local view.
    #[cfg(feature = "confirmed")]
    EpochRegression,
    /// Two different member lists claim the same epoch.
    #[cfg(feature = "confirmed")]
    EpochConflict,
}

impl fmt::Display for MembershipError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Capacity => f.write_str("membership capacity exceeded"),
            Self::EpochOverflow => f.write_str("membership epoch overflow"),
            Self::UnknownNode => f.write_str("unknown node"),
            #[cfg(feature = "confirmed")]
            Self::EmptyView => f.write_str("membership view is empty"),
            #[cfg(feature = "confirmed")]
            Self::ZeroNode => f.write_str("zero node identifier"),
            #[cfg(feature = "confirmed")]
            Self::DuplicateNode => f.write_str("duplicate node in membership view"),
            #[cfg(feature = "confirmed")]
            Self::EpochRegression => f.write_str("membership epoch regression"),
            #[cfg(feature = "confirmed")]
            Self::EpochConflict => f.write_str("conflicting membership epoch"),
        }
    }
}

/// The local, eventually consistent membership view.
///
/// The authority service confirms a global membership epoch; this table is
/// what the local kernel uses for routing and placement until a newer signed
/// epoch arrives.
#[derive(Debug)]
pub struct MembershipTable {
    epoch: u32,
    nodes: BTreeMap<NodeId, NodeRecord>,
}

impl MembershipTable {
    /// Create an empty table at `epoch`.
    pub const fn new(epoch: u32) -> Self {
        Self {
            epoch,
            nodes: BTreeMap::new(),
        }
    }

    /// Create the single-node base case.
    pub fn single(node: NodeId, now_ns: u64) -> Self {
        let mut table = Self::new(0);
        table
            .join(node, now_ns)
            .expect("a single-node membership always fits");
        table
    }

    /// Build a table from an authoritatively confirmed member list.
    ///
    /// This is the install path for an epoch confirmation: it sets the epoch
    /// exactly as confirmed and does not advance it.  The authority root is the
    /// only writer of the confirmed epoch; a local heartbeat or join must never
    /// be used in place of this constructor.  Members are sorted by
    /// `MembershipView`, so the table and its snapshot are deterministic.
    #[cfg(feature = "confirmed")]
    pub fn confirmed(epoch: u32, members: &[NodeId], now_ns: u64) -> Result<Self, MembershipError> {
        let view = validated_view(members)?;
        let mut table = Self::new(epoch);
        table.populate(&view.nodes, now_ns);
        Ok(table)
    }

    /// Replace the local view with an authoritatively confirmed snapshot.
    ///
    /// A lower epoch is rejected: after observing a newer epoch a node must
    /// not move its confirmed membership backwards.  Re-installing the same
    /// epoch is idempotent only when the member list is identical; a different
    /// list at the same epoch is an equivocation and fails closed.  Liveness
    /// state is left untouched on the idempotent path so a duplicate
    /// confirmation cannot revive a node whose failure was already confirmed.
    #[cfg(feature = "confirmed")]
    pub fn install(
        &mut self,
        epoch: u32,
        members: &[NodeId],
        now_ns: u64,
    ) -> Result<(), MembershipError> {
        let view = validated_view(members)?;
        if epoch < self.epoch {
            return Err(MembershipError::EpochRegression);
        }
        if epoch == self.epoch {
            let current: alloc::vec::Vec<NodeId> = self.nodes.keys().copied().collect();
            if current == view.nodes {
                return Ok(());
            }
            if self.nodes.is_empty() {
                // A fresh table at the same epoch is an initial install, not
                // an equivocation.
                self.populate(&view.nodes, now_ns);
                return Ok(());
            }
            return Err(MembershipError::EpochConflict);
        }
        self.epoch = epoch;
        self.nodes.clear();
        self.populate(&view.nodes, now_ns);
        Ok(())
    }

    #[cfg(feature = "confirmed")]
    fn populate(&mut self, members: &[NodeId], now_ns: u64) {
        for node in members {
            self.nodes.insert(
                *node,
                NodeRecord {
                    state: NodeState::Alive,
                    incarnation: 0,
                    last_seen_ns: now_ns,
                },
            );
        }
    }

    /// Current membership epoch.
    pub const fn epoch(&self) -> u32 {
        self.epoch
    }

    /// Number of nodes not confirmed failed.
    pub fn active_count(&self) -> usize {
        self.nodes
            .values()
            .filter(|record| record.state != NodeState::Failed)
            .count()
    }

    /// Look up a record.
    pub fn get(&self, node: NodeId) -> Option<&NodeRecord> {
        self.nodes.get(&node)
    }

    /// Add a node or revive a failed one. Returns true when the node was new.
    pub fn join(&mut self, node: NodeId, now_ns: u64) -> Result<bool, MembershipError> {
        if let Some(record) = self.nodes.get_mut(&node) {
            record.state = NodeState::Alive;
            record.last_seen_ns = now_ns;
            return Ok(false);
        }
        if self.nodes.len() >= MAX_NODES {
            return Err(MembershipError::Capacity);
        }
        self.epoch = self
            .epoch
            .checked_add(1)
            .ok_or(MembershipError::EpochOverflow)?;
        self.nodes.insert(
            node,
            NodeRecord {
                state: NodeState::Alive,
                incarnation: 0,
                last_seen_ns: now_ns,
            },
        );
        Ok(true)
    }

    /// Apply a heartbeat. A stale incarnation is ignored.
    ///
    /// Returns true when the local record changed.
    pub fn heartbeat(
        &mut self,
        node: NodeId,
        incarnation: u64,
        now_ns: u64,
    ) -> Result<bool, MembershipError> {
        let record = self
            .nodes
            .get_mut(&node)
            .ok_or(MembershipError::UnknownNode)?;
        if incarnation < record.incarnation {
            return Ok(false);
        }
        let changed = record.state != NodeState::Alive || incarnation != record.incarnation;
        record.state = NodeState::Alive;
        record.incarnation = incarnation;
        record.last_seen_ns = now_ns;
        Ok(changed)
    }

    /// Mark a node suspect without removing it from routing.
    pub fn suspect(&mut self, node: NodeId) -> Result<bool, MembershipError> {
        let record = self
            .nodes
            .get_mut(&node)
            .ok_or(MembershipError::UnknownNode)?;
        if record.state == NodeState::Suspect {
            return Ok(false);
        }
        record.state = NodeState::Suspect;
        Ok(true)
    }

    /// Confirm a node failed. It leaves the routable snapshot.
    pub fn fail(&mut self, node: NodeId) -> Result<bool, MembershipError> {
        let record = self
            .nodes
            .get_mut(&node)
            .ok_or(MembershipError::UnknownNode)?;
        if record.state == NodeState::Failed {
            return Ok(false);
        }
        record.state = NodeState::Failed;
        Ok(true)
    }

    /// Remove a node cleanly and advance the membership epoch.
    pub fn leave(&mut self, node: NodeId) -> Result<bool, MembershipError> {
        if self.nodes.remove(&node).is_none() {
            return Ok(false);
        }
        self.epoch = self
            .epoch
            .checked_add(1)
            .ok_or(MembershipError::EpochOverflow)?;
        Ok(true)
    }

    /// Take a deterministic routing snapshot.
    ///
    /// Failed nodes are excluded; suspect nodes remain visible until the
    /// failure detector confirms the failure. The result is sorted by
    /// `mesh-core` so every node computes the same homes from the same epoch.
    pub fn snapshot(&self) -> MembershipView {
        let nodes: alloc::vec::Vec<NodeId> = self
            .nodes
            .iter()
            .filter_map(|(node, record)| {
                if record.state == NodeState::Failed {
                    None
                } else {
                    Some(*node)
                }
            })
            .collect();
        MembershipView::new(self.epoch, nodes).expect("table is bounded by MAX_NODES")
    }
}

/// Validate a confirmed member list and return its canonical sorted view.
#[cfg(feature = "confirmed")]
fn validated_view(members: &[NodeId]) -> Result<MembershipView, MembershipError> {
    if members.is_empty() {
        return Err(MembershipError::EmptyView);
    }
    if members.len() > MAX_NODES {
        return Err(MembershipError::Capacity);
    }
    let mut nodes = alloc::vec::Vec::with_capacity(members.len());
    for node in members {
        if node.is_zero() {
            return Err(MembershipError::ZeroNode);
        }
        nodes.push(*node);
    }
    let view = MembershipView::new(0, nodes).ok_or(MembershipError::Capacity)?;
    if view.nodes.len() != members.len() {
        return Err(MembershipError::DuplicateNode);
    }
    Ok(view)
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;
    use alloc::vec::Vec;
    use mesh_core::home_for;
    use sys_primitives::ObjectId;

    fn node(n: u8) -> NodeId {
        NodeId([n; 16])
    }

    #[test]
    fn single_node_is_routable_without_a_peer() {
        let table = MembershipTable::single(node(1), 0);
        let view = table.snapshot();
        assert!(view.is_single());
        assert_eq!(table.active_count(), 1);
        assert_eq!(home_for(&ObjectId([7; 32]), &view), Some(node(1)));
    }

    #[test]
    fn capacity_is_128_nodes() {
        let mut table = MembershipTable::new(0);
        for i in 0..MAX_NODES {
            let mut id = [0u8; 16];
            id[0] = i as u8;
            id[1] = (i >> 8) as u8;
            table.join(NodeId(id), 0).expect("join");
        }
        assert_eq!(table.active_count(), MAX_NODES);
        let mut overflow = [0u8; 16];
        overflow[0] = 255;
        assert_eq!(
            table.join(NodeId(overflow), 0),
            Err(MembershipError::Capacity)
        );
    }

    #[test]
    fn stale_heartbeat_does_not_revive_a_failed_node() {
        let mut table = MembershipTable::single(node(1), 0);
        table.heartbeat(node(1), 2, 1).expect("heartbeat");
        table.fail(node(1)).expect("fail");
        assert!(table.snapshot().is_empty());
        assert!(!table.heartbeat(node(1), 1, 3).expect("stale heartbeat"));
        assert_eq!(table.get(node(1)).map(|r| r.state), Some(NodeState::Failed));
        assert!(table.heartbeat(node(1), 3, 4).expect("new heartbeat"));
        assert_eq!(table.get(node(1)).map(|r| r.state), Some(NodeState::Alive));
    }

    #[test]
    fn suspect_nodes_stay_in_the_routing_view_until_failed() {
        let mut table = MembershipTable::single(node(1), 0);
        table.join(node(2), 0).expect("join");
        table.suspect(node(2)).expect("suspect");
        assert_eq!(table.snapshot().len(), 2);
        table.fail(node(2)).expect("fail");
        assert_eq!(table.snapshot().len(), 1);
    }

    #[test]
    fn join_leave_advance_the_epoch() {
        let mut table = MembershipTable::single(node(1), 0);
        let before = table.epoch();
        table.join(node(2), 0).expect("join");
        assert!(table.epoch() > before);
        let epoch = table.epoch();
        table.leave(node(2)).expect("leave");
        assert!(table.epoch() > epoch);
        assert_eq!(table.active_count(), 1);
    }

    #[test]
    fn node_128_snapshot_is_deterministic() {
        let mut table = MembershipTable::new(0);
        let mut nodes: Vec<NodeId> = Vec::new();
        for i in 0..MAX_NODES {
            let mut id = [0u8; 16];
            id[0] = i as u8;
            id[1] = (i >> 8) as u8;
            let id = NodeId(id);
            nodes.push(id);
            table.join(id, 0).expect("join");
        }
        let first = table.snapshot();
        let second = table.snapshot();
        assert_eq!(first, second);
        assert_eq!(first.len(), MAX_NODES);
        assert_eq!(first.nodes.first().copied(), Some(node(0)));
    }

    #[cfg(feature = "confirmed")]
    #[test]
    fn confirmed_view_sets_the_epoch_exactly() {
        let table = MembershipTable::confirmed(7, &[node(2), node(1)], 10).expect("confirmed");
        assert_eq!(table.epoch(), 7);
        assert_eq!(table.active_count(), 2);
        let view = table.snapshot();
        assert_eq!(view.epoch, 7);
        assert_eq!(view.nodes, vec![node(1), node(2)]);
    }

    #[cfg(feature = "confirmed")]
    #[test]
    fn confirmed_view_rejects_bad_shapes() {
        assert_eq!(
            MembershipTable::confirmed(1, &[], 0).unwrap_err(),
            MembershipError::EmptyView
        );
        assert_eq!(
            MembershipTable::confirmed(1, &[NodeId::ZERO], 0).unwrap_err(),
            MembershipError::ZeroNode
        );
        assert_eq!(
            MembershipTable::confirmed(1, &[node(1), node(1)], 0).unwrap_err(),
            MembershipError::DuplicateNode
        );
        let many: Vec<NodeId> = (0..=MAX_NODES)
            .map(|i| {
                let mut id = [0u8; 16];
                id[0] = i as u8;
                id[1] = (i >> 8) as u8;
                NodeId(id)
            })
            .collect();
        assert_eq!(
            MembershipTable::confirmed(1, &many, 0).unwrap_err(),
            MembershipError::Capacity
        );
    }

    #[cfg(feature = "confirmed")]
    #[test]
    fn install_rejects_epoch_regression_and_equivocation() {
        let mut table = MembershipTable::confirmed(4, &[node(1), node(2)], 0).expect("table");
        assert_eq!(
            table.install(3, &[node(1), node(2)], 0).unwrap_err(),
            MembershipError::EpochRegression
        );
        assert_eq!(
            table.install(4, &[node(1)], 0).unwrap_err(),
            MembershipError::EpochConflict
        );
        // The identical epoch is idempotent and keeps the view.
        table
            .install(4, &[node(2), node(1)], 0)
            .expect("idempotent");
        assert_eq!(table.epoch(), 4);
        assert_eq!(table.active_count(), 2);
    }

    #[cfg(feature = "confirmed")]
    #[test]
    fn install_moves_forward_and_excludes_departed_members() {
        let mut table = MembershipTable::confirmed(1, &[node(1)], 0).expect("table");
        table.install(2, &[node(1), node(2)], 0).expect("join");
        assert_eq!(table.epoch(), 2);
        assert_eq!(table.snapshot().len(), 2);
        table.install(3, &[node(2)], 0).expect("leave");
        assert_eq!(table.epoch(), 3);
        assert_eq!(table.snapshot().nodes, vec![node(2)]);
    }

    #[cfg(feature = "confirmed")]
    #[test]
    fn idempotent_install_does_not_revive_a_failed_member() {
        let mut table = MembershipTable::confirmed(2, &[node(1), node(2)], 0).expect("table");
        table.fail(node(2)).expect("fail");
        table.install(2, &[node(1), node(2)], 0).expect("duplicate");
        assert_eq!(
            table.get(node(2)).map(|record| record.state),
            Some(NodeState::Failed)
        );
        assert_eq!(table.snapshot().len(), 1);
    }
}
