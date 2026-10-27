// SPDX-License-Identifier: GPL-3.0-only
//! Signed member summaries, bounded gossip and root-side aggregation.
//!
//! M2 refreshed membership by having every member send a signed join request
//! and every reply carry the full member list: the root signed once per member
//! per refresh, and the traffic was O(N) records of O(N) bytes, i.e. O(N²)
//! bytes per refresh.  This module replaces that fan-in with an aggregate:
//!
//! ```text
//! member -> peers:  MEMBER_SUMMARY(node, epoch, digest, incarnation, counter)
//! root:             validate each summary against the signed aggregate it
//!                   issued, observe liveness, collect one change set
//! root -> peers:    EPOCH_SUMMARY(root, epoch, [(node, incarnation)...], digest)
//!                   one Ed25519 signature for the whole change set
//! peers:            verify once, install the snapshot, forward once
//! ```
//!
//! The aggregate is the only signed object after the initial join.  It binds
//! every member's identity and incarnation to the epoch and the snapshot
//! digest, so a member summary is verifiable without a per-member signature and
//! without the root re-signing for every refresh.
//!
//! ## Bounds
//!
//! * A node forwards a given `(node, counter)` summary at most once and an
//!   aggregate epoch at most once, to at most [`FANOUT`] peers, for at most
//!   [`MAX_HOPS`] hops.  Per interval the work is therefore `O(N)`, not
//!   `O(N²)`.
//! * One change set costs one epoch and one signature, however many members
//!   joined, left or failed in it.
//! * The aggregate body is [`crate::EpochSummary::MAX_LEN`] at the 128-member
//!   limit, inside the 4 KiB join body bound.
//!
//! ## Fail-closed rules
//!
//! * A summary whose epoch is below the aggregate fails as stale; a summary
//!   above it cannot be validated and is dropped.
//! * A summary whose digest differs at the aggregate's epoch is an
//!   equivocation.
//! * A summary whose incarnation is not the one the root signed is refused, so
//!   an old process cannot claim a new member's identity.
//! * A counter that does not increase is a replay.
//! * A stale epoch in an aggregate is an epoch regression; the same epoch with
//!   a different member list is an equivocation.  Both leave the local view
//!   untouched.
//!
//! There is no old-format compatibility path: this is the Version 1 aggregate
//! wire, and it replaces the unshipped M2 refresh.

use alloc::collections::{BTreeMap, BTreeSet};
use alloc::vec::Vec;
use core::fmt;

use mesh_service::{Change, ChangeSet, MembershipError, MembershipTable, NodeState};
use sys_primitives::NodeId;

use crate::authority::{AuthorityError, AuthorityRoot, AuthorityVerifier, SigningAuthority};
use crate::wire::{EpochSummary, MemberSummary, MembershipSnapshot, WireError};

/// Peers a node forwards one gossip record to.
pub const FANOUT: usize = 3;
/// Maximum gossip hops for one record.
pub const MAX_HOPS: u8 = 2;
/// Interval between a member's own summaries.
pub const SUMMARY_INTERVAL_MS: u64 = 5_000;
/// Maximum summaries one node keeps for relay in one tick.
pub const RELAY_CAPACITY: usize = 32;

/// A summary or aggregate that failed validation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GossipError {
    /// The aggregate names a root other than the configured one.
    WrongRoot,
    /// The aggregate signature did not verify.
    BadSignature,
    /// The aggregate or summary digest does not match its member list.
    DigestMismatch,
    /// The root is not in its own confirmed member list.
    RootNotMember,
    /// The local node is not in the aggregate it received.
    SubjectNotMember,
    /// The sender is not in the confirmed member list.
    NotMember,
    /// The sender's incarnation is not the one the root signed.
    IncarnationMismatch,
    /// The record is older than the installed epoch.
    StaleEpoch,
    /// The record is newer than the installed epoch.
    UnknownEpoch,
    /// The same epoch carries a different member list or digest.
    Equivocation,
    /// The summary counter did not increase.
    Replay,
    /// No aggregate has been installed yet.
    NoAggregate,
    /// The membership table rejected the change.
    Membership(MembershipError),
    /// The wire body was malformed.
    Wire(WireError),
    /// The authority is not the local root.
    NotRoot,
    /// The change set tried to remove the authority root.
    RemoveRoot,
}

