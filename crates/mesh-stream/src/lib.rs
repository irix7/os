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
//! tests drive it with synthetic frames; the live module drives [`StreamMux`],
//! which combines the table, the two-class scheduler and the send ledger.

#![no_std]
#![forbid(unsafe_code)]
#![deny(missing_docs)]

extern crate alloc;

use alloc::collections::VecDeque;
use core::fmt;

/// Maximum simultaneously open streams per link.
pub const MAX_STREAMS: usize = 16;
/// Maximum bytes a receiver may grant in one update.
pub const MAX_CREDIT: u32 = 64 * 1024;
/// Maximum queued data bytes per stream.
pub const MAX_QUEUED_BYTES: u32 = 64 * 1024;
/// Initial receive credit for a data stream.
pub const INITIAL_CREDIT: u32 = 16 * 1024;
/// Maximum records a link may have reserved but not yet accepted by the sink.
pub const MAX_INFLIGHT_RECORDS: usize = 64;

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

    /// Insert a fixed internal stream (control or well-known data channel).
    ///
    /// Fixed IDs do not follow the allocator, so the next dynamic ID is moved
    /// past the inserted value and keeps this side's parity.  That keeps a
    /// locally opened stream from colliding with a fixed channel.
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
        self.bump_next_id(id);
        Ok(())
    }

    /// Move the dynamic allocator past a fixed ID, preserving its parity.
    fn bump_next_id(&mut self, id: u32) {
        if id < self.next_id {
            return;
        }
        let parity = match self.side {
            Side::Initiator => 1,
            Side::Responder => 0,
        };
        let mut next = id.saturating_add(1);
        if next % 2 != parity {
            next = next.saturating_add(1);
        }
        self.next_id = next.max(1);
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

    /// Grant the peer more receive credit (we are telling the peer it may send).
    pub fn grant_receive(&mut self, id: u32, credit: u32) -> Result<(), Error> {
        if credit == 0 || credit > MAX_CREDIT {
            return Err(Error::BadCredit);
        }
        let stream = self.get_mut(id).ok_or(Error::BadStream)?;
        stream.credit_rx = stream.credit_rx.saturating_add(credit).min(MAX_CREDIT);
        Ok(())
    }

    /// Apply a peer grant to our send credit.
    pub fn grant_send(&mut self, id: u32, credit: u32) -> Result<(), Error> {
        if credit == 0 || credit > MAX_CREDIT {
            return Err(Error::BadCredit);
        }
        let stream = self.get_mut(id).ok_or(Error::BadStream)?;
        stream.credit_tx = stream.credit_tx.saturating_add(credit).min(MAX_CREDIT);
        Ok(())
    }

    /// Set the receive credit to an exact value, bounded by [`MAX_CREDIT`].
    ///
    /// A bulk request carries the origin's initial window; the responder must
    /// start from that value, not the generic [`INITIAL_CREDIT`].
    pub fn set_receive_credit(&mut self, id: u32, credit: u32) -> Result<(), Error> {
        if credit == 0 {
            return Err(Error::BadCredit);
        }
        let stream = self.get_mut(id).ok_or(Error::BadStream)?;
        stream.credit_rx = credit.min(MAX_CREDIT);
        Ok(())
    }

    /// Set the send credit to an exact value, bounded by [`MAX_CREDIT`].
    pub fn set_send_credit(&mut self, id: u32, credit: u32) -> Result<(), Error> {
        if credit == 0 {
            return Err(Error::BadCredit);
        }
        let stream = self.get_mut(id).ok_or(Error::BadStream)?;
        stream.credit_tx = credit.min(MAX_CREDIT);
        Ok(())
    }

    /// Consume receive credit for `bytes` accepted from the peer.
    ///
    /// Fails with [`Error::CreditExhausted`] rather than accepting data above
    /// the window we granted.  Control streams are not credit-managed.
    pub fn consume_receive(&mut self, id: u32, bytes: u32) -> Result<(), Error> {
        let stream = self.get_mut(id).ok_or(Error::BadStream)?;
        if stream.control {
            return Ok(());
        }
        if stream.credit_rx < bytes {
            return Err(Error::CreditExhausted);
        }
        stream.credit_rx -= bytes;
        Ok(())
    }

    /// Reserve queue space for a control frame on any stream.
    ///
    /// Control frames bypass data credit: a cancel or window update on a
    /// backpressured data stream must still be able to leave the node.
    pub fn reserve_control(&mut self, id: u32, bytes: u32) -> Result<(), Error> {
        let stream = self.get_mut(id).ok_or(Error::BadStream)?;
        if matches!(stream.state, StreamState::Reset) {
            return Err(Error::Closed);
        }
        if stream.queued.saturating_add(bytes) > MAX_QUEUED_BYTES {
            return Err(Error::QueueFull);
        }
        stream.queued = stream.queued.saturating_add(bytes);
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

    /// Bytes reserved across every open stream.
    pub fn queued_bytes(&self) -> u32 {
        self.streams
            .iter()
            .flatten()
            .map(|stream| stream.queued)
            .fold(0u32, u32::saturating_add)
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

    /// Drop every queued frame for `stream` after a reset or cancel.
    pub fn remove(&mut self, stream: u32) {
        self.control.retain(|queued| *queued != stream);
        self.data.retain(|queued| *queued != stream);
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

/// One link's live stream policy: bounded table, two-class scheduler and the
/// send ledger.
///
/// The module calls [`StreamMux::begin`] before it encodes a frame,
/// [`StreamMux::schedule`] when the frame is staged, and
/// [`StreamMux::commit`] once the transport queue accepted it.  A record stays
/// in the ledger until [`StreamMux::settle`] sees the sink accept it, so a
/// blocked socket backpressures the stream that owns the bytes instead of
/// letting it allocate without bound.  Control records keep their own class
/// and are scheduled before data.
#[derive(Debug)]
pub struct StreamMux {
    table: StreamTable,
    scheduler: Scheduler,
    ledger: VecDeque<(u32, u32)>,
    control_frames: u64,
    data_frames: u64,
    control_stalls: u64,
    data_stalls: u64,
}

impl StreamMux {
    /// Create an empty mux for one side of a link.
    pub fn new(side: Side) -> Self {
        Self {
            table: StreamTable::new(side),
            scheduler: Scheduler::new(),
            ledger: VecDeque::new(),
            control_frames: 0,
            data_frames: 0,
            control_stalls: 0,
            data_stalls: 0,
        }
    }

    /// The side this mux allocates for.
    pub const fn side(&self) -> Side {
        self.table.side()
    }

    /// Open a dynamically allocated stream and return its ID.
    pub fn open_channel(&mut self, control: bool) -> Result<u32, Error> {
        self.table.open(control)
    }

    /// Register a fixed channel.
    pub fn add_channel(&mut self, id: u32, control: bool) -> Result<(), Error> {
        self.table.insert(id, control)
    }

    /// Accept a peer-opened channel, enforcing parity and the table bound.
    pub fn accept_channel(&mut self, id: u32, control: bool) -> Result<(), Error> {
        self.table.accept(id, control)
    }

    /// One stream's bounded state.
    pub fn stream(&self, id: u32) -> Option<&Stream> {
        self.table.get(id)
    }

    /// The scheduling class of a registered stream.
    pub fn class(&self, id: u32) -> Option<Class> {
        self.table.get(id).map(|stream| {
            if stream.control {
                Class::Control
            } else {
                Class::Data
            }
        })
    }

    /// Reserve credit and queue space for a frame on `id`.
    ///
    /// A control channel is never credit-blocked.  A data channel reports
    /// [`Error::CreditExhausted`] when the peer has not granted enough window.
    pub fn begin(&mut self, id: u32, bytes: u32) -> Result<Class, Error> {
        let control = self.table.get(id).ok_or(Error::BadStream)?.control;
        let result = if control {
            self.table.reserve_control(id, bytes)
        } else {
            self.table.reserve(id, bytes)
        };
        match result {
            Ok(()) => Ok(if control { Class::Control } else { Class::Data }),
            Err(error) => {
                if control {
                    self.control_stalls = self.control_stalls.saturating_add(1);
                } else {
                    self.data_stalls = self.data_stalls.saturating_add(1);
                }
                Err(error)
            }
        }
    }

    /// Reserve queue space for a control frame on any stream.
    ///
    /// Cancel and window-update frames travel on the data stream they refer
    /// to, but must not wait for that stream's data credit.
    pub fn begin_control(&mut self, id: u32, bytes: u32) -> Result<Class, Error> {
        match self.table.reserve_control(id, bytes) {
            Ok(()) => Ok(Class::Control),
            Err(error) => {
                self.control_stalls = self.control_stalls.saturating_add(1);
                Err(error)
            }
        }
    }

    /// Place a reserved frame in the scheduler's class queue.
    ///
    /// Fails with [`Error::QueueFull`] when the per-link in-flight bound is
    /// reached.  The caller must release the reservation with
    /// [`Self::abort`] on failure.
    pub fn schedule(&mut self, id: u32, class: Class) -> Result<(), Error> {
        if self.scheduler.len() >= MAX_INFLIGHT_RECORDS {
            return Err(Error::QueueFull);
        }
        self.scheduler.push(id, class);
        match class {
            Class::Control => self.control_frames = self.control_frames.saturating_add(1),
            Class::Data => self.data_frames = self.data_frames.saturating_add(1),
        }
        Ok(())
    }

    /// Whether the send ledger has room for another record.
    pub fn can_commit(&self) -> bool {
        self.ledger.len() < MAX_INFLIGHT_RECORDS
    }

    /// Record a frame the transport queue accepted.
    ///
    /// The ledger entry is released by [`Self::settle`] only when the sink has
    /// written the record, so an unsent record still counts against the
    /// stream's queue bound.
    pub fn commit(&mut self, id: u32, bytes: u32) -> Result<(), Error> {
        if self.ledger.len() >= MAX_INFLIGHT_RECORDS {
            return Err(Error::QueueFull);
        }
        self.ledger.push_back((id, bytes));
        Ok(())
    }

    /// Release a reservation whose frame never reached the transport queue.
    pub fn abort(&mut self, id: u32, bytes: u32) {
        self.table.release(id, bytes);
    }

    /// Release ledger entries for records the sink accepted.
    ///
    /// Returns the number of entries settled.  The queue is FIFO, so the first
    /// `frames` entries are exactly the records the sink completed.
    pub fn settle(&mut self, frames: usize) -> usize {
        let mut settled = 0;
        for _ in 0..frames {
            let Some((id, bytes)) = self.ledger.pop_front() else {
                break;
            };
            self.table.release(id, bytes);
            settled += 1;
        }
        settled
    }

    /// Take the next scheduled frame, control class first.
    pub fn next_frame(&mut self) -> Option<(u32, Class)> {
        self.scheduler.pop()
    }

    /// Apply a peer window update to our send credit.
    pub fn grant_send(&mut self, id: u32, credit: u32) -> Result<(), Error> {
        self.table.grant_send(id, credit)
    }

    /// Grant the peer more receive credit.
    pub fn grant_receive(&mut self, id: u32, credit: u32) -> Result<(), Error> {
        self.table.grant_receive(id, credit)
    }

    /// Set the send credit to an exact value.
    pub fn set_send_credit(&mut self, id: u32, credit: u32) -> Result<(), Error> {
        self.table.set_send_credit(id, credit)
    }

    /// Set the receive credit to an exact value.
    pub fn set_receive_credit(&mut self, id: u32, credit: u32) -> Result<(), Error> {
        self.table.set_receive_credit(id, credit)
    }

    /// Consume receive credit for bytes accepted from the peer.
    pub fn consume_receive(&mut self, id: u32, bytes: u32) -> Result<(), Error> {
        self.table.consume_receive(id, bytes)
    }

    /// Mark the local half closed.
    pub fn close_local(&mut self, id: u32) -> Result<(), Error> {
        self.table.close_local(id)
    }

    /// Mark the remote half closed.
    pub fn close_remote(&mut self, id: u32) -> Result<(), Error> {
        self.table.close_remote(id)
    }

    /// Reset a stream, release its queued records and reclaim the slot.
    pub fn reset(&mut self, id: u32) -> Result<(), Error> {
        self.table.reset(id)?;
        self.scheduler.remove(id);
        self.ledger.retain(|(queued, _)| *queued != id);
        Ok(())
    }

    /// Drop every queued record for a link reset.
    pub fn clear(&mut self) {
        self.ledger.clear();
        self.scheduler = Scheduler::new();
    }

    /// Number of open streams.
    pub fn len(&self) -> usize {
        self.table.len()
    }

    /// Whether the table is empty.
    pub fn is_empty(&self) -> bool {
        self.table.is_empty()
    }

    /// Cumulative opens.
    pub const fn opens(&self) -> u64 {
        self.table.opens()
    }

    /// Cumulative resets.
    pub const fn resets(&self) -> u64 {
        self.table.resets()
    }

    /// Data credit stalls seen by the table.
    pub const fn credit_stalls(&self) -> u64 {
        self.table.credit_stalls()
    }

    /// Control frames that could not reserve queue space.
    pub const fn control_stalls(&self) -> u64 {
        self.control_stalls
    }

    /// Data frames that could not reserve credit or queue space.
    pub const fn data_stalls(&self) -> u64 {
        self.data_stalls
    }

    /// Control frames scheduled.
    pub const fn control_frames(&self) -> u64 {
        self.control_frames
    }

    /// Data frames scheduled.
    pub const fn data_frames(&self) -> u64 {
        self.data_frames
    }

    /// Records reserved but not yet accepted by the sink.
    pub fn inflight(&self) -> usize {
        self.ledger.len()
    }

    /// Bytes reserved across every stream.
    pub fn queued_bytes(&self) -> u32 {
        self.table.queued_bytes()
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
        table.grant_send(data, 4096).unwrap();
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
        table.grant_send(id, MAX_CREDIT).unwrap();
        assert_eq!(table.reserve(id, MAX_QUEUED_BYTES), Ok(()));
        assert_eq!(table.reserve(id, 1), Err(Error::QueueFull));
        table.release(id, MAX_QUEUED_BYTES);
        table.grant_send(id, 1).unwrap();
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

    #[test]
    fn fixed_channels_do_not_collide_with_dynamic_ids() {
        let mut initiator = StreamTable::new(Side::Initiator);
        for id in [1, 2, 3, 4, 6] {
            initiator.insert(id, id != 5).unwrap();
        }
        assert_eq!(initiator.open(false).unwrap(), 7);

        let mut responder = StreamTable::new(Side::Responder);
        for id in [1, 2, 3, 4, 6] {
            responder.insert(id, id != 5).unwrap();
        }
        assert_eq!(responder.open(false).unwrap(), 8);
    }

    fn live_mux(side: Side) -> StreamMux {
        let mut mux = StreamMux::new(side);
        mux.add_channel(1, true).unwrap();
        mux.add_channel(2, true).unwrap();
        mux.add_channel(3, true).unwrap();
        mux.add_channel(4, true).unwrap();
        mux.add_channel(6, true).unwrap();
        mux
    }

    #[test]
    fn live_mux_reserves_commits_and_settles_every_stream() {
        let mut mux = live_mux(Side::Initiator);
        let data = mux.open_channel(false).unwrap();
        for id in [1, 2, 3, 4, data, 6] {
            let class = mux.begin(id, 32).unwrap();
            mux.schedule(id, class).unwrap();
            mux.commit(id, 32).unwrap();
        }
        assert_eq!(mux.inflight(), 6);
        assert_eq!(mux.queued_bytes(), 6 * 32);
        assert_eq!(mux.settle(4), 4);
        assert_eq!(mux.inflight(), 2);
        assert_eq!(mux.queued_bytes(), 2 * 32);
        let mut order = alloc::vec::Vec::new();
        while let Some(next) = mux.next_frame() {
            order.push(next);
        }
        assert_eq!(order.len(), 6);
        assert!(order[..5].iter().all(|(_, class)| *class == Class::Control));
        assert_eq!(order[5].1, Class::Data);
    }

    #[test]
    fn control_is_never_credit_blocked_by_a_backpressured_data_stream() {
        let mut mux = live_mux(Side::Initiator);
        let data = mux.open_channel(false).unwrap();
        assert_eq!(
            mux.begin(data, INITIAL_CREDIT + 1),
            Err(Error::CreditExhausted)
        );
        assert_eq!(mux.begin(3, 64).unwrap(), Class::Control);
        assert_eq!(mux.data_stalls(), 1);
        assert_eq!(mux.control_stalls(), 0);
    }

    #[test]
    fn control_on_a_data_stream_bypasses_credit_for_cancel() {
        let mut mux = live_mux(Side::Initiator);
        let data = mux.open_channel(false).unwrap();
        assert_eq!(
            mux.begin(data, INITIAL_CREDIT + 1),
            Err(Error::CreditExhausted)
        );
        assert_eq!(mux.begin_control(data, 4).unwrap(), Class::Control);
        mux.schedule(data, Class::Control).unwrap();
        assert_eq!(mux.next_frame(), Some((data, Class::Control)));
    }

    #[test]
    fn a_blocked_sink_backpressures_only_the_data_stream() {
        let mut mux = live_mux(Side::Initiator);
        let data = mux.open_channel(false).unwrap();
        mux.set_send_credit(data, MAX_CREDIT).unwrap();
        let mut sent = 0u32;
        let error = loop {
            match mux.begin(data, 16 * 1024) {
                Ok(_) => {
                    mux.schedule(data, Class::Data).unwrap();
                    mux.commit(data, 16 * 1024).unwrap();
                    // No settle: the socket has not accepted these records.
                    sent += 1;
                    assert!(sent < 8, "queue bound did not stop the data stream");
                }
                Err(stall) => break stall,
            }
        };
        assert_eq!(error, Error::QueueFull);
        assert_eq!(mux.begin(2, 512).unwrap(), Class::Control);
        assert_eq!(mux.data_stalls(), 1);
    }

    #[test]
    fn reset_reclaims_a_stream_with_a_queued_chunk() {
        let mut mux = live_mux(Side::Initiator);
        let data = mux.open_channel(false).unwrap();
        mux.begin(data, 4096).unwrap();
        mux.schedule(data, Class::Data).unwrap();
        mux.commit(data, 4096).unwrap();
        mux.reset(data).unwrap();
        assert!(mux.stream(data).is_none());
        assert_eq!(mux.queued_bytes(), 0);
        assert_eq!(mux.inflight(), 0);
        assert!(mux.next_frame().is_none());
        assert_eq!(mux.begin(data, 1), Err(Error::BadStream));
        assert_eq!(mux.resets(), 1);
        assert!(mux.open_channel(false).is_ok());
    }

    #[test]
    fn a_send_credit_grant_does_not_change_the_receive_window() {
        let mut mux = live_mux(Side::Initiator);
        let data = mux.open_channel(false).unwrap();
        let before = mux.stream(data).unwrap().credit_rx;
        mux.grant_send(data, 4096).unwrap();
        assert_eq!(mux.stream(data).unwrap().credit_rx, before);
        assert_eq!(mux.stream(data).unwrap().credit_tx, before + 4096);
    }
}
