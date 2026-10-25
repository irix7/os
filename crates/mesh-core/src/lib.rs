// SPDX-License-Identifier: GPL-3.0-only
//! `mesh_core` — the testable part of the global kernel's sharding policy.
//!
//! This crate is deliberately small and `no_std` (with `alloc`):
//!
//! * rendezvous hashing from an object/actor identity to a home node;
//! * a bounded route cache with home epochs;
//! * leases with fencing epochs;
//! * a placement view built from per-node summaries.
//!
//! It does **not** implement networking, membership gossip or consensus. Those
//! live in the native shim and the mesh service; this crate is the policy that
//! must be identical on every node.
//!
//! The sketch uses a deterministic 64-bit mixer for rendezvous scores. The
//! production implementation replaces it with `BLAKE3-64("I7-HOME-v1" || oid ||
//! node || vnode)`; the algorithm and its properties are the same. The mixer is
//! host-checked and has no cryptographic claim.

#![no_std]
#![forbid(unsafe_code)]
#![deny(missing_docs)]

extern crate alloc;

use alloc::collections::BTreeMap;
use alloc::vec::Vec;
use sys_primitives::{ActorId, NodeId, ObjectId};

/// Hard maximum number of kernel replicas in one logical computer.
pub const MAX_NODES: usize = 128;

/// Virtual nodes per physical node for rendezvous balance.
pub const VIRTUAL_NODES: u32 = 32;

/// A membership snapshot.  It is immutable; a new epoch is a new value.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MembershipView {
    /// Membership epoch.
    pub epoch: u32,
    /// Nodes in the view, at most [`MAX_NODES`].
    pub nodes: Vec<NodeId>,
}

impl MembershipView {
    /// Create a view.  More than [`MAX_NODES`] is rejected by returning `None`.
    pub fn new(epoch: u32, nodes: Vec<NodeId>) -> Option<Self> {
        if nodes.len() > MAX_NODES {
            return None;
        }
        let mut sorted = nodes;
        sorted.sort_unstable();
        sorted.dedup();
        Some(Self {
            epoch,
            nodes: sorted,
        })
    }

    /// The single-node base case: one member, no network, no quorum.
    pub fn single(node: NodeId) -> Self {
        Self {
            epoch: 0,
            nodes: alloc::vec![node],
        }
    }

    /// Whether the view has exactly one node.
    pub fn is_single(&self) -> bool {
        self.nodes.len() == 1
    }

    /// Whether `node` is a member.
    pub fn contains(&self, node: NodeId) -> bool {
        self.nodes.binary_search(&node).is_ok()
    }

    /// Number of members.
    pub fn len(&self) -> usize {
        self.nodes.len()
    }

    /// Whether the view is empty.
    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }
}

/// Compute the home node for an object under a membership view.
///
/// Rendezvous hashing: for each physical node and virtual node, compute a
/// score from `(object, node, vnode)`; the highest score wins.  Adding or
/// removing a node only moves the objects that mapped to that node, which is
/// the property the mesh needs for bounded rebalancing.
pub fn home_for(object: &ObjectId, view: &MembershipView) -> Option<NodeId> {
    if view.nodes.is_empty() {
        return None;
    }
    let mut best_score = 0u64;
    let mut best_node = None;
    for node in &view.nodes {
        for vnode in 0..VIRTUAL_NODES {
            let score = rendezvous_score(object.as_bytes(), node.as_bytes(), vnode);
            if best_node.is_none() || score > best_score {
                best_score = score;
                best_node = Some(*node);
            }
        }
    }
    best_node
}

/// Compute the home node for an actor.  Actors use the same rendezvous space
/// as objects so the directory is sharded without a central index.
pub fn actor_home(actor: &ActorId, view: &MembershipView) -> Option<NodeId> {
    if view.nodes.is_empty() {
        return None;
    }
    let mut best_score = 0u64;
    let mut best_node = None;
    for node in &view.nodes {
        for vnode in 0..VIRTUAL_NODES {
            let score = rendezvous_score(&actor.0, node.as_bytes(), vnode);
            if best_node.is_none() || score > best_score {
                best_score = score;
                best_node = Some(*node);
            }
        }
    }
    best_node
}

