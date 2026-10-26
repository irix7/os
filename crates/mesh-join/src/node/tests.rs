// SPDX-License-Identifier: GPL-3.0-only
//! Join state-machine tests.
//!
//! These drive the real `mesh-service` table and the real `cap-engine`
//! Ed25519 proof with an injected clock.  The iris rig is the end-to-end
//! evidence; these tests cover the failure paths that are hard to stage on
//! two live kernels.

use alloc::vec;
use alloc::vec::Vec;

use super::*;
use crate::wire::{EpochConfirmation, JoinAck, JoinRequest, MembershipSnapshot, RefusalReason};
use sys_primitives::NodeKey;

fn node(byte: u8) -> NodeId {
    NodeId([byte; 16])
}

fn signing(seed: u8) -> SigningAuthority {
    SigningAuthority::from_bytes(&[seed; 32])
}

fn key(seed: u8) -> NodeKey {
    signing(seed).public()
}

struct Pair {
    root: JoinNode,
    member: JoinNode,
    root_id: NodeId,
    member_id: NodeId,
}

fn pair() -> Pair {
    let root_id = node(1);
    let member_id = node(2);
    let root = JoinNode::new_root(root_id, signing(9), 1, 0).expect("root");
    let member = JoinNode::new_member(member_id, root_id, key(9), 1, 0).expect("member");
    Pair {
        root,
        member,
        root_id,
        member_id,
    }
}

/// Run one complete join and return the confirmation the root sent.
fn join(pair: &mut Pair, now: u64) -> EpochConfirmation {
    let request = pair.member.on_link_up(now).expect("member request");
    assert_eq!(pair.root.on_link_up(now), None);
    let outcome = pair
        .root
        .on_message(pair.member_id, JoinMessage::Request(request), now);
    let confirmation = match outcome.reply {
        Some(JoinMessage::Confirm(confirmation)) => confirmation,
        other => panic!("expected confirmation, got {other:?}"),
    };
    assert!(matches!(outcome.event, JoinEvent::Confirmed(_)));
    let outcome = pair.member.on_message(
        pair.root_id,
        JoinMessage::Confirm(confirmation.clone()),
        now,
    );
    let ack = match outcome.reply {
        Some(JoinMessage::Ack(ack)) => ack,
        other => panic!("expected ack, got {other:?}"),
    };
    match outcome.event {
        JoinEvent::Converged(snapshot) => {
            assert_eq!(snapshot, pair.member.snapshot());
        }
        other => panic!("expected convergence, got {other:?}"),
    }
    let outcome = pair
        .root
        .on_message(pair.member_id, JoinMessage::Ack(ack), now);
    assert!(matches!(outcome.event, JoinEvent::Acked { .. }));
    confirmation
}

#[test]
fn two_nodes_converge_on_one_epoch_and_snapshot() {
    let mut pair = pair();
    assert_eq!(pair.root.role(), Role::Root);
    assert_eq!(pair.member.role(), Role::Member);
    assert_eq!(pair.root.epoch(), 1);
    assert_eq!(pair.member.epoch(), 1);
    assert!(pair.member.converged().is_none());

    let confirmation = join(&mut pair, 1_000);
    assert_eq!(confirmation.epoch, 2);
    assert_eq!(confirmation.members, vec![node(1), node(2)]);
    assert_eq!(pair.root.epoch(), 2);
    assert_eq!(pair.member.epoch(), 2);
    let root_snapshot = pair.root.snapshot();
    let member_snapshot = pair.member.snapshot();
    assert_eq!(root_snapshot, member_snapshot);
    assert_eq!(root_snapshot.digest(), member_snapshot.digest());
    assert_eq!(pair.member.converged(), Some(&member_snapshot));
    // The root is the only epoch writer: one join advanced it once.
    assert_eq!(root_snapshot.epoch, 2);
}

