// SPDX-License-Identifier: GPL-3.0-only
//! Fabric policy: tags, streams and bounded completion handling.
//!
//! This crate is deliberately independent of the transport. It can run on the
//! host, inside the Wasm core and in the native shim's test harness. The final
//! hot-path completion queue is an SPSC ring; [`CompletionBuffer`] is a safe,
//! allocation-backed reference model used to prove ordering and backpressure
//! before the lock-free ring is introduced.

#![no_std]
#![forbid(unsafe_code)]
#![deny(missing_docs)]

extern crate alloc;

use alloc::collections::{BTreeMap, BTreeSet, VecDeque};
use core::fmt;
use sys_primitives::{Completion, StreamId, Tag};

/// Default number of in-flight request tags.
pub const DEFAULT_TAG_CAPACITY: u32 = 4096;

/// Failure to allocate a request tag.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TagError {
    /// Every tag in the configured capacity is in use.
    Exhausted,
}

impl fmt::Display for TagError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Exhausted => f.write_str("request tags exhausted"),
        }
    }
}

/// Allocates 32-bit request tags and detects double frees.
///
/// Tag zero is reserved for "no request" by the frame contract, so allocation
/// starts at one. The allocator is policy only; the transport owns the
/// per-stream tables and the completion matching.
#[derive(Debug)]
pub struct TagAllocator {
    capacity: u32,
    in_use: BTreeSet<u32>,
}

impl TagAllocator {
    /// Create an allocator with `capacity` usable tags.
    ///
    /// A zero capacity is treated as one tag so a caller cannot accidentally
    /// create an allocator that can never make progress.
    pub fn new(capacity: u32) -> Self {
        Self {
            capacity: capacity.max(1),
            in_use: BTreeSet::new(),
        }
    }

    /// Allocate one tag, or report exhaustion.
    pub fn alloc(&mut self) -> Result<Tag, TagError> {
        if self.in_use.len() as u32 >= self.capacity {
            return Err(TagError::Exhausted);
        }
        for candidate in 1..=self.capacity {
            if self.in_use.insert(candidate) {
                return Ok(Tag(candidate));
            }
        }
        Err(TagError::Exhausted)
    }

    /// Release a tag. Returns false if it was not allocated.
    pub fn free(&mut self, tag: Tag) -> bool {
        self.in_use.remove(&tag.0)
    }

    /// Whether the tag is currently allocated.
    pub fn is_allocated(&self, tag: Tag) -> bool {
        self.in_use.contains(&tag.0)
    }

    /// Number of allocated tags.
    pub fn in_flight(&self) -> usize {
        self.in_use.len()
    }

    /// Configured capacity.
    pub const fn capacity(&self) -> u32 {
        self.capacity
    }
}

/// State of one multiplexed stream.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StreamState {
    /// Nothing is open; the id may be reused after a full close.
    Idle,
    /// Both directions are open.
    Open,
    /// The local side has sent its final frame.
    HalfClosedLocal,
    /// The remote side has sent its final frame.
    HalfClosedRemote,
    /// Both directions are closed; the id is ready for reuse.
    Closed,
}

/// One stream's flow-control and lifecycle state.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Stream {
    /// Stream identity.
    pub id: StreamId,
    /// Current lifecycle state.
    pub state: StreamState,
    /// Bytes the local side may still send.
    pub send_window: u32,
    /// Bytes the local side may receive before advertising more.
    pub receive_window: u32,
}

impl Stream {
    /// Create an open stream with symmetric initial windows.
    pub const fn new(id: StreamId, window: u32) -> Self {
        Self {
            id,
            state: StreamState::Open,
            send_window: window,
            receive_window: window,
        }
    }

    /// Close the local direction.
    pub fn close_local(&mut self) {
        self.state = match self.state {
            StreamState::Open => StreamState::HalfClosedLocal,
            StreamState::HalfClosedRemote => StreamState::Closed,
            other => other,
        };
    }