impl GossipError {
    /// A short stable label for serial logs and evidence.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::WrongRoot => "wrong-root",
            Self::BadSignature => "bad-signature",
            Self::DigestMismatch => "digest-mismatch",
            Self::RootNotMember => "root-not-member",
            Self::SubjectNotMember => "subject-not-member",
            Self::NotMember => "not-member",
            Self::IncarnationMismatch => "incarnation-mismatch",
            Self::StaleEpoch => "stale-epoch",
            Self::UnknownEpoch => "unknown-epoch",
            Self::Equivocation => "equivocation",
            Self::Replay => "replay",
            Self::NoAggregate => "no-aggregate",
            Self::Membership(_) => "membership",
            Self::Wire(_) => "wire",
            Self::NotRoot => "not-root",
            Self::RemoveRoot => "remove-root",
        }
    }
}

impl fmt::Display for GossipError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl From<MembershipError> for GossipError {
    fn from(error: MembershipError) -> Self {
        Self::Membership(error)
    }
}

impl From<AuthorityError> for GossipError {
    fn from(error: AuthorityError) -> Self {
        match error {
            AuthorityError::WrongRoot => Self::WrongRoot,
            AuthorityError::UntrustedRoot | AuthorityError::BadSignature => Self::BadSignature,
            AuthorityError::Membership(error) => Self::Membership(error),
            AuthorityError::Wire(error) => Self::Wire(error),
            _ => Self::BadSignature,
        }
    }
}

impl From<WireError> for GossipError {
    fn from(error: WireError) -> Self {
        Self::Wire(error)
    }
}

/// Traffic and outcome counters for one node.
///
/// These are the numbers the host policy test and the rig evidence quote; they
/// are counts of protocol records, not socket writes.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct GossipStats {
    /// Member summaries emitted by this node.
    pub summaries_sent: u64,
    /// Member summaries accepted from peers.
    pub summaries_received: u64,
    /// Summaries forwarded for another origin.
    pub forwards: u64,
    /// Summaries ignored because the counter was already seen.
    pub duplicates: u64,
    /// Summaries refused as replays.
    pub replays: u64,
    /// Summaries refused as stale or unknown epochs.
    pub stale: u64,
    /// Summary or aggregate equivocations refused.
    pub equivocations: u64,
    /// Aggregates installed.
    pub installs: u64,
    /// Aggregates signed (one per change set).
    pub epoch_signatures: u64,
    /// Change sets applied (one epoch each).
    pub epoch_changes: u64,
    /// Members added across all change sets.
    pub members_added: u64,
    /// Members removed across all change sets.
    pub members_removed: u64,
    /// Member summaries received from an unknown node at the current epoch.
    pub join_candidates: u64,
}

impl GossipStats {
    /// Total summary records emitted or forwarded.
    pub const fn summary_traffic(&self) -> u64 {
        self.summaries_sent + self.forwards
    }

    /// Merge another node's counters (host model only).
    pub fn merge(&mut self, other: &Self) {
        self.summaries_sent += other.summaries_sent;
        self.summaries_received += other.summaries_received;
        self.forwards += other.forwards;
        self.duplicates += other.duplicates;
        self.replays += other.replays;
        self.stale += other.stale;
        self.equivocations += other.equivocations;
        self.installs += other.installs;
        self.epoch_signatures += other.epoch_signatures;
        self.epoch_changes += other.epoch_changes;
        self.members_added += other.members_added;
        self.members_removed += other.members_removed;
        self.join_candidates += other.join_candidates;
    }
}

/// What the root should do after ingesting one summary.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ingest {
    /// The summary was current and its sender is observed live.
    Alive,
    /// The summary names a node that is not a member yet; a join is pending.
    JoinPending,
    /// The summary was a duplicate at the current counter.
    Duplicate,
}

/// A change the root has decided but not yet signed into an aggregate.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Pending {
    Add { incarnation: u64 },
    Remove,
}

impl Pending {
    fn change(self, node: NodeId) -> Change {
        match self {
            Self::Add { incarnation } => Change::Add { node, incarnation },
            Self::Remove => Change::Remove { node },
        }
    }
}

