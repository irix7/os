// SPDX-License-Identifier: GPL-3.0-only
//! Deterministic host policy model for aggregated/gossip membership.
//!
//! The model runs the production [`Aggregator`] and [`Follower`] over an
//! in-memory overlay with injected time.  It answers the #249 policy
//! questions that a four-kernel lab cannot answer cheaply:
//!
//! * does every node converge on the same epoch and digest?
//! * how many records and bytes does one join/removal change set cost at
//!   2, 8 and 128 nodes?
//! * is the cost linear in the node count, or quadratic?
//!
//! The overlay is deterministic: nodes are numbered `1..=N`, the root is node
//! 1, and each node's peer list is the sorted set of power-of-two ring jumps
//! plus the root.  No randomness, no wall clock and no sockets, so the same
//! input produces the same counters on every host.
//!
//! The model is **not** evidence of kernel behaviour; the rig evidence is.  It
//! is the traffic-complexity gate the real clusters are too small to provide.

#[cfg(test)]
extern crate std;

use alloc::collections::BTreeMap;
use alloc::vec::Vec;

use sys_primitives::NodeId;

use crate::authority::{AuthorityError, SigningAuthority};
use crate::gossip::{Aggregator, Follower, GossipError, GossipStats, FANOUT, MAX_HOPS};
use crate::wire::{EpochSummary, MemberSummary, MAX_MEMBERS};

/// Bytes of `mesh-transport` envelope and header around one body.
///
/// The model counts protocol records; the envelope constant keeps the byte
/// figures comparable with the serial `rx_bytes` counters without pretending
/// to model TCP/IP framing.
pub const ENVELOPE_BYTES: u64 = 48;
/// Maximum rounds one phase may take before the model declares non-convergence.
pub const MAX_ROUNDS: u32 = 256;
/// Relay budget per node per round.
///
/// Each node forwards at most this many summaries per round, so a round costs
/// `O(N * FANOUT)` records even in the worst case.  The aggregate has its own
/// once-per-epoch rule.
pub const RELAY_BUDGET: usize = FANOUT;

/// How a phase ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Convergence {
    /// Every member installed the root's epoch and the same digest.
    Converged,
    /// The round budget ran out first.
    NotConverged,
}

/// One model run at one node count.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ModelResult {
    /// Nodes in the run, including the root.
    pub nodes: u8,
    /// Final confirmed epoch.
    pub epoch: u32,
    /// Final snapshot digest.
    pub digest: [u8; 16],
    /// Members in the final view.
    pub members: usize,
    /// Rounds the join phase needed.
    pub join_rounds: u32,
    /// Rounds the removal phase needed.
    pub removal_rounds: u32,
    /// Whether both phases converged.
    pub convergence: Convergence,
    /// Total protocol records in the join phase.
    pub join_messages: u64,
    /// Total protocol bytes in the join phase.
    pub join_bytes: u64,
    /// Total protocol records in the removal phase.
    pub removal_messages: u64,
    /// Total protocol bytes in the removal phase.
    pub removal_bytes: u64,
    /// Largest per-node record count across both phases.
    pub max_node_records: u64,
    /// Largest record count for a *member* (the root is O(N) by design and is
    /// excluded; a member's gossip work must stay bounded).
    pub max_member_records: u64,
    /// Authority signatures produced across both phases.
    pub signatures: u64,
    /// Member summaries the root accepted directly.
    pub summaries_accepted: u64,
    /// Replays and duplicates observed by any node.
    pub replays: u64,
    /// Aggregates installed by members.
    pub installs: u64,
    /// Aggregate installs that failed.
    pub install_errors: u64,
    /// The first install failure label, if any.
    pub install_error: &'static str,
}

impl ModelResult {
    /// Records per node for the join phase.
    pub fn join_records_per_node(&self) -> f64 {
        self.join_messages as f64 / self.nodes as f64
    }

    /// Records per node for the removal phase.
    pub fn removal_records_per_node(&self) -> f64 {
        self.removal_messages as f64 / self.nodes as f64
    }
}

/// A model run failed before it could converge.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ModelError {
    /// The node count is outside `2..=128`.
    NodeCount,
    /// Membership or gossip rejected a step.
    Gossip(GossipError),
}

impl From<GossipError> for ModelError {
    fn from(error: GossipError) -> Self {
        Self::Gossip(error)
    }
}

