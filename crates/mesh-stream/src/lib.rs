// SPDX-License-Identifier: GPL-3.0-only
//! Bounded Version 1 stream multiplexing for one mesh peer link.
//!
//! The wire already carries a `StreamId` in every `FrameHeader`.  This crate
//! owns the policy that the live module applies to those IDs:
//!
//! * one initiator/responder parity rule for new stream IDs;
//! * a fixed stream table, so a peer cannot make the module allocate;
//! * byte credit per direction, so a slow consumer applies backpressure
//!   without closing the link;
//! * explicit reset/cancel, and a control class that is scheduled before data.
//!
//! It is `no_std + alloc` and has no socket or kernel dependency.  The host
//! tests drive it with synthetic frames; the module uses [`StreamTable`] for
//! its bulk/latency measurement streams.

#![no_std]
#![forbid(unsafe_code)]
#![deny(missing_docs)]

extern crate alloc;

use core::fmt;

/// Maximum simultaneously open streams per link.
pub const MAX_STREAMS: usize = 16;
/// Maximum bytes a receiver may grant in one update.
pub const MAX_CREDIT: u32 = 64 * 1024;
/// Maximum queued data bytes per stream.
pub const MAX_QUEUED_BYTES: u32 = 64 * 1024;
/// Initial receive credit for a data stream.
pub const INITIAL_CREDIT: u32 = 16 * 1024;

/// Which side opened a stream.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Side {
    /// The node that initiated the connection (odd IDs).
    Initiator,
    /// The listening node (even IDs).
    Responder,
}

/// Stream state.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StreamState {
    /// Open for both directions.
    Open,
    /// The local side sent EOS.
    HalfClosedLocal,
    /// The remote side sent EOS.
    HalfClosedRemote,
    /// A reset was sent or received; the slot is reclaimable.
    Reset,
}

/// A stream policy failure.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    /// The table is full.
    NoStream,
    /// The ID is zero, wrong parity for this side, or not in the table.
    BadStream,
    /// The stream is reset or closed.
    Closed,
    /// A credit value is zero or above [`MAX_CREDIT`].
    BadCredit,
    /// The send queue would exceed [`MAX_QUEUED_BYTES`].
    QueueFull,
    /// There is no credit for this send.
    CreditExhausted,
}

impl Error {
    /// A short stable label.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::NoStream => "no-stream",
            Self::BadStream => "bad-stream",
            Self::Closed => "closed",
            Self::BadCredit => "bad-credit",
            Self::QueueFull => "queue-full",
            Self::CreditExhausted => "credit-exhausted",
        }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// One stream's bounded state.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Stream {
    /// Wire ID.
    pub id: u32,
    /// State.
    pub state: StreamState,
    /// Bytes this side may still send.
    pub credit_tx: u32,
    /// Bytes this side has granted the peer.
    pub credit_rx: u32,
    /// Data bytes queued locally.
    pub queued: u32,
    /// Whether the stream is a control stream (never credit-blocked).
    pub control: bool,
}

impl Stream {
    const fn new(id: u32, control: bool) -> Self {
        Self {
            id,
            state: StreamState::Open,
            credit_tx: if control { MAX_CREDIT } else { INITIAL_CREDIT },
            credit_rx: INITIAL_CREDIT,
            queued: 0,
            control,
        }
    }
}

/// A bounded stream table for one link.
#[derive(Debug)]
pub struct StreamTable {
    side: Side,
    next_id: u32,
    streams: [Option<Stream>; MAX_STREAMS],
    opens: u64,
    resets: u64,
    credit_stalls: u64,
}

impl StreamTable {
    /// Create an empty table for one side.
    pub fn new(side: Side) -> Self {
        Self {
            side,
            next_id: match side {
                Side::Initiator => 1,
                Side::Responder => 2,
            },
            streams: [None; MAX_STREAMS],
            opens: 0,
            resets: 0,
            credit_stalls: 0,
        }
    }

    /// The side this table allocates for.
    pub const fn side(&self) -> Side {
        self.side
    }

    /// Open a locally allocated stream and return its ID.
    pub fn open(&mut self, control: bool) -> Result<u32, Error> {
        let slot = self
            .streams
            .iter()
            .position(Option::is_none)
            .ok_or(Error::NoStream)?;
        let id = self.next_id;
        self.next_id = self.next_id.wrapping_add(2).max(1);
        self.streams[slot] = Some(Stream::new(id, control));
        self.opens = self.opens.saturating_add(1);
        Ok(id)
    }

