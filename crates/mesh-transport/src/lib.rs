// SPDX-License-Identifier: GPL-3.0-only
//! M2-1 mesh transport primitives.
//!
//! The M2 transport is a TCP byte stream.  This crate owns the parts that are
//! independent of the socket implementation:
//!
//! * the record framing: a 32-bit little-endian envelope length followed by
//!   exactly that many bytes of [`MeshEnvelope`];
//! * a bounded incremental decoder, so a short read never turns into a partial
//!   frame;
//! * a bounded send queue, so a short write never drops bytes;
//! * deadlines, heartbeat policy and reconnect backoff;
//! * request-tag allocation, cancellation and release.
//!
//! The crate is `no_std` with `alloc`, and has no socket or kernel dependency.
//! The module and the host test harness provide the byte source/sink.  The
//! wire bytes are produced by `sys-primitives`; this crate never serialises a
//! `#[repr(C)]` structure directly.
//!
//! ## Wire contract
//!
//! ```text
//! u32 little-endian length L
//! L bytes of MeshEnvelope
//! ```
//!
//! `L` counts only the envelope.  `L` must be at least
//! [`MIN_ENVELOPE_LEN`] and at most [`MAX_RECORD_LEN`] (64 KiB for M2).  The
//! embedded `MeshHeader::frame_len` remains the authority for the envelope
//! body; the decoder requires it to agree with `L`.  `MeshEnvelope::decode`
//! validates the header magic, version, length, CRC32C and the embedded
//! `MessageFrame` before this crate accepts the record.

#![no_std]
#![forbid(unsafe_code)]
#![deny(missing_docs)]

extern crate alloc;

use alloc::vec::Vec;
use core::fmt;

use sys_primitives::{MeshEnvelope, MeshHeader, Tag};

/// Width of the little-endian record length prefix.
pub const LENGTH_PREFIX_LEN: usize = 4;

/// Maximum envelope length accepted on the M2 wire (64 KiB).
pub const MAX_RECORD_LEN: usize = 64 * 1024;

/// Minimum valid envelope: mesh header, frame header and payload descriptor.
pub const MIN_ENVELOPE_LEN: usize = MeshHeader::LEN + 128 + 64;

/// Maximum number of bytes a [`SendQueue`] buffers before reporting backpressure.
pub const MAX_QUEUED_BYTES: usize = MAX_RECORD_LEN + LENGTH_PREFIX_LEN;

/// Bytes of a [`SendQueue`] reserved for control records.
///
/// A data record may only use the space above this reserve, so a queued bulk
/// chunk can never make a join, lease or route acknowledgement fail with
/// [`Error::QueueFull`].  Eight KiB is one maximum-size control record plus a
/// few hundred bytes of join/route traffic.
pub const CONTROL_RESERVE_BYTES: usize = 8 * 1024;

/// Connect deadline for a new TCP link (ADR M2-0002: 5 s).
pub const CONNECT_DEADLINE_MS: u64 = 5_000;

/// Deadline for one record once its first byte has arrived.
///
/// The M2 module is single-threaded: while it is signing a confirmation or
/// writing an MCP HTTP response, the mesh receive path can be idle for longer
/// than the old 10 s deadline.  A 388-byte confirmation with 8 bytes buffered
/// was observed closing a healthy link.  The record deadline now matches the
/// heartbeat window; a truly dead peer is still detected by
/// [`HEARTBEAT_TIMEOUT_MS`].
pub const RECEIVE_DEADLINE_MS: u64 = 30_000;

/// Deadline for making progress on a non-empty send queue.
///
/// Same rationale as [`RECEIVE_DEADLINE_MS`]: the peer may be busy in a
/// single-threaded tick, so backpressure is allowed the heartbeat window.
pub const SEND_DEADLINE_MS: u64 = 30_000;

/// Interval between mesh heartbeats on an otherwise idle link.
pub const HEARTBEAT_INTERVAL_MS: u64 = 5_000;

/// A link with no traffic at all for this long is treated as dead.
///
/// This is not shorter than [`RECEIVE_DEADLINE_MS`]: a partial record is a
/// record deadline, while a silent established link is a heartbeat timeout.
pub const HEARTBEAT_TIMEOUT_MS: u64 = 30_000;

/// First reconnect delay.
pub const RECONNECT_MIN_MS: u64 = 250;

/// Reconnect delay cap.
pub const RECONNECT_MAX_MS: u64 = 10_000;

const fn at_least(value: u64, minimum: u64) -> u64 {
    if value < minimum {
        minimum
    } else {
        value
    }
}

/// A framing or transport-policy failure.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    /// The input ended before the four-byte length prefix.
    ShortPrefix,
    /// The declared envelope is below [`MIN_ENVELOPE_LEN`].
    TooShort,
    /// The declared envelope is above [`MAX_RECORD_LEN`].
    Oversize,
    /// The input ended before the declared envelope length.
    ShortEnvelope,
    /// A complete-record buffer did not contain exactly one declared record.
    LengthMismatch,
    /// The output buffer was smaller than the encoded record.
    OutputTooSmall,
    /// The send queue is already at [`MAX_QUEUED_BYTES`].
    QueueFull,
    /// The decoder is poisoned by an earlier framing error.
    Poisoned,
    /// The decoder has a complete record that must be consumed first.
    Full,
    /// The decoder buffer is at its fixed bound; the peer sent a record larger
    /// than this link's configured limit.
    DecoderFull,
    /// No complete record is available yet.
    NotReady,
    /// No request tag is available.
    NoTag,
    /// The embedded envelope failed validation.
    Envelope(sys_primitives::Error),
}

impl Error {
    /// A short stable label for serial logs and evidence.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ShortPrefix => "short-prefix",
            Self::TooShort => "too-short",
            Self::Oversize => "oversize",
            Self::ShortEnvelope => "short-envelope",
            Self::LengthMismatch => "length-mismatch",
            Self::OutputTooSmall => "output-too-small",
            Self::QueueFull => "queue-full",
            Self::Poisoned => "poisoned",
            Self::Full => "full",
            Self::DecoderFull => "decoder-full",
            Self::NotReady => "not-ready",
            Self::NoTag => "no-tag",
            Self::Envelope(error) => envelope_error_label(error),
        }
    }
}