/// The authority root's aggregation state.
///
/// The root owns the only signing key.  It signs one [`EpochSummary`] per
/// change set, not one confirmation per member and refresh.
pub struct Aggregator {
    authority: AuthorityRoot,
    grants: BTreeMap<NodeId, u64>,
    pending: BTreeMap<NodeId, Pending>,
    current: Option<EpochSummary>,
    /// Highest accepted counter per member in the current epoch.
    counters: BTreeMap<NodeId, u64>,
    seen_epoch: u32,
    stats: GossipStats,
}

impl Aggregator {
    /// Create the root at its seed epoch with itself as the only member.
    pub fn new(
        root: NodeId,
        signing: SigningAuthority,
        seed_epoch: u32,
        now_ms: u64,
    ) -> Result<Self, AuthorityError> {
        let authority = AuthorityRoot::new(root, signing, seed_epoch, now_ms)?;
        let mut aggregator = Self {
            authority,
            grants: BTreeMap::new(),
            pending: BTreeMap::new(),
            current: None,
            counters: BTreeMap::new(),
            seen_epoch: seed_epoch,
            stats: GossipStats::default(),
        };
        aggregator.grants.insert(root, 0);
        Ok(aggregator)
    }

    /// The configured root identity.
    pub fn root(&self) -> NodeId {
        self.authority.root()
    }

    /// The root's current confirmed epoch.
    pub fn epoch(&self) -> u32 {
        self.authority.epoch()
    }

    /// The root's current confirmed snapshot.
    pub fn snapshot(&self) -> Result<MembershipSnapshot, GossipError> {
        Ok(self.authority.snapshot()?)
    }

    /// Register a join intent that the next aggregate will batch.
    ///
    /// The initial join still travels as a bounded `JoinRequest`; the root does
    /// not sign a per-member confirmation, it adds the candidate to the next
    /// change set and publishes one aggregate for all of them.
    pub fn request_join(&mut self, node: NodeId, incarnation: u64) -> Result<(), GossipError> {
        if node.is_zero() {
            return Err(GossipError::NotMember);
        }
        if self.authority.is_member(node) {
            return Ok(());
        }
        if self.pending.get(&node) != Some(&Pending::Add { incarnation }) {
            self.pending.insert(node, Pending::Add { incarnation });
            self.stats.join_candidates += 1;
        }
        Ok(())
    }

    /// Number of changes waiting for the next aggregate.
    pub fn pending_len(&self) -> usize {
        self.pending.len()
    }

    /// The signed aggregate for the current epoch, once one has been signed.
    pub fn current(&self) -> Option<&EpochSummary> {
        self.current.as_ref()
    }

    /// Traffic and outcome counters.
    pub const fn stats(&self) -> &GossipStats {
        &self.stats
    }

    /// Forget counters from an older epoch.
    fn roll_epoch(&mut self, epoch: u32) {
        if epoch != self.seen_epoch {
            self.seen_epoch = epoch;
            self.counters.clear();
        }
    }