impl From<AuthorityError> for ModelError {
    fn from(error: AuthorityError) -> Self {
        Self::Gossip(GossipError::from(error))
    }
}

fn node(id: u8) -> NodeId {
    let mut bytes = [0u8; 16];
    bytes[0] = id;
    NodeId(bytes)
}

/// The deterministic overlay.
///
/// Peer lists are symmetric ring jumps of 1, 2, 4, 8, 16, 32 and 64 positions
/// plus the root.  The diameter is logarithmic, so a bounded relay budget
/// still reaches every node in a few rounds.
fn overlay(nodes: u8) -> Vec<Vec<NodeId>> {
    let n = nodes as usize;
    let mut peers = Vec::with_capacity(n);
    for index in 0..n {
        let mut list = Vec::new();
        for shift in 0..7 {
            let jump = 1usize << shift;
            if jump >= n {
                break;
            }
            list.push(node(((index + jump) % n) as u8 + 1));
            list.push(node(((index + n - jump) % n) as u8 + 1));
        }
        list.push(node(1));
        list.sort_unstable();
        list.dedup();
        list.retain(|peer| *peer != node(index as u8 + 1));
        peers.push(list);
    }
    peers
}

fn summary_bytes() -> u64 {
    MemberSummary::LEN as u64 + ENVELOPE_BYTES
}

fn aggregate_bytes(members: usize) -> u64 {
    EpochSummary::len(members) as u64 + ENVELOPE_BYTES
}

/// Traffic counters for one phase.
#[derive(Clone, Copy, Debug)]
struct Traffic {
    messages: u64,
    bytes: u64,
    per_node: [u64; MAX_MEMBERS],
    signatures: u64,
    summaries_accepted: u64,
    install_errors: u64,
    install_error: &'static str,
}

impl Default for Traffic {
    fn default() -> Self {
        Self {
            messages: 0,
            bytes: 0,
            per_node: [0; MAX_MEMBERS],
            signatures: 0,
            summaries_accepted: 0,
            install_errors: 0,
            install_error: "",
        }
    }
}

impl Traffic {
    fn record(&mut self, from: usize, bytes: u64) {
        self.messages += 1;
        self.bytes += bytes;
        self.per_node[from] += 1;
    }

    fn max_node(&self) -> u64 {
        self.per_node.iter().copied().max().unwrap_or(0)
    }

    /// The largest count for any node except the root (index 0).
    fn max_member(&self) -> u64 {
        self.per_node[1..].iter().copied().max().unwrap_or(0)
    }
}