#[test]
fn wrong_authority_key_fails_closed() {
    let root_id = node(1);
    let member_id = node(2);
    let mut root = JoinNode::new_root(root_id, signing(9), 1, 0).expect("root");
    let mut member = JoinNode::new_member(member_id, root_id, key(10), 1, 0).expect("member");
    let request = member.on_link_up(0).expect("request");
    let outcome = root.on_message(member_id, JoinMessage::Request(request), 0);
    let confirmation = match outcome.reply {
        Some(JoinMessage::Confirm(confirmation)) => confirmation,
        other => panic!("expected confirmation, got {other:?}"),
    };
    let outcome = member.on_message(root_id, JoinMessage::Confirm(confirmation), 0);
    assert_eq!(outcome.reply, None);
    assert_eq!(outcome.event, JoinEvent::Rejected("bad-signature"));
    assert_eq!(member.epoch(), 1);
    assert!(member.converged().is_none());
    // A rejected confirmation is terminal for this attempt; the member keeps
    // its seed view and does not storm the root.
    assert_eq!(member.poll(RETRY_INTERVAL_MS * 2), Tick::Idle);
}

#[test]
fn wrong_root_identity_fails_closed() {
    let root_id = node(1);
    let member_id = node(2);
    let mut root = JoinNode::new_root(root_id, signing(9), 1, 0).expect("root");
    // The member trusts the right key but the wrong root identity.
    let mut member = JoinNode::new_member(member_id, node(3), key(9), 1, 0).expect("member");
    let request = member.on_link_up(0).expect("request");
    let outcome = root.on_message(member_id, JoinMessage::Request(request), 0);
    let confirmation = match outcome.reply {
        Some(JoinMessage::Confirm(confirmation)) => confirmation,
        other => panic!("expected confirmation, got {other:?}"),
    };
    let outcome = member.on_message(root_id, JoinMessage::Confirm(confirmation), 0);
    assert_eq!(outcome.event, JoinEvent::Rejected("wrong-root"));
    assert_eq!(member.epoch(), 1);
}

#[test]
fn duplicate_confirmation_is_idempotent_and_a_new_epoch_is_not() {
    let mut pair = pair();
    let first = join(&mut pair, 1_000);
    assert_eq!(pair.member.epoch(), 2);

    // A rejoin at the same epoch: a new request and the same member list.
    let request = pair.member.on_link_up(2_000).expect("rejoin request");
    assert_eq!(request.last_epoch, 2);
    let outcome = pair
        .root
        .on_message(pair.member_id, JoinMessage::Request(request), 2_000);
    let confirmation = match outcome.reply {
        Some(JoinMessage::Confirm(confirmation)) => confirmation,
        other => panic!("expected confirmation, got {other:?}"),
    };
    let outcome = pair
        .member
        .on_message(pair.root_id, JoinMessage::Confirm(confirmation), 2_000);
    assert!(matches!(outcome.event, JoinEvent::Duplicate(ref snapshot) if snapshot.epoch == 2));
    assert_eq!(pair.member.epoch(), 2);
    assert_eq!(pair.member.snapshot(), pair.root.snapshot());
    // The duplicate is terminal; the member does not retry the rejoin.
    assert_eq!(pair.member.poll(2_000 + RETRY_INTERVAL_MS * 2), Tick::Idle);
    let _ = first;
}

#[test]
fn same_epoch_with_a_different_list_is_an_equivocation() {
    let verifier = crate::authority::AuthorityVerifier::new(node(1), key(9)).expect("verifier");
    let previous = MembershipSnapshot::new(2, &[node(1), node(2)]).expect("snapshot");
    let mut confirmation = EpochConfirmation {
        root: node(1),
        epoch: 2,
        members: vec![node(1), node(2), node(3)],
        subject: node(2),
        request_nonce: 7,
        incarnation: 8,
        signature: [0u8; 64],
    };
    confirmation.signature = signing(9).sign(&confirmation.signed_bytes());
    let expectation = Expectation {
        subject: node(2),
        nonce: 7,
        incarnation: 8,
        minimum_epoch: 2,
        previous: Some(previous),
    };
    assert_eq!(
        verifier.verify(&confirmation, &expectation),
        Err(AuthorityError::EpochConflict)
    );
}

