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
fn eight_members_converge_through_bounded_signed_refresh() {
    let root_id = node(1);
    let member_ids: Vec<NodeId> = (2..=8).map(node).collect();
    let mut root = JoinNode::new_root(root_id, signing(9), 1, 0).expect("root");
    let mut members: Vec<JoinNode> = member_ids
        .iter()
        .map(|id| JoinNode::new_member(*id, root_id, key(9), 1, 0).expect("member"))
        .collect();

    // Each member joins in turn.  Every request advances the root epoch once;
    // an earlier member keeps the smaller view it confirmed at that time.
    for (index, (member, member_id)) in members.iter_mut().zip(&member_ids).enumerate() {
        let now = 1_000 + index as u64;
        let request = member.on_link_up(now).expect("request");
        let outcome = root.on_message(*member_id, JoinMessage::Request(request), now);
        let confirmation = match outcome.reply {
            Some(JoinMessage::Confirm(confirmation)) => confirmation,
            other => panic!("expected confirmation, got {other:?}"),
        };
        assert_eq!(confirmation.epoch, 2 + index as u32);
        let outcome = member.on_message(root_id, JoinMessage::Confirm(confirmation), now);
        assert!(matches!(outcome.event, JoinEvent::Converged(_)));
        let ack = match outcome.reply {
            Some(JoinMessage::Ack(ack)) => ack,
            other => panic!("expected ack, got {other:?}"),
        };
        let outcome = root.on_message(*member_id, JoinMessage::Ack(ack), now);
        assert!(matches!(outcome.event, JoinEvent::Acked { .. }));
    }

    // The later members advanced the epoch past the earlier members' views.
    assert_eq!(root.epoch(), 8);
    assert_eq!(root.snapshot().len(), 8);
    assert_eq!(members[0].epoch(), 2);
    assert_eq!(members[6].epoch(), 8);

    // Every converged member installs the root's signed aggregate, then
    // reports liveness with a small summary instead of a full join request.
    // The root accepts each in-epoch summary (refreshing liveness, no epoch
    // advance) and signs one aggregate for the current view.
    let refresh_at = 1_000 + REFRESH_INTERVAL_MS + 10;
    let aggregate = root
        .publish_aggregate()
        .expect("root publishes an aggregate");
    assert_eq!(aggregate.epoch, 8, "a refresh must not advance the epoch");
    for (index, member) in members.iter_mut().enumerate() {
        let outcome = member.on_message(root_id, JoinMessage::Epoch(aggregate.clone()), refresh_at);
        assert!(
            matches!(
                outcome.event,
                JoinEvent::Converged(ref snapshot) | JoinEvent::Refreshed(ref snapshot)
                    if snapshot.epoch == 8 && snapshot.len() == 8
            ),
            "member {index} did not install the eight-node view: {:?}",
            outcome.event
        );
    }

    // All members now hold epoch 8; their summaries match the signed aggregate.
    let summary_at = refresh_at + REFRESH_INTERVAL_MS + 1;
    for (index, (member, member_id)) in members.iter_mut().zip(&member_ids).enumerate() {
        assert_eq!(
            member.poll(summary_at),
            Tick::Summary,
            "member {index} expected a summary tick"
        );
        let summary = member.member_summary().expect("member summary");
        assert_eq!(summary.epoch, 8);
        let outcome = root.on_message(*member_id, JoinMessage::Summary(summary), refresh_at);
        assert!(
            matches!(outcome.event, JoinEvent::None),
            "root rejected summary from member {index}: {:?}",
            outcome.event
        );
    }

    // A second aggregate for the unchanged view still signs at epoch 8.
    let again = root.publish_aggregate().expect("root re-publishes");
    assert_eq!(again.epoch, 8, "a re-publish must not advance the epoch");
    assert_eq!(
        again.signature, aggregate.signature,
        "same view yields the same signature"
    );

    let root_snapshot = root.snapshot();
    assert_eq!(root.epoch(), 8, "a refresh must not advance the epoch");
    for (index, member) in members.iter().enumerate() {
        assert_eq!(
            member.snapshot(),
            root_snapshot,
            "member {index} did not converge on the root snapshot"
        );
        assert_eq!(member.snapshot().digest(), root_snapshot.digest());
        assert!(member.authority_current());
    }
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
    // The duplicate is terminal for the rejoin attempt.  M3 then schedules one
    // bounded epoch refresh; it does not retry the same request.
    assert_eq!(pair.member.poll(2_000 + RETRY_INTERVAL_MS - 1), Tick::Idle);
    assert_eq!(pair.member.poll(2_000 + REFRESH_INTERVAL_MS), Tick::Summary);
    let summary = pair.member.member_summary().expect("summary");
    assert_eq!(summary.epoch, 2);
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
fn stored_root_record_restores_the_confirmed_epoch() {
    let mut pair = pair();
    let confirmation = join(&mut pair, 1_000);
    let epoch = pair.root.epoch();
    let snapshot = pair.root.snapshot();
    assert_eq!(epoch, 2);

    // The root signs its confirmed view and restarts.  The record restores the
    // exact epoch and member list instead of the NVRAM seed.
    let record = pair.root.epoch_record().expect("root record");
    assert_eq!(record.epoch, epoch);
    assert_eq!(record.members, snapshot.members);
    let mut restarted = JoinNode::new_root(pair.root_id, signing(9), 1, 4_000).expect("root");
    assert_eq!(restarted.epoch(), 1);
    let restored = restarted
        .restore_record(&record, 4_000)
        .expect("restore record");
    assert_eq!(restored, snapshot);
    assert_eq!(restarted.epoch(), epoch);
    assert_eq!(restarted.snapshot(), snapshot);

    // The member also restarts from its stored signed confirmation.  It must
    // present the confirmed epoch, not the seed.
    let mut member =
        JoinNode::new_member(pair.member_id, pair.root_id, key(9), 1, 4_000).expect("member");
    member
        .restore_confirmation(&confirmation, 4_000)
        .expect("restore confirmation");
    assert_eq!(member.epoch(), epoch);
    assert_eq!(member.snapshot(), snapshot);
    assert!(!member.authority_current(), "restore is not live authority");
    let request = member.on_link_up(4_100).expect("rejoin request");
    assert_eq!(request.last_epoch, epoch);
    let outcome = restarted.on_message(pair.member_id, JoinMessage::Request(request), 4_100);
    let confirmation = match outcome.reply {
        Some(JoinMessage::Confirm(confirmation)) => confirmation,
        other => panic!("expected confirmation, got {other:?}"),
    };
    assert_eq!(confirmation.epoch, epoch);
    let outcome = member.on_message(pair.root_id, JoinMessage::Confirm(confirmation), 4_100);
    assert!(matches!(outcome.event, JoinEvent::Duplicate(_)));
    assert_eq!(member.epoch(), epoch);
    assert_eq!(member.snapshot(), snapshot);
    assert!(member.authority_current());
    assert_eq!(member.snapshot(), restarted.snapshot());
}

#[test]
fn corrupt_or_foreign_epoch_records_fail_closed() {
    let mut pair = pair();
    let confirmation = join(&mut pair, 1_000);
    let record = pair.root.epoch_record().expect("root record");

    // A tampered member list no longer matches the signature.
    let mut forged = record.clone();
    forged.members = vec![node(1), node(2), node(3)];
    // Keep the list sorted as the encoder requires; the signature is stale.
    let mut restarted = JoinNode::new_root(pair.root_id, signing(9), 1, 4_000).expect("root");
    assert_eq!(
        restarted.restore_record(&forged, 4_000),
        Err(AuthorityError::BadSignature)
    );
    assert_eq!(restarted.epoch(), 1);
    assert_eq!(restarted.snapshot().members, vec![pair.root_id]);

    // A record signed by another key is not this root's view.
    let mut wrong = JoinNode::new_root(pair.root_id, signing(8), 1, 4_000).expect("root");
    assert_eq!(
        wrong.restore_record(&record, 4_000),
        Err(AuthorityError::BadSignature)
    );
    assert_eq!(wrong.epoch(), 1);

    // A record whose root is a different NodeId is refused before the
    // signature is trusted.
    let mut foreign = record.clone();
    foreign.root = node(7);
    let mut restarted = JoinNode::new_root(pair.root_id, signing(9), 1, 4_000).expect("root");
    assert_eq!(
        restarted.restore_record(&foreign, 4_000),
        Err(AuthorityError::WrongRoot)
    );

    // A tampered confirmation does not verify.  The real confirmation came
    // from the live handshake above; changing a signed field is enough.
    let mut tampered = confirmation.clone();
    tampered.epoch = 7;
    let mut member =
        JoinNode::new_member(pair.member_id, pair.root_id, key(9), 1, 4_000).expect("member");
    assert_eq!(
        member.restore_confirmation(&tampered, 4_000),
        Err(AuthorityError::BadSignature)
    );
    assert_eq!(member.epoch(), 1);
    assert!(member.converged().is_none());

    // A member cannot restore a confirmation addressed to another node.
    let mut other = JoinNode::new_member(node(3), pair.root_id, key(9), 1, 4_000).expect("member");
    assert_eq!(
        other.restore_confirmation(&confirmation, 4_000),
        Err(AuthorityError::WrongSubject)
    );
    assert_eq!(other.epoch(), 1);
    assert!(other.converged().is_none());

    // A root record cannot be restored by a member, and a member cannot
    // produce one.
    assert_eq!(
        member.restore_record(&record, 4_000),
        Err(AuthorityError::NotRoot)
    );
    assert_eq!(pair.member.epoch_record(), Err(AuthorityError::NotRoot));
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
            Tick::Refresh(_) => panic!("a pending request must not refresh"),
            Tick::Summary => panic!("pre-convergence member must not refresh"),
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
            Tick::Refresh(_) => panic!("a pending rejoin must not refresh"),
            Tick::Summary => panic!("pre-convergence member must not refresh"),
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
fn an_ack_for_an_older_epoch_is_stale_not_an_equivocation() {
    let mut pair = pair();
    join(&mut pair, 1_000);
    // A later member joins and advances the root before the first member's
    // ack reaches it.  The ack is for the epoch that was current when issued.
    let outcome = pair.root.on_message(
        node(3),
        JoinMessage::Request(JoinRequest {
            node: node(3),
            incarnation: 1,
            last_epoch: 1,
            last_digest: [0u8; 16],
            nonce: 2,
        }),
        2_000,
    );
    assert!(matches!(outcome.event, JoinEvent::Confirmed(_)));
    assert_eq!(pair.root.epoch(), 3);
    let outcome = pair.root.on_message(
        pair.member_id,
        JoinMessage::Ack(JoinAck {
            node: pair.member_id,
            epoch: 2,
            digest: pair.member.snapshot().digest(),
            request_nonce: 1,
        }),
        2_001,
    );
    assert_eq!(outcome.event, JoinEvent::Rejected("ack-stale"));
    assert_eq!(pair.root.epoch(), 3);
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

#[test]
fn authority_signing_is_root_only_and_matches_the_configured_key() {
    let pair = pair();
    let root_key = pair.root.authority_key();
    assert_eq!(root_key, key(9));
    let message = b"IRIX7-M2-AUTHORITY-SEAM-v1";
    let signature = pair
        .root
        .sign_authority(message)
        .expect("the root can sign");
    assert!(cap_engine::verify(&root_key, message, &signature).is_ok());

    assert_eq!(pair.member.authority_key(), key(9));
    assert_eq!(pair.member.sign_authority(message), None);
}

fn events_named(events: &[JoinEvent], label: &str) -> usize {
    events
        .iter()
        .filter(|event| {
            matches!(
                (label, event),
                ("suspect", JoinEvent::Suspect { .. })
                    | ("failed", JoinEvent::Failed { .. })
                    | ("removed_batch", JoinEvent::RemovedBatch { .. })
                    | ("lost", JoinEvent::AuthorityLost { .. })
                    | ("recovered", JoinEvent::Recovered { .. })
            )
        })
        .count()
}

#[test]
fn root_removes_a_failed_member_at_boundaries_and_only_once() {
    let mut pair = pair();
    join(&mut pair, 1_000);
    assert_eq!(pair.root.epoch(), 2);
    assert_eq!(pair.root.snapshot().len(), 2);

    // One millisecond before the boundary nothing changes.
    assert!(pair
        .root
        .poll_failures(1_000 + crate::SUSPECT_AFTER_MS - 1)
        .is_empty());
    // Exactly at the boundary the node is suspect and still routable.
    let suspect = pair.root.poll_failures(1_000 + crate::SUSPECT_AFTER_MS);
    assert_eq!(events_named(&suspect, "suspect"), 1);
    assert_eq!(pair.root.epoch(), 2);
    assert_eq!(pair.root.snapshot().len(), 2);
    // The suspect transition is not repeated.
    assert!(pair
        .root
        .poll_failures(1_000 + crate::SUSPECT_AFTER_MS + 1)
        .is_empty());

    let failed = pair.root.poll_failures(1_000 + crate::FAIL_AFTER_MS);
    assert_eq!(events_named(&failed, "failed"), 1);
    assert_eq!(events_named(&failed, "removed_batch"), 1);
    assert_eq!(pair.root.epoch(), 3);
    assert_eq!(pair.root.snapshot().members, vec![pair.root_id]);
    assert_eq!(pair.root.snapshot().digest(), pair.root.snapshot().digest());
    // A duplicate failure, or a bare heartbeat, cannot remove or revive.
    assert!(pair
        .root
        .poll_failures(1_000 + crate::FAIL_AFTER_MS * 4)
        .is_empty());
    assert_eq!(pair.root.epoch(), 3);
    assert_eq!(pair.root.observe_peer(pair.member_id, 60_000), None);
    assert_eq!(pair.root.epoch(), 3);
    assert_eq!(pair.root.snapshot().len(), 1);
}

#[test]
fn removal_moves_only_the_failed_nodes_homes() {
    use mesh_core::{fixture_objects, home_for};
    let mut pair = pair();
    join(&mut pair, 1_000);
    let before = pair.root.snapshot();
    let objects = fixture_objects(32);
    let before_homes: Vec<NodeId> = objects
        .iter()
        .map(|object| home_for(object, &before.view()).expect("home"))
        .collect();
    pair.root.poll_failures(1_000 + crate::FAIL_AFTER_MS);
    let after = pair.root.snapshot();
    let after_homes: Vec<NodeId> = objects
        .iter()
        .map(|object| home_for(object, &after.view()).expect("home"))
        .collect();
    let mut moved = 0;
    for (object, (old, new)) in objects.iter().zip(before_homes.iter().zip(&after_homes)) {
        // No object may remain homed at the failed node, and an object that
        // was already homed at the survivor must not churn.
        assert_ne!(
            *new, pair.member_id,
            "object {object:?} still homes at the removed node"
        );
        if *old == pair.member_id {
            moved += 1;
        } else {
            assert_eq!(old, new);
        }
    }
    assert!(
        moved > 0,
        "the failed node must have homed some fixture objects"
    );
}

#[test]
fn member_loses_authority_without_removing_the_root() {
    let mut pair = pair();
    join(&mut pair, 1_000);
    assert!(pair.member.authority_current());
    assert!(pair.member.mutable_available());
    let confirmed = pair.member.snapshot();

    let suspect = pair.member.poll_failures(1_000 + crate::SUSPECT_AFTER_MS);
    assert_eq!(events_named(&suspect, "suspect"), 1);
    // A suspect authority is still current and routable.
    assert!(pair.member.authority_current());
    assert!(pair.member.mutable_available());

    let lost = pair.member.poll_failures(1_000 + crate::FAIL_AFTER_MS);
    assert_eq!(events_named(&lost, "failed"), 1);
    assert_eq!(events_named(&lost, "lost"), 1);
    assert!(!pair.member.authority_current());
    assert!(!pair.member.mutable_available());
    // The member does not remove the root and does not advance the epoch.
    assert_eq!(pair.member.epoch(), confirmed.epoch);
    assert_eq!(pair.member.snapshot(), confirmed);
    assert_eq!(pair.member.converged(), Some(&confirmed));
    // The loss is reported once, not once per tick.
    assert!(pair
        .member
        .poll_failures(1_000 + crate::FAIL_AFTER_MS * 3)
        .is_empty());
    // A bare heartbeat does not restore authority; only a signed confirmation
    // does.
    assert_eq!(pair.member.observe_peer(pair.root_id, 90_000), None);
    assert!(!pair.member.authority_current());
}

#[test]
fn member_rejoin_after_removal_is_a_new_epoch_and_restores_service() {
    let mut pair = pair();
    join(&mut pair, 1_000);
    pair.root.poll_failures(1_000 + crate::FAIL_AFTER_MS);
    assert_eq!(pair.root.epoch(), 3);
    assert_eq!(pair.root.snapshot().len(), 1);
    // The member still holds its last confirmed view at epoch 2.
    assert_eq!(pair.member.epoch(), 2);
    assert_eq!(pair.member.snapshot().len(), 2);

    let request = pair.member.on_link_up(30_000).expect("rejoin request");
    assert_eq!(request.last_epoch, 2);
    let outcome = pair
        .root
        .on_message(pair.member_id, JoinMessage::Request(request), 30_000);
    let confirmation = match outcome.reply {
        Some(JoinMessage::Confirm(confirmation)) => confirmation,
        other => panic!("expected confirmation, got {other:?}"),
    };
    assert_eq!(confirmation.epoch, 4);
    assert_eq!(confirmation.members, vec![pair.root_id, pair.member_id]);
    let outcome = pair
        .member
        .on_message(pair.root_id, JoinMessage::Confirm(confirmation), 30_000);
    assert!(matches!(outcome.event, JoinEvent::Converged(_)));
    assert_eq!(pair.member.epoch(), 4);
    assert_eq!(pair.root.epoch(), 4);
    assert_eq!(pair.root.snapshot(), pair.member.snapshot());
    assert!(pair.member.authority_current());
    assert!(pair.member.mutable_available());
    assert!(pair.root.mutable_available());
}

#[test]
fn a_suspect_authority_recovery_does_not_advance_the_epoch() {
    let mut pair = pair();
    join(&mut pair, 1_000);
    let epoch = pair.member.epoch();
    let suspect = pair.member.poll_failures(1_000 + crate::SUSPECT_AFTER_MS);
    assert_eq!(events_named(&suspect, "suspect"), 1);
    // A frame arrives before the failure boundary: the suspect state clears.
    let recovered = pair
        .member
        .observe_peer(pair.root_id, 1_000 + crate::SUSPECT_AFTER_MS + 1)
        .expect("recovered event");
    assert!(matches!(recovered, JoinEvent::Recovered { .. }));
    assert!(pair.member.authority_current());
    assert!(pair
        .member
        .poll_failures(1_000 + crate::FAIL_AFTER_MS)
        .is_empty());
    assert_eq!(pair.member.epoch(), epoch);
}

#[test]
fn a_member_requests_one_rejoin_after_losing_the_authority() {
    let mut pair = pair();
    join(&mut pair, 1_000);
    // Both sides see the silence: the root removes the member (epoch 3) and
    // the member loses authority without changing its own view.
    pair.root.poll_failures(1_000 + crate::FAIL_AFTER_MS);
    assert_eq!(pair.root.epoch(), 3);
    pair.member.poll_failures(1_000 + crate::FAIL_AFTER_MS);
    assert!(!pair.member.authority_current());
    let request = pair.member.request_rejoin(30_000).expect("rejoin request");
    assert_eq!(request.last_epoch, 2);
    assert_eq!(request.node, pair.member_id);
    // One outstanding request: a second frame does not create another.
    assert!(pair.member.request_rejoin(30_100).is_none());
    let outcome = pair
        .root
        .on_message(pair.member_id, JoinMessage::Request(request), 30_000);
    let confirmation = match outcome.reply {
        Some(JoinMessage::Confirm(confirmation)) => confirmation,
        other => panic!("expected confirmation, got {other:?}"),
    };
    assert_eq!(confirmation.epoch, 4);
    let outcome = pair
        .member
        .on_message(pair.root_id, JoinMessage::Confirm(confirmation), 30_000);
    assert!(matches!(outcome.event, JoinEvent::Converged(_)));
    assert!(pair.member.authority_current());
}
