// SPDX-License-Identifier: GPL-3.0-only
//! Host tests for the M2 remote capability route.

extern crate std;

use alloc::collections::VecDeque;
use alloc::string::ToString;
use alloc::vec;
use alloc::vec::Vec;

use cap_engine::SigningAuthority;
use mesh_core::{fixture_objects, home_for, MembershipView, ObjectRoute};
use sys_primitives::{Capability, MessageKind, NodeId, NodeKey, ObjectId, OpCode, Rights, Status};

use super::*;

const A: NodeId = NodeId([
    0x08, 0x00, 0x69, 0x95, 0xf5, 0x00, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
]);
const B: NodeId = NodeId([
    0x08, 0x00, 0x69, 0x69, 0xb6, 0x01, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
]);
const DIGEST: [u8; 16] = [
    0x2e, 0xda, 0x13, 0xc5, 0xac, 0x6f, 0x72, 0x61, 0x32, 0xe0, 0x47, 0x9d, 0xfe, 0x60, 0x45, 0xb7,
];

fn view() -> MembershipView {
    MembershipView::new(2, vec![A, B]).expect("two-node view")
}

fn object_home(object: &ObjectId) -> NodeId {
    home_for(object, &view()).expect("fixture home")
}

struct Pair {
    a: RouteNode,
    b: RouteNode,
    signer: SigningAuthority,
    to_a: VecDeque<Wire>,
    to_b: VecDeque<Wire>,
    a_events: Vec<Event>,
    b_events: Vec<Event>,
    now: u64,
}

impl Pair {
    fn new() -> Self {
        let signer = SigningAuthority::from_bytes(&[7u8; 32]);
        let key = signer.public();
        let mut a = RouteNode::new(A, A, key, true, 8).expect("route node A");
        let mut b = RouteNode::new(B, A, key, false, 8).expect("route node B");
        if let Some(step) = a.on_snapshot(2, &[A, B], DIGEST, 0) {
            for event in &step.events {
                a.note_event(event);
            }
        }
        if let Some(step) = b.on_snapshot(2, &[A, B], DIGEST, 0) {
            for event in &step.events {
                b.note_event(event);
            }
        }
        a.set_peer(B, 0);
        b.set_peer(A, 0);
        Self {
            a,
            b,
            signer,
            to_a: VecDeque::new(),
            to_b: VecDeque::new(),
            a_events: Vec::new(),
            b_events: Vec::new(),
            now: 1_000,
        }
    }

    fn record(&mut self, step: Step, from_a: bool) {
        if let Some(wire) = step.send {
            if from_a {
                self.to_b.push_back(wire);
            } else {
                self.to_a.push_back(wire);
            }
        }
        if from_a {
            for event in &step.events {
                self.a.note_event(event);
            }
            self.a_events.extend(step.events);
        } else {
            for event in &step.events {
                self.b.note_event(event);
            }
            self.b_events.extend(step.events);
        }
    }

    fn deliver(&mut self) {
        while let Some(wire) = self.to_b.pop_front() {
            let header = wire.header(A);
            let step = self.b.on_wire(
                A,
                &header,
                wire.trace,
                wire.object,
                wire.kind,
                wire.opcode,
                wire.status,
                &wire.body,
                self.now,
                &self.signer,
            );
            self.record(step, false);
        }
        while let Some(wire) = self.to_a.pop_front() {
            let header = wire.header(B);
            let step = self.a.on_wire(
                B,
                &header,
                wire.trace,
                wire.object,
                wire.kind,
                wire.opcode,
                wire.status,
                &wire.body,
                self.now,
                &self.signer,
            );
            self.record(step, true);
        }
    }

    fn a_has(&self, predicate: impl Fn(&Event) -> bool) -> bool {
        self.a_events.iter().any(predicate)
    }

    fn b_has(&self, predicate: impl Fn(&Event) -> bool) -> bool {
        self.b_events.iter().any(predicate)
    }

    fn a_count(&self, predicate: impl Fn(&Event) -> bool) -> usize {
        self.a_events
            .iter()
            .filter(|event| predicate(event))
            .count()
    }

    fn b_count(&self, predicate: impl Fn(&Event) -> bool) -> usize {
        self.b_events
            .iter()
            .filter(|event| predicate(event))
            .count()
    }

    fn capability(&self, object: ObjectId, home: NodeId, home_epoch: u32) -> Capability {
        self.a
            .issue_capability(
                object,
                &ObjectRoute {
                    object,
                    home,
                    home_epoch,
                    object_epoch: OBJECT_EPOCH,
                    lease_until: 0,
                },
                OP_STAT,
                1,
                &self.signer,
            )
            .expect("capability")
    }

    fn invoke(
        &mut self,
        object: ObjectId,
        home: NodeId,
        home_epoch: u32,
        request_id: u64,
        operation: u16,
        capability: Capability,
    ) -> Wire {
        let mut buffer = [0u8; Invoke::LEN];
        let len = Invoke {
            capability,
            operation,
        }
        .encode(&mut buffer)
        .expect("invoke body");
        Wire {
            dst: B,
            kind: MessageKind::Request,
            opcode: OpCode::Invoke,
            status: Status::Ok,
            flags: 0,
            src_epoch: 2,
            dst_epoch: 2,
            home,
            home_epoch,
            object_epoch: OBJECT_EPOCH,
            object,
            request_id,
            reply_to: 0,
            body: buffer[..len].to_vec(),
            trace: None,
        }
    }

    fn deliver_raw(&mut self, wire: Wire) -> Step {
        let header = wire.header(A);
        self.b.on_wire(
            A,
            &header,
            wire.trace,
            wire.object,
            wire.kind,
            wire.opcode,
            wire.status,
            &wire.body,
            self.now,
            &self.signer,
        )
    }
}

fn remote_object() -> ObjectId {
    fixture_objects(8)
        .into_iter()
        .find(|object| object_home(object) == B)
        .expect("a fixture object homes to B")
}