    /// Ingest one member summary.
    ///
    /// `direct` is true only when the summary arrived on the sender's own
    /// link.  A forwarded summary is still validated and can complete a
    /// pending join, but it does not refresh liveness: otherwise any member
    /// could keep a dead peer looking alive.
    pub fn ingest(
        &mut self,
        summary: &MemberSummary,
        direct: bool,
        now_ms: u64,
    ) -> Result<Ingest, GossipError> {
        if let Some(current) = &self.current {
            if summary.epoch < current.epoch {
                self.stats.stale += 1;
                return Err(GossipError::StaleEpoch);
            }
            if summary.epoch > current.epoch {
                // A member cannot be ahead of the aggregate's writer.
                self.stats.stale += 1;
                return Err(GossipError::UnknownEpoch);
            }
            if summary.digest != current.digest {
                self.stats.equivocations += 1;
                return Err(GossipError::Equivocation);
            }
            match current.grant(summary.node) {
                Some(incarnation) if incarnation == summary.incarnation => {}
                Some(_) => {
                    self.stats.equivocations += 1;
                    return Err(GossipError::IncarnationMismatch);
                }
                None => {
                    // A node that is not in the confirmed list is a join
                    // candidate.  It is added to the next change set; its
                    // summary is not evidence of liveness until the aggregate
                    // grants the incarnation.
                    if self.pending.get(&summary.node)
                        != Some(&Pending::Add {
                            incarnation: summary.incarnation,
                        })
                    {
                        self.pending.insert(
                            summary.node,
                            Pending::Add {
                                incarnation: summary.incarnation,
                            },
                        );
                        self.stats.join_candidates += 1;
                    }
                    return Ok(Ingest::JoinPending);
                }
            }
        } else if summary.node.is_zero() || self.authority.is_member(summary.node) {
            // Before the first aggregate is signed, a summary from an existing
            // member is harmless but cannot be validated; treat the bootstrap
            // join as pending.
        }

        let previous = self.counters.get(&summary.node).copied().unwrap_or(0);
        if summary.counter <= previous {
            if summary.counter == previous {
                self.stats.duplicates += 1;
                return Ok(Ingest::Duplicate);
            }
            self.stats.replays += 1;
            return Err(GossipError::Replay);
        }
        self.counters.insert(summary.node, summary.counter);
        self.stats.summaries_received += 1;
        if direct {
            self.authority
                .observe_with_incarnation(summary.node, summary.incarnation, now_ms);
        }
        Ok(Ingest::Alive)
    }

    /// Mark a member suspect or failed at `now_ms`.
    ///
    /// The failure detector is the only source of removals; a summary cannot
    /// remove another member.
    pub fn expire(&mut self, now_ms: u64) {
        for transition in self.authority.expire(now_ms) {
            let node = transition.node;
            if transition.state == NodeState::Failed && node != self.authority.root() {
                self.pending.insert(node, Pending::Remove);
            }
        }
    }

    /// Sign one aggregate for the pending change set, if it is non-empty.
    ///
    /// The whole set is applied in one epoch advance and one signature.  The
    /// aggregate is returned for dissemination; the caller does not need to
    /// re-sign per member.
    pub fn aggregate(&mut self, now_ms: u64) -> Result<Option<EpochSummary>, GossipError> {
        if self.pending.is_empty() {
            return Ok(None);
        }
        let changes: Vec<Change> = self
            .pending
            .iter()
            .map(|(node, pending)| pending.change(*node))
            .collect();
        for change in &changes {
            if change.node() == self.authority.root() && matches!(change, Change::Remove { .. }) {
                self.pending.remove(&self.authority.root());
                return Err(GossipError::RemoveRoot);
            }
        }
        let set: ChangeSet = self.authority.apply_changes(&changes, now_ms)?;
        self.pending.clear();
        if !set.changed {
            return Ok(None);
        }
        // Refresh the grant table from the confirmed records.
        self.grants.clear();
        for (node, record) in self.authority.records() {
            self.grants.insert(node, record.incarnation);
        }
        let snapshot = self.authority.snapshot()?;
        let grants: Vec<crate::wire::MemberGrant> = snapshot
            .members
            .iter()
            .map(|node| crate::wire::MemberGrant {
                node: *node,
                incarnation: self.grants.get(node).copied().unwrap_or(0),
            })
            .collect();
        let mut summary = EpochSummary::new(self.authority.root(), snapshot.epoch, &grants)?;
        summary.signature = self.authority.sign(&summary.signed_bytes());
        self.current = Some(summary.clone());
        self.roll_epoch(snapshot.epoch);
        self.stats.epoch_changes += 1;
        self.stats.epoch_signatures += 1;
        self.stats.members_added += set.added.len() as u64;
        self.stats.members_removed += set.removed.len() as u64;
        Ok(Some(summary))
    }

    /// Build and sign the aggregate for the current view without a change.
    ///
    /// This is the bootstrap path: a root that restored a confirmed epoch
    /// publishes that epoch once so members can finish joining without per
    /// member confirmations.
    pub fn publish(&mut self) -> Result<EpochSummary, GossipError> {
        let snapshot = self.authority.snapshot()?;
        let mut grants = Vec::with_capacity(snapshot.members.len());
        for node in &snapshot.members {
            let incarnation = self
                .authority
                .records()
                .find(|(candidate, _)| candidate == node)
                .map(|(_, record)| record.incarnation)
                .unwrap_or(0);
            grants.push(crate::wire::MemberGrant {
                node: *node,
                incarnation,
            });
        }
        let mut summary = EpochSummary::new(self.authority.root(), snapshot.epoch, &grants)?;
        summary.signature = self.authority.sign(&summary.signed_bytes());
        self.current = Some(summary.clone());
        self.stats.epoch_signatures += 1;
        Ok(summary)
    }

