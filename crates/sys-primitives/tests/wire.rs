// SPDX-License-Identifier: GPL-3.0-only
//! Host-side checks for the fabric wire contract.

use core::mem::{offset_of, size_of};
use sys_primitives::{
    CapHandle, Capability, Completion, CompletionId, CompletionKind, FrameHeader, MeshEnvelope,
    MeshHeader, MessageFrame, MessageKind, NodeId, NodeKey, ObjectId, OpCode, PayloadDescriptor,
    PrincipalId, Rights, Status, StreamId, Tag, TraceContext,
};

#[test]
fn layout_is_pinned() {
    assert_eq!(size_of::<ObjectId>(), 32);
    assert_eq!(size_of::<NodeKey>(), 32);
    assert_eq!(size_of::<NodeId>(), 16);
    assert_eq!(size_of::<CapHandle>(), 8);
    assert_eq!(size_of::<Capability>(), 320);
    assert_eq!(offset_of!(Capability, object), 8);
    assert_eq!(offset_of!(Capability, home), 104);
    assert_eq!(offset_of!(Capability, delegation), 200);
    assert_eq!(offset_of!(Capability, signature), 256);
    assert_eq!(size_of::<FrameHeader>(), 128);
    assert_eq!(offset_of!(FrameHeader, object), 40);
    assert_eq!(offset_of!(FrameHeader, header_crc32c), 124);
    assert_eq!(size_of::<PayloadDescriptor>(), 64);
    assert_eq!(size_of::<MeshHeader>(), 128);
    assert_eq!(offset_of!(MeshHeader, src), 8);
    assert_eq!(offset_of!(MeshHeader, home), 40);
    assert_eq!(offset_of!(MeshHeader, frame_len), 108);
    assert_eq!(size_of::<Completion>(), 32);
}

#[test]
fn frame_round_trip_and_crc() {
    let object = ObjectId([0x42; 32]);
    let mut frame = MessageFrame::inline(
        MessageKind::Request,
        OpCode::Read,
        StreamId(7),
        Tag(9),
        b"hello fabric",
    )
    .expect("frame");
    frame.header.cap = CapHandle::new(3, 11);
    frame.header.object = object;
    frame.header.offset = 4096;
    frame.header.trace_id = [0xab; 16];
    frame.header.span_id = [0xcd; 8];
    frame.header.trace_flags = 1;
    frame.header.flags |= sys_primitives::flags::TRACE | sys_primitives::flags::CAP;

    let mut bytes = [0u8; 256];
    let written = frame.encode(&mut bytes).expect("encode");
    assert_eq!(written, frame.encoded_len());
    let (decoded, consumed) = MessageFrame::decode(&bytes[..written]).expect("decode");
    assert_eq!(consumed, written);
    assert_eq!(decoded.body, b"hello fabric");
    assert_eq!(decoded.header.tag, Tag(9));
    assert_eq!(decoded.header.object, object);
    assert_eq!(decoded.header.cap, CapHandle::new(3, 11));

    let mut corrupt = bytes;
    corrupt[40] ^= 1;
    assert!(MessageFrame::decode(&corrupt[..written]).is_err());
}

#[test]
fn capability_round_trip_and_window() {
    let object = ObjectId([0x42; 32]);
    let token = Capability {
        magic: Capability::MAGIC,
        version: Capability::VERSION,
        rights: Rights::READ.union(Rights::INVOKE),
        flags: 0,
        object,
        issuer: NodeKey([1; 32]),
        subject: PrincipalId([2; 32]),
        home: NodeId([5; 16]),
        not_before: 100,
        not_after: 200,
        object_epoch: 1,
        home_epoch: 2,
        root_epoch: 3,
        reserved0: 0,
        nonce: [3; 16],
        parent: ObjectId::ZERO,
        delegation: ObjectId([6; 32]),
        reserved1: [0; 24],
        signature: [4; 64],
    };
    let encoded = token.encode();
    let decoded = Capability::decode(&encoded).expect("capability");
    assert_eq!(decoded, token);
    assert!(decoded.is_valid_at(150));
    assert!(!decoded.is_valid_at(250));
}

#[test]
fn mesh_envelope_round_trip() {
    let frame = MessageFrame::inline(
        MessageKind::Request,
        OpCode::Ping,
        StreamId(1),
        Tag(2),
        b"mesh",
    )
    .expect("frame");
    let mesh = MeshEnvelope {
        header: MeshHeader::new(NodeId([0x11; 16]), NodeId([0x22; 16]), NodeId([0x22; 16])),
        frame,
    };
    let mut bytes = [0u8; 512];
    let written = mesh.encode(&mut bytes).expect("mesh encode");
    let (decoded, consumed) = MeshEnvelope::decode(&bytes[..written]).expect("mesh decode");
    assert_eq!(consumed, written);
    assert_eq!(decoded.header.src, NodeId([0x11; 16]));
    assert_eq!(decoded.header.dst, NodeId([0x22; 16]));
    assert_eq!(decoded.header.home, NodeId([0x22; 16]));
    assert_eq!(decoded.frame.header.tag, Tag(2));
    assert_eq!(decoded.frame.body, b"mesh");

    let mut corrupt = bytes;
    corrupt[40] ^= 1;
    assert!(MeshEnvelope::decode(&corrupt[..written]).is_err());
}

#[test]
fn completion_and_trace_round_trip() {
    let completion = Completion {
        event: CompletionId(1),
        tag: Tag(9),
        status: Status::Ok,
        kind: CompletionKind::Bytes,
        flags: 0,
        reserved: 0,
        value0: 13,
        value1: 0,
    };
    let bytes = completion.encode();
    assert_eq!(Completion::decode(&bytes).expect("completion"), completion);

    let trace = TraceContext {
        trace_id: [0x11; 16],
        span_id: [0x22; 8],
        flags: 1,
    };
    let mut parent = [0u8; 55];
    trace.traceparent(&mut parent).expect("traceparent");
    assert_eq!(
        &parent[..],
        b"00-11111111111111111111111111111111-2222222222222222-01"
    );
}