/// A route to an object's current home.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ObjectRoute {
    /// Object identity.
    pub object: ObjectId,
    /// Current home node.
    pub home: NodeId,
    /// Home assignment epoch.
    pub home_epoch: u32,
    /// Object revocation epoch.
    pub object_epoch: u32,
    /// Lease expiry in Unix nanoseconds (0 = no lease).
    pub lease_until: u64,
}

impl ObjectRoute {
    /// Whether the cached route has the same home and epochs as `current`.
    pub fn matches(&self, current: &ObjectRoute) -> bool {
        self.object == current.object
            && self.home == current.home
            && self.home_epoch == current.home_epoch
            && self.object_epoch >= current.object_epoch
    }

    /// Whether the route's lease is valid at `now_ns`.
    pub fn lease_valid(&self, now_ns: u64) -> bool {
        self.lease_until == 0 || now_ns <= self.lease_until
    }
}

/// A bounded route cache.  It is not authoritative; the home node is.
#[derive(Debug)]
pub struct RouteCache {
    map: BTreeMap<[u8; 32], ObjectRoute>,
    capacity: usize,
}

impl RouteCache {
    /// Create a cache with a hard capacity.
    pub fn new(capacity: usize) -> Self {
        Self {
            map: BTreeMap::new(),
            capacity,
        }
    }

    /// Look up a route.
    pub fn get(&self, object: &ObjectId) -> Option<&ObjectRoute> {
        self.map.get(object.as_bytes())
    }

    /// Insert or replace a route.  Evicts the lowest key when full.
    pub fn insert(&mut self, route: ObjectRoute) {
        if self.capacity == 0 {
            return;
        }
        if self.map.len() >= self.capacity && !self.map.contains_key(route.object.as_bytes()) {
            if let Some(first) = self.map.keys().next().copied() {
                self.map.remove(&first);
            }
        }
        self.map.insert(*route.object.as_bytes(), route);
    }

    /// Remove a route (for example, after `MOVED` or `STALE`).
    pub fn invalidate(&mut self, object: &ObjectId) {
        self.map.remove(object.as_bytes());
    }

    /// Number of cached routes.
    pub fn len(&self) -> usize {
        self.map.len()
    }

    /// Whether the cache is empty.
    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }
}

/// A lease granted by a home node.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Lease {
    /// Node holding the lease.
    pub owner: NodeId,
    /// Epoch of the lease (fencing token).
    pub epoch: u32,
    /// Expiry in Unix nanoseconds.
    pub expires_ns: u64,
}

impl Lease {
    /// Whether the lease is valid at `now_ns`.
    pub fn is_valid(&self, now_ns: u64) -> bool {
        now_ns <= self.expires_ns
    }

    /// Whether `other` fences this lease.
    pub fn fenced_by(&self, other: &Lease) -> bool {
        other.epoch > self.epoch
    }
}

/// A per-node load summary.  It is an aggregate, not a global run queue.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NodeSummary {
    /// Node identity.
    pub node: NodeId,
    /// CPU utilisation in per-mille (0–1000).
    pub cpu_permille: u16,
    /// Free memory in MiB.
    pub mem_free_mib: u32,
    /// Runnable actors on the node.
    pub runnable: u16,
    /// Whether the node has a usable GPU.
    pub gpu: bool,
    /// Summary epoch.
    pub epoch: u32,
}

/// A placement view: the bounded, eventually consistent aggregate of node
/// summaries.  It is not a scheduler and it owns no run queue.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlacementView {
    /// Summaries, at most [`MAX_NODES`].
    pub summaries: Vec<NodeSummary>,
    /// View epoch.
    pub epoch: u32,
}

impl PlacementView {
    /// Create a view, rejecting more than [`MAX_NODES`] summaries.
    pub fn new(epoch: u32, summaries: Vec<NodeSummary>) -> Option<Self> {
        if summaries.len() > MAX_NODES {
            return None;
        }
        Some(Self { summaries, epoch })
    }