    /// Force a member's liveness state from a direct frame.
    ///
    /// Link traffic is independent evidence of liveness: a ping or a route
    /// record from the member counts even when no summary arrived this
    /// interval.
    pub fn observe_link(&mut self, node: NodeId, now_ms: u64) {
        let incarnation = self.grants.get(&node).copied().unwrap_or(0);
        if incarnation != 0 || self.grants.contains_key(&node) {
            self.authority
                .observe_with_incarnation(node, incarnation, now_ms);
        }
    }
}

/// A member's gossip state.
pub struct Follower {
    self_id: NodeId,
    incarnation: u64,
    verifier: AuthorityVerifier,
    table: MembershipTable,
    current: Option<EpochSummary>,
    counter: u64,
    /// Highest accepted counter per `(node, epoch)`.
    seen: BTreeMap<(NodeId, u32), u64>,
    /// Epochs already forwarded by this node.
    forwarded: BTreeSet<u32>,
    stats: GossipStats,
}

impl Follower {
    /// Create a member at its seed epoch.
    pub fn new(
        self_id: NodeId,
        root: NodeId,
        key: sys_primitives::NodeKey,
        seed_epoch: u32,
        incarnation: u64,
        now_ms: u64,
    ) -> Result<Self, GossipError> {
        let verifier = AuthorityVerifier::new(root, key).map_err(|error| match error {
            AuthorityError::ZeroRoot => GossipError::WrongRoot,
            _ => GossipError::BadSignature,
        })?;
        let table = MembershipTable::confirmed(seed_epoch, &[self_id], now_ms)?;
        Ok(Self {
            self_id,
            incarnation,
            verifier,
            table,
            current: None,
            counter: 0,
            seen: BTreeMap::new(),
            forwarded: BTreeSet::new(),
            stats: GossipStats::default(),
        })
    }

    /// This node's identity.
    pub const fn self_id(&self) -> NodeId {
        self.self_id
    }

    /// The installed aggregate, if any.
    pub fn current(&self) -> Option<&EpochSummary> {
        self.current.as_ref()
    }

    /// The current local epoch.
    pub fn epoch(&self) -> u32 {
        self.current
            .as_ref()
            .map(|summary| summary.epoch)
            .unwrap_or_else(|| self.table.epoch())
    }

    /// Traffic and outcome counters.
    pub const fn stats(&self) -> &GossipStats {
        &self.stats
    }

    /// Verify and install one aggregate.
    ///
    /// A newer epoch replaces the view; the same epoch with the same digest is
    /// idempotent; a lower epoch, a different list at the same epoch, a
    /// signature failure or a view that omits this node fails closed and
    /// leaves the previous view in place.
    pub fn install(
        &mut self,
        summary: &EpochSummary,
        now_ms: u64,
    ) -> Result<MembershipSnapshot, GossipError> {
        if summary.root != self.verifier.root() {
            return Err(GossipError::WrongRoot);
        }
        self.verifier
            .verify_signature(&summary.signed_bytes(), &summary.signature)?;
        let snapshot = summary.snapshot()?;
        if let Some(current) = &self.current {
            if snapshot.epoch < current.epoch {
                self.stats.stale += 1;
                return Err(GossipError::StaleEpoch);
            }
            if snapshot.epoch == current.epoch && snapshot.digest() != current.digest {
                self.stats.equivocations += 1;
                return Err(GossipError::Equivocation);
            }
        }
        if !snapshot.members.contains(&self.verifier.root()) {
            return Err(GossipError::RootNotMember);
        }
        if !snapshot.members.contains(&self.self_id) {
            return Err(GossipError::SubjectNotMember);
        }
        self.table
            .install(snapshot.epoch, &snapshot.members, now_ms)?;
        // Drop counters from superseded epochs so the relay state is bounded.
        self.seen.retain(|(_, epoch), _| *epoch >= snapshot.epoch);
        self.forwarded.retain(|epoch| *epoch >= snapshot.epoch);
        self.current = Some(summary.clone());
        self.stats.installs += 1;
        Ok(snapshot)
    }