fn local_object() -> ObjectId {
    fixture_objects(8)
        .into_iter()
        .find(|object| object_home(object) == A)
        .expect("a fixture object homes to A")
}

fn error_reason(step: &Step) -> Reason {
    let wire = step.send.as_ref().expect("an error reply");
    match RouteBody::decode(&wire.body).expect("route body") {
        RouteBody::Error(body) => body.reason,
        other => panic!("expected an error body, got {other:?}"),
    }
}

#[test]
fn remote_operation_completes_at_the_confirmed_home() {
    let mut pair = Pair::new();
    let object = remote_object();
    assert_eq!(object_home(&object), B);
    let step = pair.a.begin_operation(object, pair.now, &pair.signer);
    let wire = step.send.as_ref().expect("an invoke");
    assert_eq!(wire.dst, B);
    assert_eq!(wire.home, B);
    assert_eq!(wire.home_epoch, 2);
    pair.record(step, true);
    pair.deliver();
    assert!(pair.b_has(|event| matches!(event, Event::Complete { role: "home", .. })));
    assert!(pair.a_has(|event| matches!(
        event,
        Event::Result {
            attempts: 1,
            moved: false,
            ..
        }
    )));
    let value = route_value(&object, 2, OBJECT_EPOCH);
    assert!(pair.a_has(|event| matches!(
        event,
        Event::Result { value: found, .. } if *found == value
    )));
}

#[test]
fn a_node_that_is_not_the_home_does_not_redirect() {
    let mut pair = Pair::new();
    let object = local_object();
    assert_eq!(object_home(&object), A);
    let capability = pair.capability(object, B, 2);
    let wire = pair.invoke(object, B, 2, 1, OP_STAT, capability);
    let step = pair.deliver_raw(wire);
    assert_eq!(error_reason(&step), Reason::NotHome);
    assert!(!pair.b_has(|event| matches!(event, Event::Moved { .. })));
}

#[test]
fn a_stale_route_epoch_is_rejected_without_a_redirect() {
    let mut pair = Pair::new();
    let object = remote_object();
    let capability = pair.capability(object, B, 1);
    let wire = pair.invoke(object, B, 1, 1, OP_STAT, capability);
    let step = pair.deliver_raw(wire);
    assert_eq!(error_reason(&step), Reason::StaleEpoch);
    assert!(!pair.b_has(|event| matches!(event, Event::Moved { .. })));
}

#[test]
fn one_moved_is_retried_once_at_the_new_home() {
    let mut pair = Pair::new();
    let object = remote_object();

    // Move the object from B to A.  The sender keeps its stale B route.
    let move_step = pair.a.start_move(object, B, A, 2, pair.now, &pair.signer);
    assert!(move_step.send.is_some());
    pair.record(move_step, true);
    pair.deliver();
    assert!(pair.b_has(|event| matches!(event, Event::Move { role: "old", .. })));

    // A still has the stale B route, so the next operation goes to B.
    let step = pair.a.begin_operation(object, pair.now, &pair.signer);
    let wire = step.send.as_ref().expect("an invoke to the old home");
    assert_eq!(wire.dst, B);
    assert_eq!(wire.home_epoch, 2);
    pair.record(step, true);
    pair.deliver();

    assert_eq!(
        pair.b_count(|event| matches!(
            event,
            Event::Moved {
                source: "remote",
                ..
            }
        )),
        1
    );
    assert_eq!(
        pair.a_count(|event| matches!(event, Event::Retry { attempt: 2, .. })),
        1
    );
    assert!(pair.a_has(|event| matches!(
        event,
        Event::Result {
            attempts: 2,
            moved: true,
            home: A,
            ..
        }
    )));
    assert!(pair.a_has(|event| matches!(
        event,
        Event::Complete {
            role: "home",
            home: A,
            ..
        }
    )));
}

#[test]
fn a_second_moved_is_rejected() {
    let mut pair = Pair::new();
    let object = local_object();

    // Move the object from A to B.  A is the old home and drains; the first
    // operation answers MOVED locally and retries at B.
    let move_step = pair.a.start_move(object, A, B, 2, pair.now, &pair.signer);
    pair.record(move_step, true);
    pair.deliver();
    assert!(pair.b_has(|event| matches!(event, Event::Move { role: "new", .. })));

    let step = pair.a.begin_operation(object, pair.now, &pair.signer);
    let request_id = {
        let retry = step.send.as_ref().expect("a retry to B");
        assert_eq!(retry.dst, B);
        assert_eq!(retry.home_epoch, 3);
        retry.request_id
    };
    pair.record(step, true);
    assert!(pair.a_has(|event| matches!(
        event,
        Event::Moved {
            source: "local",
            ..
        }
    )));

    // Replace the real retry with a second MOVED from B before B can answer.
    pair.to_b.pop_back();
    let moved = Moved {
        object,
        old_home: B,
        new_home: A,
        old_home_epoch: 3,
        new_home_epoch: 4,
        object_epoch: OBJECT_EPOCH,
        lease_ms: 60_000,
        grace_ms: 30_000,
        reason: Reason::None,
    };
    let mut body = [0u8; Moved::LEN];
    let len = moved.encode(&mut body).expect("moved body");
    let wire = Wire {
        dst: A,
        kind: MessageKind::Response,
        opcode: OpCode::Invoke,
        status: Status::Stale,
        flags: sys_primitives::mesh_flags::MOVED,
        src_epoch: 2,
        dst_epoch: 2,
        home: B,
        home_epoch: 3,
        object_epoch: OBJECT_EPOCH,
        object,
        request_id,
        reply_to: request_id,
        body: body[..len].to_vec(),
        trace: None,
    };
    pair.to_a.push_back(wire);
    pair.deliver();
    assert!(pair.a_has(|event| matches!(
        event,
        Event::Reject {
            reason: Reason::RepeatedMoved,
            ..
        }
    )));
    assert_eq!(
        pair.a_count(|event| matches!(event, Event::Retry { .. })),
        1,
        "there must be no third attempt"
    );
}

