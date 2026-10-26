// SPDX-License-Identifier: GPL-3.0-only
//! Two-node frame-path test for W3C trace-context propagation.
//!
//! Every record crosses the real `MeshEnvelope` + `mesh-transport` path: a
//! `Wire` is wrapped in a `MessageFrame`, encoded as a length-prefixed record,
//! fed through a `StreamDecoder`, and only then handed to the peer's
//! `RouteNode::on_wire`. The request trace id must survive to the response and
//! back to the sender's completion.

use cap_engine::SigningAuthority;
use mesh_route::{Event, Reason, RouteNode, Wire, OP_STAT};
use mesh_transport::{decode_record, encode_record, record_len, StreamDecoder};
use sys_primitives::{
    flags, FrameHeader, MeshEnvelope, MeshHeader, MessageFrame, NodeId, ObjectId, StreamId, Tag,
    TraceContext,
};

const A: NodeId = NodeId([
    0x08, 0x00, 0x69, 0x95, 0xf5, 0x00, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
]);
const B: NodeId = NodeId([
    0x08, 0x00, 0x69, 0x69, 0xb6, 0x01, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
]);
const DIGEST: [u8; 16] = [
    0x2e, 0xda, 0x13, 0xc5, 0xac, 0x6f, 0x72, 0x61, 0x32, 0xe0, 0x47, 0x9d, 0xfe, 0x60, 0x45, 0xb7,
];

struct Nodes {
    a: RouteNode,
    b: RouteNode,
    signer: SigningAuthority,
    object: ObjectId,
}

impl Nodes {
    fn new() -> Self {
        let signer = SigningAuthority::from_bytes(&[7u8; 32]);
        let key = signer.public();
        let mut a = RouteNode::new(A, A, key, true, 8).expect("route node A");
        let mut b = RouteNode::new(B, A, key, false, 8).expect("route node B");
        assert!(a.on_snapshot(2, &[A, B], DIGEST, 0).is_some());
        assert!(b.on_snapshot(2, &[A, B], DIGEST, 0).is_some());
        assert!(a.set_peer(B, 0).is_none());
        assert!(b.set_peer(A, 0).is_none());
        let object = a.first_home(B).expect("an object homed at B");
        Self {
            a,
            b,
            signer,
            object,
        }
    }
}

struct Decoded {
    mesh: MeshHeader,
    frame: FrameHeader,
    body: Vec<u8>,
}

fn record_for(wire: &Wire, src: NodeId) -> Vec<u8> {
    let mut frame = MessageFrame::inline(
        wire.kind,
        wire.opcode,
        StreamId(3),
        Tag(wire.request_id as u32),
        &wire.body,
    )
    .expect("frame");
    frame.header.object = wire.object;
    frame.header.status = wire.status;
    wire.apply_trace(&mut frame.header);
    let envelope = MeshEnvelope {
        header: wire.header(src),
        frame,
    };
    let mut record = vec![0u8; record_len(&envelope).expect("record length")];
    let written = encode_record(&envelope, &mut record).expect("encode record");
    assert_eq!(written, record.len());
    record
}

fn feed(decoder: &mut StreamDecoder, bytes: &[u8]) -> Vec<Decoded> {
    let mut decoded = Vec::new();
    let mut offset = 0usize;
    loop {
        if let Some(record) = decoder.next_record().expect("decoder") {
            let entry = {
                let envelope = decode_record(record).expect("decode record");
                Decoded {
                    mesh: envelope.header,
                    frame: envelope.frame.header,
                    body: envelope.frame.body.to_vec(),
                }
            };
            decoder.consume_record().expect("consume record");
            decoded.push(entry);
            continue;
        }
        if offset >= bytes.len() {
            return decoded;
        }
        offset += decoder.push(&bytes[offset..]).expect("push bytes");
    }
}

fn trace_of(frame: &FrameHeader) -> Option<TraceContext> {
    if frame.flags & flags::TRACE == 0 {
        return None;
    }
    Some(TraceContext {
        trace_id: frame.trace_id,
        span_id: frame.span_id,
        flags: frame.trace_flags,
    })
}