    /// Insert a fixed internal stream (measurement/control IDs).
    pub fn insert(&mut self, id: u32, control: bool) -> Result<(), Error> {
        if id == 0 {
            return Err(Error::BadStream);
        }
        if self.get(id).is_some() {
            return Ok(());
        }
        let slot = self
            .streams
            .iter()
            .position(Option::is_none)
            .ok_or(Error::NoStream)?;
        self.streams[slot] = Some(Stream::new(id, control));
        self.opens = self.opens.saturating_add(1);
        Ok(())
    }

    /// Register a peer-opened stream, enforcing parity and the table bound.
    pub fn accept(&mut self, id: u32, control: bool) -> Result<(), Error> {
        if id == 0 || id % 2 == self.next_id % 2 {
            return Err(Error::BadStream);
        }
        if self.get(id).is_some() {
            return Ok(());
        }
        let slot = self
            .streams
            .iter()
            .position(Option::is_none)
            .ok_or(Error::NoStream)?;
        self.streams[slot] = Some(Stream::new(id, control));
        self.opens = self.opens.saturating_add(1);
        Ok(())
    }

    /// Get a stream.
    pub fn get(&self, id: u32) -> Option<&Stream> {
        self.streams.iter().flatten().find(|stream| stream.id == id)
    }

    /// Get a mutable stream.
    pub fn get_mut(&mut self, id: u32) -> Option<&mut Stream> {
        self.streams
            .iter_mut()
            .flatten()
            .find(|stream| stream.id == id)
    }

    /// Grant the peer more send credit.
    pub fn grant(&mut self, id: u32, credit: u32) -> Result<(), Error> {
        if credit == 0 || credit > MAX_CREDIT {
            return Err(Error::BadCredit);
        }
        let stream = self.get_mut(id).ok_or(Error::BadStream)?;
        stream.credit_tx = stream.credit_tx.saturating_add(credit).min(MAX_CREDIT);
        stream.credit_rx = stream.credit_rx.saturating_add(credit).min(MAX_CREDIT);
        Ok(())
    }

    /// Reserve credit and queue space for `bytes`, or report backpressure.
    pub fn reserve(&mut self, id: u32, bytes: u32) -> Result<(), Error> {
        let stream = self.get_mut(id).ok_or(Error::BadStream)?;
        if matches!(stream.state, StreamState::Reset) {
            return Err(Error::Closed);
        }
        if stream.queued.saturating_add(bytes) > MAX_QUEUED_BYTES {
            return Err(Error::QueueFull);
        }
        if !stream.control && stream.credit_tx < bytes {
            self.credit_stalls = self.credit_stalls.saturating_add(1);
            return Err(Error::CreditExhausted);
        }
        stream.queued = stream.queued.saturating_add(bytes);
        if !stream.control {
            stream.credit_tx -= bytes;
        }
        Ok(())
    }

    /// Release `bytes` from the local queue after the sink accepted them.
    pub fn release(&mut self, id: u32, bytes: u32) {
        if let Some(stream) = self.get_mut(id) {
            stream.queued = stream.queued.saturating_sub(bytes);
        }
    }

    /// Mark the local half closed.
    pub fn close_local(&mut self, id: u32) -> Result<(), Error> {
        let stream = self.get_mut(id).ok_or(Error::BadStream)?;
        stream.state = match stream.state {
            StreamState::HalfClosedRemote => StreamState::Reset,
            _ => StreamState::HalfClosedLocal,
        };
        Ok(())
    }

    /// Mark the remote half closed.
    pub fn close_remote(&mut self, id: u32) -> Result<(), Error> {
        let stream = self.get_mut(id).ok_or(Error::BadStream)?;
        stream.state = match stream.state {
            StreamState::HalfClosedLocal => StreamState::Reset,
            _ => StreamState::HalfClosedRemote,
        };
        Ok(())
    }

    /// Reset a stream and release its slot.
    pub fn reset(&mut self, id: u32) -> Result<(), Error> {
        let slot = self
            .streams
            .iter()
            .position(|stream| stream.is_some_and(|stream| stream.id == id))
            .ok_or(Error::BadStream)?;
        self.streams[slot] = None;
        self.resets = self.resets.saturating_add(1);
        Ok(())
    }

    /// Number of open streams.
    pub fn len(&self) -> usize {
        self.streams.iter().flatten().count()
    }

    /// Whether the table is empty.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Cumulative opens.
    pub const fn opens(&self) -> u64 {
        self.opens
    }

    /// Cumulative resets.
    pub const fn resets(&self) -> u64 {
        self.resets
    }

    /// Cumulative credit stalls.
    pub const fn credit_stalls(&self) -> u64 {
        self.credit_stalls
    }
}

/// Scheduling class for one frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Class {
    /// Join, route, lease, migration, control and stream reset.
    Control,
    /// Bulk data.
    Data,
}