#[test]
fn missing_peer_and_deadlines_fail_closed() {
    let signer = SigningAuthority::from_bytes(&[7u8; 32]);
    let key = signer.public();
    let mut lonely = RouteNode::new(A, A, key, true, 8).expect("route node");
    lonely.on_snapshot(2, &[A, B], DIGEST, 0);
    let object = remote_object();
    let step = lonely.begin_operation(object, 1_000, &signer);
    assert!(step.send.is_none());
    assert!(step.events.iter().any(|event| matches!(
        event,
        Event::Reject {
            reason: Reason::NoPeer,
            ..
        }
    )));

    let mut pair = Pair::new();
    let step = pair.a.begin_operation(object, pair.now, &pair.signer);
    assert!(step.send.is_some());
    pair.record(step, true);
    let timeout = pair
        .a
        .tick(pair.now + OPERATION_DEADLINE_MS + 1, &pair.signer);
    assert!(timeout.send.is_none());
    assert!(timeout.events.iter().any(|event| matches!(
        event,
        Event::Timeout {
            phase: "operation",
            ..
        }
    )));
    assert!(timeout.events.iter().any(|event| matches!(
        event,
        Event::Reject {
            reason: Reason::Timeout,
            ..
        }
    )));
    let retry = pair
        .a
        .tick(pair.now + OPERATION_DEADLINE_MS + 2, &pair.signer);
    assert!(retry.send.is_none());
}

#[test]
fn malformed_bodies_are_rejected() {
    // Direct codec checks for the common corruptions.
    assert_eq!(Invoke::decode(&[]), Err(WireError::LengthMismatch));
    assert_eq!(
        Invoke::decode(&[0u8; Invoke::LEN]),
        Err(WireError::BadMagic)
    );
    let mut body = [0u8; Invoke::LEN];
    body[..4].copy_from_slice(&MAGIC.to_le_bytes());
    body[4] = 9;
    assert_eq!(Invoke::decode(&body), Err(WireError::BadVersion));
    body[4] = VERSION;
    assert_eq!(
        Invoke::decode(&body[..Invoke::LEN - 1]),
        Err(WireError::LengthMismatch)
    );

    let mut pair = Pair::new();
    let object = remote_object();
    let capability = pair.capability(object, B, 2);
    let wire = pair.invoke(object, B, 2, 1, OP_STAT, capability);
    let mut corrupt = wire.clone();
    corrupt.body[0] ^= 0xff;
    let step = pair.deliver_raw(corrupt);
    assert!(step.send.is_none(), "a malformed body is not answered");
    assert!(step.events.iter().any(|event| matches!(
        event,
        Event::Reject {
            reason: Reason::Malformed,
            ..
        }
    )));

    let mut replay = pair.invoke(object, B, 2, 1, OP_STAT, capability);
    replay.body = wire.body.clone();
    // Request id 1 was already accepted before the malformed body; the
    // malformed body did not advance the replay window.
    let step = pair.deliver_raw(replay);
    assert_eq!(step.send.as_ref().map(|wire| wire.status), Some(Status::Ok));
}

#[test]
fn capability_mismatches_fail_closed() {
    let mut pair = Pair::new();
    let object = remote_object();
    let base = pair.capability(object, B, 2);

    let cases: Vec<(&str, Capability, Reason)> = vec![
        (
            "wrong-object",
            Capability {
                object: local_object(),
                ..base
            },
            Reason::WrongObject,
        ),
        (
            "wrong-subject",
            Capability {
                subject: principal_for(&B),
                ..base
            },
            Reason::WrongSubject,
        ),
        (
            "insufficient-rights",
            Capability {
                rights: Rights::NONE,
                ..base
            },
            Reason::InsufficientRights,
        ),
        (
            "expired",
            Capability {
                not_after: 1,
                ..base
            },
            Reason::Expired,
        ),
        (
            "stale-capability",
            Capability {
                home_epoch: 1,
                ..base
            },
            Reason::StaleCapability,
        ),
    ];
    let mut request_id = 1;
    for (name, capability, expected) in cases {
        let mut capability = capability;
        capability.signature = pair.signer.sign(&capability.signed_bytes());
        let wire = pair.invoke(object, B, 2, request_id, OP_STAT, capability);
        let step = pair.deliver_raw(wire);
        assert_eq!(error_reason(&step), expected, "{name}");
        request_id += 1;
    }

    // A tampered signature and an untrusted issuer are both refused.
    let mut bad = base;
    bad.signature[0] ^= 0xff;
    let wire = pair.invoke(object, B, 2, request_id, OP_STAT, bad);
    assert_eq!(error_reason(&pair.deliver_raw(wire)), Reason::BadSignature);
    request_id += 1;

    let other = SigningAuthority::from_bytes(&[9u8; 32]);
    let mut untrusted = base;
    untrusted.issuer = other.public();
    untrusted.signature = other.sign(&untrusted.signed_bytes());
    let wire = pair.invoke(object, B, 2, request_id, OP_STAT, untrusted);
    assert_eq!(
        error_reason(&pair.deliver_raw(wire)),
        Reason::UntrustedIssuer
    );
    request_id += 1;

    // An unsupported operation is refused (2 is the supported commit code).
    let capability = pair.capability(object, B, 2);
    let wire = pair.invoke(object, B, 2, request_id, 99, capability);
    assert_eq!(
        error_reason(&pair.deliver_raw(wire)),
        Reason::WrongOperation
    );
}