    /// Close the remote direction.
    pub fn close_remote(&mut self) {
        self.state = match self.state {
            StreamState::Open => StreamState::HalfClosedRemote,
            StreamState::HalfClosedLocal => StreamState::Closed,
            other => other,
        };
    }

    /// Consume send credit. Returns false if the window would go negative.
    pub fn consume_send(&mut self, bytes: u32) -> bool {
        if bytes > self.send_window {
            return false;
        }
        self.send_window -= bytes;
        true
    }

    /// Add send credit from a window update.
    pub fn grant_send(&mut self, bytes: u32) {
        self.send_window = self.send_window.saturating_add(bytes);
    }

    /// Consume receive credit. Returns false if the window would go negative.
    pub fn consume_receive(&mut self, bytes: u32) -> bool {
        if bytes > self.receive_window {
            return false;
        }
        self.receive_window -= bytes;
        true
    }

    /// Add receive credit after the consumer has drained bytes.
    pub fn grant_receive(&mut self, bytes: u32) {
        self.receive_window = self.receive_window.saturating_add(bytes);
    }
}

/// A bounded collection of streams, keyed by stream id.
#[derive(Debug, Default)]
pub struct StreamTable {
    streams: BTreeMap<u32, Stream>,
}

impl StreamTable {
    /// Create an empty table.
    pub const fn new() -> Self {
        Self {
            streams: BTreeMap::new(),
        }
    }

    /// Open a stream. Returns false if the id is already present.
    pub fn open(&mut self, id: StreamId, window: u32) -> bool {
        if self.streams.contains_key(&id.0) {
            return false;
        }
        self.streams.insert(id.0, Stream::new(id, window));
        true
    }

    /// Look up a stream.
    pub fn get(&self, id: StreamId) -> Option<&Stream> {
        self.streams.get(&id.0)
    }

    /// Look up a mutable stream.
    pub fn get_mut(&mut self, id: StreamId) -> Option<&mut Stream> {
        self.streams.get_mut(&id.0)
    }

    /// Remove a stream. Returns whether it existed.
    pub fn remove(&mut self, id: StreamId) -> bool {
        self.streams.remove(&id.0).is_some()
    }

    /// Number of live streams.
    pub fn len(&self) -> usize {
        self.streams.len()
    }

    /// Whether the table is empty.
    pub fn is_empty(&self) -> bool {
        self.streams.is_empty()
    }

    /// Iterate over streams in id order.
    pub fn iter(&self) -> impl Iterator<Item = &Stream> {
        self.streams.values()
    }
}

/// A bounded FIFO for completion records.
///
/// This is the safe reference model for the SPSC completion ring used on the
/// hot path. It never grows beyond `capacity`.
#[derive(Debug)]
pub struct CompletionBuffer {
    queue: VecDeque<Completion>,
    capacity: usize,
}

impl CompletionBuffer {
    /// Create a buffer with a fixed capacity.
    pub fn new(capacity: usize) -> Self {
        Self {
            queue: VecDeque::with_capacity(capacity),
            capacity,
        }
    }

    /// Push one completion. Returns false when the buffer is full.
    pub fn push(&mut self, completion: Completion) -> bool {
        if self.queue.len() >= self.capacity {
            return false;
        }
        self.queue.push_back(completion);
        true
    }

    /// Pop the oldest completion.
    pub fn pop(&mut self) -> Option<Completion> {
        self.queue.pop_front()
    }

    /// Number of pending completions.
    pub fn len(&self) -> usize {
        self.queue.len()
    }

    /// Whether the buffer is empty.
    pub fn is_empty(&self) -> bool {
        self.queue.is_empty()
    }

    /// Configured capacity.
    pub const fn capacity(&self) -> usize {
        self.capacity
    }
}

/// A bounded set of cancelled request tags.
#[derive(Debug, Default)]
pub struct CancelSet {
    tags: BTreeSet<u32>,
}

impl CancelSet {
    /// Create an empty set.
    pub const fn new() -> Self {
        Self {
            tags: BTreeSet::new(),
        }
    }