const fn envelope_error_label(error: sys_primitives::Error) -> &'static str {
    match error {
        sys_primitives::Error::Truncated => "envelope-truncated",
        sys_primitives::Error::BadMagic => "bad-magic",
        sys_primitives::Error::BadVersion => "bad-version",
        sys_primitives::Error::BadLength => "bad-length",
        sys_primitives::Error::BadCrc => "bad-crc",
        sys_primitives::Error::BadKind => "bad-kind",
        sys_primitives::Error::BadStatus => "bad-status",
        sys_primitives::Error::BadRights => "bad-rights",
        sys_primitives::Error::NonCanonical => "non-canonical",
        sys_primitives::Error::Protocol => "protocol",
        sys_primitives::Error::BadSignature => "bad-signature",
        sys_primitives::Error::NotYetValid => "not-yet-valid",
        sys_primitives::Error::Expired => "expired",
        sys_primitives::Error::WrongObject => "wrong-object",
        sys_primitives::Error::InsufficientRights => "insufficient-rights",
        sys_primitives::Error::Revoked => "revoked",
        sys_primitives::Error::Replay => "replay",
        sys_primitives::Error::StaleHandle => "stale-handle",
    }
}

impl From<sys_primitives::Error> for Error {
    fn from(error: sys_primitives::Error) -> Self {
        Self::Envelope(error)
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// The encoded size of one record, including its length prefix.
pub fn record_len(envelope: &MeshEnvelope<'_>) -> Result<usize, Error> {
    let envelope_len = envelope.encoded_len();
    if envelope_len < MIN_ENVELOPE_LEN {
        return Err(Error::TooShort);
    }
    if envelope_len > MAX_RECORD_LEN {
        return Err(Error::Oversize);
    }
    Ok(LENGTH_PREFIX_LEN + envelope_len)
}

/// Encode one record into `out`.  Returns the number of bytes written.
pub fn encode_record(envelope: &MeshEnvelope<'_>, out: &mut [u8]) -> Result<usize, Error> {
    let total = record_len(envelope)?;
    if out.len() < total {
        return Err(Error::OutputTooSmall);
    }
    let envelope_len = total - LENGTH_PREFIX_LEN;
    out[..LENGTH_PREFIX_LEN].copy_from_slice(&(envelope_len as u32).to_le_bytes());
    let written = envelope
        .encode(&mut out[LENGTH_PREFIX_LEN..total])
        .map_err(Error::Envelope)?;
    if written != envelope_len {
        return Err(Error::LengthMismatch);
    }
    Ok(total)
}

/// Decode one complete record (length prefix included) and validate it.
///
/// The slice must contain exactly one declared record.  A stream decoder hands
/// out exactly such a slice; this strictness makes a prefix/body disagreement a
/// hard error rather than a silently ignored trailing byte.
pub fn decode_record(record: &[u8]) -> Result<MeshEnvelope<'_>, Error> {
    if record.len() < LENGTH_PREFIX_LEN {
        return Err(Error::ShortPrefix);
    }
    let envelope_len = u32::from_le_bytes([record[0], record[1], record[2], record[3]]) as usize;
    if envelope_len > MAX_RECORD_LEN {
        return Err(Error::Oversize);
    }
    if envelope_len < MIN_ENVELOPE_LEN {
        return Err(Error::TooShort);
    }
    let total = LENGTH_PREFIX_LEN
        .checked_add(envelope_len)
        .ok_or(Error::LengthMismatch)?;
    if record.len() < total {
        return Err(Error::ShortEnvelope);
    }
    if record.len() > total {
        return Err(Error::LengthMismatch);
    }
    let (envelope, consumed) =
        MeshEnvelope::decode(&record[LENGTH_PREFIX_LEN..total]).map_err(Error::Envelope)?;
    if consumed != envelope_len {
        return Err(Error::LengthMismatch);
    }
    let expected = MeshHeader::LEN
        .checked_add(envelope.header.frame_len as usize)
        .ok_or(Error::LengthMismatch)?;
    if expected != envelope_len {
        return Err(Error::LengthMismatch);
    }
    Ok(envelope)
}

/// Incremental record decoder for a TCP byte stream.
///
/// `push` buffers at most one declared record plus its prefix.  Once a record
/// is complete it refuses to buffer more until `consume_record` is called, so
/// the caller naturally applies backpressure to the socket.  The buffer keeps
/// its allocation across records.
#[derive(Debug)]
pub struct StreamDecoder {
    buffer: Vec<u8>,
    /// Total record length (prefix + envelope) once the prefix is known.
    expected: Option<usize>,
    /// Hard cap on the buffer, independent of `MAX_QUEUED_BYTES`.
    ///
    /// A module can pre-allocate an exact 32 KiB and stream a 64 KiB logical
    /// payload in 16 KiB frames; it must never grow the bump-allocated `Vec`
    /// on the data path.
    limit: usize,
    poisoned: bool,
}

impl Default for StreamDecoder {
    fn default() -> Self {
        Self::new()
    }
}

impl StreamDecoder {
    /// Create a decoder with the module default: a 32 KiB fixed allocation.
    pub fn new() -> Self {
        Self::with_limit(32 * 1024, 32 * 1024)
    }

    /// Create a decoder with the given initial allocation.
    pub fn with_capacity(capacity: usize) -> Self {
        Self::with_limit(capacity, capacity.min(MAX_QUEUED_BYTES))
    }

    /// Create a decoder with an exact initial allocation and hard limit.
    ///
    /// The limit is clamped to [`MAX_QUEUED_BYTES`].  A record larger than the
    /// limit is rejected with [`Error::DecoderFull`] before the buffer grows.
    pub fn with_limit(capacity: usize, limit: usize) -> Self {
        let limit = limit.clamp(LENGTH_PREFIX_LEN, MAX_QUEUED_BYTES);
        let capacity = capacity.clamp(LENGTH_PREFIX_LEN, limit);
        Self {
            buffer: Vec::with_capacity(capacity),
            expected: None,
            limit,
            poisoned: false,
        }
    }

    /// The hard buffer bound in bytes.
    pub const fn limit(&self) -> usize {
        self.limit
    }

