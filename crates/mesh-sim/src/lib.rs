// SPDX-License-Identifier: GPL-3.0-only
//! Host simulation of the IRIX 7 logical computer.
//!
//! This is deliberately policy-level: it uses the production membership and
//! rendezvous crates, but no sockets, gossip, authority signatures or Wasm
//! runtime. It is a deterministic gate for the 1/2/8/128-node shape.

#![forbid(unsafe_code)]
#![deny(missing_docs)]

use std::collections::BTreeSet;

use mesh_core::{actor_home, home_for, MAX_NODES};
use mesh_service::MembershipTable;
use sys_primitives::{ActorId, NodeId, ObjectId};

/// Result of one simulation run.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Stats {
    /// Number of nodes in the membership view.
    pub nodes: usize,
    /// Objects placed.
    pub objects: u32,
    /// Actors placed.
    pub actors: u32,
    /// Smallest per-node object count.
    pub min_objects: u32,
    /// Largest per-node object count.
    pub max_objects: u32,
    /// Number of nodes that own at least one object.
    pub object_nodes: usize,
    /// Membership epoch after joins.
    pub epoch: u32,
}

/// Simulation failure.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SimError {
    /// The requested node count is outside `1..=128`.
    NodeCount,
    /// Membership rejected a node.
    Membership(mesh_service::MembershipError),
}

impl From<mesh_service::MembershipError> for SimError {
    fn from(value: mesh_service::MembershipError) -> Self {
        Self::Membership(value)
    }
}

fn node(index: u8) -> NodeId {
    let mut bytes = [0u8; 16];
    bytes[0] = index;
    NodeId(bytes)
}

fn object(index: u32) -> ObjectId {
    let mut bytes = [0u8; 32];
    bytes[0..4].copy_from_slice(&index.to_le_bytes());
    bytes[4] = 0xa5;
    ObjectId(bytes)
}

fn actor(index: u32) -> ActorId {
    let mut bytes = [0u8; 16];
    bytes[0..4].copy_from_slice(&index.to_le_bytes());
    bytes[4] = 0x5a;
    ActorId(bytes)
}

fn index_of(id: NodeId) -> usize {
    id.0[0] as usize
}

/// Run a deterministic mesh placement simulation.
pub fn run(nodes: u8, objects: u32, actors: u32) -> Result<Stats, SimError> {
    if nodes == 0 || nodes as usize > MAX_NODES {
        return Err(SimError::NodeCount);
    }
    let mut membership = MembershipTable::new(0);
    for i in 0..nodes {
        membership.join(node(i), 0)?;
    }
    let view = membership.snapshot();
    let count = nodes as usize;
    let mut objects_per_node = vec![0u32; count];
    let mut actor_homes = BTreeSet::new();

    for i in 0..objects {
        let home = home_for(&object(i), &view).expect("non-empty view");
        objects_per_node[index_of(home)] += 1;
    }
    for i in 0..actors {
        let home = actor_home(&actor(i), &view).expect("non-empty view");
        actor_homes.insert(home);
    }

    let min_objects = *objects_per_node.iter().min().unwrap_or(&0);
    let max_objects = *objects_per_node.iter().max().unwrap_or(&0);
    let object_nodes = objects_per_node.iter().filter(|&&n| n > 0).count();
    Ok(Stats {
        nodes: count,
        objects,
        actors,
        min_objects,
        max_objects,
        object_nodes,
        epoch: membership.epoch(),
    })
}

/// Show that failed nodes leave the routing view and are never selected.
pub fn simulate_failure(nodes: u8, failed: u8, objects: u32) -> Result<usize, SimError> {
    if nodes == 0 || nodes as usize > MAX_NODES || failed > nodes {
        return Err(SimError::NodeCount);
    }
    let mut membership = MembershipTable::new(0);
    for i in 0..nodes {
        membership.join(node(i), 0)?;
    }
    for i in nodes - failed..nodes {
        membership.fail(node(i))?;
    }
    let view = membership.snapshot();
    if view.len() != (nodes - failed) as usize {
        return Err(SimError::NodeCount);
    }
    let mut seen = BTreeSet::new();
    for i in 0..objects {
        let home = home_for(&object(i), &view).expect("non-empty view");
        if home.0[0] >= nodes - failed {
            return Err(SimError::NodeCount);
        }
        seen.insert(home);
    }
    Ok(seen.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn single_node_is_local() {
        let stats = run(1, 1_000, 100).expect("single node");
        assert_eq!(stats.nodes, 1);
        assert_eq!(stats.object_nodes, 1);
        assert_eq!(stats.min_objects, 1_000);
        assert_eq!(stats.max_objects, 1_000);
    }

    #[test]
    fn two_and_eight_nodes_share_the_namespace() {
        for nodes in [2, 8] {
            let stats = run(nodes, 4_096, 256).expect("mesh");
            assert_eq!(stats.nodes, nodes as usize);
            assert_eq!(stats.object_nodes, nodes as usize);
            assert!(stats.min_objects > 0);
            assert!(stats.max_objects < (4_096 / nodes as u32) * 3);
        }
    }

    #[test]
    fn one_hundred_and_eight_nodes_are_bounded() {
        let stats = run(128, 4_096, 512).expect("128 nodes");
        assert_eq!(stats.nodes, 128);
        assert_eq!(stats.object_nodes, 128);
        assert!(stats.min_objects > 0);
        assert!(stats.max_objects < (4_096 / 128) * 3);
    }

    #[test]
    fn failed_nodes_are_not_routable() {
        let homes = simulate_failure(128, 8, 4_096).expect("failure");
        assert!(homes > 0);
    }
}