#[test]
fn home_moves_require_the_authority_and_the_confirmed_view() {
    let mut pair = Pair::new();
    let object = remote_object();
    let wrong = SigningAuthority::from_bytes(&[9u8; 32]);
    let step = pair.a.start_move(object, B, A, 2, pair.now, &wrong);
    assert!(step.events.iter().any(|event| matches!(
        event,
        Event::Reject {
            reason: Reason::WrongSubject,
            ..
        }
    )));

    let mut directive = HomeMove {
        object,
        old_home: B,
        new_home: A,
        membership_epoch: 2,
        old_home_epoch: 2,
        new_home_epoch: 3,
        object_epoch: OBJECT_EPOCH,
        lease_ms: 60_000,
        grace_ms: 30_000,
        membership_digest: [0xee; 16],
        signature: [0; 64],
    };
    directive.signature = pair.signer.sign(&directive.signed_bytes());
    let mut body = [0u8; HomeMove::LEN];
    let len = directive.encode(&mut body).expect("move body");
    let wire = Wire {
        dst: B,
        kind: MessageKind::Request,
        opcode: OpCode::RouteQuery,
        status: Status::Ok,
        flags: 0,
        src_epoch: 2,
        dst_epoch: 2,
        home: B,
        home_epoch: 2,
        object_epoch: OBJECT_EPOCH,
        object,
        request_id: 1,
        reply_to: 0,
        body: body[..len].to_vec(),
        trace: None,
    };
    let step = pair.deliver_raw(wire);
    let ack = RouteBody::decode(&step.send.as_ref().expect("move ack").body).expect("body");
    match ack {
        RouteBody::MoveAck(ack) => {
            assert!(!ack.accepted);
            assert_eq!(ack.reason, Reason::StaleEpoch);
        }
        other => panic!("expected a move ack, got {other:?}"),
    }
}

#[test]
fn the_full_route_sequence_fills_the_report() {
    let mut pair = Pair::new();
    let remote = remote_object();
    let local = local_object();

    // 1. A -> B, remote home, one attempt.
    let step = pair.a.begin_operation(remote, pair.now, &pair.signer);
    assert_eq!(step.send.as_ref().expect("remote invoke").dst, B);
    pair.record(step, true);
    pair.deliver();
    assert!(pair.a_has(|event| matches!(
        event,
        Event::Result {
            attempts: 1,
            moved: false,
            home: B,
            ..
        }
    )));

    // 2. Move the remote object from B to A and consume the wire MOVED.
    let step = pair.a.start_move(remote, B, A, 2, pair.now, &pair.signer);
    pair.record(step, true);
    pair.deliver();
    let step = pair.a.begin_operation(remote, pair.now, &pair.signer);
    pair.record(step, true);
    pair.deliver();
    assert!(pair.a_has(|event| matches!(
        event,
        Event::Result {
            attempts: 2,
            moved: true,
            home: A,
            ..
        }
    )));

    // 3. Move the local object from A to B and consume the local MOVED with a
    //    remote retry.
    let step = pair.a.start_move(local, A, B, 2, pair.now, &pair.signer);
    pair.record(step, true);
    pair.deliver();
    let step = pair.a.begin_operation(local, pair.now, &pair.signer);
    pair.record(step, true);
    pair.deliver();
    assert!(pair.a_has(|event| matches!(
        event,
        Event::Result {
            attempts: 2,
            moved: true,
            home: B,
            ..
        }
    )));

    let report = pair.a.report();
    assert_eq!(report.view_epoch, 2);
    assert_eq!(report.view_members, 2);
    assert_eq!(report.move_old, Some(local));
    assert_eq!(report.move_new, Some(remote));
    assert_eq!(report.moved_remote, 1);
    assert_eq!(report.moved_local, 1);
    assert_eq!(report.retries, 2);
    assert_eq!(report.completes, 1);
    assert_eq!(report.rejects, 0);
    assert_eq!(report.results.len(), 3);
    assert!(report.results.iter().any(|result| {
        result.object == remote && result.home == B && result.attempts == 1 && !result.moved
    }));
    assert!(report.results.iter().any(|result| {
        result.object == remote && result.home == A && result.attempts == 2 && result.moved
    }));
    assert!(report.results.iter().any(|result| {
        result.object == local && result.home == B && result.attempts == 2 && result.moved
    }));

    let b_report = pair.b.report();
    assert_eq!(b_report.move_old, Some(remote));
    assert_eq!(b_report.move_new, Some(local));
    assert_eq!(b_report.moved_remote, 1);
    assert_eq!(b_report.completes, 2);
    assert_eq!(b_report.rejects, 0);
}

#[test]
fn move_codec_is_bounded_and_canonical() {
    let object = remote_object();
    let mut directive = HomeMove {
        object,
        old_home: B,
        new_home: A,
        membership_epoch: 2,
        old_home_epoch: 2,
        new_home_epoch: 3,
        object_epoch: OBJECT_EPOCH,
        lease_ms: 60_000,
        grace_ms: 30_000,
        membership_digest: DIGEST,
        signature: [0; 64],
    };
    directive.signature = SigningAuthority::from_bytes(&[7u8; 32]).sign(&directive.signed_bytes());
    let mut body = [0u8; HomeMove::LEN];
    let len = directive.encode(&mut body).expect("encode");
    assert_eq!(len, HomeMove::LEN);
    assert_eq!(HomeMove::decode(&body[..len]), Ok(directive));
    assert_eq!(
        HomeMove::decode(&body[..len - 1]),
        Err(WireError::LengthMismatch)
    );
    assert!(directive
        .verify(&SigningAuthority::from_bytes(&[7u8; 32]).public())
        .is_ok());
    assert_eq!(
        directive.verify(&SigningAuthority::from_bytes(&[9u8; 32]).public()),
        Err(WireError::BadSignature)
    );
    let mut tampered = directive;
    tampered.new_home_epoch = 4;
    assert_eq!(
        tampered.verify(&SigningAuthority::from_bytes(&[7u8; 32]).public()),
        Err(WireError::BadSignature)
    );
}

#[test]
fn route_values_are_deterministic_and_distinct() {
    let object = remote_object();
    let first = route_value(&object, 2, 1);
    assert_eq!(first, route_value(&object, 2, 1));
    assert_ne!(first, route_value(&object, 3, 1));
    assert_ne!(first, route_value(&object, 2, 2));
    // Pin the domain so a refactor cannot silently change the fixture value.
    assert_eq!(&first[..4], &[0xfd, 0x0b, 0x0c, 0xc8]);
}