    /// Feed bytes from a short read.  Returns the number of bytes consumed.
    ///
    /// The return value can be less than `bytes.len()` when the input contains
    /// the start of a second record: the caller must process and consume the
    /// complete record before feeding the remainder again.
    pub fn push(&mut self, bytes: &[u8]) -> Result<usize, Error> {
        if self.poisoned {
            return Err(Error::Poisoned);
        }
        let mut used = 0usize;
        loop {
            if self.expected.is_none() {
                let want = LENGTH_PREFIX_LEN.saturating_sub(self.buffer.len());
                let take = want.min(bytes.len() - used);
                if take > 0 {
                    self.buffer.extend_from_slice(&bytes[used..used + take]);
                    used += take;
                }
                if self.buffer.len() < LENGTH_PREFIX_LEN {
                    return Ok(used);
                }
                if let Err(error) = self.inspect_prefix() {
                    self.poisoned = true;
                    return Err(error);
                }
            }

            let total = self.expected.ok_or(Error::LengthMismatch)?;
            if self.buffer.len() >= total {
                return Ok(used);
            }
            let take = (total - self.buffer.len()).min(bytes.len() - used);
            if take == 0 {
                return Ok(used);
            }
            self.buffer.extend_from_slice(&bytes[used..used + take]);
            used += take;
            if self.buffer.len() == total {
                return Ok(used);
            }
        }
    }

    /// The complete record, if one is buffered.
    ///
    /// The returned slice includes the four-byte length prefix and is exactly
    /// one record; pass it to [`decode_record`].  Call [`Self::consume_record`]
    /// before pushing more bytes.
    pub fn next_record(&self) -> Result<Option<&[u8]>, Error> {
        if self.poisoned {
            return Err(Error::Poisoned);
        }
        match self.expected {
            Some(total) if self.buffer.len() == total => Ok(Some(&self.buffer[..total])),
            _ => Ok(None),
        }
    }

    /// Release the complete record returned by [`Self::next_record`].
    pub fn consume_record(&mut self) -> Result<(), Error> {
        if self.poisoned {
            return Err(Error::Poisoned);
        }
        match self.expected {
            Some(total) if self.buffer.len() == total => {
                self.buffer.clear();
                self.expected = None;
                Ok(())
            }
            _ => Err(Error::NotReady),
        }
    }

    /// Drop any partial record and clear the poison flag.
    ///
    /// Used when a link is torn down and re-established: a partial record from
    /// the old connection must never be joined to bytes from the new one.
    pub fn reset(&mut self) {
        self.buffer.clear();
        self.expected = None;
        self.poisoned = false;
    }

    /// Bytes currently buffered, including the length prefix.
    pub fn buffered_len(&self) -> usize {
        self.buffer.len()
    }

    /// The declared total record length, once the prefix has arrived.
    pub fn expected_record_len(&self) -> Option<usize> {
        self.expected
    }

    /// Whether an earlier framing error poisoned this decoder.
    pub fn is_poisoned(&self) -> bool {
        self.poisoned
    }

    fn inspect_prefix(&mut self) -> Result<(), Error> {
        if self.buffer.len() < LENGTH_PREFIX_LEN {
            return Ok(());
        }
        let envelope_len = u32::from_le_bytes([
            self.buffer[0],
            self.buffer[1],
            self.buffer[2],
            self.buffer[3],
        ]) as usize;
        if envelope_len > MAX_RECORD_LEN {
            return Err(Error::Oversize);
        }
        if envelope_len < MIN_ENVELOPE_LEN {
            return Err(Error::TooShort);
        }
        let total = LENGTH_PREFIX_LEN + envelope_len;
        if total > self.limit {
            return Err(Error::DecoderFull);
        }
        self.expected = Some(total);
        Ok(())
    }
}

/// A non-blocking byte sink.
pub trait ByteSink {
    /// Write as much of `data` as the transport accepts.
    fn write(&mut self, data: &[u8]) -> WriteOutcome;
}

/// A non-blocking byte source.
pub trait ByteSource {
    /// Read into `out`, returning at most `out.len()` bytes.
    fn read(&mut self, out: &mut [u8]) -> ReadOutcome;
}

/// The result of one non-blocking write.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WriteOutcome {
    /// The sink accepted this many bytes.
    Wrote(usize),
    /// The sink has no room now; retry later.
    WouldBlock,
    /// The sink is closed or failed.
    Closed,
}

/// The result of one non-blocking read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReadOutcome {
    /// The source produced this many bytes.
    Data(usize),
    /// No bytes are available now; retry later.
    WouldBlock,
    /// The source is closed or failed.
    Closed,
}

/// One report from [`SendQueue::flush`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FlushReport {
    /// Bytes accepted by the sink during this flush.
    pub bytes_written: usize,
    /// Whole records accepted by the sink during this flush.
    pub frames_written: usize,
    /// Whether the sink asked the caller to retry later.
    pub blocked: bool,
    /// Whether the sink reported a closed link.
    pub closed: bool,
    /// Number of sink writes issued during this flush.
    pub writes: usize,
}

/// A bounded queue of encoded records.
///
/// Records are encoded into the queue's own buffer.  A short write leaves the
/// remainder in place and reports `blocked`, so the caller can retry after the
/// next socket poll without losing bytes.  The allocation is reused after the
/// queue drains; the module's bump allocator therefore does not grow on every
/// heartbeat.
#[derive(Debug)]
pub struct SendQueue {
    bytes: Vec<u8>,
    offset: usize,
    ends: Vec<usize>,
    first_queued_ms: Option<u64>,
    /// Total capacity, fixed at construction.
    capacity: usize,
    /// Bytes data records may not use.
    control_reserve: usize,
    /// High-water mark of queued bytes, for the module's memory evidence.
    peak_pending: usize,
    /// Number of sink writes issued by the most recent [`Self::flush`].
    last_writes: usize,
}

impl Default for SendQueue {
    fn default() -> Self {
        Self::new()
    }
}

impl SendQueue {
    /// Create an empty send queue with the module default: a 32 KiB fixed
    /// allocation and an 8 KiB control reserve.
    pub fn new() -> Self {
        Self::with_capacity(32 * 1024)
    }

    /// Create a queue with an exact capacity, clamped to the record bound.
    pub fn with_capacity(capacity: usize) -> Self {
        let capacity = capacity.clamp(LENGTH_PREFIX_LEN + MIN_ENVELOPE_LEN, MAX_QUEUED_BYTES);
        Self {
            bytes: Vec::with_capacity(capacity),
            offset: 0,
            ends: Vec::with_capacity(64),
            first_queued_ms: None,
            capacity,
            control_reserve: CONTROL_RESERVE_BYTES.min(capacity / 4),
            peak_pending: 0,
            last_writes: 0,
        }
    }

