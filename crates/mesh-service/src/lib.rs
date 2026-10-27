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

impl NodeState {
    /// A short stable label for logs and evidence.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Alive => "alive",
            Self::Suspect => "suspect",
            Self::Failed => "failed",
        }
    }
}

/// Time without an accepted heartbeat before a member becomes suspect.
///
/// The M2-1 heartbeat cadence is 5 s.  One missed heartbeat plus a margin is
/// the suspect boundary; two missed heartbeats is the failure boundary.  Both
/// are bounded, deterministic and independent of wall-clock time.
pub const SUSPECT_AFTER_MS: u64 = 6_000;

/// Time without an accepted heartbeat before a member is confirmed failed.
pub const FAIL_AFTER_MS: u64 = 12_000;

/// One liveness transition produced by [`FailureDetector::tick`].
///
/// A transition is emitted exactly once per state change.  A duplicate
/// heartbeat never produces a transition, and a node that is already failed
/// does not produce a second `Failed`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Transition {
    /// The node whose liveness changed.
    pub node: NodeId,
    /// The new state.
    pub state: NodeState,
    /// Milliseconds since the last accepted heartbeat at the transition.
    pub silent_ms: u64,
}

/// How an observed heartbeat was applied by the failure detector.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Observe {
    /// The record was refreshed and the node is alive.
    Fresh,
    /// A suspect node was revived by a current heartbeat.
    Revived,
    /// The incarnation is older than the recorded one; the heartbeat is a
    /// replay and was ignored.
    Stale,
    /// The node is not in the local view.
    Unknown,
    /// The node was already confirmed failed.  Only a new join may re-admit it;
    /// a bare heartbeat must not revive a member in the confirmed view.
    RequiresJoin,
}

impl Observe {
    /// A short stable label for logs and evidence.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Fresh => "fresh",
            Self::Revived => "revived",
            Self::Stale => "stale",
            Self::Unknown => "unknown",
            Self::RequiresJoin => "requires-join",
        }
    }
}

/// The bounded, deterministic failure detector policy.
///
/// This is the one failure model: it drives the existing
/// [`MembershipTable`] state machine and adds no second directory or
/// liveness store.  It is stateless; the `MembershipTable` owns the records.
/// Both the authority root and a member use the same boundaries, so they agree
/// on when a silent peer is suspect or failed.
///
/// Fail-closed rules:
///
/// * A heartbeat with a stale incarnation is ignored; a replay cannot refresh
///   a record.
/// * A failed node is never revived by a bare heartbeat.  Re-admission is a
///   membership change and therefore needs a new authority-confirmed join.
/// * The detector never removes a node from the table.  A removal (and the
///   epoch advance that goes with it) is a separate, authority-owned step.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FailureDetector {
    suspect_after_ms: u64,
    fail_after_ms: u64,
}

impl FailureDetector {
    /// Create a detector with explicit boundaries.
    ///
    /// Returns `None` unless `0 < suspect < fail` and `fail` is non-zero, so a
    /// zero or inverted window cannot mark a live node failed immediately.
    pub const fn new(suspect_after_ms: u64, fail_after_ms: u64) -> Option<Self> {
        if suspect_after_ms == 0 || fail_after_ms == 0 || suspect_after_ms >= fail_after_ms {
            return None;
        }
        Some(Self {
            suspect_after_ms,
            fail_after_ms,
        })
    }

    /// The M2 policy: suspect after one missed heartbeat, failed after two.
    pub const fn m2() -> Self {
        Self {
            suspect_after_ms: SUSPECT_AFTER_MS,
            fail_after_ms: FAIL_AFTER_MS,
        }
    }

    /// The suspect boundary in milliseconds.
    pub const fn suspect_after_ms(&self) -> u64 {
        self.suspect_after_ms
    }

    /// The failure boundary in milliseconds.
    pub const fn fail_after_ms(&self) -> u64 {
        self.fail_after_ms
    }