#[test]
fn events_format_stable_evidence_lines() {
    let event = Event::Moved {
        object: remote_object(),
        from: B,
        to: A,
        old_home_epoch: 2,
        new_home_epoch: 3,
        lease_ms: 60_000,
        grace_ms: 30_000,
        source: "remote",
    };
    let line = event.to_string();
    assert!(line.starts_with("mesh-route: moved object="));
    assert!(line.contains("source=remote"));
    assert!(line.contains("old_home_epoch=2 new_home_epoch=3"));
    let event = Event::Reject {
        object: Some(remote_object()),
        reason: Reason::RepeatedMoved,
    };
    assert!(event
        .to_string()
        .starts_with("mesh-route: reject reason=repeated-moved object="));
}

#[test]
fn node_keys_are_stable() {
    let signer = SigningAuthority::from_bytes(&[7u8; 32]);
    let key: NodeKey = signer.public();
    assert_eq!(key, signer.key());
    assert_eq!(signer.sign(b"x").len(), 64);
}

#[test]
fn rig_steps_select_the_same_objects_without_a_central_directory() {
    let mut pair = Pair::new();
    let remote = remote_object();
    let local = local_object();

    let step = pair.a.command(1, pair.now, &pair.signer);
    assert_eq!(step.send.as_ref().expect("step 1 invoke").dst, B);
    pair.record(step, true);
    pair.deliver();
    assert!(pair.a.report().results.iter().any(|result| {
        result.object == remote && result.home == B && result.attempts == 1 && !result.moved
    }));

    let step = pair.a.command(2, pair.now, &pair.signer);
    assert_eq!(step.send.as_ref().expect("step 2 move").dst, B);
    pair.record(step, true);
    pair.deliver();
    assert_eq!(pair.a.report().move_new, Some(remote));
    assert_eq!(pair.b.report().move_old, Some(remote));

    let step = pair.a.command(3, pair.now, &pair.signer);
    pair.record(step, true);
    pair.deliver();
    assert!(pair.a.report().results.iter().any(|result| {
        result.object == remote && result.home == A && result.attempts == 2 && result.moved
    }));
    assert_eq!(pair.b.report().moved_remote, 1);

    let step = pair.a.command(4, pair.now, &pair.signer);
    assert_eq!(step.send.as_ref().expect("step 4 move").dst, B);
    pair.record(step, true);
    pair.deliver();
    assert_eq!(pair.a.report().move_old, Some(local));
    assert_eq!(pair.b.report().move_new, Some(local));

    let step = pair.a.command(5, pair.now, &pair.signer);
    pair.record(step, true);
    pair.deliver();
    assert!(pair.a.report().results.iter().any(|result| {
        result.object == local && result.home == B && result.attempts == 2 && result.moved
    }));
    assert_eq!(pair.a.report().retries, 2);
    assert_eq!(pair.a.report().moved_remote, 1);
    assert_eq!(pair.a.report().moved_local, 1);
    assert_eq!(pair.a.report().completes, 1);
    assert_eq!(pair.b.report().completes, 2);
    assert_eq!(pair.a.report().rejects, 0);
    assert_eq!(pair.b.report().rejects, 0);
}

#[test]
fn rig_steps_fail_closed_without_a_peer() {
    let signer = SigningAuthority::from_bytes(&[7u8; 32]);
    let key = signer.public();
    let mut lonely = RouteNode::new(A, A, key, true, 8).expect("route node");
    lonely.on_snapshot(2, &[A, B], DIGEST, 0);
    let step = lonely.command(1, 1_000, &signer);
    assert!(step.send.is_none());
    assert!(step.events.iter().any(|event| matches!(
        event,
        Event::Reject {
            reason: Reason::NoPeer,
            ..
        }
    )));
}

#[test]
fn a_stale_object_epoch_and_an_unknown_home_epoch_are_rejected() {
    let mut pair = Pair::new();
    let object = remote_object();
    let mut capability = pair.capability(object, B, 2);
    capability.object_epoch = 0;
    capability.signature = pair.signer.sign(&capability.signed_bytes());
    let mut wire = pair.invoke(object, B, 2, 1, OP_STAT, capability);
    wire.object_epoch = 0;
    assert_eq!(
        error_reason(&pair.deliver_raw(wire)),
        Reason::StaleObjectEpoch
    );

    let capability = pair.capability(object, B, 2);
    let wire = pair.invoke(object, B, 3, 2, OP_STAT, capability);
    assert_eq!(error_reason(&pair.deliver_raw(wire)), Reason::UnknownEpoch);
}

#[test]
fn replayed_requests_and_link_loss_fail_closed() {
    let mut pair = Pair::new();
    let object = remote_object();
    let capability = pair.capability(object, B, 2);
    let wire = pair.invoke(object, B, 2, 1, OP_STAT, capability);
    let step = pair.deliver_raw(wire.clone());
    assert_eq!(step.send.as_ref().map(|wire| wire.status), Some(Status::Ok));
    let step = pair.deliver_raw(wire);
    assert!(step.events.iter().any(|event| matches!(
        event,
        Event::Reject {
            reason: Reason::Replay,
            ..
        }
    )));

    let step = pair.a.begin_operation(object, pair.now, &pair.signer);
    assert!(step.send.is_some());
    pair.record(step, true);
    assert!(pair.a.on_link_down().is_some());
    assert!(pair.a.peer().is_none());
    let step = pair.a.tick(pair.now + 1, &pair.signer);
    assert!(step.send.is_none());
}

#[test]
fn an_oversize_route_body_is_rejected_before_it_is_decoded() {
    let mut pair = Pair::new();
    let object = remote_object();
    let capability = pair.capability(object, B, 2);
    let mut wire = pair.invoke(object, B, 2, 1, OP_STAT, capability);
    wire.body = vec![0u8; MAX_BODY + 1];
    let step = pair.deliver_raw(wire);
    assert!(step.send.is_none());
    assert!(step.events.iter().any(|event| matches!(
        event,
        Event::Reject {
            reason: Reason::Malformed,
            ..
        }
    )));
}