    /// Create a queue with an explicit control reserve.
    pub fn with_reserve(capacity: usize, control_reserve: usize) -> Self {
        let mut queue = Self::with_capacity(capacity);
        queue.control_reserve = control_reserve.min(queue.capacity / 2);
        queue
    }

    /// Encode and append one control record.  `now_ms` starts the send
    /// deadline when the queue becomes non-empty.
    pub fn push_record(&mut self, envelope: &MeshEnvelope<'_>, now_ms: u64) -> Result<(), Error> {
        self.push_with_limit(envelope, now_ms, self.capacity)
    }

    /// Encode and append one data record.  Data may not consume the control
    /// reserve, so a bulk stream cannot starve a join or lease acknowledgement.
    pub fn push_data_record(
        &mut self,
        envelope: &MeshEnvelope<'_>,
        now_ms: u64,
    ) -> Result<(), Error> {
        let limit = self.capacity.saturating_sub(self.control_reserve);
        self.push_with_limit(envelope, now_ms, limit)
    }

    fn push_with_limit(
        &mut self,
        envelope: &MeshEnvelope<'_>,
        now_ms: u64,
        limit: usize,
    ) -> Result<(), Error> {
        let total = record_len(envelope)?;
        if self.offset > 0 {
            let remaining = self.bytes.len() - self.offset;
            self.bytes.copy_within(self.offset.., 0);
            self.bytes.truncate(remaining);
            for end in &mut self.ends {
                *end -= self.offset;
            }
            self.offset = 0;
        }
        let pending = self.bytes.len();
        if pending + total > limit {
            return Err(Error::QueueFull);
        }
        let start = self.bytes.len();
        self.bytes.resize(start + total, 0);
        match encode_record(envelope, &mut self.bytes[start..]) {
            Ok(written) if written == total => {
                self.ends.push(start + total);
                if self.first_queued_ms.is_none() {
                    self.first_queued_ms = Some(now_ms);
                }
                self.peak_pending = self.peak_pending.max(self.bytes.len());
                Ok(())
            }
            Ok(_) => {
                self.bytes.truncate(start);
                Err(Error::LengthMismatch)
            }
            Err(error) => {
                self.bytes.truncate(start);
                Err(error)
            }
        }
    }

    /// Write as much of the queued bytes as `sink` accepts now.
    pub fn flush(&mut self, sink: &mut impl ByteSink) -> FlushReport {
        let mut report = FlushReport::default();
        self.last_writes = 0;
        loop {
            if self.offset == self.bytes.len() {
                self.bytes.clear();
                self.offset = 0;
                self.ends.clear();
                self.first_queued_ms = None;
                report.writes = self.last_writes;
                return report;
            }
            match sink.write(&self.bytes[self.offset..]) {
                WriteOutcome::Wrote(0) | WriteOutcome::WouldBlock => {
                    report.blocked = true;
                    report.writes = self.last_writes;
                    return report;
                }
                WriteOutcome::Wrote(count) => {
                    self.last_writes += 1;
                    let count = count.min(self.bytes.len() - self.offset);
                    self.offset += count;
                    report.bytes_written += count;
                    while self.ends.first().is_some_and(|end| *end <= self.offset) {
                        self.ends.remove(0);
                        report.frames_written += 1;
                    }
                    report.writes = self.last_writes;
                }
                WriteOutcome::Closed => {
                    report.closed = true;
                    report.writes = self.last_writes;
                    return report;
                }
            }
        }
    }

    /// Discard all queued records, for a link reset.
    pub fn clear(&mut self) {
        self.bytes.clear();
        self.offset = 0;
        self.ends.clear();
        self.first_queued_ms = None;
    }

    /// Bytes queued but not yet accepted by the sink.
    pub fn pending_bytes(&self) -> usize {
        self.bytes.len().saturating_sub(self.offset)
    }

    /// Records queued but not yet completely accepted by the sink.
    pub fn pending_frames(&self) -> usize {
        self.ends.len()
    }

    /// Configured capacity in bytes.
    pub const fn capacity(&self) -> usize {
        self.capacity
    }

    /// Configured control reserve in bytes.
    pub const fn control_reserve(&self) -> usize {
        self.control_reserve
    }

    /// High-water mark of queued bytes since construction.
    pub const fn peak_pending(&self) -> usize {
        self.peak_pending
    }

    /// Whether a non-empty queue has exceeded the send deadline.
    pub fn send_deadline_expired(&self, now_ms: u64) -> bool {
        self.first_queued_ms
            .is_some_and(|queued| now_ms.saturating_sub(queued) >= SEND_DEADLINE_MS)
    }
}

/// Cumulative transport counters.
///
/// These are the observable numbers the M2 gate asks for.  They are plain
/// fields so the caller can add a socket-specific counter without hiding it in
/// a logger.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Stats {
    /// Accepted TCP connections.
    pub connects: u64,
    /// Lost or closed TCP connections.
    pub disconnects: u64,
    /// Records handed to the byte sink completely.
    pub frames_sent: u64,
    /// Records decoded successfully.
    pub frames_received: u64,
    /// Record bytes, including length prefixes, accepted by the sink.
    pub bytes_sent: u64,
    /// Record bytes, including length prefixes, read from the source.
    pub bytes_received: u64,
    /// Reads that returned fewer bytes than requested.
    pub short_reads: u64,
    /// Writes that returned fewer bytes than requested.
    pub short_writes: u64,
    /// Records abandoned because the receive deadline expired.
    pub receive_deadlines: u64,
    /// Records abandoned because the send deadline expired.
    pub send_deadlines: u64,
    /// Connections abandoned because the connect deadline expired.
    pub connect_deadlines: u64,
    /// Records rejected for an oversize length prefix.
    pub oversize_rejects: u64,
    /// Records rejected for a short length prefix or envelope.
    pub short_frame_rejects: u64,
    /// Records rejected by the envelope CRC or header validation.
    pub header_rejects: u64,
    /// Records rejected for a routing or protocol mismatch.
    pub protocol_rejects: u64,
    /// Heartbeats sent.
    pub heartbeats_sent: u64,
    /// Links abandoned because the heartbeat timeout expired.
    pub heartbeat_timeouts: u64,
    /// Request tags released by cancellation.
    pub cancelled_tags: u64,
    /// Request tags released by their deadline.
    pub expired_tags: u64,
    /// Sink writes issued by the send queue.
    pub writes: u64,
    /// Flushes that wrote more than one record in one sink call.
    pub batched_flushes: u64,
    /// High-water mark of queued send bytes.
    pub peak_queued_bytes: usize,
    /// High-water mark of decoder bytes.
    pub peak_decoder_bytes: usize,
    /// Bulk chunks accepted from a peer.
    pub bulk_chunks_received: u64,
    /// Bulk chunks sent to a peer.
    pub bulk_chunks_sent: u64,
    /// Bulk transfers abandoned by a reset or link loss.
    pub bulk_aborts: u64,
}

