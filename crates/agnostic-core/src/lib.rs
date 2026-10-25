// SPDX-License-Identifier: GPL-3.0-only
//! Architecture-independent actor state.
//!
//! The native shim owns instruction contexts; this crate owns the portable
//! actor identity, home, host, lifecycle and placement epoch. The same state
//! runs in the host tests and, later, inside the Wasm core.

#![no_std]
#![forbid(unsafe_code)]
#![deny(missing_docs)]

extern crate alloc;

use alloc::collections::{BTreeMap, BTreeSet, VecDeque};
use core::fmt;
use mesh_core::{actor_home, MembershipView};
use sys_primitives::{ActorId, NodeId};

/// Lifecycle state of an actor.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ActorState {
    /// Created but not yet running.
    Created,
    /// Runnable or running on its host.
    Running,
    /// Waiting for a completion, timer or capability.
    Blocked,
    /// Exited; the record may be retained for a wait.
    Dead,
}

/// One actor record.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Actor {
    /// Stable actor identity.
    pub id: ActorId,
    /// Rendezvous home and placement authority.
    pub home: NodeId,
    /// Current execution host.
    pub host: NodeId,
    /// Lifecycle state.
    pub state: ActorState,
    /// Placement generation.
    pub epoch: u32,
}

/// Actor-table failure.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ActorError {
    /// The actor already exists.
    Duplicate,
    /// The actor is absent.
    Missing,
    /// The requested node is not in the membership view.
    UnknownNode,
    /// The actor has exited.
    Dead,
}

impl fmt::Display for ActorError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Duplicate => f.write_str("actor already exists"),
            Self::Missing => f.write_str("actor not found"),
            Self::UnknownNode => f.write_str("node not in membership"),
            Self::Dead => f.write_str("actor is dead"),
        }
    }
}

/// Portable actor directory for a node.
#[derive(Debug)]
pub struct ActorTable {
    membership: MembershipView,
    actors: BTreeMap<ActorId, Actor>,
}

impl ActorTable {
    /// Create an empty table over a membership view.
    pub const fn new(membership: MembershipView) -> Self {
        Self {
            membership,
            actors: BTreeMap::new(),
        }
    }

    /// Current membership view.
    pub const fn membership(&self) -> &MembershipView {
        &self.membership
    }

    /// Look up an actor.
    pub fn get(&self, id: ActorId) -> Option<&Actor> {
        self.actors.get(&id)
    }

    /// Create an actor at its rendezvous home.
    pub fn spawn(&mut self, id: ActorId) -> Result<Actor, ActorError> {
        if self.actors.contains_key(&id) {
            return Err(ActorError::Duplicate);
        }
        let home = actor_home(&id, &self.membership).ok_or(ActorError::UnknownNode)?;
        let actor = Actor {
            id,
            home,
            host: home,
            state: ActorState::Created,
            epoch: 0,
        };
        self.actors.insert(id, actor);
        Ok(actor)
    }

    /// Set the actor lifecycle state.
    pub fn set_state(&mut self, id: ActorId, state: ActorState) -> Result<(), ActorError> {
        let actor = self.actors.get_mut(&id).ok_or(ActorError::Missing)?;
        if actor.state == ActorState::Dead && state != ActorState::Dead {
            return Err(ActorError::Dead);
        }
        actor.state = state;
        Ok(())
    }

    /// Place or migrate an actor onto `host`.
    ///
    /// The home is fixed by the membership view; the host is a placement
    /// decision. The epoch fences stale migration messages.
    pub fn place(&mut self, id: ActorId, host: NodeId) -> Result<u32, ActorError> {
        if !self.membership.contains(host) {
            return Err(ActorError::UnknownNode);
        }
        let actor = self.actors.get_mut(&id).ok_or(ActorError::Missing)?;
        if actor.state == ActorState::Dead {
            return Err(ActorError::Dead);
        }
        actor.epoch = actor.epoch.saturating_add(1);
        actor.host = host;
        actor.state = ActorState::Running;
        Ok(actor.epoch)
    }

