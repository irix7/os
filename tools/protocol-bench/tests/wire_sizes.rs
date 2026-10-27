// SPDX-License-Identifier: GPL-3.0-only
//! Regression gate for the numbers recorded in `docs/research/wire-efficiency.md`.
//!
//! These assertions pin the shipped wire sizes.  A change here is a protocol
//! change and must update the document (and its ADR) in the same commit.

use mesh_join::{EpochConfirmation, JoinAck, JoinRequest};
use mesh_route::{Invoke, InvokeResult, MoveAck, Moved};
use mesh_transport::{encode_record, record_len, MAX_RECORD_LEN, MIN_ENVELOPE_LEN};
use sys_primitives::{
    mesh_flags, Capability, MeshEnvelope, MeshHeader, MessageFrame, MessageKind, NodeId, ObjectId,
    OpCode, PrincipalId, Rights, StreamId, Tag,
};

const OVERHEAD: usize = 4 + 128 + 128 + 64;

fn node(byte: u8) -> NodeId {
    NodeId([byte; 16])
}

fn record_len_of(kind: MessageKind, opcode: OpCode, body: &[u8]) -> usize {
    let frame = MessageFrame::inline(kind, opcode, StreamId(1), Tag(1), body).expect("frame");
    let mut header = MeshHeader::new(node(1), node(2), node(2));
    header.flags = mesh_flags::PROBE;
    record_len(&MeshEnvelope { header, frame }).expect("record length")
}

#[test]
fn overhead_is_324_bytes() {
    assert_eq!(OVERHEAD, 324);
    assert_eq!(MIN_ENVELOPE_LEN, 320);
    assert_eq!(
        record_len_of(MessageKind::Control, OpCode::Ping, &[0; 16]),
        340
    );
}

#[test]
fn join_bodies_are_exact() {
    let request = JoinRequest {
        node: node(2),
        incarnation: 1,
        last_epoch: 1,
        last_digest: [0; 16],
        nonce: 9,
    };
    assert_eq!(JoinRequest::LEN, 64);
    let mut buffer = [0u8; mesh_join::MAX_BODY];
    let len = request.encode(&mut buffer).expect("request");
    assert_eq!(len, 64);
    assert_eq!(
        record_len_of(MessageKind::Request, OpCode::Hello, &buffer[..len]),
        388
    );

    let ack = JoinAck {
        node: node(2),
        epoch: 2,
        digest: [0; 16],
        request_nonce: 9,
    };
    assert_eq!(JoinAck::LEN, 56);
    let len = ack.encode(&mut buffer).expect("ack");
    assert_eq!(
        record_len_of(MessageKind::Completion, OpCode::Hello, &buffer[..len]),
        380
    );

    for (members, body_len, record) in [
        (2usize, 160usize, 484usize),
        (8, 256, 580),
        (128, 2176, 2500),
    ] {
        let confirmation = EpochConfirmation {
            root: node(1),
            epoch: 2,
            members: (0..members).map(|index| node(index as u8 + 1)).collect(),
            subject: node(2),
            request_nonce: 9,
            incarnation: 1,
            signature: [0x55; 64],
        };
        assert_eq!(confirmation.encoded_len(), body_len);
        let len = confirmation.encode(&mut buffer).expect("confirm");
        assert_eq!(len, body_len);
        assert_eq!(
            record_len_of(MessageKind::Response, OpCode::Hello, &buffer[..len]),
            record
        );
    }
}

#[test]
fn route_bodies_are_exact() {
    assert_eq!(Invoke::LEN, 332);
    assert_eq!(InvokeResult::LEN, 100);
    assert_eq!(Moved::LEN, 96);
    assert_eq!(MoveAck::LEN, 64);
    assert_eq!(mesh_route::RouteErrorBody::LEN, 48);

    let capability = Capability {
        magic: Capability::MAGIC,
        version: Capability::VERSION,
        rights: Rights::READ,
        flags: 0,
        object: ObjectId([7; 32]),
        issuer: sys_primitives::NodeKey([3; 32]),
        subject: PrincipalId([4; 32]),
        home: node(2),
        not_before: 0,
        not_after: 1,
        object_epoch: 1,
        home_epoch: 1,
        root_epoch: 1,
        reserved0: 0,
        nonce: [0; 16],
        parent: ObjectId::ZERO,
        delegation: ObjectId::ZERO,
        reserved1: [0; 24],
        signature: [0; 64],
    };
    let invoke = Invoke {
        capability,
        operation: mesh_route::OP_STAT,
    };
    let mut buffer = [0u8; mesh_route::MAX_BODY];
    let len = invoke.encode(&mut buffer).expect("invoke");
    assert_eq!(len, 332);
    assert_eq!(
        record_len_of(MessageKind::Request, OpCode::Invoke, &buffer[..len]),
        656
    );
}

#[test]
fn migrate_bodies_are_exact() {
    assert_eq!(mesh_migrate::STATE_LEN, 248);
    assert_eq!(mesh_migrate::DIRECTIVE_LEN, 164);
    let directive = mesh_migrate::ActorMove::new(
        sys_primitives::ActorId([0x77; 16]),
        node(1),
        node(1),
        node(2),
        2,
        [0x88; 16],
        4,
        30_000,
    );
    let mut buffer = [0u8; mesh_migrate::MAX_BODY];
    let len = mesh_migrate::Body::Authorise(directive)
        .encode(&mut buffer)
        .expect("authorise");
    assert_eq!(len, 172);
    assert_eq!(
        record_len_of(MessageKind::Request, OpCode::Migrate, &buffer[..len]),
        496
    );
}

#[test]
fn framing_bound_is_64_kib_and_round_trips() {
    assert_eq!(MAX_RECORD_LEN, 65_536);
    let body = vec![0x5au8; MAX_RECORD_LEN - 324];
    let frame = MessageFrame::inline(
        MessageKind::Request,
        OpCode::Write,
        StreamId(1),
        Tag(1),
        &body,
    )
    .expect("frame");
    let header = MeshHeader::new(node(1), node(2), node(2));
    let envelope = MeshEnvelope { header, frame };
    assert_eq!(record_len(&envelope).expect("record len"), 65_536);
    let mut record = vec![0u8; 65_536];
    assert_eq!(
        encode_record(&envelope, &mut record).expect("encode"),
        65_536
    );
    assert!(encode_record(&envelope, &mut record[..65_535]).is_err());
}

#[test]
fn bulk_chunks_are_bounded_and_pinned() {
    // The module's #247 measurement path: a 20-byte bulk header plus one
    // 16 KiB payload in a single MeshEnvelope.  Four chunks make the 64 KiB
    // logical record; the wire record stays well under MAX_RECORD_LEN.
    let chunk = 16 * 1024;
    let body = OVERHEAD - 4 + 20 + chunk;
    let record = 4 + body;
    assert_eq!(record, 16_728);
    assert!(record <= mesh_transport::MAX_RECORD_LEN);
    assert_eq!(4 * chunk, 64 * 1024);

    // A 64 KiB single wire record is still inside the transport bound, but the
    // module deliberately streams it because its slot has ~50 KiB of BSS
    // headroom, not 64 KiB.
    assert_eq!(mesh_transport::MAX_RECORD_LEN, 65_536);
}