/// Heartbeat scheduling for an established link.
#[derive(Clone, Copy, Debug)]
pub struct Heartbeat {
    interval_ms: u64,
    timeout_ms: u64,
    last_rx_ms: u64,
    last_tx_ms: u64,
}

impl Heartbeat {
    /// Create a heartbeat policy.
    pub const fn new(interval_ms: u64, timeout_ms: u64) -> Self {
        Self {
            interval_ms,
            timeout_ms,
            last_rx_ms: 0,
            last_tx_ms: 0,
        }
    }

    /// The M2 default policy.
    pub const fn m2() -> Self {
        Self::new(HEARTBEAT_INTERVAL_MS, HEARTBEAT_TIMEOUT_MS)
    }

    /// Start or restart the timers at `now_ms`.
    pub fn reset(&mut self, now_ms: u64) {
        self.last_rx_ms = now_ms;
        self.last_tx_ms = now_ms;
    }

    /// Note that bytes arrived.
    pub fn on_receive(&mut self, now_ms: u64) {
        self.last_rx_ms = now_ms;
    }

    /// Note that a heartbeat was queued or sent.
    pub fn on_transmit(&mut self, now_ms: u64) {
        self.last_tx_ms = now_ms;
    }

    /// Whether an idle link should send a heartbeat now.
    pub fn due(&self, now_ms: u64) -> bool {
        now_ms.saturating_sub(self.last_tx_ms) >= self.interval_ms
    }

    /// Whether the peer has been silent for the timeout.
    pub fn timed_out(&self, now_ms: u64) -> bool {
        now_ms.saturating_sub(self.last_rx_ms) >= self.timeout_ms
    }

    /// Milliseconds since the last received byte.
    pub fn silent_ms(&self, now_ms: u64) -> u64 {
        now_ms.saturating_sub(self.last_rx_ms)
    }
}

/// Exponential reconnect backoff with deterministic jitter.
///
/// The M2 guest has no connect service, so the listening side re-arms instead
/// of reconnecting.  The policy is still implemented here for the rig's mesh
/// relay and for the first transport that does have a connect primitive.
#[derive(Clone, Copy, Debug)]
pub struct Backoff {
    base_ms: u64,
    max_ms: u64,
    next_ms: u64,
    attempts: u32,
    seed: u64,
}

impl Backoff {
    /// Create a backoff with a base and cap.
    pub const fn new(base_ms: u64, max_ms: u64) -> Self {
        let base = at_least(base_ms, 1);
        let max = at_least(max_ms, 1);
        Self {
            base_ms: base,
            max_ms: max,
            next_ms: base,
            attempts: 0,
            seed: 0x9e37_79b9_7f4a_7c15,
        }
    }

    /// The M2 default backoff.
    pub const fn m2() -> Self {
        Self::new(RECONNECT_MIN_MS, RECONNECT_MAX_MS)
    }

    /// Return the next delay and advance the schedule.
    pub fn next_delay_ms(&mut self) -> u64 {
        let base = self.next_ms.min(self.max_ms);
        // Jitter is +/- 25% of the current base, deterministic for tests.
        let jitter = (base / 4).max(1);
        let random = self.next_random();
        let offset = random % (jitter * 2 + 1);
        let delay = base.saturating_sub(jitter).saturating_add(offset);
        self.next_ms = base.saturating_mul(2).min(self.max_ms);
        self.attempts = self.attempts.saturating_add(1);
        delay.min(self.max_ms)
    }

    /// Return to the first delay after a successful connection.
    pub fn reset(&mut self) {
        self.next_ms = self.base_ms;
        self.attempts = 0;
    }

    /// Number of delays returned since the last reset.
    pub fn attempts(&self) -> u32 {
        self.attempts
    }

    fn next_random(&mut self) -> u64 {
        let mut value = self.seed;
        value ^= value << 13;
        value ^= value >> 7;
        value ^= value << 17;
        self.seed = value;
        value
    }
}

/// One in-flight request tag.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Pending {
    /// The tag allocated for the request.
    pub tag: Tag,
    /// Absolute arrival deadline in milliseconds.
    pub deadline_ms: u64,
    /// The wire request id (the tag's numeric value in M2-1).
    pub request_id: u64,
}

/// The result of cancelling a tag.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CancelOutcome {
    /// The tag was in flight and has been released.
    Cancelled,
    /// The tag was unknown or had already completed/cancelled.
    Unknown,
}

/// A small in-flight request table with terminal tag release.
///
/// `cancel` releases the tag immediately because cancellation is a fabric
/// operation that ends with a terminal completion; a TCP reset is a link
/// failure and is handled by clearing the whole link, not by cancelling
/// individual tags.  `expire` performs the same release for deadlines.
#[derive(Clone, Debug)]
pub struct PendingTable {
    capacity: u32,
    next: u32,
    entries: Vec<Pending>,
}

impl PendingTable {
    /// Create a table with `capacity` usable tags.
    pub fn new(capacity: u32) -> Self {
        Self {
            capacity: capacity.max(1),
            next: 0,
            entries: Vec::new(),
        }
    }

    /// Allocate one tag with the given absolute deadline.
    pub fn begin_at(&mut self, deadline_ms: u64) -> Result<Tag, Error> {
        for _ in 0..=self.capacity {
            self.next = self.next.wrapping_add(1);
            if self.next == 0 {
                self.next = 1;
            }
            let candidate = self.next;
            if self.entries.len() as u32 >= self.capacity {
                return Err(Error::NoTag);
            }
            if self.entries.iter().all(|entry| entry.tag.0 != candidate) {
                let tag = Tag(candidate);
                self.entries.push(Pending {
                    tag,
                    deadline_ms,
                    request_id: u64::from(candidate),
                });
                return Ok(tag);
            }
        }
        Err(Error::NoTag)
    }