/// Run the model at `nodes` members including the root.
///
/// The run has two phases against one membership: a batch join of every
/// member, then a batch removal of `nodes / 4` members (at least one).  Both
/// phases must converge on the same epoch and digest at every node.
pub fn run(nodes: u8) -> Result<ModelResult, ModelError> {
    if nodes < 2 || nodes as usize > MAX_MEMBERS {
        return Err(ModelError::NodeCount);
    }
    let count = nodes as usize;
    let root = node(1);
    let signing = SigningAuthority::from_bytes(&[7u8; 32]);
    let mut aggregator = Aggregator::new(root, signing, 1, 0)?;
    let mut followers: Vec<Follower> = Vec::with_capacity(count - 1);
    for id in 2..=nodes {
        followers.push(Follower::new(
            node(id),
            root,
            SigningAuthority::from_bytes(&[7u8; 32]).public(),
            1,
            0x100 + id as u64,
            0,
        )?);
    }
    let peers = overlay(nodes);
    let mut total = Traffic::default();
    let mut stats = GossipStats::default();

    // ---- Phase 1: every member joins in one change set -------------------
    for id in 2..=nodes {
        aggregator.request_join(node(id), 0x100 + id as u64)?;
    }
    let mut join = Traffic::default();
    let join_rounds = disseminate_until_converged(
        &mut aggregator,
        &mut followers,
        &peers,
        &mut join,
        &mut stats,
        0,
    )?;
    total.record(0, 0);
    merge(&mut total, &join);

    // ---- Phase 2: a batch removal of up to a quarter of the view --------
    //
    // The surviving members send one summary each, so the detector sees them
    // live; the removal targets stay silent since the join.  That is the
    // "k members stopped" shape: one change set, one epoch, one signature.
    let removals = (count / 4).max(1);
    let removal_targets: Vec<u8> = (2..=nodes).take(removals).collect();
    let joined_at = join_rounds as u64 * 5_000;
    let live_at = joined_at + 1_500;
    let expire_at = joined_at + 12_500;
    let mut removal = Traffic::default();
    for (index, follower) in followers.iter_mut().enumerate() {
        let id = (index + 2) as u8;
        if removal_targets.contains(&id) {
            continue;
        }
        if let Some(summary) = follower.summary() {
            if aggregator.ingest(&summary, true, live_at).is_ok() {
                removal.record(index + 1, summary_bytes());
            }
        }
    }
    // Targets are 12.5 s silent and fail; survivors are 11.5 s silent and stay
    // inside the failure window.
    aggregator.expire(expire_at);
    aggregator.expire(expire_at + 100);
    let removal_rounds = disseminate_until_converged(
        &mut aggregator,
        &mut followers,
        &peers,
        &mut removal,
        &mut stats,
        expire_at + 100,
    )?;
    merge(&mut total, &removal);

    let final_snapshot = aggregator.snapshot()?;
    let converged = followers.iter().enumerate().all(|(index, follower)| {
        let id = node((index + 2) as u8);
        if !final_snapshot.members.contains(&id) {
            // A removed member is not told; it keeps its last confirmed view
            // and must not be counted as converged on the new epoch.
            return true;
        }
        follower.epoch() == final_snapshot.epoch
            && follower
                .current()
                .is_some_and(|current| current.digest == final_snapshot.digest())
    });
    Ok(ModelResult {
        nodes,
        epoch: final_snapshot.epoch,
        digest: final_snapshot.digest(),
        members: final_snapshot.len(),
        join_rounds,
        removal_rounds,
        convergence: if converged && join_rounds < MAX_ROUNDS && removal_rounds < MAX_ROUNDS {
            Convergence::Converged
        } else {
            Convergence::NotConverged
        },
        join_messages: join.messages,
        join_bytes: join.bytes,
        removal_messages: removal.messages,
        removal_bytes: removal.bytes,
        max_node_records: total.max_node(),
        max_member_records: total.max_member(),
        signatures: total.signatures,
        summaries_accepted: total.summaries_accepted,
        replays: stats.replays + stats.duplicates,
        installs: stats.installs,
        install_errors: total.install_errors,
        install_error: total.install_error,
    })
}

fn merge(into: &mut Traffic, from: &Traffic) {
    into.messages += from.messages;
    into.bytes += from.bytes;
    into.signatures += from.signatures;
    into.summaries_accepted += from.summaries_accepted;
    into.install_errors += from.install_errors;
    if into.install_error.is_empty() {
        into.install_error = from.install_error;
    }
    for (index, count) in from.per_node.iter().enumerate() {
        into.per_node[index] += count;
    }
}