fn exchange(nodes: &mut Nodes, trace: TraceContext) -> Vec<Event> {
    let step = nodes
        .a
        .begin_operation_traced(nodes.object, OP_STAT, trace, 1_000, &nodes.signer);
    let request = step.send.expect("traced request");
    assert_eq!(request.trace, Some(trace));
    assert!(
        request.trace.map(|context| context.trace_id) == Some(trace.trace_id),
        "the request Wire must carry the trace"
    );

    // Deliberately feed one byte at a time: the trace has to survive the real
    // stream decoder, not a direct struct hand-off.
    let record = record_for(&request, A);
    let mut decoder = StreamDecoder::new();
    let mut decoded = Vec::new();
    for byte in &record {
        decoded.extend(feed(&mut decoder, &[*byte]));
    }
    assert_eq!(decoded.len(), 1, "one request record");
    let request = decoded.pop().expect("request");

    let request_trace = trace_of(&request.frame);
    assert_eq!(
        request_trace.map(|context| context.trace_id),
        Some(trace.trace_id),
        "request trace after the frame path"
    );
    assert_eq!(
        request_trace.map(|context| context.span_id),
        Some(trace.span_id)
    );

    let step = nodes.b.on_wire(
        A,
        &request.mesh,
        request_trace,
        request.frame.object,
        request.frame.kind,
        request.frame.opcode,
        request.frame.status,
        &request.body,
        1_000,
        &nodes.signer,
    );
    let response = step.send.expect("response");
    let response_trace = response.trace.expect("response trace");
    assert_eq!(response_trace.trace_id, trace.trace_id);
    assert_ne!(
        response_trace.span_id, trace.span_id,
        "a response is a child span"
    );

    let record = record_for(&response, B);
    let mut decoder = StreamDecoder::new();
    let decoded = feed(&mut decoder, &record);
    assert_eq!(decoded.len(), 1, "one response record");
    let response = &decoded[0];
    assert_eq!(
        trace_of(&response.frame).map(|context| context.trace_id),
        Some(trace.trace_id),
        "response trace after the frame path"
    );

    let step = nodes.a.on_wire(
        B,
        &response.mesh,
        trace_of(&response.frame),
        response.frame.object,
        response.frame.kind,
        response.frame.opcode,
        response.frame.status,
        &response.body,
        1_000,
        &nodes.signer,
    );
    step.events
}

#[test]
fn trace_id_follows_request_and_response_across_two_nodes() {
    let mut nodes = Nodes::new();
    let trace = TraceContext {
        trace_id: [0x11; 16],
        span_id: [0x22; 8],
        flags: 1,
    };
    let events = exchange(&mut nodes, trace);
    assert!(events.iter().any(|event| matches!(
        event,
        Event::Result {
            object,
            ..
        } if *object == nodes.object
    )));
}

#[test]
fn response_with_a_different_trace_id_is_refused() {
    let mut nodes = Nodes::new();
    let trace = TraceContext {
        trace_id: [0x33; 16],
        span_id: [0x44; 8],
        flags: 1,
    };

    let step = nodes
        .a
        .begin_operation_traced(nodes.object, OP_STAT, trace, 1_000, &nodes.signer);
    let request = step.send.expect("request");
    let record = record_for(&request, A);
    let mut decoder = StreamDecoder::new();
    let decoded = feed(&mut decoder, &record);
    let request = &decoded[0];

    let step = nodes.b.on_wire(
        A,
        &request.mesh,
        trace_of(&request.frame),
        request.frame.object,
        request.frame.kind,
        request.frame.opcode,
        request.frame.status,
        &request.body,
        1_000,
        &nodes.signer,
    );
    let mut response = step.send.expect("response");
    response.trace = Some(TraceContext {
        trace_id: [0x99; 16],
        span_id: [0xaa; 8],
        flags: 1,
    });

    let record = record_for(&response, B);
    let mut decoder = StreamDecoder::new();
    let decoded = feed(&mut decoder, &record);
    let response = &decoded[0];
    let step = nodes.a.on_wire(
        B,
        &response.mesh,
        trace_of(&response.frame),
        response.frame.object,
        response.frame.kind,
        response.frame.opcode,
        response.frame.status,
        &response.body,
        1_000,
        &nodes.signer,
    );
    assert!(step.events.iter().any(|event| matches!(
        event,
        Event::Reject {
            reason: Reason::TraceMismatch,
            ..
        }
    )));
    assert!(!step
        .events
        .iter()
        .any(|event| matches!(event, Event::Result { .. })));
}
