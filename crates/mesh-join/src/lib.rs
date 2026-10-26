// SPDX-License-Identifier: GPL-3.0-only
//! M2 join handshake and membership-epoch convergence.
//!
//! This crate is the policy layer between the M2-1 mesh transport and the
//! membership table.  It does not own a socket and it does not invent a second
//! membership model: the local view is [`mesh_service::MembershipTable`], the
//! routing snapshot is [`mesh_core::MembershipView`], and the authority proof
//! is Ed25519 through [`cap_engine`].
//!
//! The two-node handshake is:
//!
//! ```text
//! member -> root:  JOIN_REQUEST(node, incarnation, last_epoch, last_digest, nonce)
//! root:            add the member to its table if absent; epoch is the root's
//! root -> member:  EPOCH_CONFIRM(root, epoch, members, subject, nonce, incarnation,
//!                                Ed25519 signature)
//! member:          verify the root, signature, subject, nonce and epoch;
//!                  install the full member list at the confirmed epoch
//! member -> root:  JOIN_ACK(node, epoch, digest, nonce)
//! ```
//!
//! The confirmation carries the **full member list**, not just an epoch
//! number.  The member installs exactly that list, so both nodes converge on
//! the same epoch and the same snapshot digest.  A member never increments the
//! epoch itself: a local view may be at most the last confirmed snapshot.
//!
//! ## Fail-closed rules
//!
//! * The confirmation must name the configured authority root (`mesh.authority`).
//! * Its Ed25519 signature must verify under the configured root public key
//!   (`mesh.pub`) through a `cap-engine` trust store.
//! * It must name this node as the subject and echo the outstanding request's
//!   nonce and incarnation, so a reply cannot be replayed into another attempt.
//! * A lower epoch is a regression; the same epoch with a different member list
//!   is an equivocation.  Both are rejected and the previous view is kept.
//! * The root refuses a request whose `last_epoch` is above the root's own:
//!   the 1-of-1 authority cannot explain a higher epoch it did not issue, and
//!   lowering the member's view would be an epoch regression.
//!
//! ## Bounds and deadlines
//!
//! A join body is at most [`MAX_BODY`] bytes, comfortably inside the 64 KiB
//! record bound of `mesh-transport`.  The member retries the same request at
//! [`RETRY_INTERVAL_MS`] up to [`MAX_ATTEMPTS`] times; then it stops, logs the
//! timeout, and keeps its last confirmed view.  A new link resets the attempt
//! budget.  There is no unbounded retry loop and no background epoch change.
//!
//! ## Failure detection and removal (M2-6)
//!
//! The join node also owns the membership half of failure handling.  It uses
//! the one failure model in [`mesh_service::FailureDetector`]:
//!
//! * a confirmed peer that has been silent for
//!   [`mesh_service::SUSPECT_AFTER_MS`] is suspect but stays routable;
//! * one silent for [`mesh_service::FAIL_AFTER_MS`] is failed;
//! * the root removes a failed member with one epoch advance and keeps the
//!   remaining confirmed list; it never removes itself;
//! * a member that loses the root records `AuthorityLost` and keeps its last
//!   confirmed view.  It does not remove the root, advance the epoch or create
//!   a second authoritative home;
//! * a failed node is re-admitted only by a fresh signed join, which advances
//!   the epoch again.
//!
//! The clock is injected and the boundaries are exact, so a host test can
//! drive every boundary without sleeping.  There is no gossip and no persisted
//! epoch store; the restart rules are in [`node`] and in M2-ADR-0005/M2-ADR-0008.

#![no_std]
#![forbid(unsafe_code)]
#![deny(missing_docs)]

extern crate alloc;

mod authority;
mod node;
mod wire;

pub use authority::{
    AuthorityError, AuthorityRoot, AuthorityVerifier, Expectation, SigningAuthority,
};
pub use mesh_service::{
    FailureDetector, NodeState, Observe, Transition, FAIL_AFTER_MS, SUSPECT_AFTER_MS,
};
pub use node::{JoinEvent, JoinNode, Outcome, Role, Tick, MAX_ATTEMPTS, RETRY_INTERVAL_MS};
pub use wire::{
    EpochConfirmation, EpochRefusal, JoinAck, JoinMessage, JoinRequest, MembershipSnapshot,
    RefusalReason, WireError, MAX_BODY, MAX_MEMBERS, SNAPSHOT_VERSION, VERSION,
};

pub use mesh_core::MembershipView;
pub use sys_primitives::{NodeId, NodeKey};
