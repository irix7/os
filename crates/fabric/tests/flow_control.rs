// SPDX-License-Identifier: GPL-3.0-only
//! Flow-control exhaustion against a slow in-process consumer.
//!
//! The sender uses the fabric stream window and the real `mesh-transport`
//! send queue; the receiver uses the real `StreamDecoder` and record decoder.
//! Records only move as bytes through the queue and the decoder, so a dropped
//! or reordered record fails the body check at the end.

use fabric::Stream;
use mesh_transport::{
    decode_record, encode_record, record_len, ByteSink, Error as TransportError, SendQueue,
    StreamDecoder, WriteOutcome, MAX_QUEUED_BYTES, SEND_DEADLINE_MS,
};
use sys_primitives::{
    MeshEnvelope, MeshHeader, MessageFrame, MessageKind, NodeId, OpCode, StreamId, Tag,
};

const A: NodeId = NodeId([0x0a; 16]);
const B: NodeId = NodeId([0x0b; 16]);
const STREAM: StreamId = StreamId(7);

struct Request {
    index: u32,
    body: Vec<u8>,
    record: Vec<u8>,
}

impl Request {
    fn new(index: u32) -> Self {
        let body = vec![(index & 0xff) as u8; 256];
        let mut request = Self {
            index,
            body,
            record: Vec::new(),
        };
        let envelope = request.envelope();
        let mut record = vec![0u8; record_len(&envelope).expect("record length")];
        let written = encode_record(&envelope, &mut record).expect("encode record");
        assert_eq!(written, record.len());
        request.record = record;
        request
    }

    fn envelope(&self) -> MeshEnvelope<'_> {
        let mut header = MeshHeader::new(A, B, B);
        header.request_id = u64::from(self.index) + 1;
        let frame = MessageFrame::inline(
            MessageKind::Request,
            OpCode::Invoke,
            STREAM,
            Tag(self.index + 1),
            &self.body,
        )
        .expect("frame");
        MeshEnvelope { header, frame }
    }
}

/// Accepts a few bytes at a time, then reports backpressure. The budget is
/// replenished only when the test explicitly lets the consumer run.
struct SlowSink {
    budget: usize,
    bytes: Vec<u8>,
}

impl SlowSink {
    fn new() -> Self {
        Self {
            budget: 0,
            bytes: Vec::new(),
        }
    }
}

impl ByteSink for SlowSink {
    fn write(&mut self, data: &[u8]) -> WriteOutcome {
        if self.budget == 0 {
            return WriteOutcome::WouldBlock;
        }
        let count = data.len().min(self.budget).min(64);
        self.budget -= count;
        self.bytes.extend_from_slice(&data[..count]);
        WriteOutcome::Wrote(count)
    }
}

fn feed(decoder: &mut StreamDecoder, bytes: &[u8], bodies: &mut Vec<Vec<u8>>) {
    let mut offset = 0usize;
    loop {
        if let Some(record) = decoder.next_record().expect("decoder") {
            let body = {
                let envelope = decode_record(record).expect("record");
                envelope.frame.body.to_vec()
            };
            decoder.consume_record().expect("consume");
            bodies.push(body);
            continue;
        }
        if offset >= bytes.len() {
            return;
        }
        offset += decoder.push(&bytes[offset..]).expect("push");
    }
}

fn pump(
    queue: &mut SendQueue,
    sink: &mut SlowSink,
    decoder: &mut StreamDecoder,
    bodies: &mut Vec<Vec<u8>>,
) {
    sink.budget = 64;
    let _ = queue.flush(sink);
    let bytes = core::mem::take(&mut sink.bytes);
    feed(decoder, &bytes, bodies);
}

#[test]
fn send_queue_exhausts_against_a_slow_consumer_and_recovers() {
    let requests: Vec<Request> = (0..300).map(Request::new).collect();
    let mut queue = SendQueue::new();
    let mut sink = SlowSink::new();
    let mut decoder = StreamDecoder::new();
    let mut bodies = Vec::new();
    let now = 1_000u64;
    let mut full = false;
    let mut index = 0usize;

    while index < requests.len() {
        match queue.push_record(&requests[index].envelope(), now) {
            Ok(()) => index += 1,
            Err(TransportError::QueueFull) => {
                full = true;
                assert!(
                    queue.send_deadline_expired(now + SEND_DEADLINE_MS),
                    "a blocked queue must expose its send deadline"
                );
                pump(&mut queue, &mut sink, &mut decoder, &mut bodies);
            }
            Err(error) => panic!("unexpected send-queue error: {error:?}"),
        }
        assert!(queue.pending_bytes() <= MAX_QUEUED_BYTES);
        if queue.pending_bytes() > 0 {
            pump(&mut queue, &mut sink, &mut decoder, &mut bodies);
        }
    }

    while queue.pending_bytes() > 0 {
        pump(&mut queue, &mut sink, &mut decoder, &mut bodies);
    }

    assert!(full, "the slow consumer must fill the bounded queue");
    assert_eq!(queue.pending_bytes(), 0);
    assert_eq!(bodies.len(), requests.len());
    for (body, request) in bodies.iter().zip(&requests) {
        assert_eq!(body, &request.body, "record {}", request.index);
    }
}

#[test]
fn stream_window_exhausts_and_resumes_after_a_window_update() {
    const WINDOW: u32 = 2_000;
    let requests: Vec<Request> = (0..32).map(Request::new).collect();
    let mut sender = Stream::new(STREAM, WINDOW);
    let mut receiver = Stream::new(STREAM, WINDOW);
    let mut blocked = false;

    for request in &requests {
        let length = request.record.len() as u32;
        if !sender.consume_send(length) {
            blocked = true;
            let before = sender.send_window;
            assert!(
                !sender.consume_send(length),
                "a failed consume must not move the window"
            );
            assert_eq!(sender.send_window, before);

            // The receiver drained the bytes in flight and grants the same
            // credit back. This is the window-update path, not an unbounded
            // reset of the window.
            let consumed = WINDOW - before;
            assert!(receiver.consume_receive(consumed));
            receiver.grant_receive(consumed);
            sender.grant_send(consumed);
            assert!(sender.consume_send(length));
        }
    }

    assert!(blocked, "the stream window must bind before the queue does");
    assert!(sender.send_window < WINDOW);
}
