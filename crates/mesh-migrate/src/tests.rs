// SPDX-License-Identifier: GPL-3.0-only
//! Host tests for the L0 migration policy.

use alloc::vec::Vec;

use cap_engine::SigningAuthority;
use sys_primitives::{Capability, NodeId, NodeKey, ObjectId, PrincipalId, Rights};

use crate::wire::{ActorMove, Body, Wire};
use crate::*;

const N0: NodeId = NodeId([0x10; 16]);
const N1: NodeId = NodeId([0x20; 16]);
const DIGEST: [u8; 16] = [0x5a; 16];

struct TestSigner(SigningAuthority);

impl Signer for TestSigner {
    fn key(&self) -> NodeKey {
        self.0.public()
    }

    fn sign(&self, message: &[u8]) -> Option<[u8; 64]> {
        Some(self.0.sign(message))
    }
}

struct Harness {
    a: MigrateNode,
    b: MigrateNode,
    signer: TestSigner,
    now: u64,
}

impl Harness {
    fn new() -> Self {
        let signer = TestSigner(SigningAuthority::from_bytes(&[0x33; 32]));
        let key = signer.0.public();
        let mut a = MigrateNode::new(N0, N0, key, true).expect("root");
        let mut b = MigrateNode::new(N1, N0, key, false).expect("member");
        let members = [N0, N1];
        a.on_snapshot(2, &members, DIGEST).expect("root view");
        b.on_snapshot(2, &members, DIGEST).expect("member view");
        a.set_peer(N1);
        b.set_peer(N0);
        Self {
            a,
            b,
            signer,
            now: 10_000,
        }
    }

    fn exchange(&mut self, start_on_a: bool, step: Step) -> Vec<Event> {
        let mut current = step;
        let mut events = core::mem::take(&mut current.events);
        let mut sender_is_a = start_on_a;
        for _ in 0..12 {
            let Some(wire) = current.send.take() else {
                break;
            };
            self.now += 10;
            let (src, dst) = if sender_is_a {
                (N0, &mut self.b)
            } else {
                (N1, &mut self.a)
            };
            let header = wire.header(src);
            current = dst.on_wire(
                src,
                &header,
                wire.kind,
                wire.status,
                &wire.body,
                self.now,
                &self.signer,
            );
            events.extend(current.events.iter().cloned());
            sender_is_a = !sender_is_a;
        }
        events
    }

    fn command(&mut self, step: u8) -> Vec<Event> {
        let directive = self.a.command(step, self.now, &self.signer);
        self.exchange(true, directive)
    }
}

#[test]
fn state_codec_round_trips_and_hashes_the_logical_state() {
    let actor = actor_for(0);
    let mut state = ActorState::new(actor, N0, N0, Some(Handle::fixture(N0)));
    let first = message_for(0);
    let queued = message_for(1);
    assert!(matches!(
        state.deliver(first, DeliveryMode::Step),
        Delivery::Applied { .. }
    ));
    assert!(matches!(
        state.deliver(queued, DeliveryMode::Queue),
        Delivery::Queued { .. }
    ));
    let hash = state.hash();
    let mut bytes = [0u8; STATE_LEN];
    let len = state.encode(&mut bytes).expect("encode");
    assert_eq!(len, STATE_LEN);
    let decoded = ActorState::decode(&bytes).expect("decode");
    assert_eq!(decoded, state);
    assert_eq!(decoded.hash(), hash);
    assert_eq!(state.queued, Some(queued));
    assert_eq!(state.unacked.expect("unacked").id, first.id);

    // Placement is not part of the logical hash.
    let mut moved = state.clone();
    moved.place(N1, 2).expect("place");
    assert_eq!(moved.hash(), hash);

    // Truncation, extra bytes, unknown schema and non-canonical absent fields
    // all fail closed.
    assert_eq!(
        ActorState::decode(&bytes[..STATE_LEN - 1]),
        Err(Error::Truncated)
    );
    let mut over = bytes;
    over[STATE_LEN - 1] = 0;
    let mut extra = [0u8; STATE_LEN + 1];
    extra[..STATE_LEN].copy_from_slice(&over);
    assert_eq!(ActorState::decode(&extra), Err(Error::BadLength));
    let mut schema = over;
    schema[4] ^= 0xff;
    assert_eq!(ActorState::decode(&schema), Err(Error::UnknownSchema));
    let mut noncanonical = over;
    noncanonical[108] = 0;
    noncanonical[149] = 1;
    assert_eq!(ActorState::decode(&noncanonical), Err(Error::NonCanonical));
}