/// Feed summaries upward and the aggregate downward until stable.
///
/// One round is: every installed member emits its own summary and gossips it
/// within its relay budget; every node that receives a new aggregate installs
/// it and forwards it once to up to [`FANOUT`] peers; the root aggregates any
/// pending change set once.
fn disseminate_until_converged(
    aggregator: &mut Aggregator,
    followers: &mut [Follower],
    peers: &[Vec<NodeId>],
    traffic: &mut Traffic,
    stats: &mut GossipStats,
    start_ms: u64,
) -> Result<u32, ModelError> {
    let root = aggregator.root();
    let count = peers.len();
    let mut aggregate_queue: BTreeMap<usize, Vec<(EpochSummary, u8)>> = BTreeMap::new();
    let mut summary_queue: BTreeMap<usize, Vec<(MemberSummary, bool)>> = BTreeMap::new();
    let mut forward_counts = [0usize; MAX_MEMBERS];
    let mut forwarded_epoch = [0u32; MAX_MEMBERS];
    let mut forward_seen: BTreeMap<(usize, u32, u8), ()> = BTreeMap::new();
    let mut seen_summaries: BTreeMap<(NodeId, u32, u64), ()> = BTreeMap::new();
    let mut round = 0u32;

    loop {
        round += 1;
        if round > MAX_ROUNDS {
            return Ok(MAX_ROUNDS);
        }
        let now = start_ms + round as u64 * 5_000;
        let mut emitted = 0usize;

        // 1. Members emit summaries and gossip them.
        for (index, follower) in followers.iter_mut().enumerate() {
            if follower.current().is_none() {
                continue;
            }
            if let Some(summary) = follower.summary() {
                emitted += 1;
                let from = index + 1; // node index (0-based) + 1
                let chosen = follower.fanout(&peers[from], &[], round as u64);
                for target in &chosen {
                    let target_index = target.0[0] as usize - 1;
                    traffic.record(from, summary_bytes());
                    summary_queue
                        .entry(target_index)
                        .or_default()
                        .push((summary, true));
                }
                if chosen.is_empty() {
                    // The last node standing still accounts for its record.
                    traffic.record(from, summary_bytes());
                }
            }
        }
        let _ = emitted;

        // 2. Deliver summaries, forward within the relay budget.
        let delivered = core::mem::take(&mut summary_queue);
        for (target_index, batch) in delivered {
            forward_counts[target_index] = 0;
            for (summary, direct) in batch {
                let key = (summary.node, summary.epoch, summary.counter);
                if seen_summaries.insert(key, ()).is_some() {
                    stats.duplicates += 1;
                    continue;
                }
                if target_index == 0 {
                    match aggregator.ingest(&summary, direct, now) {
                        Ok(_) => traffic.summaries_accepted += 1,
                        Err(error) => match error {
                            GossipError::Replay => stats.replays += 1,
                            GossipError::StaleEpoch | GossipError::UnknownEpoch => stats.stale += 1,
                            _ => {}
                        },
                    }
                    continue;
                }
                let follower = &mut followers[target_index - 1];
                match follower.ingest(&summary) {
                    Ok(_) => traffic.summaries_accepted += 1,
                    Err(GossipError::Replay) => {
                        stats.replays += 1;
                        continue;
                    }
                    Err(_) => continue,
                }
                if forward_counts[target_index] >= RELAY_BUDGET {
                    continue;
                }
                forward_counts[target_index] += 1;
                let chosen = follower.fanout(&peers[target_index], &[], round as u64);
                for target in chosen {
                    let next = target.0[0] as usize - 1;
                    if next == target_index {
                        continue;
                    }
                    traffic.record(target_index, summary_bytes());
                    summary_queue
                        .entry(next)
                        .or_default()
                        .push((summary, false));
                }
            }
        }

        // 3. Root aggregates the pending change set exactly once.
        if aggregator.pending_len() > 0 {
            if let Some(summary) = aggregator.aggregate(now)? {
                traffic.signatures += 1;
                let members = summary.grants.len();
                let member_ids: Vec<NodeId> =
                    summary.grants.iter().map(|grant| grant.node).collect();
                let chosen = member_targets(&member_ids);
                for target_index in chosen {
                    traffic.record(0, aggregate_bytes(members));
                    aggregate_queue
                        .entry(target_index)
                        .or_default()
                        .push((summary.clone(), 1));
                }
            }
        }

        // 4. Deliver aggregates; each node installs once and forwards once per
        //    hop, up to MAX_HOPS, so the flood is bounded but cannot leave a
        //    disconnected corner of the overlay behind.
        let delivered = core::mem::take(&mut aggregate_queue);
        for (target_index, batch) in delivered {
            for (summary, hop) in batch {
                let mut fresh = false;
                if summary.epoch > forwarded_epoch[target_index] {
                    forwarded_epoch[target_index] = summary.epoch;
                    let follower = &mut followers[target_index - 1];
                    match follower.install(&summary, now) {
                        Ok(_) => {
                            stats.installs += 1;
                            fresh = true;
                        }
                        Err(error) => {
                            traffic.install_errors += 1;
                            traffic.install_error = error.as_str();
                        }
                    }
                }
                if !fresh || hop >= MAX_HOPS {
                    continue;
                }
                if forward_seen
                    .insert((target_index, summary.epoch, hop), ())
                    .is_some()
                {
                    continue;
                }
                let follower = &mut followers[target_index - 1];
                let chosen = follower.fanout(&peers[target_index], &[], round as u64 + hop as u64);
                for target in chosen {
                    let next = target.0[0] as usize - 1;
                    if next == 0 || next == target_index {
                        continue;
                    }
                    if !summary.grants.iter().any(|grant| grant.node == target) {
                        // A removed member is not a dissemination target.
                        continue;
                    }
                    traffic.record(target_index, aggregate_bytes(summary.grants.len()));
                    aggregate_queue
                        .entry(next)
                        .or_default()
                        .push((summary.clone(), hop + 1));
                }
            }
        }

        // 5. Converged when the root has no pending change and every member
        //    holds the root's epoch and digest.
        let converged_epoch = aggregator.epoch();
        let converged_digest = aggregator.snapshot()?.digest();
        let view = aggregator.snapshot()?;
        let members_ready = followers.iter().enumerate().all(|(index, follower)| {
            let id = node((index + 2) as u8);
            if !view.members.contains(&id) {
                // Removed members keep their old view by design.
                return true;
            }
            follower.epoch() == converged_epoch
                && follower
                    .current()
                    .is_some_and(|current| current.digest == converged_digest)
        });
        if members_ready && aggregator.pending_len() == 0 && aggregate_queue.is_empty() {
            let _ = root;
            let _ = count;
            let _ = MAX_HOPS;
            return Ok(round);
        }
    }
}