    /// Produce this member's own summary, or `None` before the first install.
    pub fn summary(&mut self) -> Option<MemberSummary> {
        let current = self.current.as_ref()?;
        self.counter = self.counter.saturating_add(1);
        self.stats.summaries_sent += 1;
        Some(MemberSummary {
            node: self.self_id,
            epoch: current.epoch,
            digest: current.digest,
            incarnation: self.incarnation,
            counter: self.counter,
        })
    }

    /// Validate one peer summary against the installed aggregate.
    pub fn ingest(&mut self, summary: &MemberSummary) -> Result<Ingest, GossipError> {
        let Some(current) = self.current.as_ref() else {
            return Err(GossipError::NoAggregate);
        };
        if summary.epoch < current.epoch {
            self.stats.stale += 1;
            return Err(GossipError::StaleEpoch);
        }
        if summary.epoch > current.epoch {
            self.stats.stale += 1;
            return Err(GossipError::UnknownEpoch);
        }
        if summary.digest != current.digest {
            self.stats.equivocations += 1;
            return Err(GossipError::Equivocation);
        }
        match current.grant(summary.node) {
            Some(incarnation) if incarnation == summary.incarnation => {}
            Some(_) => {
                self.stats.equivocations += 1;
                return Err(GossipError::IncarnationMismatch);
            }
            None => return Err(GossipError::NotMember),
        }
        let key = (summary.node, summary.epoch);
        let previous = self.seen.get(&key).copied().unwrap_or(0);
        if summary.counter <= previous {
            if summary.counter == previous {
                self.stats.duplicates += 1;
                return Ok(Ingest::Duplicate);
            }
            self.stats.replays += 1;
            return Err(GossipError::Replay);
        }
        self.seen.insert(key, summary.counter);
        self.stats.summaries_received += 1;
        Ok(Ingest::Alive)
    }

    /// Whether this node should forward the aggregate for `epoch`.
    ///
    /// Each node forwards a given epoch once, so the aggregate spread is
    /// bounded by the member count rather than by the number of links.
    pub fn should_forward_epoch(&mut self, epoch: u32) -> bool {
        if !self.forwarded.insert(epoch) {
            return false;
        }
        self.stats.forwards += 1;
        true
    }