    /// Release a tag after its terminal completion.
    pub fn complete(&mut self, tag: Tag) -> bool {
        match self.position(tag) {
            Some(index) => {
                self.entries.remove(index);
                true
            }
            None => false,
        }
    }

    /// Cancel a tag and release it immediately.
    pub fn cancel(&mut self, tag: Tag) -> CancelOutcome {
        match self.position(tag) {
            Some(index) => {
                self.entries.remove(index);
                CancelOutcome::Cancelled
            }
            None => CancelOutcome::Unknown,
        }
    }

    /// Release every tag whose deadline has arrived.  Returns the number
    /// released.
    pub fn expire(&mut self, now_ms: u64) -> usize {
        let before = self.entries.len();
        self.entries.retain(|entry| entry.deadline_ms > now_ms);
        before - self.entries.len()
    }

    /// One in-flight entry, for deadline checks.
    pub fn get(&self, tag: Tag) -> Option<&Pending> {
        self.entries.iter().find(|entry| entry.tag == tag)
    }

    /// Number of in-flight tags.
    pub fn in_flight(&self) -> usize {
        self.entries.len()
    }

    /// Configured capacity.
    pub const fn capacity(&self) -> u32 {
        self.capacity
    }

    fn position(&self, tag: Tag) -> Option<usize> {
        self.entries.iter().position(|entry| entry.tag == tag)
    }
}

