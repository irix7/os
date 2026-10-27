// SPDX-License-Identifier: GPL-3.0-only
//! The 1-of-1 authority root and its verification seam.
//!
//! M2-ADR-0003 makes node 0 the only writer of the membership epoch.  This
//! module is the small `authority_root`/`confirm_epoch`/`verify_confirmation`
//! seam the ADR asks for: the join state machine does not know whether the
//! root is one key or a threshold.  The M2 implementation uses one Ed25519 key
//! from `cap-engine`; a later threshold implementation can replace
//! [`AuthorityRoot`] without changing the join messages.
//!
//! The root's key is a **lab fixture** in M2: the rig writes the 32-byte seed
//! into the root's DS1386 image and the public key into the member's image.
//! It is not a production key store and it is not secret in the rig.  The
//! property being demonstrated is that an epoch is not trusted without a proof
//! from the configured root, not key custody.

use core::fmt;

pub use cap_engine::SigningAuthority;
use cap_engine::{KeyError, TrustStore};
use mesh_service::{
    FailureDetector, MembershipError, MembershipTable, NodeState, Observe, Transition,
};
use sys_primitives::{NodeId, NodeKey};

use crate::wire::{
    EpochConfirmation, EpochRecord, JoinRequest, MembershipSnapshot, WireError, MAX_MEMBERS,
};

/// An authority or membership failure.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AuthorityError {
    /// The configured root is the zero sentinel.
    ZeroRoot,
    /// The configured authority key is the zero sentinel.
    ZeroKey,
    /// A confirmed epoch cannot be zero.
    EpochZero,
    /// The seed epoch cannot be zero.
    BadSeed,
    /// The subject is the root itself or the zero sentinel.
    WrongSubject,
    /// The operation needs the authority root, but this node is a member.
    NotRoot,
    /// The member's last epoch is above the root's own.
    EpochRegression {
        /// The local (root or member) epoch.
        local: u32,
        /// The peer's epoch.
        remote: u32,
    },
    /// The confirmation names a root other than the configured one.
    WrongRoot,
    /// The confirmation's key is not the configured root key.
    UntrustedRoot,
    /// The Ed25519 signature did not verify.
    BadSignature,
    /// The confirmation's request nonce does not match the outstanding one.
    NonceMismatch,
    /// The confirmation's incarnation does not match the outstanding one.
    IncarnationMismatch,
    /// The root is missing from the confirmed member list.
    RootNotMember,
    /// The subject is missing from the confirmed member list.
    SubjectNotMember,
    /// A different member list claims an epoch the local node already has.
    EpochConflict,
    /// The membership table rejected the view.
    Membership(MembershipError),
    /// The snapshot could not be encoded canonically.
    Wire(WireError),
}

impl AuthorityError {
    /// A short stable label for serial logs and evidence.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ZeroRoot => "zero-root",
            Self::ZeroKey => "zero-key",
            Self::EpochZero => "epoch-zero",
            Self::BadSeed => "bad-seed",
            Self::WrongSubject => "wrong-subject",
            Self::NotRoot => "not-root",
            Self::EpochRegression { .. } => "epoch-regression",
            Self::WrongRoot => "wrong-root",
            Self::UntrustedRoot => "untrusted-root",
            Self::BadSignature => "bad-signature",
            Self::NonceMismatch => "nonce-mismatch",
            Self::IncarnationMismatch => "incarnation-mismatch",
            Self::RootNotMember => "root-not-member",
            Self::SubjectNotMember => "subject-not-member",
            Self::EpochConflict => "epoch-conflict",
            Self::Membership(_) => "membership",
            Self::Wire(_) => "wire",
        }
    }

    /// The local epoch and the peer's epoch for a regression.
    pub const fn epochs(self) -> Option<(u32, u32)> {
        match self {
            Self::EpochRegression { local, remote } => Some((local, remote)),
            _ => None,
        }
    }
}

impl fmt::Display for AuthorityError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl From<MembershipError> for AuthorityError {
    fn from(error: MembershipError) -> Self {
        Self::Membership(error)
    }
}

impl From<WireError> for AuthorityError {
    fn from(error: WireError) -> Self {
        Self::Wire(error)
    }
}

/// The authority root's confirmed membership table and signing key.
pub struct AuthorityRoot {
    root: NodeId,
    signing: SigningAuthority,
    table: MembershipTable,
    failures: FailureDetector,
}