#[test]
fn message_boundary_hash_chain_is_deterministic() {
    let actor = actor_for(0);
    let mut state = ActorState::new(actor, N0, N0, None);
    let first = message_for(0);
    let queued = message_for(1);
    let initial = state.hash();
    let applied = state.deliver(first, DeliveryMode::Step);
    let after_first = applied.hash().expect("hash");
    assert_ne!(initial, after_first);
    assert_eq!(
        state.deliver(first, DeliveryMode::Step),
        Delivery::Duplicate {
            id: first.id,
            applied: 1,
            logical: state.logical,
            hash: after_first,
        }
    );
    assert!(matches!(
        state.deliver(queued, DeliveryMode::Queue),
        Delivery::Queued { .. }
    ));
    let queued_hash = state.hash();
    assert_ne!(queued_hash, after_first);
    let resumed = state.resume();
    assert!(matches!(resumed, Delivery::Applied { id, applied: 2, .. } if id == queued.id));
    assert_eq!(state.applied, 2);
    assert_eq!(state.last, queued.id);
    assert_eq!(state.queued, None);
    assert_eq!(state.unacked.expect("ack").id, queued.id);
    assert_eq!(
        state.deliver(queued, DeliveryMode::Step),
        Delivery::Duplicate {
            id: queued.id,
            applied: 2,
            logical: state.logical,
            hash: state.hash(),
        }
    );
}

#[test]
fn one_way_and_return_migration_preserve_the_state_hash() {
    let mut h = Harness::new();
    let events = h.command(1);
    assert!(events
        .iter()
        .any(|event| matches!(event, Event::Seed { .. })));
    assert!(h.a.actor().is_some());
    let boundary_hash = h.a.actor().expect("actor").hash();
    let boundary_applied = h.a.actor().expect("actor").applied;
    let boundary_queued = h.a.actor().expect("actor").queued.expect("queued");

    let events = h.command(2);
    assert!(events
        .iter()
        .any(|event| matches!(event, Event::Moved { role: "source", .. })));
    h.a.build_report();
    h.b.build_report();
    let a = h.a.report().actor.clone().expect("root fence row");
    let b = h.b.report().actor.clone().expect("member actor row");
    assert_eq!(a.state, "fenced");
    assert_eq!(b.state, "active");
    assert_eq!(a.hash, b.hash);
    assert_eq!(a.hash, boundary_hash);
    assert_eq!(b.host, N1);
    assert_eq!(b.epoch, 2);
    assert_eq!(b.applied, boundary_applied);
    assert_eq!(b.queued, Some(boundary_queued));
    assert_eq!(a.home, b.home);

    // Resume at the destination: the queued message is applied once.
    let events = h.command(3);
    assert!(events
        .iter()
        .any(|event| matches!(event, Event::Resume { .. })));
    h.b.build_report();
    let after_resume = h.b.report().actor.clone().expect("member actor");
    assert_eq!(after_resume.applied, 2);
    assert_eq!(after_resume.queued, None);
    assert_ne!(after_resume.hash, boundary_hash);

    // Queue the next message, then move back.
    let events = h.command(6);
    assert!(events
        .iter()
        .any(|event| matches!(event, Event::Queued { .. })));
    h.b.build_report();
    let before_return = h.b.report().actor.clone().expect("member actor");
    let events = h.command(7);
    assert!(
        events.iter().any(|event| matches!(
            event,
            Event::Moved {
                role: "destination",
                ..
            }
        )),
        "events: {events:?}"
    );
    h.a.build_report();
    h.b.build_report();
    let a = h.a.report().actor.clone().expect("root actor row");
    let b = h.b.report().actor.clone().expect("member fence row");
    assert_eq!(a.state, "active");
    assert_eq!(b.state, "fenced");
    assert_eq!(a.hash, b.hash);
    assert_eq!(a.hash, before_return.hash);
    assert_eq!(a.host, N0);
    assert_eq!(a.epoch, 3);
    assert_eq!(a.queued, before_return.queued);

    // Resume at the returned home.
    let events = h.command(8);
    assert!(events
        .iter()
        .any(|event| matches!(event, Event::Resume { .. })));
    h.a.build_report();
    let final_row = h.a.report().actor.clone().expect("root actor");
    assert_eq!(final_row.applied, 3);
    assert_eq!(final_row.queued, None);
}

#[test]
fn stale_old_home_is_refused_after_each_move() {
    let mut h = Harness::new();
    h.command(1);
    h.command(2);
    let events = h.command(4);
    assert!(events.iter().any(|event| matches!(
        event,
        Event::Stale {
            reason: Error::StaleEpoch,
            expected_epoch: 2,
            got_epoch: 1,
            ..
        }
    )));
    h.a.build_report();
    assert_eq!(h.a.report().counters.stale, 1);

    h.command(3);
    h.command(6);
    h.command(7);
    let events = h.command(9);
    assert!(events.iter().any(|event| matches!(
        event,
        Event::Stale {
            reason: Error::StaleEpoch,
            ..
        }
    )));
    h.b.build_report();
    assert!(
        h.b.report().counters.stale >= 1,
        "b report: {:?}",
        h.b.report()
    );
}