    /// Choose a host for `actor`.
    ///
    /// The score prefers low CPU, low runnable count, enough free memory and
    /// (when required) a GPU.  Ties are broken by a rendezvous score so the
    /// decision is deterministic and spread across nodes.
    pub fn choose_host(
        &self,
        actor: &ActorId,
        require_gpu: bool,
        min_mem_mib: u32,
    ) -> Option<NodeId> {
        let mut best: Option<(u32, u64, NodeId)> = None;
        for summary in &self.summaries {
            if require_gpu && !summary.gpu {
                continue;
            }
            if summary.mem_free_mib < min_mem_mib {
                continue;
            }
            let mem_pressure = 4096u32.saturating_sub(summary.mem_free_mib) / 64;
            let load = summary.cpu_permille as u32 + summary.runnable as u32 * 8 + mem_pressure;
            let tie = rendezvous_score(&actor.0, summary.node.as_bytes(), 0);
            let replace = match best {
                None => true,
                Some((best_load, best_tie, _)) => {
                    load < best_load || (load == best_load && tie > best_tie)
                }
            };
            if replace {
                best = Some((load, tie, summary.node));
            }
        }
        best.map(|(_, _, node)| node)
    }
}

/// A deterministic 64-bit score for the host-checked sketch.  Production uses
/// BLAKE3-64 with a domain separator; see the module docs.
fn rendezvous_score(identity: &[u8], node: &[u8; 16], vnode: u32) -> u64 {
    let mut h = 0xcbf2_9ce4_8422_2325u64;
    for byte in b"I7-HOME-v1"
        .iter()
        .chain(identity.iter())
        .chain(node.iter())
    {
        h ^= *byte as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h ^= vnode as u64;
    h = h.wrapping_mul(0x9e37_79b9_7f4a_7c15);
    h ^= h >> 30;
    h = h.wrapping_mul(0xbf58_476d_1ce4_e5b9);
    h ^= h >> 27;
    h = h.wrapping_mul(0x94d0_49bb_1331_11eb);
    h ^ (h >> 31)
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    fn node(n: u8) -> NodeId {
        NodeId([n; 16])
    }

    #[test]
    fn single_node_is_home_for_everything() {
        let view = MembershipView::single(node(1));
        assert!(view.is_single());
        for i in 0..1000u32 {
            let oid = ObjectId([(i & 0xff) as u8; 32]);
            assert_eq!(home_for(&oid, &view), Some(node(1)));
        }
    }

    #[test]
    fn rendezvous_spreads_across_128_nodes() {
        let nodes: Vec<NodeId> = (0..128u8).map(node).collect();
        let view = MembershipView::new(1, nodes).expect("view");
        let mut counts = [0u32; 128];
        for i in 0..1_024u32 {
            let mut oid = [0u8; 32];
            oid[0..4].copy_from_slice(&i.to_le_bytes());
            oid[4] = 0xa5;
            let home = home_for(&ObjectId(oid), &view).expect("home");
            counts[home.0[0] as usize] += 1;
        }
        let average = 1_024 / 128;
        let mut min = u32::MAX;
        let mut max = 0;
        for count in counts {
            min = min.min(count);
            max = max.max(count);
        }
        assert!(min > 0, "every node should own something");
        assert!(max < average * 3, "distribution should be bounded");
    }

    #[test]
    fn route_cache_is_bounded() {
        let mut cache = RouteCache::new(2);
        for i in 0..4u8 {
            cache.insert(ObjectRoute {
                object: ObjectId([i; 32]),
                home: node(i),
                home_epoch: 1,
                object_epoch: 1,
                lease_until: 0,
            });
        }
        assert_eq!(cache.len(), 2);
    }

    #[test]
    fn lease_expiry_and_fencing() {
        let lease = Lease {
            owner: node(1),
            epoch: 4,
            expires_ns: 100,
        };
        assert!(lease.is_valid(100));
        assert!(!lease.is_valid(101));
        let newer = Lease {
            owner: node(2),
            epoch: 5,
            expires_ns: 200,
        };
        assert!(lease.fenced_by(&newer));
    }

    #[test]
    fn placement_prefers_gpu_when_required() {
        let view = PlacementView::new(
            1,
            vec![
                NodeSummary {
                    node: node(1),
                    cpu_permille: 100,
                    mem_free_mib: 4096,
                    runnable: 0,
                    gpu: false,
                    epoch: 1,
                },
                NodeSummary {
                    node: node(2),
                    cpu_permille: 900,
                    mem_free_mib: 4096,
                    runnable: 8,
                    gpu: true,
                    epoch: 1,
                },
            ],
        )
        .expect("view");
        assert_eq!(view.choose_host(&ActorId([7; 16]), true, 64), Some(node(2)));
        assert_eq!(
            view.choose_host(&ActorId([7; 16]), false, 64),
            Some(node(1))
        );
    }
}