/// The removal is the root's membership change; it advances the epoch exactly
/// once and carries the full remaining member list.  The root installs it
/// locally.  A live member installs it only through a signed confirmation when
/// it next joins, so a partition cannot create a second authoritative view.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Removal {
    /// The removed node.
    pub node: NodeId,
    /// The confirmed view after the removal.
    pub snapshot: MembershipSnapshot,
}

impl AuthorityRoot {
    /// Create the root at its configured seed epoch with itself as the only
    /// member.
    ///
    /// The seed is a local bootstrap value (`mesh.epoch`), not a confirmation.
    /// The root is the authority, so this is the base of its confirmed view.
    pub fn new(
        root: NodeId,
        signing: SigningAuthority,
        seed_epoch: u32,
        now_ns: u64,
    ) -> Result<Self, AuthorityError> {
        if root.is_zero() {
            return Err(AuthorityError::ZeroRoot);
        }
        if seed_epoch == 0 {
            return Err(AuthorityError::BadSeed);
        }
        let table = MembershipTable::confirmed(seed_epoch, &[root], now_ns)?;
        Ok(Self {
            root,
            signing,
            table,
            failures: FailureDetector::m2(),
        })
    }

    /// The failure detector boundaries this root uses.
    pub const fn detector(&self) -> FailureDetector {
        self.failures
    }

    /// The configured root identity.
    pub const fn root(&self) -> NodeId {
        self.root
    }

    /// The root's current confirmed epoch.
    pub const fn epoch(&self) -> u32 {
        self.table.epoch()
    }

    /// Whether `node` is in the root's current confirmed table.
    pub fn is_member(&self, node: NodeId) -> bool {
        self.table.get(node).is_some()
    }

    /// Refresh a member's liveness record after one accepted frame.
    ///
    /// Any well-formed frame from the peer proves liveness; the incarnation is
    /// the join incarnation recorded for that member.  A stale or failed
    /// record is not revived by a bare frame.
    pub fn observe(&mut self, node: NodeId, now_ns: u64) -> Observe {
        let incarnation = self
            .table
            .get(node)
            .map(|record| record.incarnation)
            .unwrap_or(0);
        self.failures
            .observe(&mut self.table, node, incarnation, now_ns)
    }

    /// Refresh liveness using the incarnation the caller validated.
    ///
    /// The gossip aggregate carries the confirmed incarnation, so the root can
    /// observe a member without re-reading the table (and without accepting a
    /// summary whose incarnation it did not sign).
    pub fn observe_with_incarnation(
        &mut self,
        node: NodeId,
        incarnation: u64,
        now_ns: u64,
    ) -> Observe {
        self.failures
            .observe(&mut self.table, node, incarnation, now_ns)
    }