#[test]
fn duplicate_delivery_is_idempotent_and_does_not_advance_state() {
    let mut h = Harness::new();
    h.command(1);
    h.command(2);
    h.command(3);
    h.b.build_report();
    let before = h.b.report().actor.clone().expect("actor");
    let events = h.command(5);
    assert!(
        events
            .iter()
            .any(|event| matches!(event, Event::Duplicate { applied: 2, .. })),
        "events: {events:?}"
    );
    h.b.build_report();
    let after = h.b.report().actor.clone().expect("actor");
    assert_eq!(after.hash, before.hash);
    assert_eq!(after.applied, before.applied);
    assert_eq!(h.b.report().counters.duplicates, 1);
}

#[test]
fn malformed_state_and_bad_hash_are_refused() {
    let mut h = Harness::new();
    h.command(1);
    // A Prepare reserves the actor; the wrong hash is caught when the state
    // arrives, before the destination installs anything.
    let actor = h.a.actor().expect("actor").clone();
    let mut directive = ActorMove::new(
        actor.actor,
        actor.home,
        N0,
        N1,
        2,
        DIGEST,
        actor.epoch,
        MAX_LEASE_MS,
    );
    directive.sign(&h.signer).expect("sign");
    let prepare = Body::Prepare {
        directive,
        state_len: STATE_LEN as u16,
        hash: [0xee; 32],
    };
    let mut bytes = [0u8; MAX_BODY];
    let len = prepare.encode(&mut bytes).expect("encode");
    let wire = Wire {
        dst: N1,
        kind: sys_primitives::MessageKind::Request,
        status: sys_primitives::Status::Ok,
        src_epoch: 2,
        dst_epoch: 2,
        home: directive.home,
        home_epoch: directive.old_epoch,
        actor: directive.actor,
        request_id: 99,
        reply_to: 0,
        body: bytes[..len].to_vec(),
    };
    let header = wire.header(N0);
    let step = h.b.on_wire(
        N0,
        &header,
        wire.kind,
        wire.status,
        &wire.body,
        h.now,
        &h.signer,
    );
    assert!(
        step.send.is_some(),
        "the destination should reserve: {:?}",
        step.events
    );

    let mut state = [0u8; STATE_LEN];
    actor.encode(&mut state).expect("state");
    let state_body = Body::State {
        directive,
        state: state.to_vec(),
    };
    let mut state_bytes = [0u8; MAX_BODY];
    let state_len = state_body.encode(&mut state_bytes).expect("encode");
    let state_wire = Wire {
        dst: N1,
        kind: sys_primitives::MessageKind::Request,
        status: sys_primitives::Status::Ok,
        src_epoch: 2,
        dst_epoch: 2,
        home: directive.home,
        home_epoch: directive.old_epoch,
        actor: directive.actor,
        request_id: 100,
        reply_to: 0,
        body: state_bytes[..state_len].to_vec(),
    };
    let state_header = state_wire.header(N0);
    let step = h.b.on_wire(
        N0,
        &state_header,
        state_wire.kind,
        state_wire.status,
        &state_wire.body,
        h.now,
        &h.signer,
    );
    assert!(
        step.events.iter().any(|event| matches!(
            event,
            Event::Reject {
                reason: Error::StaleLease,
                ..
            }
        )),
        "events: {:?}",
        step.events
    );
    assert!(
        h.b.actor().is_none(),
        "the destination must not install a bad hash"
    );

    // A truncated body is rejected by the decoder.
    assert!(matches!(
        Body::decode(&bytes[..len - 1]),
        Err(Error::Truncated)
    ));
    // A state whose schema is unknown is rejected, not silently decoded.
    let actor_state = h.a.actor().expect("actor");
    let mut schema_state = [0u8; STATE_LEN];
    actor_state.encode(&mut schema_state).expect("state");
    schema_state[4] ^= 0xff;
    assert_eq!(ActorState::decode(&schema_state), Err(Error::UnknownSchema));
}