fn other_digest(byte: u8) -> [u8; 16] {
    [byte; 16]
}

#[test]
fn a_membership_change_fences_a_stale_route_and_never_contacts_the_removed_home() {
    let mut pair = Pair::new();
    let object = remote_object();
    // A remote read establishes a cached route to B and puts one frame on the
    // wire.  Deliver and complete it so nothing is pending.
    let step = pair.a.begin_operation(object, pair.now, &pair.signer);
    assert!(step.send.is_some());
    pair.record(step, true);
    pair.deliver();
    assert_eq!(pair.a.report().completes, 0);
    assert!(pair.a.cached_route(&object, pair.now).is_some());

    // B is removed from the confirmed view at epoch 3.
    let step = pair
        .a
        .on_snapshot(3, &[A], other_digest(0x11), pair.now + 1)
        .expect("snapshot step");
    assert!(step.send.is_none());
    let fence = step
        .events
        .iter()
        .find_map(|event| match event {
            Event::Fence {
                object: fenced,
                stale_home,
                stale_home_epoch,
                home,
                home_epoch,
                reason,
            } if *fenced == object => {
                Some((*stale_home, *stale_home_epoch, *home, *home_epoch, *reason))
            }
            _ => None,
        })
        .expect("a fence row for the object");
    assert_eq!(fence, (B, 2, A, 3, "left-view"));
    for event in &step.events {
        pair.a.note_event(event);
    }
    assert!(pair.a.report().fences >= 1);
    assert!(pair
        .a
        .report()
        .fence_rows
        .iter()
        .any(|row| row.object == object && row.stale_home == B && row.home == A));

    // The next read is re-routed locally, with no frame to the removed node
    // and no timeout: the survivor does not wait for the failed peer.
    let step = pair.a.begin_operation(object, pair.now + 2, &pair.signer);
    assert!(step.send.is_none(), "a fenced route must not send");
    assert!(step.events.iter().any(|event| matches!(
        event,
        Event::Result {
            home,
            attempts: 1,
            moved: false,
            ..
        } if *home == A
    )));
    assert_eq!(pair.a.report().timeouts, 0);
    assert_eq!(pair.a.report().rejects, 0);
    // The route cache now names the survivor.
    assert_eq!(
        pair.a
            .cached_route(&object, pair.now + 2)
            .map(|route| route.home),
        Some(A)
    );
}

#[test]
fn a_duplicate_snapshot_is_idempotent_and_does_not_repeat_a_fence() {
    let mut pair = Pair::new();
    let step = pair
        .a
        .on_snapshot(3, &[A], other_digest(0x22), pair.now)
        .expect("snapshot");
    for event in &step.events {
        pair.a.note_event(event);
    }
    let fences = pair.a.report().fences;
    assert!(pair
        .a
        .on_snapshot(3, &[A], other_digest(0x22), pair.now + 1)
        .is_none());
    assert_eq!(pair.a.report().fences, fences);
    // A lower epoch is a regression and never fences or relabels.
    assert!(pair
        .a
        .on_snapshot(2, &[A, B], DIGEST, pair.now + 2)
        .is_none());
    assert_eq!(pair.a.epoch(), 3);
}

#[test]
fn a_replayed_stale_lease_frame_is_refused_by_the_epoch_fence() {
    let mut pair = Pair::new();
    let object = remote_object();
    // Step 1 sends the frame that a delayed duplicate would carry.
    let step = pair.a.command(1, pair.now, &pair.signer);
    assert!(step.send.is_some());
    pair.record(step, true);
    pair.deliver();
    assert!(pair
        .a
        .report()
        .results
        .iter()
        .any(|row| row.object == object));

    // The member leaves the view; the saved frame names the old epoch.
    let step = pair
        .a
        .on_snapshot(3, &[A], other_digest(0x33), pair.now + 1)
        .expect("snapshot");
    for event in &step.events {
        pair.a.note_event(event);
    }
    let step = pair.a.command(7, pair.now + 2, &pair.signer);
    pair.record(step.clone(), true);
    assert!(step.send.is_none(), "a stale frame must not be answered");
    assert!(step.events.iter().any(|event| matches!(
        event,
        Event::Replay {
            reason: Reason::StaleEpoch,
            src_epoch: 2,
            epoch: 3,
        }
    )));
    assert_eq!(pair.a.report().replays, 1);
    assert_eq!(pair.a.report().last_replay, Some(Reason::StaleEpoch));
    assert_eq!(pair.a.report().rejects, 1);
    assert_eq!(pair.a.report().timeouts, 0);
}

#[test]
fn a_mutable_commit_requires_a_current_authoritative_view() {
    let mut pair = Pair::new();
    let object = local_object();
    // Current view: the local commit completes at the authority.
    let step = pair
        .a
        .begin_operation_with(object, OP_COMMIT, pair.now, &pair.signer);
    pair.record(step.clone(), true);
    assert!(step.send.is_none());
    assert!(step.events.iter().any(|event| matches!(
        event,
        Event::Result {
            home,
            operation: OP_COMMIT,
            ..
        } if *home == A
    )));
    assert_eq!(pair.a.report().commits, 1);

    // Loss of the authority fails the mutable path closed immediately...
    let events = pair.a.on_authority_lost().expect("authority lost");
    pair.a.note_event(&events);
    assert!(!pair.a.mutable_available_at(pair.now + 1));
    let step = pair
        .a
        .begin_operation_with(object, OP_COMMIT, pair.now + 1, &pair.signer);
    pair.record(step.clone(), true);
    assert!(step.send.is_none());
    assert!(step.events.iter().any(|event| matches!(
        event,
        Event::Reject {
            reason: Reason::NoAuthority,
            ..
        }
    )));
    // ...while a read is still served from the unexpired lease.
    let step = pair.a.begin_operation(object, pair.now + 2, &pair.signer);
    pair.record(step.clone(), true);
    assert!(step.events.iter().any(|event| matches!(
        event,
        Event::Result {
            operation: OP_STAT,
            ..
        }
    )));
    // After lease expiry the object is unavailable, not re-homed.
    let step = pair
        .a
        .begin_operation(object, pair.now + HOME_LEASE_MS + 1, &pair.signer);
    pair.record(step.clone(), true);
    assert!(step.send.is_none());
    assert!(step.events.iter().any(|event| matches!(
        event,
        Event::Reject {
            reason: Reason::NotCurrent,
            ..
        }
    )));
}