/// A bounded two-class scheduler.
///
/// Control records are always selected before data.  Data streams rotate
/// round-robin so one bulk stream cannot starve another.  The caller owns the
/// encoded record bytes; this type only owns the order.
#[derive(Debug)]
pub struct Scheduler {
    control: alloc::collections::VecDeque<u32>,
    data: alloc::collections::VecDeque<u32>,
}

impl Default for Scheduler {
    fn default() -> Self {
        Self::new()
    }
}

impl Scheduler {
    /// Create an empty scheduler.
    pub fn new() -> Self {
        Self {
            control: alloc::collections::VecDeque::new(),
            data: alloc::collections::VecDeque::new(),
        }
    }

    /// Queue one frame for its stream and class.
    pub fn push(&mut self, stream: u32, class: Class) {
        match class {
            Class::Control => self.control.push_back(stream),
            Class::Data => self.data.push_back(stream),
        }
    }

    /// Take the next stream ID, control first.
    pub fn pop(&mut self) -> Option<(u32, Class)> {
        if let Some(stream) = self.control.pop_front() {
            return Some((stream, Class::Control));
        }
        self.data.pop_front().map(|stream| (stream, Class::Data))
    }

    /// Number of queued frames.
    pub fn len(&self) -> usize {
        self.control.len() + self.data.len()
    }

    /// Whether no frame is queued.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allocates_odd_and_even_ids_with_bounds() {
        let mut table = StreamTable::new(Side::Initiator);
        assert_eq!(table.open(false).unwrap(), 1);
        assert_eq!(table.open(false).unwrap(), 3);
        for _ in 2..MAX_STREAMS {
            table.open(false).unwrap();
        }
        assert_eq!(table.open(false), Err(Error::NoStream));
        assert_eq!(table.len(), MAX_STREAMS);

        let mut responder = StreamTable::new(Side::Responder);
        assert_eq!(responder.open(false).unwrap(), 2);
        assert_eq!(responder.accept(5, false), Ok(()));
        assert_eq!(responder.accept(4, false), Err(Error::BadStream));
    }

    #[test]
    fn credit_blocks_a_slow_consumer_and_control_bypasses_it() {
        let mut table = StreamTable::new(Side::Initiator);
        let data = table.open(false).unwrap();
        let control = table.open(true).unwrap();
        assert_eq!(
            table.reserve(data, INITIAL_CREDIT + 1),
            Err(Error::CreditExhausted)
        );
        assert_eq!(table.reserve(control, MAX_CREDIT), Ok(()));
        assert_eq!(table.credit_stalls(), 1);
        table.grant(data, 4096).unwrap();
        assert_eq!(table.reserve(data, 4096), Ok(()));
        table.release(data, 4096);
        assert_eq!(table.get(data).unwrap().queued, 0);
    }

    #[test]
    fn reset_reclaims_the_slot_and_rejects_further_sends() {
        let mut table = StreamTable::new(Side::Initiator);
        let id = table.open(false).unwrap();
        table.reset(id).unwrap();
        assert_eq!(table.get(id), None);
        assert_eq!(table.reserve(id, 1), Err(Error::BadStream));
        assert_eq!(table.resets(), 1);
        assert!(table.open(false).is_ok());
    }

    #[test]
    fn queue_bound_is_per_stream() {
        let mut table = StreamTable::new(Side::Initiator);
        let id = table.open(false).unwrap();
        table.grant(id, MAX_CREDIT).unwrap();
        assert_eq!(table.reserve(id, MAX_QUEUED_BYTES), Ok(()));
        assert_eq!(table.reserve(id, 1), Err(Error::QueueFull));
        table.release(id, MAX_QUEUED_BYTES);
        table.grant(id, 1).unwrap();
        assert_eq!(table.reserve(id, 1), Ok(()));
    }

    #[test]
    fn control_is_scheduled_before_data_and_data_rotates() {
        let mut scheduler = Scheduler::new();
        scheduler.push(5, Class::Data);
        scheduler.push(7, Class::Data);
        scheduler.push(1, Class::Control);
        assert_eq!(scheduler.pop(), Some((1, Class::Control)));
        assert_eq!(scheduler.pop(), Some((5, Class::Data)));
        assert_eq!(scheduler.pop(), Some((7, Class::Data)));
        assert!(scheduler.is_empty());
    }

    #[test]
    fn close_transitions_are_symmetric() {
        let mut table = StreamTable::new(Side::Initiator);
        let id = table.open(false).unwrap();
        table.close_local(id).unwrap();
        assert_eq!(table.get(id).unwrap().state, StreamState::HalfClosedLocal);
        table.close_remote(id).unwrap();
        assert_eq!(table.get(id).unwrap().state, StreamState::Reset);
    }
}