    /// Apply a heartbeat to `table`.
    ///
    /// A replay with an older incarnation is ignored.  A failed record is not
    /// revived: the caller must run a join.
    pub fn observe(
        &self,
        table: &mut MembershipTable,
        node: NodeId,
        incarnation: u64,
        now_ns: u64,
    ) -> Observe {
        let Some(record) = table.get(node) else {
            return Observe::Unknown;
        };
        if record.state == NodeState::Failed {
            return Observe::RequiresJoin;
        }
        if incarnation < record.incarnation {
            return Observe::Stale;
        }
        let previous = record.state;
        if table.heartbeat(node, incarnation, now_ns).is_err() {
            return Observe::Unknown;
        }
        if previous == NodeState::Suspect {
            Observe::Revived
        } else {
            Observe::Fresh
        }
    }

    /// The next liveness state for a node last heard at `last_seen_ms`.
    ///
    /// This is the single transition rule.  It is `const` and pure so a member
    /// that cannot mutate its confirmed table (it may not remove the
    /// authority) can apply the same boundaries without inventing a second
    /// model.
    pub const fn next_state(
        &self,
        current: NodeState,
        last_seen_ms: u64,
        now_ms: u64,
    ) -> NodeState {
        if matches!(current, NodeState::Failed) {
            return NodeState::Failed;
        }
        let silent = now_ms.saturating_sub(last_seen_ms);
        if silent >= self.fail_after_ms {
            NodeState::Failed
        } else if silent >= self.suspect_after_ms {
            NodeState::Suspect
        } else {
            NodeState::Alive
        }
    }

    /// Advance every remote record in `table` to its deterministic liveness
    /// state.  `self_id` is the observing node; a node must never mark itself
    /// suspect or failed, because that would remove it from its own snapshot.
    ///
    /// Boundaries are inclusive: a node silent for exactly
    /// `suspect_after_ms` is suspect, and one silent for exactly
    /// `fail_after_ms` is failed.  Failed records are excluded from
    /// [`MembershipTable::snapshot`] by the table itself.
    pub fn tick(
        &self,
        table: &mut MembershipTable,
        self_id: NodeId,
        now_ms: u64,
    ) -> alloc::vec::Vec<Transition> {
        let mut transitions = alloc::vec::Vec::new();
        let nodes: alloc::vec::Vec<(NodeId, NodeRecord)> = table
            .nodes
            .iter()
            .filter(|(node, _)| **node != self_id)
            .map(|(node, record)| (*node, *record))
            .collect();
        for (node, record) in nodes {
            let next = self.next_state(record.state, record.last_seen_ns, now_ms);
            if next == record.state {
                continue;
            }
            let silent_ms = now_ms.saturating_sub(record.last_seen_ns);
            // A tick that crosses both boundaries still reports the path:
            // suspect first, then failed.  The module can skip over the 6 s
            // window (a slow emulated tick), but the state machine must remain
            // observable and the rig must be able to assert suspect -> failed.
            if next == NodeState::Failed
                && record.state == NodeState::Alive
                && table.suspect(node).unwrap_or(false)
            {
                transitions.push(Transition {
                    node,
                    state: NodeState::Suspect,
                    silent_ms,
                });
            }
            let changed = match next {
                NodeState::Suspect => table.suspect(node),
                NodeState::Failed => table.fail(node),
                NodeState::Alive => Ok(true),
            };
            if changed.unwrap_or(false) {
                transitions.push(Transition {
                    node,
                    state: next,
                    silent_ms,
                });
            }
        }
        transitions
    }
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

/// One requested membership change inside a [`MembershipTable::apply`] set.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Change {
    /// Add a node, or revive and re-incarnate an existing one.
    Add {
        /// The node to add.
        node: NodeId,
        /// The joining incarnation, echoed in signed summaries.
        incarnation: u64,
    },
    /// Remove a node from the confirmed view.
    Remove {
        /// The node to remove.
        node: NodeId,
    },
}

impl Change {
    /// The node this change names.
    pub const fn node(&self) -> NodeId {
        match self {
            Self::Add { node, .. } | Self::Remove { node } => *node,
        }
    }
}

/// The result of applying one [`MembershipTable::apply`] change set.
///
/// `changed` is false when the set was idempotent; `epoch` is then the
/// unchanged epoch.  A changed set advances the epoch exactly once regardless
/// of how many nodes it names.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChangeSet {
    /// The epoch after the set (or the unchanged epoch).
    pub epoch: u32,
    /// Nodes inserted, revived or re-incarnated.
    pub added: alloc::vec::Vec<NodeId>,
    /// Nodes removed.
    pub removed: alloc::vec::Vec<NodeId>,
    /// Whether the set changed the table.
    pub changed: bool,
}