    /// Remove a dead actor and return its final record.
    pub fn reap(&mut self, id: ActorId) -> Result<Actor, ActorError> {
        let actor = self.actors.get(&id).ok_or(ActorError::Missing)?;
        if actor.state != ActorState::Dead {
            return Err(ActorError::Dead);
        }
        Ok(self.actors.remove(&id).expect("checked above"))
    }

    /// Iterate over the retained actor identifiers.
    pub fn ids(&self) -> impl Iterator<Item = ActorId> + '_ {
        self.actors.keys().copied()
    }

    /// Number of retained actors.
    pub fn len(&self) -> usize {
        self.actors.len()
    }

    /// Whether the table is empty.
    pub fn is_empty(&self) -> bool {
        self.actors.is_empty()
    }
}

/// Scheduling class recorded for a queued actor.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum QueueClass {
    /// Weighted fair queue at a priority (0 is highest).
    Fair(u8),
    /// Earliest-deadline queue with a monotonic tie-breaker.
    Deadline {
        /// Absolute deadline in nanoseconds.
        deadline: u64,
        /// Tie-breaker for equal deadlines.
        sequence: u64,
    },
}

/// Run-queue failure.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum QueueError {
    /// The actor is already queued.
    Duplicate,
    /// The actor is not queued.
    Missing,
}

/// Per-node run queue: EDF first, then weighted fair priorities.
#[derive(Debug, Default)]
pub struct RunQueue {
    fair: BTreeMap<u8, VecDeque<ActorId>>,
    deadline: BTreeSet<(u64, u64, ActorId)>,
    classes: BTreeMap<ActorId, QueueClass>,
}

impl RunQueue {
    /// Create an empty queue.
    pub const fn new() -> Self {
        Self {
            fair: BTreeMap::new(),
            deadline: BTreeSet::new(),
            classes: BTreeMap::new(),
        }
    }

    /// Queue a fair actor. Priority is clamped to 0..=31.
    pub fn push_fair(&mut self, actor: ActorId, priority: u8) -> Result<(), QueueError> {
        if self.classes.contains_key(&actor) {
            return Err(QueueError::Duplicate);
        }
        let priority = priority.min(31);
        self.fair.entry(priority).or_default().push_back(actor);
        self.classes.insert(actor, QueueClass::Fair(priority));
        Ok(())
    }

    /// Queue a deadline actor.
    pub fn push_deadline(
        &mut self,
        actor: ActorId,
        deadline: u64,
        sequence: u64,
    ) -> Result<(), QueueError> {
        if self.classes.contains_key(&actor) {
            return Err(QueueError::Duplicate);
        }
        self.deadline.insert((deadline, sequence, actor));
        self.classes
            .insert(actor, QueueClass::Deadline { deadline, sequence });
        Ok(())
    }

    /// Pick the next runnable actor: the earliest deadline, else the highest
    /// fair priority.
    pub fn pick(&mut self) -> Option<ActorId> {
        if let Some(&(deadline, sequence, actor)) = self.deadline.iter().next() {
            self.deadline.remove(&(deadline, sequence, actor));
            self.classes.remove(&actor);
            return Some(actor);
        }
        let priority = *self.fair.keys().next()?;
        let queue = self.fair.get_mut(&priority)?;
        let actor = queue.pop_front()?;
        if queue.is_empty() {
            self.fair.remove(&priority);
        }
        self.classes.remove(&actor);
        Some(actor)
    }

    /// Remove an actor from the queue.
    pub fn remove(&mut self, actor: ActorId) -> Result<(), QueueError> {
        let class = self.classes.remove(&actor).ok_or(QueueError::Missing)?;
        match class {
            QueueClass::Fair(priority) => {
                if let Some(queue) = self.fair.get_mut(&priority) {
                    if let Some(index) = queue.iter().position(|queued| *queued == actor) {
                        queue.remove(index);
                    }
                    if queue.is_empty() {
                        self.fair.remove(&priority);
                    }
                }
            }
            QueueClass::Deadline { deadline, sequence } => {
                self.deadline.remove(&(deadline, sequence, actor));
            }
        }
        Ok(())
    }

    /// Whether the actor is currently queued.
    pub fn contains(&self, actor: &ActorId) -> bool {
        self.classes.contains_key(actor)
    }