#[test]
fn an_isolated_home_refuses_a_remote_commit_without_moving_the_object() {
    let mut pair = Pair::new();
    let object = remote_object();
    // A is the root and can issue the commit capability; B is the home.
    let mut capability = pair.capability(object, B, 2);
    capability.rights = Rights::READ.union(Rights::WRITE);
    capability.signature = pair.signer.sign(&capability.signed_bytes());
    let wire = pair.invoke(object, B, 2, 1, OP_COMMIT, capability);
    let step = pair.deliver_raw(wire.clone());
    pair.record(step.clone(), false);
    assert_eq!(step.send.as_ref().map(|wire| wire.status), Some(Status::Ok));
    assert!(pair.b.report().completes >= 1);
    assert!(step.events.iter().any(|event| matches!(
        event,
        Event::Complete {
            operation: OP_COMMIT,
            ..
        }
    )));

    // The member loses the authority; the same commit is now unavailable.
    pair.b.on_authority_lost();
    let mut capability = pair.capability(object, B, 2);
    capability.rights = Rights::READ.union(Rights::WRITE);
    capability.signature = pair.signer.sign(&capability.signed_bytes());
    let wire = pair.invoke(object, B, 2, 5, OP_COMMIT, capability);
    let step = pair.deliver_raw(wire);
    pair.record(step.clone(), false);
    assert_eq!(error_reason(&step), Reason::NoAuthority);
    // The lease and home are unchanged: no second authoritative home.
    let lease = pair.b.home_lease(&object).expect("lease");
    assert_eq!(lease.home, B);
    assert_eq!(lease.home_epoch, 2);
    assert_eq!(lease.state, LeaseState::Active);
    // A read under the unexpired lease is still allowed.
    let mut capability = pair.capability(object, B, 2);
    capability.signature = pair.signer.sign(&capability.signed_bytes());
    let wire = pair.invoke(object, B, 2, 6, OP_STAT, capability);
    let step = pair.deliver_raw(wire);
    pair.record(step.clone(), false);
    assert_eq!(step.send.as_ref().map(|wire| wire.status), Some(Status::Ok));
}

#[test]
fn only_the_authority_renews_leases_and_expiry_makes_the_object_unavailable() {
    let mut pair = Pair::new();
    let remote = remote_object();
    let local = local_object();
    let future = pair.now + HOME_LEASE_MS + 1;

    // The authority renews its own homes from its confirmed view.
    pair.a.tick(future, &pair.signer);
    let step = pair.a.begin_operation(local, future, &pair.signer);
    pair.record(step.clone(), true);
    assert!(step
        .events
        .iter()
        .any(|event| matches!(event, Event::Result { .. })));

    // The member cannot: its lease expires and the object is not re-homed.
    pair.b.on_authority_lost();
    pair.b.tick(future, &pair.signer);
    assert!(pair.b.home_lease(&remote).is_none());
    let step = pair.b.begin_operation(remote, future + 1, &pair.signer);
    pair.record(step.clone(), false);
    assert!(step.send.is_none());
    assert!(step.events.iter().any(|event| matches!(
        event,
        Event::Reject {
            reason: Reason::NotCurrent,
            ..
        }
    )));
}

#[test]
fn the_removed_node_is_never_used_as_a_router() {
    let mut pair = Pair::new();
    let object = remote_object();
    // A stale cached route to B exists.
    let _ = pair.a.begin_operation(object, pair.now, &pair.signer);
    pair.deliver();
    // After the view change the removed node has no lease, so it cannot
    // answer MOVED for an object it no longer homes.
    pair.b
        .on_snapshot(3, &[A], other_digest(0x44), pair.now + 1);
    let capability = pair.capability(object, B, 2);
    let wire = pair.invoke(object, B, 2, 9, OP_STAT, capability);
    let step = pair.deliver_raw(wire);
    assert!(step.send.is_none(), "the removed node does not answer");
    assert!(step.events.iter().any(|event| matches!(
        event,
        Event::Reject {
            reason: Reason::StaleEpoch,
            ..
        }
    )));
    assert!(!pair
        .b_events
        .iter()
        .any(|event| matches!(event, Event::Moved { .. })));
}

#[test]
fn the_write_lease_expires_before_the_failure_window() {
    let mut pair = Pair::new();
    let object = remote_object();
    // The member's write lease was renewed at the snapshot (t=0).
    assert_eq!(WRITE_LEASE_MS, 6_000);
    assert!(pair.b.mutable_available_at(WRITE_LEASE_MS));
    assert!(!pair.b.mutable_available_at(WRITE_LEASE_MS + 1));
    // A commit at the home after the write lease expired fails closed even
    // though the read lease is still valid.
    pair.now = WRITE_LEASE_MS + 1;
    let mut capability = pair.capability(object, B, 2);
    capability.rights = Rights::READ.union(Rights::WRITE);
    capability.signature = pair.signer.sign(&capability.signed_bytes());
    let wire = pair.invoke(object, B, 2, 20, OP_COMMIT, capability);
    let step = pair.deliver_raw(wire);
    assert_eq!(error_reason(&step), Reason::NoAuthority);

    // A read is still served from the unexpired home lease.
    let capability = pair.capability(object, B, 2);
    let wire = pair.invoke(object, B, 2, 21, OP_STAT, capability);
    let step = pair.deliver_raw(wire);
    assert_eq!(step.send.as_ref().map(|wire| wire.status), Some(Status::Ok));

    // One accepted frame renews the write lease; the commit completes.
    pair.b.on_peer_frame(pair.now);
    let mut capability = pair.capability(object, B, 2);
    capability.rights = Rights::READ.union(Rights::WRITE);
    capability.signature = pair.signer.sign(&capability.signed_bytes());
    let wire = pair.invoke(object, B, 2, 22, OP_COMMIT, capability);
    let step = pair.deliver_raw(wire);
    assert_eq!(step.send.as_ref().map(|wire| wire.status), Some(Status::Ok));
}