#[test]
fn a_member_claiming_the_root_epoch_with_a_different_view_is_refused() {
    let mut pair = pair();
    join(&mut pair, 1_000);
    let request = JoinRequest {
        node: pair.member_id,
        incarnation: 9,
        last_epoch: 2,
        last_digest: [0u8; 16],
        nonce: 10,
    };
    let outcome = pair
        .root
        .on_message(pair.member_id, JoinMessage::Request(request), 2_000);
    let refusal = match outcome.reply {
        Some(JoinMessage::Refused(refusal)) => refusal,
        other => panic!("expected refusal, got {other:?}"),
    };
    assert_eq!(refusal.reason, RefusalReason::EpochConflict);
    assert_eq!(pair.root.epoch(), 2);
}

#[test]
fn stale_epoch_is_rejected() {
    let verifier = crate::authority::AuthorityVerifier::new(node(1), key(9)).expect("verifier");
    let mut confirmation = EpochConfirmation {
        root: node(1),
        epoch: 2,
        members: vec![node(1), node(2)],
        subject: node(2),
        request_nonce: 7,
        incarnation: 8,
        signature: [0u8; 64],
    };
    confirmation.signature = signing(9).sign(&confirmation.signed_bytes());
    let expectation = Expectation {
        subject: node(2),
        nonce: 7,
        incarnation: 8,
        minimum_epoch: 3,
        previous: None,
    };
    assert_eq!(
        verifier.verify(&confirmation, &expectation),
        Err(AuthorityError::EpochRegression {
            local: 3,
            remote: 2
        })
    );
}

#[test]
fn member_restart_rejoins_at_the_same_epoch() {
    let mut pair = pair();
    join(&mut pair, 1_000);
    let epoch = pair.root.epoch();
    let digest = pair.root.snapshot().digest();

    // B restarts with the configured seed and a new attempt sequence.
    let mut restarted =
        JoinNode::new_member(pair.member_id, pair.root_id, key(9), 1, 3_000).expect("member");
    let request = restarted.on_link_up(3_000).expect("rejoin request");
    assert_eq!(request.last_epoch, 1);
    let outcome = pair
        .root
        .on_message(pair.member_id, JoinMessage::Request(request), 3_000);
    let confirmation = match outcome.reply {
        Some(JoinMessage::Confirm(confirmation)) => confirmation,
        other => panic!("expected confirmation, got {other:?}"),
    };
    assert_eq!(confirmation.epoch, epoch);
    let outcome = restarted.on_message(pair.root_id, JoinMessage::Confirm(confirmation), 3_000);
    assert!(matches!(outcome.event, JoinEvent::Converged(_)));
    assert_eq!(restarted.epoch(), epoch);
    assert_eq!(restarted.snapshot().digest(), digest);
    assert_eq!(pair.root.epoch(), epoch);
    assert_eq!(restarted.snapshot(), pair.root.snapshot());
}

#[test]
fn root_restart_refuses_to_lower_a_member_epoch() {
    let mut pair = pair();
    join(&mut pair, 1_000);
    let epoch = pair.member.epoch();
    let digest = pair.member.snapshot().digest();

    // The root restarts from the seed and does not remember epoch 2.
    let mut restarted_root = JoinNode::new_root(pair.root_id, signing(9), 1, 4_000).expect("root");
    assert_eq!(restarted_root.epoch(), 1);
    let request = pair.member.on_link_up(4_000).expect("rejoin request");
    assert_eq!(request.last_epoch, 2);
    let outcome = restarted_root.on_message(pair.member_id, JoinMessage::Request(request), 4_000);
    let refusal = match outcome.reply {
        Some(JoinMessage::Refused(refusal)) => refusal,
        other => panic!("expected refusal, got {other:?}"),
    };
    assert_eq!(refusal.reason, RefusalReason::EpochRegression);
    assert_eq!(refusal.epoch, 1);
    let outcome = pair
        .member
        .on_message(pair.root_id, JoinMessage::Refused(refusal), 4_000);
    assert_eq!(
        outcome.event,
        JoinEvent::Refused {
            reason: RefusalReason::EpochRegression,
            epoch: 1
        }
    );
    // The member keeps its confirmed view; it does not follow the root down.
    assert_eq!(pair.member.epoch(), epoch);
    assert_eq!(pair.member.snapshot().digest(), digest);
    assert_eq!(restarted_root.epoch(), 1);
}