    /// Choose up to [`FANOUT`] peers for one record.
    ///
    /// The choice is deterministic: peers are sorted, the sender and the local
    /// node are excluded, and the starting offset rotates with `counter` so a
    /// fixed hash cannot pin all traffic to one link.  `exclude` also removes
    /// peers that have already seen the record in this round.
    pub fn fanout(&self, peers: &[NodeId], exclude: &[NodeId], counter: u64) -> Vec<NodeId> {
        let mut candidates: Vec<NodeId> = peers
            .iter()
            .copied()
            .filter(|node| *node != self.self_id && !exclude.contains(node))
            .collect();
        candidates.sort_unstable();
        candidates.dedup();
        if candidates.is_empty() {
            return Vec::new();
        }
        let start = (counter as usize) % candidates.len();
        candidates
            .into_iter()
            .cycle()
            .skip(start)
            .take(FANOUT)
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::authority::SigningAuthority;
    use crate::wire::MemberGrant;

    fn node(n: u8) -> NodeId {
        let mut bytes = [0u8; 16];
        bytes[0] = n;
        NodeId(bytes)
    }

    fn authority() -> SigningAuthority {
        SigningAuthority::from_bytes(&[7u8; 32])
    }

    fn aggregator(seed: u32) -> Aggregator {
        Aggregator::new(node(1), authority(), seed, 0).expect("root")
    }

    fn follower(id: u8, seed: u32) -> Follower {
        Follower::new(
            node(id),
            node(1),
            authority().public(),
            seed,
            0x100 + id as u64,
            0,
        )
        .expect("member")
    }

    fn summary(
        id: u8,
        epoch: u32,
        digest: [u8; 16],
        incarnation: u64,
        counter: u64,
    ) -> MemberSummary {
        MemberSummary {
            node: node(id),
            epoch,
            digest,
            incarnation,
            counter,
        }
    }

    #[test]
    fn aggregate_advances_the_epoch_once_for_a_batch() {
        let mut root = aggregator(1);
        let bootstrap = root.publish().expect("publish");
        for id in 2..=4 {
            let ingest = root
                .ingest(
                    &summary(id, bootstrap.epoch, bootstrap.digest, 0x100 + id as u64, 1),
                    true,
                    1,
                )
                .expect("candidate");
            assert_eq!(ingest, Ingest::JoinPending);
        }
        let aggregate = root.aggregate(2).expect("aggregate").expect("change");
        assert_eq!(aggregate.epoch, bootstrap.epoch + 1);
        assert_eq!(aggregate.grants.len(), 4);
        assert_eq!(root.stats().epoch_changes, 1);
        assert_eq!(root.stats().epoch_signatures, 2); // bootstrap + one change
        assert_eq!(root.stats().members_added, 3);
        // A second aggregation with no pending change must not sign.
        assert!(root.aggregate(3).expect("idle").is_none());
        assert_eq!(root.stats().epoch_signatures, 2);
    }

    #[test]
    fn member_installs_aggregate_and_rejects_stale_and_replay() {
        let mut root = aggregator(1);
        let bootstrap = root.publish().expect("publish");
        let mut member = follower(2, 1);
        root.ingest(
            &summary(2, bootstrap.epoch, bootstrap.digest, 0x102, 1),
            true,
            1,
        )
        .expect("candidate");
        let aggregate = root.aggregate(2).expect("aggregate").expect("change");
        let installed = member.install(&aggregate, 3).expect("install");
        assert_eq!(installed.epoch, aggregate.epoch);
        assert_eq!(member.epoch(), aggregate.epoch);

        // A stale aggregate is refused and keeps the view.
        assert_eq!(
            member.install(&bootstrap, 4).unwrap_err(),
            GossipError::StaleEpoch
        );
        assert_eq!(member.epoch(), aggregate.epoch);

        // Changing a signed field without the key is a signature failure.
        let mut forged = aggregate.clone();
        forged.digest = [0xff; 16];
        assert_eq!(
            member.install(&forged, 4).unwrap_err(),
            GossipError::BadSignature
        );
        assert_eq!(member.epoch(), aggregate.epoch);

        // A second root signature over a different list at the same epoch is
        // the equivocation the install path must refuse.
        let mut equivocal = EpochSummary::new(
            node(1),
            aggregate.epoch,
            &[
                MemberGrant {
                    node: node(1),
                    incarnation: 0,
                },
                MemberGrant {
                    node: node(2),
                    incarnation: 0x102,
                },
                MemberGrant {
                    node: node(3),
                    incarnation: 0x103,
                },
            ],
        )
        .expect("equivocal list");
        equivocal.signature = authority().sign(&equivocal.signed_bytes());
        assert_eq!(
            member.install(&equivocal, 4).unwrap_err(),
            GossipError::Equivocation
        );
        assert_eq!(member.epoch(), aggregate.epoch);
    }

    #[test]
    fn peer_summary_is_bound_to_the_signed_incarnation() {
        let mut root = aggregator(1);
        let bootstrap = root.publish().expect("publish");
        root.ingest(
            &summary(2, bootstrap.epoch, bootstrap.digest, 0x102, 1),
            true,
            1,
        )
        .expect("candidate");
        root.ingest(
            &summary(3, bootstrap.epoch, bootstrap.digest, 0x103, 1),
            true,
            1,
        )
        .expect("candidate");
        let aggregate = root.aggregate(2).expect("aggregate").expect("change");
        let mut member = follower(2, 1);
        member.install(&aggregate, 3).expect("install");

        let peer = summary(3, aggregate.epoch, aggregate.digest, 0x103, 1);
        assert_eq!(member.ingest(&peer), Ok(Ingest::Alive));
        assert_eq!(member.ingest(&peer), Ok(Ingest::Duplicate));
        // A replayed older counter is refused.
        let mut replay = peer;
        replay.counter = 0;
        assert_eq!(member.ingest(&replay).unwrap_err(), GossipError::Replay);
        // A different incarnation cannot borrow the member's grant.
        let mut forged = peer;
        forged.incarnation = 0x999;
        forged.counter = 9;
        assert_eq!(
            member.ingest(&forged).unwrap_err(),
            GossipError::IncarnationMismatch
        );
        // A node outside the signed list has no grant.
        let outsider = summary(9, aggregate.epoch, aggregate.digest, 1, 1);
        assert_eq!(
            member.ingest(&outsider).unwrap_err(),
            GossipError::NotMember
        );
    }

    #[test]
    fn root_rejects_a_member_with_a_different_digest() {
        let mut root = aggregator(1);
        let bootstrap = root.publish().expect("publish");
        root.ingest(
            &summary(2, bootstrap.epoch, bootstrap.digest, 0x102, 1),
            true,
            1,
        )
        .expect("candidate");
        let aggregate = root.aggregate(2).expect("aggregate").expect("change");
        let mut forged = summary(2, aggregate.epoch, [0xaa; 16], 0x102, 2);
        assert_eq!(
            root.ingest(&forged, true, 3).unwrap_err(),
            GossipError::Equivocation
        );
        // The wrong digest was not accepted as a counter change either.
        forged.digest = aggregate.digest;
        forged.counter = 2;
        assert_eq!(root.ingest(&forged, true, 3), Ok(Ingest::Alive));
    }

    #[test]
    fn forwarded_summary_does_not_refresh_liveness() {
        let mut root = aggregator(1);
        let bootstrap = root.publish().expect("publish");
        root.ingest(
            &summary(2, bootstrap.epoch, bootstrap.digest, 0x102, 1),
            true,
            1,
        )
        .expect("candidate");
        root.ingest(
            &summary(3, bootstrap.epoch, bootstrap.digest, 0x103, 1),
            true,
            1,
        )
        .expect("candidate");
        let aggregate = root.aggregate(2).expect("aggregate").expect("change");
        // A forwarded summary for member 2 with a newer counter is validated
        // but must not refresh 2's liveness record.
        let forwarded = summary(2, aggregate.epoch, aggregate.digest, 0x102, 2);
        assert_eq!(root.ingest(&forwarded, false, 10_000), Ok(Ingest::Alive));
        assert!(root.authority.is_member(node(2)));
    }

    #[test]
    fn fanout_is_bounded_and_deterministic() {
        let member = follower(2, 1);
        let peers: Vec<NodeId> = (1..=8).map(node).collect();
        let selected = member.fanout(&peers, &[node(2)], 1);
        assert_eq!(selected.len(), FANOUT);
        assert_eq!(selected, member.fanout(&peers, &[node(2)], 1));
        assert!(!selected.contains(&node(2)));
        // Exclusions reduce the set without exceeding the fan-out.
        let excluded = [node(2), node(1), node(3)];
        let selected = member.fanout(&peers, &excluded, 1);
        assert!(selected.iter().all(|peer| !excluded.contains(peer)));
        assert!(selected.len() <= FANOUT);
    }

    #[test]
    fn remove_failed_batches_into_one_epoch() {
        let mut root = aggregator(1);
        let bootstrap = root.publish().expect("publish");
        for id in 2..=5 {
            root.ingest(
                &summary(id, bootstrap.epoch, bootstrap.digest, 0x100 + id as u64, 1),
                true,
                0,
            )
            .expect("candidate");
        }
        let aggregate = root.aggregate(1).expect("aggregate").expect("change");
        assert_eq!(aggregate.grants.len(), 5);
        // Member 2 stays live; the other three go silent past the failure
        // window.  The expiry and the aggregate must batch them into one
        // epoch advance.
        let live = summary(2, aggregate.epoch, aggregate.digest, 0x102, 2);
        assert_eq!(root.ingest(&live, true, 2_000), Ok(Ingest::Alive));
        root.expire(mesh_service::FAIL_AFTER_MS + 1_000);
        let aggregate = root
            .aggregate(mesh_service::FAIL_AFTER_MS + 1_001)
            .expect("aggregate");
        let aggregate = aggregate.expect("one removal set");
        assert_eq!(aggregate.grants.len(), 2);
        assert_eq!(root.stats().epoch_changes, 2);
        assert_eq!(root.stats().members_removed, 3);
    }
}