    /// Number of queued actors.
    pub fn len(&self) -> usize {
        self.classes.len()
    }

    /// Whether the queue is empty.
    pub fn is_empty(&self) -> bool {
        self.classes.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec::Vec;

    fn node(n: u8) -> NodeId {
        NodeId([n; 16])
    }

    fn membership(nodes: &[u8]) -> MembershipView {
        MembershipView::new(1, nodes.iter().copied().map(node).collect::<Vec<_>>()).expect("view")
    }

    #[test]
    fn single_node_actor_stays_local() {
        let mut table = ActorTable::new(MembershipView::single(node(1)));
        let actor = table.spawn(ActorId([7; 16])).expect("spawn");
        assert_eq!(actor.home, node(1));
        assert_eq!(actor.host, node(1));
        assert_eq!(table.set_state(actor.id, ActorState::Running), Ok(()));
        assert_eq!(table.place(actor.id, node(1)), Ok(1));
    }

    #[test]
    fn placement_moves_the_host_not_the_home() {
        let mut table = ActorTable::new(membership(&[1, 2, 3]));
        let actor = table.spawn(ActorId([8; 16])).expect("spawn");
        let epoch = table.place(actor.id, node(3)).expect("place");
        assert_eq!(epoch, 1);
        let moved = table.get(actor.id).expect("actor");
        assert_eq!(moved.home, actor.home);
        assert_eq!(moved.host, node(3));
        assert_eq!(moved.state, ActorState::Running);
    }

    #[test]
    fn placement_rejects_unknown_or_dead_actors() {
        let mut table = ActorTable::new(membership(&[1, 2]));
        let actor = table.spawn(ActorId([9; 16])).expect("spawn");
        assert_eq!(table.place(actor.id, node(9)), Err(ActorError::UnknownNode));
        assert_eq!(table.set_state(actor.id, ActorState::Dead), Ok(()));
        assert_eq!(table.place(actor.id, node(2)), Err(ActorError::Dead));
        assert_eq!(table.reap(actor.id).map(|a| a.state), Ok(ActorState::Dead));
    }

    #[test]
    fn duplicate_actor_is_rejected() {
        let mut table = ActorTable::new(MembershipView::single(node(1)));
        let id = ActorId([4; 16]);
        assert!(table.spawn(id).is_ok());
        assert_eq!(table.spawn(id), Err(ActorError::Duplicate));
    }
    #[test]
    fn deadline_beats_fair_and_is_ordered() {
        let mut queue = RunQueue::new();
        queue.push_fair(ActorId([1; 16]), 0).expect("fair");
        queue
            .push_deadline(ActorId([2; 16]), 20, 1)
            .expect("deadline 20");
        queue
            .push_deadline(ActorId([3; 16]), 10, 2)
            .expect("deadline 10");
        assert_eq!(queue.pick(), Some(ActorId([3; 16])));
        assert_eq!(queue.pick(), Some(ActorId([2; 16])));
        assert_eq!(queue.pick(), Some(ActorId([1; 16])));
        assert!(queue.is_empty());
    }

    #[test]
    fn fair_priority_and_remove() {
        let mut queue = RunQueue::new();
        queue.push_fair(ActorId([1; 16]), 5).expect("fair 5");
        queue.push_fair(ActorId([2; 16]), 1).expect("fair 1");
        assert_eq!(queue.pick(), Some(ActorId([2; 16])));
        queue.push_fair(ActorId([3; 16]), 2).expect("fair 2");
        queue.remove(ActorId([3; 16])).expect("remove");
        assert_eq!(queue.pick(), Some(ActorId([1; 16])));
        assert!(queue.is_empty());
    }

    #[test]
    fn duplicate_and_missing_fail() {
        let mut queue = RunQueue::new();
        let actor = ActorId([7; 16]);
        queue.push_fair(actor, 0).expect("push");
        assert_eq!(queue.push_fair(actor, 0), Err(QueueError::Duplicate));
        assert_eq!(queue.remove(ActorId([8; 16])), Err(QueueError::Missing));
    }
}