    /// Every confirmed record, including failed nodes.
    pub fn records(&self) -> impl Iterator<Item = (NodeId, mesh_service::NodeRecord)> + '_ {
        self.table.records()
    }

    /// Apply one batch of membership changes, advancing the epoch once.
    ///
    /// The authority root is never removed: a root that removed itself would
    /// leave no epoch writer.  Every other change is delegated to the single
    /// table implementation so the epoch rule lives in one place.
    pub fn apply_changes(
        &mut self,
        changes: &[mesh_service::Change],
        now_ns: u64,
    ) -> Result<mesh_service::ChangeSet, AuthorityError> {
        if changes.iter().any(
            |change| matches!(change, mesh_service::Change::Remove { node } if *node == self.root),
        ) {
            return Err(AuthorityError::WrongSubject);
        }
        Ok(self.table.apply(changes, now_ns)?)
    }

    /// Advance every member's liveness state at `now_ns`.
    pub fn expire(&mut self, now_ns: u64) -> alloc::vec::Vec<Transition> {
        self.failures.tick(&mut self.table, self.root, now_ns)
    }

    /// Remove every member the detector has confirmed failed.
    ///
    /// Each removal is a separate authority-confirmed epoch advance, exactly
    /// as a clean leave would be.  The root is never removed.  A node that is
    /// merely suspect stays in the routing snapshot.
    pub fn remove_failed(&mut self) -> alloc::vec::Vec<Removal> {
        let failed: alloc::vec::Vec<NodeId> = self
            .table
            .records()
            .filter(|(node, record)| *node != self.root && record.state == NodeState::Failed)
            .map(|(node, _)| node)
            .collect();
        if failed.is_empty() {
            return alloc::vec::Vec::new();
        }
        let snapshot = match self.snapshot() {
            Ok(s) => s,
            Err(_) => return alloc::vec::Vec::new(),
        };
        failed
            .into_iter()
            .map(|node| Removal {
                node,
                snapshot: snapshot.clone(),
            })
            .collect()
    }

    /// The root's public key.  A member hears the same key through
    /// `mesh.pub`; the root derives it from `mesh.key`.
    pub fn public_key(&self) -> NodeKey {
        self.signing.public()
    }

    /// Sign one canonical authority message.  The capability and route layers
    /// use this seam so they never receive the private seed itself.
    pub fn sign(&self, message: &[u8]) -> [u8; 64] {
        self.signing.sign(message)
    }

    /// The root's current confirmed snapshot.
    pub fn snapshot(&self) -> Result<MembershipSnapshot, AuthorityError> {
        Ok(MembershipSnapshot::from_view(&self.table.snapshot())?)
    }

    /// Sign the current confirmed view as a persistent epoch record.
    ///
    /// The record is for the local epoch store, not for the wire.  The root is
    /// the only node that can produce one; a member stores the signed
    /// confirmation it installed instead.
    pub fn record(&self) -> Result<EpochRecord, AuthorityError> {
        let snapshot = self.snapshot()?;
        let mut record = EpochRecord {
            root: self.root,
            epoch: snapshot.epoch,
            members: snapshot.members,
            signature: [0u8; 64],
        };
        record.signature = self.signing.sign(&record.signed_bytes());
        Ok(record)
    }

    /// Restore the confirmed view from a signed epoch record.
    ///
    /// The signature is verified before any field is trusted, and the record
    /// must name this root and contain it in the member list.  `install`
    /// refuses a lower epoch, so a stale or replayed record cannot move the
    /// root backwards.  Any failure leaves the seed view untouched.
    pub fn restore_record(
        &mut self,
        record: &EpochRecord,
        now_ns: u64,
    ) -> Result<MembershipSnapshot, AuthorityError> {
        if record.root != self.root {
            return Err(AuthorityError::WrongRoot);
        }
        match cap_engine::verify(
            &self.public_key(),
            &record.signed_bytes(),
            &record.signature,
        ) {
            Ok(()) => {}
            Err(KeyError::Untrusted | KeyError::InvalidKey) => {
                return Err(AuthorityError::UntrustedRoot);
            }
            Err(_) => return Err(AuthorityError::BadSignature),
        }
        let snapshot = MembershipSnapshot::new(record.epoch, &record.members)?;
        if !snapshot.members.contains(&self.root) {
            return Err(AuthorityError::RootNotMember);
        }
        self.table
            .install(snapshot.epoch, &snapshot.members, now_ns)?;
        Ok(snapshot)
    }

    /// Confirm a join request, or fail closed without changing the table.
    ///
    /// A duplicate request from an existing member is idempotent: it returns
    /// the current epoch and list, which is the rejoin path.  A request from a
    /// node whose last epoch is above the root's own is refused: the root
    /// cannot explain a higher epoch it did not issue, and lowering the member
    /// would be an epoch regression.
    pub fn confirm(
        &mut self,
        request: &JoinRequest,
        now_ns: u64,
    ) -> Result<EpochConfirmation, AuthorityError> {
        if request.node.is_zero() || request.node == self.root {
            return Err(AuthorityError::WrongSubject);
        }
        if request.last_epoch > self.table.epoch() {
            return Err(AuthorityError::EpochRegression {
                local: self.table.epoch(),
                remote: request.last_epoch,
            });
        }
        // A node that is already a member and claims the root's own epoch
        // must have the same view.  A different digest is an equivocation; the
        // root must not silently bless it with a new signature.  A newcomer at
        // the seed epoch is not a conflict: its single-node view is expected.
        if self.table.get(request.node).is_some() && request.last_epoch == self.table.epoch() {
            let digest = self.snapshot()?.digest();
            if request.last_digest != digest {
                return Err(AuthorityError::EpochConflict);
            }
        }
        if self.table.get(request.node).is_none() {
            self.table.join(request.node, now_ns)?;
        }
        let snapshot = self.snapshot()?;
        let mut confirmation = EpochConfirmation {
            root: self.root,
            epoch: snapshot.epoch,
            members: snapshot.members,
            subject: request.node,
            request_nonce: request.nonce,
            incarnation: request.incarnation,
            signature: [0u8; 64],
        };
        confirmation.signature = self.signing.sign(&confirmation.signed_bytes());
        Ok(confirmation)
    }
}