    /// Record a cancellation. Returns false if it was already present.
    pub fn cancel(&mut self, tag: Tag) -> bool {
        self.tags.insert(tag.0)
    }

    /// Whether a tag was cancelled.
    pub fn is_cancelled(&self, tag: Tag) -> bool {
        self.tags.contains(&tag.0)
    }

    /// Remove a cancellation after its completion is delivered.
    pub fn clear(&mut self, tag: Tag) -> bool {
        self.tags.remove(&tag.0)
    }

    /// Number of outstanding cancellations.
    pub fn len(&self) -> usize {
        self.tags.len()
    }

    /// Whether no cancellation is outstanding.
    pub fn is_empty(&self) -> bool {
        self.tags.is_empty()
    }
}

/// Failure while linking a wait dependency.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GraphError {
    /// A request cannot depend on itself.
    SelfLink,
    /// The child already has a parent.
    Duplicate,
    /// The dependency chain would exceed the protocol depth.
    TooDeep,
    /// The link would create a cycle.
    Cycle,
}

/// Parent/child completion dependencies, bounded to keep failure propagation
/// finite.
#[derive(Debug, Default)]
pub struct WaitGraph {
    parents: BTreeMap<u32, u32>,
    children: BTreeMap<u32, BTreeSet<u32>>,
}

impl WaitGraph {
    /// Maximum ancestor or descendant depth.
    pub const MAX_DEPTH: usize = 8;

    /// Create an empty graph.
    pub const fn new() -> Self {
        Self {
            parents: BTreeMap::new(),
            children: BTreeMap::new(),
        }
    }

    /// Link a parent to a child. The child may have only one parent.
    pub fn link(&mut self, parent: Tag, child: Tag) -> Result<(), GraphError> {
        if parent == child {
            return Err(GraphError::SelfLink);
        }
        if self.parents.contains_key(&child.0) {
            return Err(GraphError::Duplicate);
        }
        let mut cursor = parent.0;
        let mut depth = 0usize;
        loop {
            if cursor == child.0 {
                return Err(GraphError::Cycle);
            }
            let Some(&next) = self.parents.get(&cursor) else {
                break;
            };
            cursor = next;
            depth += 1;
            if depth > Self::MAX_DEPTH {
                return Err(GraphError::TooDeep);
            }
        }
        if depth + 1 + self.descendant_depth(child) > Self::MAX_DEPTH {
            return Err(GraphError::TooDeep);
        }
        self.parents.insert(child.0, parent.0);
        self.children.entry(parent.0).or_default().insert(child.0);
        Ok(())
    }

    fn descendant_depth(&self, tag: Tag) -> usize {
        let mut best = 0usize;
        let mut stack = alloc::vec![(tag.0, 0usize)];
        while let Some((current, depth)) = stack.pop() {
            best = best.max(depth);
            if let Some(children) = self.children.get(&current) {
                for child in children {
                    stack.push((*child, depth + 1));
                }
            }
        }
        best
    }

