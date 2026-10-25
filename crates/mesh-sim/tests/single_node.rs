// SPDX-License-Identifier: GPL-3.0-only
//! M1 evidence: the local policies meet on one node with no mesh envelope.

use agnostic_core::{ActorState, ActorTable, RunQueue};
use cap_engine::{node_key, sign_capability, verify_capability, TrustStore};
use cas_core::ObjectKind;
use cas_store::Namespace;
use ed25519_dalek::SigningKey;
use mesh_core::{actor_home, home_for, MembershipView};
use mesh_service::MembershipTable;
use sys_primitives::{ActorId, Capability, NodeId, ObjectId, PrincipalId, Rights};

fn node() -> NodeId {
    NodeId([0x11; 16])
}

#[test]
fn one_node_has_complete_local_policy() {
    let node = node();
    let view = MembershipView::single(node);
    assert!(view.is_single());
    assert_eq!(view.len(), 1);
    assert_eq!(view.epoch, 0);

    // The real membership service produces a one-node routable snapshot.
    // Joining the first node advances its epoch from 0 to 1.
    let table = MembershipTable::single(node, 0);
    let snapshot = table.snapshot();
    assert_eq!(snapshot.nodes, vec![node]);
    assert!(snapshot.is_single());
    assert_eq!(table.active_count(), 1);

    // Namespace + CAS: every path resolves locally and every object stays on
    // this node. No `fabric` frame is constructed by this test.
    let mut namespace = Namespace::new();
    let root = namespace.root();
    assert_eq!(home_for(&root, &view), Some(node));
    namespace.create_dir_all("/etc").expect("etc");
    namespace.create_dir_all("/etc/ssl").expect("ssl");
    let motd = namespace
        .put("/etc/motd", ObjectKind::Blob, b"Welcome to IRIX 7.\n")
        .expect("motd");
    assert_eq!(home_for(&motd, &view), Some(node));
    assert_eq!(namespace.lookup("/etc/motd"), Ok(motd));
    assert_eq!(
        namespace.get("/etc/motd").expect("get"),
        (ObjectKind::Blob, b"Welcome to IRIX 7.\n".as_slice())
    );
    assert_eq!(
        namespace.list("/etc").expect("list"),
        vec![String::from("motd"), String::from("ssl")]
    );

    // Actor directory + per-node run queue.
    let mut actors = ActorTable::new(view.clone());
    let service = ActorId([0x22; 16]);
    let media = ActorId([0x23; 16]);
    let record = actors.spawn(service).expect("service actor");
    assert_eq!(record.home, node);
    assert_eq!(record.host, node);
    assert_eq!(record.state, ActorState::Created);
    assert_eq!(actor_home(&service, &view), Some(node));
    assert!(actors.place(service, node).is_ok());
    assert_eq!(
        actors.get(service).expect("service").state,
        ActorState::Running
    );
    actors.spawn(media).expect("media actor");

    let mut queue = RunQueue::new();
    queue.push_fair(media, 3).expect("fair");
    queue.push_deadline(service, 10, 0).expect("deadline");
    assert_eq!(queue.pick(), Some(service));
    assert_eq!(queue.pick(), Some(media));
    assert!(queue.is_empty());

    // Capability: the local 1-of-1 issuer signs the root object and the token
    // verifies for this home, with no directory lookup or remote route.
    let signing = SigningKey::from_bytes(&[7u8; 32]);
    let issuer = node_key(&signing);
    let mut trust = TrustStore::new();
    assert!(trust.insert(issuer));
    let mut token = Capability {
        magic: Capability::MAGIC,
        version: Capability::VERSION,
        rights: Rights::READ,
        flags: 0,
        object: root,
        issuer,
        subject: PrincipalId([0x33; 32]),
        home: node,
        not_before: 0,
        not_after: 1_000,
        object_epoch: 0,
        home_epoch: view.epoch,
        root_epoch: 0,
        reserved0: 0,
        nonce: [0x44; 16],
        parent: ObjectId::ZERO,
        delegation: ObjectId::ZERO,
        reserved1: [0; 24],
        signature: [0; 64],
    };
    sign_capability(&signing, &mut token);
    assert!(verify_capability(&trust, &token).is_ok());
    assert!(token.verify(&trust, 10, Rights::READ, root).is_ok());
    assert!(token.verify(&trust, 10, Rights::WRITE, root).is_err());
    let encoded = token.encode();
    assert_eq!(Capability::decode(&encoded), Ok(token));
    assert_eq!(token.home, node);
}