/// The root's initial fan-out over the overlay.
/// Every non-root member, as a 0-based node index.
///
/// The root holds one module TCP slot per member, so it seeds the aggregate on
/// each link: `O(N)` records for the change, not a broadcast to arbitrary
/// peers.  Gossip fan-out then carries the aggregate to peers that are not
/// directly attached to the root.
fn member_targets(members: &[NodeId]) -> Vec<usize> {
    let mut targets: Vec<usize> = members
        .iter()
        .map(|member| member.0[0] as usize - 1)
        .filter(|index| *index != 0)
        .collect();
    targets.sort_unstable();
    targets.dedup();
    targets
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn two_nodes_converge_with_one_signature_per_change() {
        let result = run(2).expect("model");
        assert_eq!(result.convergence, Convergence::Converged);
        assert_eq!(result.epoch, 3);
        assert_eq!(result.members, 1);
        assert_eq!(result.signatures, 2);
        assert_eq!(result.join_rounds, 1);
    }

    #[test]
    fn eight_nodes_converge_and_batch_removals() {
        let result = run(8).expect("model");
        assert_eq!(result.convergence, Convergence::Converged);
        assert_eq!(result.epoch, 3);
        assert_eq!(result.members, 6);
        assert_eq!(result.signatures, 2);
        assert!(result.join_rounds <= MAX_HOPS as u32 + 1);
    }

    #[test]
    fn one_hundred_and_twenty_eight_nodes_stay_linear() {
        let result = run(128).expect("model");
        assert_eq!(result.convergence, Convergence::Converged);
        assert_eq!(result.epoch, 3);
        assert_eq!(result.members, 96);
        assert_eq!(result.signatures, 2);
        // One summary and one aggregate record per member per phase, plus the
        // bounded relay fan-out.  Assert the linear shape rather than a byte
        // count that would encode an implementation detail.
        let per_node = result.join_records_per_node();
        assert!(per_node <= 16.0, "join records per node {per_node}");
        // One signed aggregate per member link, plus bounded gossip relay:
        // well under a constant per node, and far under the quadratic fan-in
        // (~128 * 128 = 16 384 records) the old per-member refresh produced.
        assert!(result.join_messages <= 16 * 128, "{}", result.join_messages);
        assert!(
            result.removal_messages <= 16 * 128,
            "{}",
            result.removal_messages
        );
        assert!(result.join_messages < 16_384 / 4);
        // 16x the nodes must not cost more than ~16x the records.
        let eight = run(8).expect("8");
        assert!(
            result.join_messages <= 20 * eight.join_messages,
            "128 nodes cost {} records vs 8 nodes {}",
            result.join_messages,
            eight.join_messages
        );
        let digest_128 = result.digest;
        let again = run(128).expect("deterministic");
        assert_eq!(again.digest, digest_128);
        assert_eq!(again.join_messages, result.join_messages);
    }

    #[test]
    fn traffic_per_node_is_bounded_for_every_size() {
        let two = run(2).expect("2");
        let eight = run(8).expect("8");
        let large = run(128).expect("128");
        // The root aggregates N summaries and pushes N links: its count is
        // linear by construction.  A member's own gossip work must stay a
        // small constant as the view grows.
        assert!(
            eight.max_member_records <= 64,
            "8-node member max {}",
            eight.max_member_records
        );
        assert!(
            large.max_member_records <= 64,
            "128-node member max {}",
            large.max_member_records
        );
        assert!(large.max_member_records <= 2 * eight.max_member_records + 16);
        // The total must stay linear: 127 joins cost far less than the
        // quadratic fan-in.
        assert!(large.join_messages < 16_384 / 4);
        assert!(large.join_messages as f64 / 128.0 < 32.0);
        assert!(two.join_messages > 0);
    }
}
