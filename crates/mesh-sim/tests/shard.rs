// SPDX-License-Identifier: GPL-3.0-only
//! M3 shard evidence at the policy layer.
//!
//! These gates use the production `mesh-service` membership table and the
//! production `mesh-core` rendezvous function.  They do not add a directory or
//! a second placement model: a node computes its own shard by filtering the
//! same `HomeTable` every other node computes from the confirmed snapshot.

use mesh_core::{shard_objects, HomeTable, MembershipView, ShardBalance, SHARD_FIXTURE_DEFAULT};
use mesh_service::{MembershipError, MembershipTable};
use sys_primitives::{NodeId, ObjectId};

fn node(index: u8) -> NodeId {
    // Keep every test ID non-zero and distinct.  The kernel's NodeId is a MAC
    // followed by zeros; this shape is enough for the policy gate.
    let mut bytes = [0u8; 16];
    bytes[0] = index;
    bytes[1] = 0x69;
    bytes[2] = 0x95;
    NodeId(bytes)
}

fn confirmed(nodes: u8, epoch: u32) -> MembershipView {
    let table = MembershipTable::confirmed(epoch, &(1..=nodes).map(node).collect::<Vec<_>>(), 0)
        .expect("confirmed table");
    table.snapshot()
}

fn digest(byte: u8) -> [u8; 16] {
    [byte; 16]
}

fn table(view: &MembershipView, count: u32) -> HomeTable {
    HomeTable::new(view, digest(view.epoch as u8), &shard_objects(count)).expect("shard table")
}

fn generated(count: u32) -> Vec<ObjectId> {
    (0..count).map(mesh_core::shard_fixture_object).collect()
}

fn table_of(view: &MembershipView, objects: &[ObjectId]) -> HomeTable {
    HomeTable::new(view, digest(view.epoch as u8), objects).expect("shard table")
}

fn assert_balance(balance: ShardBalance, nodes: usize, min_owners: usize, max_ratio: usize) {
    assert!(
        balance.owners >= min_owners,
        "expected at least {min_owners} owners, got {:?}",
        balance
    );
    assert!(balance.owners <= nodes);
    assert!(balance.min > 0, "the smallest shard must be non-empty");
    let average = balance.objects / balance.owners;
    assert!(
        balance.max <= average * max_ratio,
        "shard balance {balance:?} exceeds {max_ratio}x the {average}-object average"
    );
}

#[test]
fn one_two_eight_and_128_node_views_are_deterministic() {
    for (nodes, epoch, count, min_owners, ratio) in [
        (1u8, 1u32, SHARD_FIXTURE_DEFAULT, 1usize, 1usize),
        (2, 2, SHARD_FIXTURE_DEFAULT, 2, 3),
        (8, 3, 1024, 8, 3),
        (128, 4, 4096, 120, 3),
    ] {
        let view = confirmed(nodes, epoch);
        let objects = generated(count);
        let first = table_of(&view, &objects);
        let second = table_of(&view, &objects);
        assert_eq!(first, second, "{nodes}-node table must be deterministic");
        assert_eq!(first.len(), objects.len());
        assert_eq!(first.epoch, epoch);
        let mut owned = 0;
        for member in &view.nodes {
            owned += first.owned_count(*member);
            assert_eq!(first.owned_by(*member).len(), first.owned_count(*member));
        }
        assert_eq!(owned, first.len(), "one owner per object");
        assert_balance(first.balance(), nodes as usize, min_owners, ratio);
    }
}

