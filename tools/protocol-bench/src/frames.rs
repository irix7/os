// SPDX-License-Identifier: GPL-3.0-only
//! Exact wire bytes for every protocol message, from the shipped encoders.
//!
//! The envelope overhead is fixed: a 4-byte record length, a 128-byte
//! `MeshHeader`, a 128-byte `FrameHeader` and a 64-byte `PayloadDescriptor`.

use crate::stats::{header, row};
use mesh_join::{EpochConfirmation, EpochRefusal, JoinAck, JoinRequest, RefusalReason};
use mesh_migrate::{ActorMove, Body, DIRECTIVE_LEN};
use mesh_route::{HomeMove, Invoke, InvokeResult, MoveAck, Moved, RouteErrorBody};
use sys_primitives::{
    ActorId, Capability, MeshEnvelope, MeshHeader, MessageFrame, MessageKind, NodeId, ObjectId,
    OpCode, PrincipalId, Rights, StreamId, Tag,
};

use crate::proto::envelope_record_len;

/// Fixed envelope overhead per record, including the 4-byte length prefix.
pub const OVERHEAD: usize = 4 + 128 + 128 + 64;

fn node(byte: u8) -> NodeId {
    NodeId([byte; 16])
}

fn object(byte: u8) -> ObjectId {
    ObjectId([byte; 32])
}

fn capability() -> Capability {
    Capability {
        magic: Capability::MAGIC,
        version: Capability::VERSION,
        rights: Rights::READ,
        flags: 0,
        object: object(7),
        issuer: sys_primitives::NodeKey([0x33; 32]),
        subject: PrincipalId([0x44; 32]),
        home: node(2),
        not_before: 0,
        not_after: 60_000_000_000,
        object_epoch: 4,
        home_epoch: 2,
        root_epoch: 1,
        reserved0: 0,
        nonce: [5; 16],
        parent: ObjectId::ZERO,
        delegation: ObjectId::ZERO,
        reserved1: [0; 24],
        signature: [0x66; 64],
    }
}

fn directive() -> ActorMove {
    ActorMove::new(
        ActorId([0x77; 16]),
        node(1),
        node(1),
        node(2),
        2,
        [0x88; 16],
        4,
        30_000,
    )
}

fn envelope_len(kind: MessageKind, opcode: OpCode, body: &[u8]) -> usize {
    let frame = MessageFrame::inline(kind, opcode, StreamId(1), Tag(1), body).expect("frame");
    let header = MeshHeader::new(node(1), node(2), node(2));
    envelope_record_len(&MeshEnvelope { header, frame })
}

fn migration_len(body: &Body) -> usize {
    let mut buffer = [0u8; mesh_migrate::MAX_BODY];
    let len = body.encode(&mut buffer).expect("encode migration body");
    envelope_len(body.message_kind(), OpCode::Migrate, &buffer[..len])
}

fn migration_body_len(body: &Body) -> usize {
    let mut buffer = [0u8; mesh_migrate::MAX_BODY];
    body.encode(&mut buffer).expect("encode migration body")
}