#[test]
fn current_epoch_at_the_old_home_is_still_refused() {
    let mut h = Harness::new();
    h.command(1);
    h.command(2);
    // Move completed: the root is fenced at new epoch 2.  A delivery that
    // names the old host but the *new* epoch must still be refused as a stale
    // home, not served as if the actor were still resident.
    let fence = h.a.fence().expect("fence");
    let actor = fence.state.actor;
    let capability = Capability {
        magic: Capability::MAGIC,
        version: Capability::VERSION,
        rights: Rights::READ.union(Rights::INVOKE),
        flags: 0,
        object: actor_object(&actor),
        issuer: h.signer.key(),
        subject: actor_principal(&actor),
        home: N0,
        not_before: 0,
        not_after: 1_000_000_000_000,
        object_epoch: 2,
        home_epoch: 2,
        root_epoch: 1,
        reserved0: 0,
        nonce: actor.0,
        parent: ObjectId::ZERO,
        delegation: ObjectId::ZERO,
        reserved1: [0; 24],
        signature: [0; 64],
    };
    let body = Body::Deliver {
        actor,
        epoch: 2,
        capability,
        message: message_for(1),
        mode: DeliveryMode::Step.as_u8(),
    };
    let mut bytes = [0u8; MAX_BODY];
    let len = body.encode(&mut bytes).expect("encode");
    let wire = Wire {
        dst: N0,
        kind: sys_primitives::MessageKind::Request,
        status: sys_primitives::Status::Ok,
        src_epoch: 2,
        dst_epoch: 2,
        home: fence.state.home,
        home_epoch: 2,
        actor,
        request_id: 200,
        reply_to: 0,
        body: bytes[..len].to_vec(),
    };
    let header = wire.header(N1);
    let step = h.a.on_wire(
        N1,
        &header,
        wire.kind,
        wire.status,
        &wire.body,
        h.now,
        &h.signer,
    );
    assert!(
        step.events.iter().any(|event| matches!(
            event,
            Event::Reject {
                reason: Error::StaleHome,
                ..
            }
        )),
        "events: {:?}",
        step.events
    );
}

#[test]
fn capability_failure_is_refused() {
    let mut h = Harness::new();
    h.command(1);
    let events = h.command(11);
    assert!(events
        .iter()
        .any(|event| matches!(event, Event::Capability { .. })));
    h.a.build_report();
    assert_eq!(h.a.report().counters.capability_rejects, 1);
    assert_eq!(h.a.actor().expect("actor").applied, 1);
}

#[test]
fn wrong_authority_signature_is_refused() {
    let mut h = Harness::new();
    h.command(1);
    let actor = h.a.actor().expect("actor");
    let mut directive = ActorMove::new(
        actor.actor,
        actor.home,
        N0,
        N1,
        2,
        DIGEST,
        actor.epoch,
        MAX_LEASE_MS,
    );
    let wrong = SigningAuthority::from_bytes(&[0x99; 32]);
    directive.signature = wrong.sign(&directive.signed_bytes());
    let body = Body::Authorise(directive);
    let mut bytes = [0u8; MAX_BODY];
    let len = body.encode(&mut bytes).expect("encode");
    let event = h.b.on_wire(
        N0,
        &Wire {
            dst: N1,
            kind: sys_primitives::MessageKind::Request,
            status: sys_primitives::Status::Ok,
            src_epoch: 2,
            dst_epoch: 2,
            home: directive.home,
            home_epoch: directive.old_epoch,
            actor: directive.actor,
            request_id: 7,
            reply_to: 0,
            body: bytes[..len].to_vec(),
        }
        .header(N0),
        sys_primitives::MessageKind::Request,
        sys_primitives::Status::Ok,
        &bytes[..len],
        h.now,
        &h.signer,
    );
    assert!(event.events.iter().any(|event| matches!(
        event,
        Event::Reject {
            reason: Error::BadSignature,
            ..
        }
    )));
}

#[test]
fn delivery_capability_requires_the_actor_subject_and_home() {
    let mut h = Harness::new();
    h.command(1);
    let actor = h.a.actor().expect("actor");
    let mut capability = Capability {
        magic: Capability::MAGIC,
        version: Capability::VERSION,
        rights: Rights::READ,
        flags: 0,
        object: actor_object(&actor.actor),
        issuer: h.signer.key(),
        subject: PrincipalId([0; 32]),
        home: N0,
        not_before: 0,
        not_after: 1_000_000_000,
        object_epoch: 1,
        home_epoch: 1,
        root_epoch: 1,
        reserved0: 0,
        nonce: actor.actor.0,
        parent: ObjectId::ZERO,
        delegation: ObjectId::ZERO,
        reserved1: [0; 24],
        signature: [0; 64],
    };
    capability.signature = h.signer.0.sign(&capability.signed_bytes());
    let error =
        h.a.verify_capability(&capability, actor, N0, 1, 10_000)
            .expect_err("wrong subject");
    assert_eq!(error, Error::WrongSubject);
}