#[test]
fn the_write_lease_is_shorter_than_the_failure_window() {
    // `mesh_service::FAIL_AFTER_MS` is 12 s (two missed 5 s heartbeats) and
    // the module carries a compile-time assertion that the write lease is
    // shorter.  The handover argument: after the link is cut the old home can
    // accept a commit for at most WRITE_LEASE_MS after its last frame, while
    // the root re-homes only after 12 s of silence.  In the worst case the
    // member's last received frame is one heartbeat (5 s) later than the
    // root's last received frame:
    //   member writes until root_last + 5 s + 6 s = root_last + 11 s
    //   root re-homes at root_last + 12 s
    // so the old home is fenced before the new home can exist.
    const FAIL_AFTER_MS: u64 = 12_000;
    const HEARTBEAT_MS: u64 = 5_000;
    const _: () = assert!(WRITE_LEASE_MS < FAIL_AFTER_MS);
    const _: () = assert!(HEARTBEAT_MS + WRITE_LEASE_MS < FAIL_AFTER_MS);
}

#[test]
fn a_fresh_operation_at_a_moved_home_is_not_mistaken_for_a_stale_token() {
    let mut pair = Pair::new();
    let object = remote_object();
    // Move the object from B to A using the root-signed directive.
    let step = pair.a.start_move(object, B, A, 2, pair.now, &pair.signer);
    pair.record(step, true);
    pair.deliver();
    // Step 3-style retry uses the old-home token and succeeds.
    let step = pair.a.begin_operation(object, pair.now + 1, &pair.signer);
    pair.record(step, true);
    pair.deliver();
    // A later fresh read at the new home issues a current-home token; it must
    // be accepted, not treated as a stale capability.
    let step = pair.a.begin_operation(object, pair.now + 2, &pair.signer);
    pair.record(step.clone(), true);
    assert!(step.send.is_none());
    assert!(step.events.iter().any(|event| matches!(
        event,
        Event::Result { home, .. } if *home == A
    )));
    assert!(!pair.a_events.iter().any(|event| matches!(
        event,
        Event::Reject {
            reason: Reason::StaleCapability,
            ..
        }
    )));
}

#[test]
fn step_6_reroutes_the_object_of_the_last_stale_wire_frame() {
    let mut pair = Pair::new();
    let remote = remote_object();
    let local = local_object();
    // The M2-4 sequence, compressed: step 1 remote stat, step 2 move to A,
    // step 3 stale stat, step 4 move to B, step 5 local MOVED with a wire
    // retry.  The last frame on the wire names B for `local`.
    let step = pair.a.begin_operation(remote, pair.now, &pair.signer);
    pair.record(step, true);
    pair.deliver();
    let step = pair.a.start_move(remote, B, A, 2, pair.now, &pair.signer);
    pair.record(step, true);
    pair.deliver();
    let step = pair.a.begin_operation(remote, pair.now, &pair.signer);
    pair.record(step, true);
    pair.deliver();
    let step = pair.a.start_move(local, A, B, 2, pair.now, &pair.signer);
    pair.record(step, true);
    pair.deliver();
    let step = pair.a.begin_operation(local, pair.now, &pair.signer);
    pair.record(step.clone(), true);
    assert_eq!(step.send.as_ref().map(|wire| wire.dst), Some(B));
    pair.deliver();

    // The member leaves the view.  The root must re-route `local` locally.
    let step = pair
        .a
        .on_snapshot(3, &[A], other_digest(0x55), pair.now + 1)
        .expect("snapshot");
    for event in &step.events {
        pair.a.note_event(event);
    }
    let step = pair.a.command(6, pair.now + 2, &pair.signer);
    assert!(
        step.send.is_none(),
        "step 6 must not contact the removed node"
    );
    assert!(step.events.iter().any(|event| matches!(
        event,
        Event::Result {
            object,
            home,
            attempts: 1,
            moved: false,
            ..
        } if *object == local && *home == A
    )));
    assert!(!step.events.iter().any(|event| matches!(
        event,
        Event::Reject {
            reason: Reason::NotCurrent | Reason::NoPeer,
            ..
        }
    )));
}

#[test]
fn authority_frames_renew_read_leases_but_loss_stops_them() {
    let mut pair = Pair::new();
    let object = remote_object();
    let initial = pair.b.home_lease(&object).expect("lease").lease_until_ms;
    // A frame from the authority renews the read lease on the existing home.
    pair.b.on_peer_frame(100_000);
    let renewed = pair.b.home_lease(&object).expect("lease").lease_until_ms;
    assert!(renewed > initial);
    assert!(renewed >= 100_000 + HOME_LEASE_MS);
    // The authority renews its own read leases at half-life and serves a read.
    let local = local_object();
    let future = pair.now + HOME_LEASE_MS + 1;
    pair.a.tick(future, &pair.signer);
    let step = pair.a.begin_operation(local, future, &pair.signer);
    pair.record(step.clone(), true);
    assert!(step
        .events
        .iter()
        .any(|event| matches!(event, Event::Result { .. })));
    // After authority loss no frame renews the member's lease; the object is
    // dropped and is unavailable at the renewed expiry.
    pair.b.on_authority_lost();
    pair.b.tick(renewed + 1, &pair.signer);
    assert!(pair.b.home_lease(&object).is_none());
    let step = pair.b.begin_operation(object, renewed + 2, &pair.signer);
    pair.record(step.clone(), false);
    assert!(step.events.iter().any(|event| matches!(
        event,
        Event::Reject {
            reason: Reason::NoAuthority | Reason::NotCurrent,
            ..
        }
    )));
}