#[test]
fn retry_reuses_the_request_and_then_times_out() {
    let mut pair = pair();
    let request = pair.member.on_link_up(0).expect("request");
    assert_eq!(pair.member.poll(RETRY_INTERVAL_MS - 1), Tick::Idle);
    let mut sends = 1;
    let mut now = 0;
    loop {
        now += RETRY_INTERVAL_MS;
        match pair.member.poll(now) {
            Tick::Send(retry) => {
                assert_eq!(retry, request, "a retry must reuse the same nonce");
                sends += 1;
            }
            Tick::TimedOut => break,
            Tick::Idle => panic!("expected progress"),
        }
        assert!(sends <= MAX_ATTEMPTS);
    }
    assert_eq!(sends, MAX_ATTEMPTS);
    // No response: the member stays on its seed view and stops retrying.
    assert_eq!(pair.member.epoch(), 1);
    assert!(pair.member.converged().is_none());
    assert_eq!(pair.member.poll(now + RETRY_INTERVAL_MS), Tick::Idle);
}

#[test]
fn partition_after_convergence_keeps_the_last_view() {
    let mut pair = pair();
    join(&mut pair, 1_000);
    let epoch = pair.member.epoch();
    let digest = pair.member.snapshot().digest();

    // The link drops after convergence.  The member asks again with its
    // current epoch, but the root never answers.
    pair.member.on_link_down();
    let request = pair.member.on_link_up(2_000).expect("request");
    assert_eq!(request.last_epoch, epoch);
    let mut now = 2_000;
    loop {
        now += RETRY_INTERVAL_MS;
        match pair.member.poll(now) {
            Tick::Send(_) => {}
            Tick::TimedOut => break,
            Tick::Idle => panic!("expected progress"),
        }
    }
    // No response is not permission to invent an epoch or a member list.
    assert_eq!(pair.member.epoch(), epoch);
    assert_eq!(pair.member.snapshot().digest(), digest);
    assert_eq!(pair.member.converged(), Some(&pair.member.snapshot()));
}

#[test]
fn replaying_a_confirmation_is_rejected() {
    let mut pair = pair();
    let confirmation = join(&mut pair, 1_000);
    assert_eq!(pair.member.epoch(), 2);

    // Replay after convergence: there is no outstanding request.
    let outcome = pair.member.on_message(
        pair.root_id,
        JoinMessage::Confirm(confirmation.clone()),
        2_000,
    );
    assert_eq!(outcome.event, JoinEvent::Rejected("no-outstanding-request"));
    assert_eq!(pair.member.epoch(), 2);

    // Replay during a fresh attempt: the nonce does not match.
    let _request = pair.member.on_link_up(3_000).expect("new attempt");
    let outcome = pair
        .member
        .on_message(pair.root_id, JoinMessage::Confirm(confirmation), 3_000);
    assert_eq!(outcome.event, JoinEvent::Rejected("nonce-mismatch"));
    assert_eq!(pair.member.epoch(), 2);
}

#[test]
fn replaying_a_join_request_is_idempotent_at_the_root() {
    let mut pair = pair();
    let confirmation = join(&mut pair, 1_000);
    assert_eq!(pair.root.epoch(), 2);

    // An old request carries the old nonce; the root answers with the current
    // epoch and list and does not advance the epoch.
    let request = JoinRequest {
        node: pair.member_id,
        incarnation: confirmation.incarnation,
        last_epoch: 1,
        last_digest: [0u8; 16],
        nonce: confirmation.request_nonce,
    };
    let outcome = pair
        .root
        .on_message(pair.member_id, JoinMessage::Request(request), 2_000);
    let replay = match outcome.reply {
        Some(JoinMessage::Confirm(replay)) => replay,
        other => panic!("expected confirmation, got {other:?}"),
    };
    assert_eq!(replay.epoch, 2);
    assert_eq!(replay.members, vec![node(1), node(2)]);
    assert_eq!(pair.root.epoch(), 2);
}