impl ChangeSet {
    /// Number of node-level changes in the set.
    pub fn len(&self) -> usize {
        self.added.len() + self.removed.len()
    }

    /// Whether the set is empty after idempotence.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
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

    /// Every local record, in canonical node order.
    ///
    /// This includes failed nodes, which [`MembershipTable::snapshot`] omits.
    /// It is the view a failure detector needs in order to remove them.
    pub fn records(&self) -> impl Iterator<Item = (NodeId, NodeRecord)> + '_ {
        self.nodes.iter().map(|(node, record)| (*node, *record))
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

    /// Apply one change set atomically, advancing the epoch at most once.
    ///
    /// A change set is what a gossip round produces: a batch of joins,
    /// incarnation changes and removals that the authority decided in one
    /// aggregation step.  The epoch is the authority's change counter, not a
    /// per-node counter, so a batch of `k` changes must cost exactly one
    /// epoch.  Per-node [`MembershipTable::join`] and
    /// [`MembershipTable::leave`] remain the single-change convenience paths.
    ///
    /// The whole set is validated before any record is touched.  A zero node,
    /// a duplicate node in one set, or a resulting view above `MAX_NODES`
    /// leaves the table unchanged.  Removing a node that is already absent is
    /// a no-op, and re-adding an alive node with the same incarnation is
    /// idempotent, so a retried change set cannot inflate the epoch.
    #[cfg(feature = "confirmed")]
    pub fn apply(&mut self, changes: &[Change], now_ns: u64) -> Result<ChangeSet, MembershipError> {
        let mut seen = alloc::collections::BTreeSet::new();
        for change in changes {
            let node = change.node();
            if node.is_zero() {
                return Err(MembershipError::ZeroNode);
            }
            if !seen.insert(node) {
                return Err(MembershipError::DuplicateNode);
            }
        }
        // Validate capacity against the post-set size before mutating.
        let mut resulting = self.nodes.len();
        for change in changes {
            match change {
                Change::Add { node, .. } => {
                    if !self.nodes.contains_key(node) {
                        resulting += 1;
                    }
                }
                Change::Remove { node } => {
                    if self.nodes.contains_key(node) {
                        resulting = resulting.saturating_sub(1);
                    }
                }
            }
        }
        if resulting > MAX_NODES {
            return Err(MembershipError::Capacity);
        }

        let mut set = ChangeSet {
            epoch: self.epoch,
            added: alloc::vec::Vec::new(),
            removed: alloc::vec::Vec::new(),
            changed: false,
        };
        for change in changes {
            match change {
                Change::Add { node, incarnation } => match self.nodes.get_mut(node) {
                    Some(record) => {
                        let was_failed = record.state == NodeState::Failed;
                        let new_incarnation = *incarnation != record.incarnation;
                        record.state = NodeState::Alive;
                        record.incarnation = *incarnation;
                        record.last_seen_ns = now_ns;
                        if was_failed || new_incarnation {
                            set.changed = true;
                            set.added.push(*node);
                        }
                    }
                    None => {
                        self.nodes.insert(
                            *node,
                            NodeRecord {
                                state: NodeState::Alive,
                                incarnation: *incarnation,
                                last_seen_ns: now_ns,
                            },
                        );
                        set.changed = true;
                        set.added.push(*node);
                    }
                },
                Change::Remove { node } => {
                    if self.nodes.remove(node).is_some() {
                        set.changed = true;
                        set.removed.push(*node);
                    }
                }
            }
        }
        if set.changed {
            self.epoch = self
                .epoch
                .checked_add(1)
                .ok_or(MembershipError::EpochOverflow)?;
            set.epoch = self.epoch;
        }
        Ok(set)
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

    #[test]
    fn detector_boundaries_are_exact_and_bounded() {
        let detector = FailureDetector::new(100, 200).expect("detector");
        assert_eq!(detector.suspect_after_ms(), 100);
        assert_eq!(detector.fail_after_ms(), 200);
        assert_eq!(FailureDetector::new(0, 10), None);
        assert_eq!(FailureDetector::new(10, 10), None);
        assert_eq!(FailureDetector::new(11, 10), None);
        assert_eq!(FailureDetector::new(10, 0), None);

        let mut table = MembershipTable::single(node(1), 0);
        table.join(node(2), 0).expect("join");
        assert_eq!(table.epoch(), 2);
        assert_eq!(
            detector.next_state(NodeState::Alive, 0, 99),
            NodeState::Alive
        );
        assert_eq!(
            detector.next_state(NodeState::Alive, 0, 100),
            NodeState::Suspect
        );
        assert_eq!(
            detector.next_state(NodeState::Suspect, 0, 199),
            NodeState::Suspect
        );
        assert_eq!(
            detector.next_state(NodeState::Suspect, 0, 200),
            NodeState::Failed
        );
        // The pure transition rule skips straight to Failed; `tick` still
        // reports the suspect step before it so the path is observable.
        assert_eq!(
            detector.next_state(NodeState::Alive, 0, 200),
            NodeState::Failed
        );
        assert!(detector.tick(&mut table, node(1), 99).is_empty());
        assert!(detector.tick(&mut table, node(1), 100).iter().any(|event| {
            event.node == node(2) && event.state == NodeState::Suspect && event.silent_ms == 100
        }));
        // The suspect transition happens once.
        assert!(detector.tick(&mut table, node(1), 101).is_empty());
        let failed = detector.tick(&mut table, node(1), 200);
        assert!(failed
            .iter()
            .any(|event| event.node == node(2) && event.state == NodeState::Failed));
        assert_eq!(
            table.get(node(2)).map(|record| record.state),
            Some(NodeState::Failed)
        );
        assert_eq!(table.snapshot().nodes, vec![node(1)]);
        // Duplicate failure is idempotent.
        assert!(detector.tick(&mut table, node(1), 500).is_empty());
    }

    #[test]
    fn a_slow_tick_reports_suspect_then_failed() {
        let detector = FailureDetector::new(100, 200).expect("detector");
        let mut table = MembershipTable::single(node(1), 0);
        table.join(node(2), 0).expect("join");
        // The module did not tick at the suspect boundary; the transition is
        // still reported in order.
        let events = detector.tick(&mut table, node(1), 250);
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].node, node(2));
        assert_eq!(events[0].state, NodeState::Suspect);
        assert_eq!(events[0].silent_ms, 250);
        assert_eq!(events[1].state, NodeState::Failed);
        assert_eq!(
            table.get(node(2)).map(|record| record.state),
            Some(NodeState::Failed)
        );
        assert!(detector.tick(&mut table, node(1), 300).is_empty());
    }

    #[test]
    fn detector_ignores_replayed_heartbeats_and_requires_a_join_after_failure() {
        let detector = FailureDetector::new(100, 200).expect("detector");
        let mut table = MembershipTable::single(node(1), 0);
        table.join(node(2), 0).expect("join");
        table.heartbeat(node(2), 7, 50).expect("heartbeat");
        // A lower incarnation is a replay and does not refresh the record.
        assert_eq!(
            detector.observe(&mut table, node(2), 6, 150),
            Observe::Stale
        );
        assert_eq!(
            table.get(node(2)).map(|record| record.last_seen_ns),
            Some(50)
        );
        // The same incarnation is a current heartbeat, not a replay.
        assert_eq!(
            detector.observe(&mut table, node(2), 7, 150),
            Observe::Fresh
        );
        assert_eq!(
            table.get(node(2)).map(|record| record.last_seen_ns),
            Some(150)
        );
        // A suspect node is revived by a current heartbeat.
        table.suspect(node(2)).expect("suspect");
        assert_eq!(
            detector.observe(&mut table, node(2), 8, 160),
            Observe::Revived
        );
        // A failed node is not revived by a bare heartbeat; re-admission is a
        // join and therefore a new epoch.
        table.fail(node(2)).expect("fail");
        assert_eq!(
            detector.observe(&mut table, node(2), 9, 170),
            Observe::RequiresJoin
        );
        assert_eq!(
            table.get(node(2)).map(|record| record.state),
            Some(NodeState::Failed)
        );
        assert_eq!(
            detector.observe(&mut table, node(3), 1, 170),
            Observe::Unknown
        );
    }

    #[test]
    fn detector_suspect_recovery_cancels_the_failure() {
        let detector = FailureDetector::new(100, 200).expect("detector");
        let mut table = MembershipTable::single(node(1), 0);
        table.join(node(2), 0).expect("join");
        assert_eq!(detector.tick(&mut table, node(1), 100).len(), 1);
        assert!(detector.observe(&mut table, node(2), 1, 150) == Observe::Revived);
        assert_eq!(
            table.get(node(2)).map(|record| record.state),
            Some(NodeState::Alive)
        );
        // The timer restarted at the heartbeat, so silence is measured from it.
        assert!(detector.tick(&mut table, node(1), 249).is_empty());
        assert_eq!(
            detector.tick(&mut table, node(1), 250)[0].state,
            NodeState::Suspect
        );
        assert_eq!(
            detector.tick(&mut table, node(1), 350)[0].state,
            NodeState::Failed
        );
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

    #[cfg(feature = "confirmed")]
    #[test]
    fn change_set_advances_the_epoch_exactly_once() {
        let mut table = MembershipTable::confirmed(4, &[node(1)], 0).expect("table");
        let set = table
            .apply(
                &[
                    Change::Add {
                        node: node(2),
                        incarnation: 7,
                    },
                    Change::Add {
                        node: node(3),
                        incarnation: 9,
                    },
                    Change::Add {
                        node: node(4),
                        incarnation: 11,
                    },
                ],
                0,
            )
            .expect("batch join");
        assert!(set.changed);
        assert_eq!(set.epoch, 5);
        assert_eq!(table.epoch(), 5);
        assert_eq!(set.added, vec![node(2), node(3), node(4)]);
        assert_eq!(table.active_count(), 4);
        assert_eq!(table.get(node(2)).map(|record| record.incarnation), Some(7));
    }

    #[cfg(feature = "confirmed")]
    #[test]
    fn batched_removal_is_one_epoch_for_the_whole_set() {
        let mut table =
            MembershipTable::confirmed(2, &[node(1), node(2), node(3), node(4)], 0).expect("table");
        let set = table
            .apply(
                &[
                    Change::Remove { node: node(2) },
                    Change::Remove { node: node(3) },
                ],
                0,
            )
            .expect("batch removal");
        assert_eq!(set.epoch, 3);
        assert_eq!(set.removed, vec![node(2), node(3)]);
        assert_eq!(table.snapshot().nodes, vec![node(1), node(4)]);
    }

    #[cfg(feature = "confirmed")]
    #[test]
    fn idempotent_change_set_does_not_advance_the_epoch() {
        let mut table = MembershipTable::confirmed(6, &[node(1), node(2)], 0).expect("table");
        let set = table
            .apply(
                &[
                    Change::Add {
                        node: node(2),
                        incarnation: 0,
                    },
                    Change::Remove { node: node(9) },
                ],
                100,
            )
            .expect("idempotent");
        assert!(!set.changed);
        assert_eq!(set.epoch, 6);
        assert_eq!(table.epoch(), 6);
    }

    #[cfg(feature = "confirmed")]
    #[test]
    fn failed_member_revival_is_a_membership_change() {
        let mut table = MembershipTable::confirmed(2, &[node(1), node(2)], 0).expect("table");
        table.fail(node(2)).expect("fail");
        let set = table
            .apply(
                &[Change::Add {
                    node: node(2),
                    incarnation: 3,
                }],
                10,
            )
            .expect("revive");
        assert!(set.changed);
        assert_eq!(set.epoch, 3);
        assert_eq!(table.snapshot().len(), 2);
    }

    #[cfg(feature = "confirmed")]
    #[test]
    fn invalid_change_set_leaves_the_table_untouched() {
        let mut table = MembershipTable::confirmed(2, &[node(1)], 0).expect("table");
        assert_eq!(
            table.apply(
                &[Change::Add {
                    node: NodeId::ZERO,
                    incarnation: 1
                }],
                0
            ),
            Err(MembershipError::ZeroNode)
        );
        assert_eq!(
            table.apply(
                &[
                    Change::Remove { node: node(2) },
                    Change::Remove { node: node(2) }
                ],
                0
            ),
            Err(MembershipError::DuplicateNode)
        );
        assert_eq!(table.epoch(), 2);
        assert_eq!(table.active_count(), 1);
    }
}