#[test]
fn a_node_derives_its_shard_from_the_view_without_a_lookup() {
    let view = confirmed(8, 8);
    let first = table(&view, 512);
    // Rebuilding the view in a different insertion order must produce exactly
    // the same table.  No node needs to ask an owner or a directory.
    let unordered =
        MembershipView::new(8, view.nodes.iter().rev().copied().collect::<Vec<NodeId>>())
            .expect("view");
    let second = table(&unordered, 512);
    assert_eq!(first, second);
    for member in &view.nodes {
        let local: Vec<_> = first
            .entries
            .iter()
            .filter(|entry| entry.home == *member)
            .copied()
            .collect();
        assert_eq!(first.owned_by(*member), local);
        assert_eq!(first.owned_count(*member), local.len());
        assert!(!local.is_empty());
    }
    // Owner counts are a summary of the same rows, not a second assignment.
    let mut from_rows = std::collections::BTreeMap::new();
    for entry in &first.entries {
        *from_rows.entry(entry.home).or_insert(0usize) += 1;
    }
    assert_eq!(
        first.owner_counts(),
        from_rows.into_iter().collect::<Vec<(NodeId, usize)>>()
    );
}

#[test]
fn membership_churn_moves_only_the_changed_shard() {
    let eight = confirmed(8, 9);
    let ids = shard_objects(1024);
    let before = HomeTable::new(&eight, [9; 16], &ids).expect("before");
    let removed = node(4);
    let mut table =
        MembershipTable::confirmed(9, &(1..=8).map(node).collect::<Vec<_>>(), 0).expect("table");
    table.leave(removed).expect("leave");
    let seven = table.snapshot();
    assert_eq!(seven.epoch, 10);
    let after = HomeTable::new(&seven, [10; 16], &ids).expect("after");
    let mut moved = 0;
    let mut stayed = 0;
    for (old, new) in before.entries.iter().zip(&after.entries) {
        assert_eq!(old.object, new.object);
        if old.home == removed {
            moved += 1;
            assert_ne!(new.home, removed);
        } else {
            stayed += 1;
            assert_eq!(old.home, new.home);
        }
    }
    assert!(moved > 0, "the removed node owned a non-empty shard");
    assert!(stayed > 0, "unrelated shards must not move");

    // Re-admitting the same node moves only the objects whose rendezvous
    // winner changes back.
    let mut readded = MembershipTable::confirmed(10, &seven.nodes, 0).expect("table");
    readded.join(removed, 0).expect("rejoin");
    let nine = readded.snapshot();
    assert_eq!(nine.epoch, 11);
    let again = HomeTable::new(&nine, [11; 16], &ids).expect("again");
    let mut returned = 0;
    for (old, new) in after.entries.iter().zip(&again.entries) {
        if old.home != new.home {
            returned += 1;
            assert_eq!(new.home, removed);
        }
    }
    assert!(returned > 0);
}

#[test]
fn duplicate_and_stale_epochs_fail_closed() {
    let mut membership =
        MembershipTable::confirmed(8, &(1..=8).map(node).collect::<Vec<_>>(), 0).expect("table");
    assert_eq!(
        membership.install(8, &(1..=7).map(node).collect::<Vec<_>>(), 0),
        Err(MembershipError::EpochConflict)
    );
    assert_eq!(
        membership.install(7, &(1..=8).map(node).collect::<Vec<_>>(), 0),
        Err(MembershipError::EpochRegression)
    );
    // The identical view is idempotent and does not advance the epoch.
    membership
        .install(8, &(1..=8).map(node).collect::<Vec<_>>(), 0)
        .expect("idempotent");
    assert_eq!(membership.epoch(), 8);

    let view = membership.snapshot();
    let shard = table(&view, 64);
    assert!(shard.at_epoch(8).is_ok());
    assert!(shard.at_epoch(7).is_err());
}

#[test]
fn a_shard_is_a_bounded_fixture_not_a_second_assignment() {
    let ids = generated(1024);
    assert_eq!(ids.len(), 1024);
    let mut sorted = ids.clone();
    sorted.sort_unstable();
    sorted.dedup();
    assert_eq!(sorted.len(), ids.len(), "fixture objects are distinct");
    assert_eq!(shard_objects(mesh_core::SHARD_FIXTURE_MAX + 1).len(), 256);
    // The same object in an eight-node view has exactly one winner; that is
    // the only assignment rule in the gate.
    let view = confirmed(8, 12);
    let table = table(&view, 1024);
    for entry in &table.entries {
        assert!(view.contains(entry.home));
        assert_eq!(mesh_core::home_for(&entry.object, &view), Some(entry.home));
    }
}