#[test]
fn malformed_or_misaddressed_messages_do_not_change_the_view() {
    let mut pair = pair();
    // A request whose body subject is not the envelope source.
    let request = pair.member.on_link_up(0).expect("request");
    let outcome = pair
        .root
        .on_message(node(3), JoinMessage::Request(request), 0);
    assert_eq!(outcome.reply, None);
    assert_eq!(outcome.event, JoinEvent::Rejected("request-subject"));
    assert_eq!(pair.root.epoch(), 1);

    // A confirmation whose envelope source is not its root.
    let confirmation = join(&mut pair, 1_000);
    let outcome = pair
        .member
        .on_message(node(4), JoinMessage::Confirm(confirmation), 2_000);
    assert_eq!(outcome.event, JoinEvent::Rejected("confirm-source"));

    // An ack that claims a different epoch.
    let outcome = pair.root.on_message(
        pair.member_id,
        JoinMessage::Ack(JoinAck {
            node: pair.member_id,
            epoch: 9,
            digest: [0u8; 16],
            request_nonce: 1,
        }),
        3_000,
    );
    assert_eq!(outcome.event, JoinEvent::Rejected("ack-mismatch"));
    assert_eq!(pair.root.epoch(), 2);
}

#[test]
fn verifier_rejects_wrong_subject_nonce_and_incarnation() {
    let verifier = crate::authority::AuthorityVerifier::new(node(1), key(9)).expect("verifier");
    let snapshot = MembershipSnapshot::new(2, &[node(1), node(2)]).expect("snapshot");
    let mut confirmation = EpochConfirmation {
        root: node(1),
        epoch: snapshot.epoch,
        members: snapshot.members.clone(),
        subject: node(2),
        request_nonce: 7,
        incarnation: 8,
        signature: [0u8; 64],
    };
    confirmation.signature = signing(9).sign(&confirmation.signed_bytes());
    let base = Expectation {
        subject: node(2),
        nonce: 7,
        incarnation: 8,
        minimum_epoch: 1,
        previous: None,
    };
    assert_eq!(verifier.verify(&confirmation, &base), Ok(snapshot.clone()));

    let mut wrong = base.clone();
    wrong.subject = node(3);
    assert_eq!(
        verifier.verify(&confirmation, &wrong),
        Err(AuthorityError::WrongSubject)
    );
    let mut wrong = base.clone();
    wrong.nonce = 9;
    assert_eq!(
        verifier.verify(&confirmation, &wrong),
        Err(AuthorityError::NonceMismatch)
    );
    let mut wrong = base;
    wrong.incarnation = 9;
    assert_eq!(
        verifier.verify(&confirmation, &wrong),
        Err(AuthorityError::IncarnationMismatch)
    );

    let mut tampered = confirmation;
    tampered.epoch = 3;
    let expectation = Expectation {
        subject: node(2),
        nonce: 7,
        incarnation: 8,
        minimum_epoch: 1,
        previous: None,
    };
    assert_eq!(
        verifier.verify(&tampered, &expectation),
        Err(AuthorityError::BadSignature)
    );
}

#[test]
fn new_member_rejects_a_zero_root_key() {
    assert_eq!(
        JoinNode::new_member(node(2), node(1), NodeKey([0u8; 32]), 1, 0).err(),
        Some(AuthorityError::ZeroKey)
    );
    assert_eq!(
        JoinNode::new_root(NodeId::ZERO, signing(9), 1, 0).err(),
        Some(AuthorityError::ZeroRoot)
    );
    assert_eq!(
        JoinNode::new_root(node(1), signing(9), 0, 0).err(),
        Some(AuthorityError::BadSeed)
    );
}

#[test]
fn root_and_member_snapshot_digests_are_computed_from_the_same_canonical_bytes() {
    let mut pair = pair();
    join(&mut pair, 0);
    let root = pair.root.snapshot();
    let member = pair.member.snapshot();
    assert_eq!(root.canonical_bytes(), member.canonical_bytes());
    let expected: Vec<u8> = {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(MembershipSnapshot::DOMAIN);
        bytes.extend_from_slice(MembershipSnapshot::MAGIC);
        bytes.push(crate::SNAPSHOT_VERSION);
        bytes.push(0);
        bytes.extend_from_slice(&2u32.to_le_bytes());
        bytes.extend_from_slice(&2u16.to_le_bytes());
        bytes.extend_from_slice(&0u16.to_le_bytes());
        bytes.extend_from_slice(node(1).as_bytes());
        bytes.extend_from_slice(node(2).as_bytes());
        bytes
    };
    assert_eq!(root.canonical_bytes(), expected);
}