/// Print the per-message wire inventory.
pub fn run() {
    println!("\n## Wire inventory (exact, from the shipped encoders)\n");
    println!(
        "Envelope overhead is {OVERHEAD} bytes per record: 4 length + 128 mesh header + \
         128 frame header + 64 payload descriptor.\n"
    );
    header(&["message", "body B", "record B", "overhead %"]);

    let add = |name: &str, body: usize, record: usize| {
        let overhead = if record == 0 {
            0.0
        } else {
            (OVERHEAD as f64 / record as f64) * 100.0
        };
        row(&[
            name.to_string(),
            body.to_string(),
            record.to_string(),
            format!("{overhead:.1}"),
        ]);
    };

    // Heartbeat and handshake.
    add(
        "Control/Ping (M2-1)",
        16,
        envelope_len(MessageKind::Control, OpCode::Ping, &[0; 16]),
    );
    add(
        "Request/Ping (probe)",
        16,
        envelope_len(MessageKind::Request, OpCode::Ping, &[0; 16]),
    );
    let request = JoinRequest {
        node: node(2),
        incarnation: 1,
        last_epoch: 1,
        last_digest: [0; 16],
        nonce: 9,
    };
    let mut buffer = [0u8; mesh_join::MAX_BODY];
    let body_len = request.encode(&mut buffer).expect("join request");
    add(
        "Request/Hello JOIN_REQUEST",
        body_len,
        envelope_len(MessageKind::Request, OpCode::Hello, &buffer[..body_len]),
    );
    for members in [2usize, 8, 128] {
        let confirmation = EpochConfirmation {
            root: node(1),
            epoch: 2,
            members: (0..members).map(|index| node(index as u8 + 1)).collect(),
            subject: node(2),
            request_nonce: 9,
            incarnation: 1,
            signature: [0x55; 64],
        };
        let mut buffer = [0u8; mesh_join::MAX_BODY];
        let body_len = confirmation.encode(&mut buffer).expect("confirm");
        add(
            &format!("Response/Hello EPOCH_CONFIRM ({members} members)"),
            body_len,
            envelope_len(MessageKind::Response, OpCode::Hello, &buffer[..body_len]),
        );
    }
    let ack = JoinAck {
        node: node(2),
        epoch: 2,
        digest: [0; 16],
        request_nonce: 9,
    };
    let mut buffer = [0u8; mesh_join::MAX_BODY];
    let body_len = ack.encode(&mut buffer).expect("ack");
    add(
        "Completion/Hello JOIN_ACK",
        body_len,
        envelope_len(MessageKind::Completion, OpCode::Hello, &buffer[..body_len]),
    );
    let refusal = EpochRefusal {
        root: node(1),
        epoch: 1,
        reason: RefusalReason::Busy,
        request_nonce: 9,
    };
    let mut buffer = [0u8; mesh_join::MAX_BODY];
    let body_len = refusal.encode(&mut buffer).expect("refusal");
    add(
        "Response/Hello EPOCH_REFUSED",
        body_len,
        envelope_len(MessageKind::Response, OpCode::Hello, &buffer[..body_len]),
    );

    // Remote capability route.
    let invoke = Invoke {
        capability: capability(),
        operation: mesh_route::OP_STAT,
    };
    let mut buffer = [0u8; mesh_route::MAX_BODY];
    let body_len = invoke.encode(&mut buffer).expect("invoke");
    add(
        "Request/Invoke",
        body_len,
        envelope_len(MessageKind::Request, OpCode::Invoke, &buffer[..body_len]),
    );
    let result = InvokeResult {
        object: object(7),
        home: node(2),
        home_epoch: 2,
        object_epoch: 4,
        lease_ms: 119_000,
        value: [0x11; 32],
    };
    let mut buffer = [0u8; mesh_route::MAX_BODY];
    let body_len = result.encode(&mut buffer).expect("result");
    add(
        "Response/Invoke RESULT",
        body_len,
        envelope_len(MessageKind::Response, OpCode::Invoke, &buffer[..body_len]),
    );
    let moved = Moved {
        object: object(7),
        old_home: node(2),
        new_home: node(1),
        old_home_epoch: 2,
        new_home_epoch: 3,
        object_epoch: 4,
        lease_ms: 120_000,
        grace_ms: 30_000,
        reason: mesh_route::Reason::None,
    };
    let mut buffer = [0u8; mesh_route::MAX_BODY];
    let body_len = moved.encode(&mut buffer).expect("moved");
    add(
        "Response/Invoke MOVED",
        body_len,
        envelope_len(MessageKind::Response, OpCode::Invoke, &buffer[..body_len]),
    );
    let home_move = HomeMove {
        object: object(7),
        old_home: node(2),
        new_home: node(1),
        membership_epoch: 2,
        old_home_epoch: 2,
        new_home_epoch: 3,
        object_epoch: 4,
        lease_ms: 120_000,
        grace_ms: 30_000,
        membership_digest: [0x99; 16],
        signature: [0xaa; 64],
    };
    let mut buffer = [0u8; mesh_route::MAX_BODY];
    let body_len = home_move.encode(&mut buffer).expect("home move");
    add(
        "Request/RouteQuery HOME_MOVE",
        body_len,
        envelope_len(
            MessageKind::Request,
            OpCode::RouteQuery,
            &buffer[..body_len],
        ),
    );
    let move_ack = MoveAck {
        object: object(7),
        home: node(1),
        home_epoch: 3,
        accepted: true,
        reason: mesh_route::Reason::None,
    };
    let mut buffer = [0u8; mesh_route::MAX_BODY];
    let body_len = move_ack.encode(&mut buffer).expect("move ack");
    add(
        "Response/RouteQuery MOVE_ACK",
        body_len,
        envelope_len(
            MessageKind::Response,
            OpCode::RouteQuery,
            &buffer[..body_len],
        ),
    );
    let error = RouteErrorBody {
        object: object(7),
        reason: mesh_route::Reason::NoPeer,
        detail: 0,
    };
    let mut buffer = [0u8; mesh_route::MAX_BODY];
    let body_len = error.encode(&mut buffer).expect("error");
    add(
        "Response/* ERROR",
        body_len,
        envelope_len(MessageKind::Response, OpCode::Invoke, &buffer[..body_len]),
    );

    // Actor migration.
    let directive_len = DIRECTIVE_LEN;
    add(
        "Request/Migrate AUTHORISE",
        migration_body_len(&Body::Authorise(directive())),
        migration_len(&Body::Authorise(directive())),
    );
    let _ = directive_len;
    add(
        "Request/Migrate PREPARE",
        migration_body_len(&Body::Prepare {
            directive: directive(),
            state_len: mesh_migrate::STATE_LEN as u16,
            hash: [0x11; 32],
        }),
        migration_len(&Body::Prepare {
            directive: directive(),
            state_len: mesh_migrate::STATE_LEN as u16,
            hash: [0x11; 32],
        }),
    );
    add(
        "Response/Migrate READY",
        migration_body_len(&Body::Ready {
            directive: directive(),
            hash: [0x11; 32],
        }),
        migration_len(&Body::Ready {
            directive: directive(),
            hash: [0x11; 32],
        }),
    );
    add(
        "Response/Migrate COMMIT",
        migration_body_len(&Body::Commit {
            directive: directive(),
            hash: [0x11; 32],
        }),
        migration_len(&Body::Commit {
            directive: directive(),
            hash: [0x11; 32],
        }),
    );
    add(
        "Request/Migrate STATE (248 B state)",
        migration_body_len(&Body::State {
            directive: directive(),
            state: vec![0x22; mesh_migrate::STATE_LEN],
        }),
        migration_len(&Body::State {
            directive: directive(),
            state: vec![0x22; mesh_migrate::STATE_LEN],
        }),
    );
    add(
        "Completion/Migrate ACK",
        migration_body_len(&Body::Ack {
            directive: directive(),
            hash: [0x11; 32],
        }),
        migration_len(&Body::Ack {
            directive: directive(),
            hash: [0x11; 32],
        }),
    );
    add(
        "Request/Migrate DELIVER (capability + 32 B)",
        migration_body_len(&Body::Deliver {
            actor: ActorId([0x77; 16]),
            epoch: 4,
            capability: capability(),
            message: mesh_migrate::Message {
                id: 3,
                payload: [0x33; 32],
            },
            mode: 0,
        }),
        migration_len(&Body::Deliver {
            actor: ActorId([0x77; 16]),
            epoch: 4,
            capability: capability(),
            message: mesh_migrate::Message {
                id: 3,
                payload: [0x33; 32],
            },
            mode: 0,
        }),
    );

    add(
        "Response/Migrate RESULT",
        migration_body_len(&Body::Result {
            actor: ActorId([0x77; 16]),
            epoch: 4,
            message: 3,
            applied: 3,
            logical: [0x44; 32],
            hash: [0x11; 32],
            duplicate: false,
        }),
        migration_len(&Body::Result {
            actor: ActorId([0x77; 16]),
            epoch: 4,
            message: 3,
            applied: 3,
            logical: [0x44; 32],
            hash: [0x11; 32],
            duplicate: false,
        }),
    );
    add(
        "Request/Migrate RESUME",
        migration_body_len(&Body::Resume {
            actor: ActorId([0x77; 16]),
            epoch: 4,
            capability: capability(),
        }),
        migration_len(&Body::Resume {
            actor: ActorId([0x77; 16]),
            epoch: 4,
            capability: capability(),
        }),
    );
    add(
        "Response/Migrate ERROR",
        migration_body_len(&Body::Error {
            actor: ActorId([0x77; 16]),
            epoch: 4,
            reason: mesh_migrate::Error::StaleEpoch,
            expected_epoch: 3,
        }),
        migration_len(&Body::Error {
            actor: ActorId([0x77; 16]),
            epoch: 4,
            reason: mesh_migrate::Error::StaleEpoch,
            expected_epoch: 3,
        }),
    );
    // Bulk: the maximum legal record.
    let max_body = mesh_transport::MAX_RECORD_LEN - (128 + 128 + 64);
    add(
        "Inline payload at the 64 KiB record bound",
        max_body,
        envelope_len(MessageKind::Request, OpCode::Write, &vec![0u8; max_body]),
    );

    println!();
    println!("Notes:");
    println!(
        "- Every body is little-endian and length-checked; no `#[repr(C)]` struct is on the wire."
    );
    println!("- The 320-byte capability dominates INVOKE, DELIVER and RESUME.");
    println!(
        "- At the 64 KiB bound the overhead is {:.2}%.",
        OVERHEAD as f64 / (OVERHEAD + max_body) as f64 * 100.0
    );
}