#[cfg(test)]
extern crate std;

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;
    use alloc::vec::Vec;
    use sys_primitives::{
        mesh_flags, MeshEnvelope, MessageFrame, MessageKind, NodeId, OpCode, StreamId,
    };

    #[derive(Default)]
    struct NullSink {
        bytes: usize,
    }

    impl ByteSink for NullSink {
        fn write(&mut self, data: &[u8]) -> WriteOutcome {
            self.bytes += data.len();
            WriteOutcome::Wrote(data.len())
        }
    }

    fn node(byte: u8) -> NodeId {
        NodeId([byte; 16])
    }

    fn envelope<'a>(
        src: NodeId,
        dst: NodeId,
        kind: MessageKind,
        opcode: OpCode,
        request_id: u64,
        body: &'a [u8],
    ) -> MeshEnvelope<'a> {
        let mut header = sys_primitives::MeshHeader::new(src, dst, src);
        header.flags = mesh_flags::PROBE;
        header.request_id = request_id;
        header.ttl = 8;
        let frame = MessageFrame::inline(kind, opcode, StreamId(1), Tag(1), body).unwrap();
        MeshEnvelope { header, frame }
    }

    fn encoded(envelope: &MeshEnvelope<'_>) -> Vec<u8> {
        let mut record = vec![0u8; record_len(envelope).unwrap()];
        let written = encode_record(envelope, &mut record).unwrap();
        assert_eq!(written, record.len());
        record
    }

    #[test]
    fn record_round_trip() {
        let source = envelope(
            node(1),
            node(2),
            MessageKind::Control,
            OpCode::Ping,
            7,
            b"hello",
        );
        let record = encoded(&source);
        assert_eq!(
            u32::from_le_bytes([record[0], record[1], record[2], record[3]]) as usize,
            record.len() - LENGTH_PREFIX_LEN
        );
        let decoded = decode_record(&record).unwrap();
        assert_eq!(decoded.header.src, node(1));
        assert_eq!(decoded.header.dst, node(2));
        assert_eq!(decoded.header.request_id, 7);
        assert_eq!(decoded.frame.body, b"hello");
    }

    #[test]
    fn partial_reads_assemble_one_record() {
        let source = envelope(
            node(1),
            node(2),
            MessageKind::Control,
            OpCode::Ping,
            0,
            b"partial",
        );
        let record = encoded(&source);
        let mut decoder = StreamDecoder::new();
        for byte in &record[..record.len() - 1] {
            let used = decoder.push(&[*byte]).unwrap();
            assert_eq!(used, 1);
            assert!(decoder.next_record().unwrap().is_none());
        }
        assert_eq!(decoder.push(&record[record.len() - 1..]).unwrap(), 1);
        let complete = decoder.next_record().unwrap().unwrap();
        assert_eq!(complete, &record[..]);
        assert_eq!(decode_record(complete).unwrap().frame.body, b"partial");
        decoder.consume_record().unwrap();
        assert_eq!(decoder.buffered_len(), 0);
        assert!(decoder.next_record().unwrap().is_none());
    }

    #[test]
    fn two_records_in_one_read_are_not_concatenated() {
        let first = encoded(&envelope(
            node(1),
            node(2),
            MessageKind::Control,
            OpCode::Ping,
            1,
            b"one",
        ));
        let second = encoded(&envelope(
            node(1),
            node(2),
            MessageKind::Control,
            OpCode::Ping,
            2,
            b"two",
        ));
        let mut input = first.clone();
        input.extend_from_slice(&second);

        let mut decoder = StreamDecoder::new();
        let used = decoder.push(&input).unwrap();
        assert_eq!(used, first.len());
        assert_eq!(decoder.next_record().unwrap().unwrap(), &first[..]);
        decoder.consume_record().unwrap();
        assert_eq!(decoder.buffered_len(), 0);

        let used = decoder.push(&input[used..]).unwrap();
        assert_eq!(used, second.len());
        assert_eq!(decoder.next_record().unwrap().unwrap(), &second[..]);
    }

    #[test]
    fn oversize_prefix_is_rejected_before_allocation() {
        let mut prefix = ((MAX_RECORD_LEN + 1) as u32).to_le_bytes().to_vec();
        prefix.extend_from_slice(&[0u8; 8]);
        let mut decoder = StreamDecoder::new();
        assert_eq!(decoder.push(&prefix), Err(Error::Oversize));
        assert!(decoder.is_poisoned());
        assert_eq!(decoder.push(&[0u8; 4]), Err(Error::Poisoned));
    }

    #[test]
    fn short_prefix_is_rejected() {
        let mut decoder = StreamDecoder::new();
        let prefix = ((MIN_ENVELOPE_LEN - 1) as u32).to_le_bytes().to_vec();
        assert_eq!(decoder.push(&prefix), Err(Error::TooShort));
    }

    #[test]
    fn short_envelope_is_rejected_by_complete_record_decoder() {
        let source = envelope(node(1), node(2), MessageKind::Control, OpCode::Ping, 0, b"");
        let record = encoded(&source);
        assert_eq!(
            decode_record(&record[..record.len() - 1]),
            Err(Error::ShortEnvelope)
        );
    }

    #[test]
    fn prefix_and_header_frame_len_must_agree() {
        let source = envelope(
            node(1),
            node(2),
            MessageKind::Control,
            OpCode::Ping,
            0,
            b"body",
        );
        let mut record = encoded(&source);
        // Keep the prefix but shorten the header's frame_len and recompute the
        // header CRC so the failure is the length disagreement, not the CRC.
        let mut header = source.header;
        header.frame_len = source.frame.encoded_len() as u32 - 1;
        record[LENGTH_PREFIX_LEN..LENGTH_PREFIX_LEN + MeshHeader::LEN]
            .copy_from_slice(&header.encode());
        assert!(matches!(
            decode_record(&record),
            Err(Error::Envelope(_)) | Err(Error::LengthMismatch)
        ));
    }

    #[test]
    fn bad_crc_and_bad_magic_are_rejected() {
        let source = envelope(
            node(1),
            node(2),
            MessageKind::Control,
            OpCode::Ping,
            0,
            b"crc",
        );
        let mut record = encoded(&source);
        // Flush a body byte; the header CRC still covers the header only, so
        // flip a header byte to hit the CRC check.
        record[LENGTH_PREFIX_LEN + 8] ^= 0xff;
        assert_eq!(
            decode_record(&record),
            Err(Error::Envelope(sys_primitives::Error::BadCrc))
        );

        let mut record = encoded(&source);
        record[LENGTH_PREFIX_LEN] ^= 0xff;
        assert_eq!(
            decode_record(&record),
            Err(Error::Envelope(sys_primitives::Error::BadMagic))
        );
    }

    #[test]
    fn extra_bytes_after_one_record_are_rejected() {
        let source = envelope(node(1), node(2), MessageKind::Control, OpCode::Ping, 0, b"");
        let mut record = encoded(&source);
        record.push(0);
        assert_eq!(decode_record(&record), Err(Error::LengthMismatch));
    }

    struct ShortSink {
        limit: usize,
        written: Vec<u8>,
    }

    impl ByteSink for ShortSink {
        fn write(&mut self, data: &[u8]) -> WriteOutcome {
            let count = data.len().min(self.limit);
            if count == 0 {
                return WriteOutcome::WouldBlock;
            }
            self.written.extend_from_slice(&data[..count]);
            WriteOutcome::Wrote(count)
        }
    }

    #[test]
    fn short_writes_preserve_the_remainder() {
        let first = encoded(&envelope(
            node(1),
            node(2),
            MessageKind::Control,
            OpCode::Ping,
            1,
            b"one",
        ));
        let second = encoded(&envelope(
            node(1),
            node(2),
            MessageKind::Control,
            OpCode::Ping,
            2,
            b"two",
        ));
        let mut queue = SendQueue::new();
        queue
            .push_record(
                &envelope(
                    node(1),
                    node(2),
                    MessageKind::Control,
                    OpCode::Ping,
                    1,
                    b"one",
                ),
                0,
            )
            .unwrap();
        queue
            .push_record(
                &envelope(
                    node(1),
                    node(2),
                    MessageKind::Control,
                    OpCode::Ping,
                    2,
                    b"two",
                ),
                0,
            )
            .unwrap();
        let mut sink = ShortSink {
            limit: 3,
            written: Vec::new(),
        };
        let mut total_frames = 0;
        loop {
            let report = queue.flush(&mut sink);
            total_frames += report.frames_written;
            assert!(report.bytes_written <= 3 * 1024);
            if queue.pending_bytes() == 0 {
                assert!(!report.blocked);
                break;
            }
            assert!(report.blocked);
        }
        assert_eq!(total_frames, 2);
        let mut expected = first;
        expected.extend_from_slice(&second);
        assert_eq!(sink.written, expected);
    }

    #[test]
    fn would_block_keeps_the_queue_until_retry() {
        struct Gate {
            open: bool,
            written: Vec<u8>,
        }
        impl ByteSink for Gate {
            fn write(&mut self, data: &[u8]) -> WriteOutcome {
                if !self.open {
                    return WriteOutcome::WouldBlock;
                }
                self.written.extend_from_slice(data);
                WriteOutcome::Wrote(data.len())
            }
        }
        let source = envelope(
            node(1),
            node(2),
            MessageKind::Control,
            OpCode::Ping,
            1,
            b"blocked",
        );
        let expected = encoded(&source);
        let mut queue = SendQueue::new();
        queue.push_record(&source, 100).unwrap();
        let mut gate = Gate {
            open: false,
            written: Vec::new(),
        };
        let report = queue.flush(&mut gate);
        assert!(report.blocked);
        assert_eq!(queue.pending_frames(), 1);
        assert!(queue.send_deadline_expired(100 + SEND_DEADLINE_MS));
        gate.open = true;
        let report = queue.flush(&mut gate);
        assert_eq!(report.frames_written, 1);
        assert_eq!(queue.pending_bytes(), 0);
        assert_eq!(gate.written, expected);
    }

    #[test]
    fn send_queue_rejects_oversize_work() {
        let mut queue = SendQueue::new();
        // A legal record is far below MAX_QUEUED_BYTES.  Fill the queue with
        // repeated records until it reports backpressure.
        let source = envelope(node(1), node(2), MessageKind::Control, OpCode::Ping, 1, b"");
        let mut saw_full = false;
        for _ in 0..1_000 {
            match queue.push_record(&source, 0) {
                Ok(()) => {}
                Err(Error::QueueFull) => {
                    saw_full = true;
                    break;
                }
                Err(error) => panic!("unexpected error: {error:?}"),
            }
        }
        assert!(saw_full);
        assert!(queue.pending_bytes() <= MAX_QUEUED_BYTES);
    }

    #[test]
    fn backoff_grows_caps_and_jitters() {
        let mut backoff = Backoff::new(100, 2_000);
        let mut delays = Vec::new();
        for _ in 0..12 {
            delays.push(backoff.next_delay_ms());
        }
        assert_eq!(backoff.attempts(), 12);
        assert!(delays[0] >= 75 && delays[0] <= 125);
        assert!(delays.iter().all(|delay| *delay <= 2_000));
        assert!(delays.windows(2).any(|pair| pair[0] != pair[1]));
        assert!(delays.iter().any(|delay| *delay >= 1_500));
        backoff.reset();
        assert_eq!(backoff.attempts(), 0);
        assert!(backoff.next_delay_ms() <= 125);
    }

    #[test]
    fn heartbeat_detects_idle_link() {
        let mut heartbeat = Heartbeat::m2();
        heartbeat.reset(1_000);
        assert!(!heartbeat.due(1_000 + HEARTBEAT_INTERVAL_MS - 1));
        assert!(heartbeat.due(1_000 + HEARTBEAT_INTERVAL_MS));
        assert!(!heartbeat.timed_out(1_000 + HEARTBEAT_TIMEOUT_MS - 1));
        assert!(heartbeat.timed_out(1_000 + HEARTBEAT_TIMEOUT_MS));
        heartbeat.on_receive(5_000);
        assert_eq!(heartbeat.silent_ms(5_500), 500);
    }

    #[test]
    fn cancellation_releases_a_tag_once() {
        let mut pending = PendingTable::new(4);
        let tag = pending.begin_at(10_000).unwrap();
        assert_eq!(pending.in_flight(), 1);
        assert!(pending.get(tag).is_some());
        assert_eq!(pending.cancel(tag), CancelOutcome::Cancelled);
        assert_eq!(pending.in_flight(), 0);
        assert_eq!(pending.cancel(tag), CancelOutcome::Unknown);
        assert!(!pending.complete(tag));
        assert_eq!(pending.expire(20_000), 0);
    }

    #[test]
    fn completion_and_expiry_release_tags() {
        let mut pending = PendingTable::new(2);
        let first = pending.begin_at(5_000).unwrap();
        let second = pending.begin_at(1_000).unwrap();
        assert_eq!(pending.in_flight(), 2);
        assert!(pending.complete(first));
        assert!(!pending.complete(first));
        assert_eq!(pending.expire(1_000), 1);
        assert_eq!(pending.in_flight(), 0);
        assert_eq!(pending.expire(1_000), 0);
        assert!(pending.begin_at(2_000).is_ok());
        assert!(pending.begin_at(2_000).is_ok());
        assert_eq!(pending.begin_at(2_000), Err(Error::NoTag));
        let _ = second;
    }

    #[test]
    fn decoder_reset_drops_a_partial_record() {
        let source = envelope(
            node(1),
            node(2),
            MessageKind::Control,
            OpCode::Ping,
            0,
            b"reset",
        );
        let record = encoded(&source);
        let mut decoder = StreamDecoder::new();
        assert_eq!(decoder.push(&record[..5]).unwrap(), 5);
        assert_eq!(decoder.buffered_len(), 5);
        decoder.reset();
        assert_eq!(decoder.buffered_len(), 0);
        assert!(decoder.next_record().unwrap().is_none());
        assert_eq!(decoder.push(&record).unwrap(), record.len());
        assert!(decoder.next_record().unwrap().is_some());
    }

    #[test]
    fn decoder_hard_limit_rejects_a_larger_record_without_growth() {
        let body = vec![0x5a; 8 * 1024];
        let source = envelope(
            node(1),
            node(2),
            MessageKind::Request,
            OpCode::Read,
            7,
            &body,
        );
        let record = encoded(&source);
        let mut decoder = StreamDecoder::with_limit(1024, 1024);
        assert_eq!(decoder.limit(), 1024);
        assert_eq!(decoder.push(&record[..4]), Err(Error::DecoderFull));
        assert!(decoder.is_poisoned());
        assert_eq!(decoder.buffered_len(), 4);
    }

    #[test]
    fn send_queue_compacts_and_keeps_its_capacity() {
        let mut queue = SendQueue::with_capacity(8 * 1024);
        let capacity = queue.capacity();
        let source = envelope(
            node(1),
            node(2),
            MessageKind::Control,
            OpCode::Ping,
            1,
            b"compact",
        );
        for _ in 0..8 {
            queue.push_record(&source, 0).unwrap();
        }
        let mut sink = NullSink::default();
        let report = queue.flush(&mut sink);
        assert_eq!(report.frames_written, 8);
        assert_eq!(queue.pending_bytes(), 0);
        for _ in 0..8 {
            queue.push_record(&source, 0).unwrap();
        }
        assert_eq!(queue.capacity(), capacity);
        assert!(queue.peak_pending() <= capacity);
    }

    #[test]
    fn data_cannot_consume_the_control_reserve() {
        let mut queue = SendQueue::with_reserve(16 * 1024, 8 * 1024);
        let body = vec![0u8; 7 * 1024];
        let data = envelope(
            node(1),
            node(2),
            MessageKind::Response,
            OpCode::Read,
            9,
            &body,
        );
        queue.push_data_record(&data, 0).unwrap();
        // A second data record would cross the 8 KiB data limit.
        assert_eq!(queue.push_data_record(&data, 0), Err(Error::QueueFull));
        let control = envelope(
            node(1),
            node(2),
            MessageKind::Control,
            OpCode::Ping,
            1,
            b"control",
        );
        queue.push_record(&control, 0).unwrap();
        let mut sink = NullSink::default();
        let report = queue.flush(&mut sink);
        assert_eq!(report.frames_written, 2);
    }

    #[test]
    fn flush_batches_many_records_in_one_write() {
        struct CountingSink {
            writes: usize,
            bytes: usize,
        }
        impl ByteSink for CountingSink {
            fn write(&mut self, data: &[u8]) -> WriteOutcome {
                self.writes += 1;
                self.bytes += data.len();
                WriteOutcome::Wrote(data.len())
            }
        }
        let mut queue = SendQueue::with_capacity(16 * 1024);
        let body = [0u8; 16];
        for tag in 1..=8 {
            let source = envelope(
                node(1),
                node(2),
                MessageKind::Control,
                OpCode::Ping,
                tag,
                &body,
            );
            queue.push_record(&source, 0).unwrap();
        }
        let mut sink = CountingSink {
            writes: 0,
            bytes: 0,
        };
        let report = queue.flush(&mut sink);
        assert_eq!(report.frames_written, 8);
        assert_eq!(report.writes, 1);
        assert_eq!(sink.writes, 1);
        assert_eq!(sink.bytes, 8 * 340);
    }
}