    /// Direct children of a tag.
    pub fn children(&self, parent: Tag) -> impl Iterator<Item = Tag> + '_ {
        self.children
            .get(&parent.0)
            .into_iter()
            .flat_map(|set| set.iter().copied().map(Tag))
    }

    /// Parent of a tag, if linked.
    pub fn parent(&self, child: Tag) -> Option<Tag> {
        self.parents.get(&child.0).copied().map(Tag)
    }

    /// Remove a child link after completion.
    pub fn unlink(&mut self, child: Tag) -> bool {
        let Some(parent) = self.parents.remove(&child.0) else {
            return false;
        };
        if let Some(set) = self.children.get_mut(&parent) {
            set.remove(&child.0);
            if set.is_empty() {
                self.children.remove(&parent);
            }
        }
        true
    }

    /// Number of linked children.
    pub fn len(&self) -> usize {
        self.parents.len()
    }

    /// Whether no links exist.
    pub fn is_empty(&self) -> bool {
        self.parents.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;
    use alloc::vec::Vec;
    use sys_primitives::{CompletionId, CompletionKind, Status};

    fn completion(tag: u32) -> Completion {
        Completion {
            event: CompletionId(tag as u64),
            tag: Tag(tag),
            status: Status::Ok,
            kind: CompletionKind::None,
            flags: 0,
            reserved: 0,
            value0: 0,
            value1: 0,
        }
    }

    #[test]
    fn tags_are_unique_and_reusable() {
        let mut tags = TagAllocator::new(4);
        let a = tags.alloc().expect("a");
        let b = tags.alloc().expect("b");
        assert_ne!(a, b);
        assert!(tags.free(a));
        assert!(!tags.free(a));
        let c = tags.alloc().expect("c");
        assert_eq!(c, a);
        assert_eq!(tags.in_flight(), 2);
    }

    #[test]
    fn tags_report_exhaustion() {
        let mut tags = TagAllocator::new(2);
        assert!(tags.alloc().is_ok());
        assert!(tags.alloc().is_ok());
        assert_eq!(tags.alloc(), Err(TagError::Exhausted));
        assert_eq!(tags.in_flight(), 2);
    }

    #[test]
    fn stream_lifecycle_and_windows() {
        let mut table = StreamTable::new();
        let id = StreamId(7);
        assert!(table.open(id, 16));
        assert!(!table.open(id, 16));
        let stream = table.get_mut(id).expect("stream");
        assert!(stream.consume_send(8));
        assert!(!stream.consume_send(9));
        stream.grant_send(9);
        stream.close_local();
        assert_eq!(stream.state, StreamState::HalfClosedLocal);
        stream.close_remote();
        assert_eq!(stream.state, StreamState::Closed);
        assert!(table.remove(id));
        assert!(table.is_empty());
    }

    #[test]
    fn completion_buffer_is_bounded_and_fifo() {
        let mut buffer = CompletionBuffer::new(2);
        assert!(buffer.push(completion(1)));
        assert!(buffer.push(completion(2)));
        assert!(!buffer.push(completion(3)));
        assert_eq!(buffer.len(), 2);
        assert_eq!(buffer.pop().expect("first").tag, Tag(1));
        assert_eq!(buffer.pop().expect("second").tag, Tag(2));
        assert!(buffer.pop().is_none());
        assert!(buffer.is_empty());
    }
    #[test]
    fn cancel_set_marks_and_clears() {
        let mut cancels = CancelSet::new();
        assert!(cancels.cancel(Tag(7)));
        assert!(!cancels.cancel(Tag(7)));
        assert!(cancels.is_cancelled(Tag(7)));
        assert!(cancels.clear(Tag(7)));
        assert!(cancels.is_empty());
    }

    #[test]
    fn wait_graph_links_and_unlinks() {
        let mut graph = WaitGraph::new();
        graph.link(Tag(1), Tag(2)).expect("link 1->2");
        graph.link(Tag(2), Tag(3)).expect("link 2->3");
        assert_eq!(graph.parent(Tag(3)), Some(Tag(2)));
        assert_eq!(graph.children(Tag(1)).collect::<Vec<_>>(), vec![Tag(2)]);
        assert!(graph.unlink(Tag(2)));
        assert!(graph.parent(Tag(2)).is_none());
    }

    #[test]
    fn wait_graph_rejects_cycles_duplicates_and_depth() {
        let mut graph = WaitGraph::new();
        assert_eq!(graph.link(Tag(1), Tag(1)), Err(GraphError::SelfLink));
        graph.link(Tag(1), Tag(2)).expect("link");
        assert_eq!(graph.link(Tag(3), Tag(2)), Err(GraphError::Duplicate));
        assert_eq!(graph.link(Tag(2), Tag(1)), Err(GraphError::Cycle));
        for i in 0..8u32 {
            graph.link(Tag(100 + i), Tag(101 + i)).expect("chain");
        }
        assert_eq!(graph.link(Tag(200), Tag(100)), Err(GraphError::TooDeep));
    }
}