/// What a member expects from the confirmation it is waiting for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Expectation {
    /// This node's identity.
    pub subject: NodeId,
    /// The outstanding request nonce.
    pub nonce: u64,
    /// The outstanding request incarnation.
    pub incarnation: u64,
    /// The member's current epoch; a lower confirmation is a regression.
    pub minimum_epoch: u32,
    /// The member's current view, for same-epoch equivocation detection.
    pub previous: Option<MembershipSnapshot>,
}

/// Verifies confirmations against the configured root key.
pub struct AuthorityVerifier {
    root: NodeId,
    key: NodeKey,
    store: TrustStore,
}

impl AuthorityVerifier {
    /// Create a verifier for one root key.
    pub fn new(root: NodeId, key: NodeKey) -> Result<Self, AuthorityError> {
        if root.is_zero() {
            return Err(AuthorityError::ZeroRoot);
        }
        if key.0.iter().all(|&byte| byte == 0) {
            return Err(AuthorityError::ZeroKey);
        }
        let mut store = TrustStore::new();
        store.insert(key);
        Ok(Self { root, key, store })
    }

    /// The configured root identity.
    pub const fn root(&self) -> NodeId {
        self.root
    }

    /// The configured root public key.
    pub const fn key(&self) -> NodeKey {
        self.key
    }

    /// Verify one message signed by the configured root.
    ///
    /// This is the aggregate seam: an [`EpochSummary`](crate::wire::EpochSummary)
    /// is one signature over the whole change set, so members verify it with
    /// the same trust store the join confirmation uses.
    pub fn verify_signature(
        &self,
        message: &[u8],
        signature: &[u8; 64],
    ) -> Result<(), AuthorityError> {
        match self.store.verify_trusted(&self.key, message, signature) {
            Ok(()) => Ok(()),
            Err(KeyError::Untrusted) => Err(AuthorityError::UntrustedRoot),
            Err(_) => Err(AuthorityError::BadSignature),
        }
    }

    /// Verify a confirmation and return the snapshot it authorises.
    ///
    /// The signature is checked before any membership field is trusted.  The
    /// subject, nonce and incarnation bind the reply to one attempt; a lower
    /// epoch or a same-epoch different list fails closed.
    pub fn verify(
        &self,
        confirmation: &EpochConfirmation,
        expectation: &Expectation,
    ) -> Result<MembershipSnapshot, AuthorityError> {
        if confirmation.root != self.root {
            return Err(AuthorityError::WrongRoot);
        }
        match self.store.verify_trusted(
            &self.key,
            &confirmation.signed_bytes(),
            &confirmation.signature,
        ) {
            Ok(()) => {}
            Err(KeyError::Untrusted) => return Err(AuthorityError::UntrustedRoot),
            Err(_) => return Err(AuthorityError::BadSignature),
        }
        if confirmation.subject != expectation.subject {
            return Err(AuthorityError::WrongSubject);
        }
        if confirmation.request_nonce != expectation.nonce {
            return Err(AuthorityError::NonceMismatch);
        }
        if confirmation.incarnation != expectation.incarnation {
            return Err(AuthorityError::IncarnationMismatch);
        }
        if confirmation.epoch == 0 {
            return Err(AuthorityError::EpochZero);
        }
        if confirmation.members.len() > MAX_MEMBERS {
            return Err(AuthorityError::Membership(MembershipError::Capacity));
        }
        let snapshot = MembershipSnapshot::new(confirmation.epoch, &confirmation.members)?;
        if !snapshot.members.contains(&self.root) {
            return Err(AuthorityError::RootNotMember);
        }
        if !snapshot.members.contains(&expectation.subject) {
            return Err(AuthorityError::SubjectNotMember);
        }
        if snapshot.epoch < expectation.minimum_epoch {
            return Err(AuthorityError::EpochRegression {
                local: expectation.minimum_epoch,
                remote: snapshot.epoch,
            });
        }
        if let Some(previous) = &expectation.previous {
            if snapshot.epoch == previous.epoch && snapshot.members != previous.members {
                return Err(AuthorityError::EpochConflict);
            }
        }
        Ok(snapshot)
    }
}
