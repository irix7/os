// SPDX-License-Identifier: GPL-3.0-only
//! The join state machine for the root and for a member.
//!
//! The module owns the socket and the frame loop; this type owns only the
//! membership policy.  It is deliberately synchronous and clock-injected so a
//! host test can drive timeouts, retries and replays without sleeping.
//!
//! ## Restart rules (M2, no persisted epoch store)
//!
//! * **Member restart, root still up.** The member starts from the configured
//!   seed epoch and asks to join.  The root recognises an existing member and
//!   re-confirms its current epoch and full list; the member installs the same
//!   epoch.  This is the clean rejoin the rig exercises.
//! * **Root restart, member still up.** The root starts from the seed epoch.
//!   The member presents its higher confirmed epoch; the root refuses with
//!   `epoch-regression` rather than lower the member's view.  The member keeps
//!   its last view, does not advance the epoch, and waits for an operator to
//!   re-seed both NVRAM images.  Failing closed is the M2 rule; a persisted
//!   epoch store would let the root resume instead.
//! * **Both restart.** The same seed and the same join sequence reproduce the
//!   same epoch and snapshot deterministically.
//!
//! The module cannot write NVRAM (the ABI has `config_get` only), so the
//! confirmed epoch is in memory for one boot.  That is a documented M2 limit,
//! not a silent assumption.

use mesh_service::{MembershipError, MembershipTable};
use sys_primitives::{NodeId, NodeKey};

use crate::authority::{
    AuthorityError, AuthorityRoot, AuthorityVerifier, Expectation, SigningAuthority,
};
use crate::wire::{
    EpochRefusal, JoinAck, JoinMessage, JoinRequest, MembershipSnapshot, RefusalReason,
};

/// Interval between retries of the same join request.
pub const RETRY_INTERVAL_MS: u64 = 5_000;
/// Maximum sends of one join request, including the first.
pub const MAX_ATTEMPTS: u32 = 5;

/// Which side of the 1-of-1 authority this node is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    /// The configured authority root: the only epoch writer.
    Root,
    /// A member: it may request and install, never increment.
    Member,
}

impl Role {
    /// A short stable label for logs.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Root => "root",
            Self::Member => "member",
        }
    }
}

/// What the module should do after a timer tick.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tick {
    /// Nothing to do.
    Idle,
    /// Send (or resend) the same join request.
    Send(JoinRequest),
    /// The attempt budget is exhausted; log once and keep the last view.
    TimedOut,
}

/// A state change the module should log.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum JoinEvent {
    /// Nothing observable.
    None,
    /// The root signed a confirmation.
    Confirmed(MembershipSnapshot),
    /// The member installed a newer (or first) confirmed view.
    Converged(MembershipSnapshot),
    /// The member re-installed the same confirmed view.
    Duplicate(MembershipSnapshot),
    /// The authority refused the request; no view changed.
    Refused {
        /// The refusal reason.
        reason: RefusalReason,
        /// The root's current epoch.
        epoch: u32,
    },
    /// The root received the member's terminal acknowledgement.
    Acked {
        /// The acknowledging member.
        node: NodeId,
        /// The installed epoch.
        epoch: u32,
        /// The installed digest.
        digest: [u8; 16],
    },
    /// The message was not acceptable; the view did not change.
    Rejected(&'static str),
}

/// The result of handling one join message.
pub struct Outcome {
    /// A message to send back, if any.
    pub reply: Option<JoinMessage>,
    /// What to log.
    pub event: JoinEvent,
}

impl Outcome {
    fn rejected(label: &'static str) -> Self {
        Self {
            reply: None,
            event: JoinEvent::Rejected(label),
        }
    }
}

struct Outstanding {
    request: JoinRequest,
    sent_ms: u64,
}

struct RootState {
    authority: AuthorityRoot,
    converged: MembershipSnapshot,
}

struct MemberState {
    table: MembershipTable,
    verifier: AuthorityVerifier,
    outstanding: Option<Outstanding>,
    attempts: u32,
    timed_out: bool,
    converged: Option<MembershipSnapshot>,
}

enum Engine {
    Root(RootState),
    Member(MemberState),
}

/// The local join state for one node.
pub struct JoinNode {
    self_id: NodeId,
    root: NodeId,
    seed_epoch: u32,
    sequence: u64,
    engine: Engine,
}

impl JoinNode {
    /// Create the authority root side.
    pub fn new_root(
        self_id: NodeId,
        signing: SigningAuthority,
        seed_epoch: u32,
        now_ms: u64,
    ) -> Result<Self, AuthorityError> {
        let authority = AuthorityRoot::new(self_id, signing, seed_epoch, now_ms)?;
        let converged = authority.snapshot()?;
        Ok(Self {
            self_id,
            root: self_id,
            seed_epoch,
            sequence: 0,
            engine: Engine::Root(RootState {
                authority,
                converged,
            }),
        })
    }

    /// Create the member side.
    pub fn new_member(
        self_id: NodeId,
        root: NodeId,
        key: NodeKey,
        seed_epoch: u32,
        now_ms: u64,
    ) -> Result<Self, AuthorityError> {
        if self_id.is_zero() {
            return Err(AuthorityError::WrongSubject);
        }
        if seed_epoch == 0 {
            return Err(AuthorityError::BadSeed);
        }
        let verifier = AuthorityVerifier::new(root, key)?;
        let table = MembershipTable::confirmed(seed_epoch, &[self_id], now_ms)?;
        Ok(Self {
            self_id,
            root,
            seed_epoch,
            sequence: 0,
            engine: Engine::Member(MemberState {
                table,
                verifier,
                outstanding: None,
                attempts: 0,
                timed_out: false,
                converged: None,
            }),
        })
    }

    /// This node's role.
    pub const fn role(&self) -> Role {
        match &self.engine {
            Engine::Root(_) => Role::Root,
            Engine::Member(_) => Role::Member,
        }
    }

    /// The configured root identity.
    pub const fn root(&self) -> NodeId {
        self.root
    }

    /// This node's identity.
    pub const fn self_id(&self) -> NodeId {
        self.self_id
    }

    /// The configured seed epoch.
    pub const fn seed_epoch(&self) -> u32 {
        self.seed_epoch
    }

    /// The current local epoch (confirmed for a root or an already-joined
    /// member, the seed otherwise).
    pub fn epoch(&self) -> u32 {
        match &self.engine {
            Engine::Root(state) => state.authority.epoch(),
            Engine::Member(state) => state.table.epoch(),
        }
    }

    /// The current local snapshot.
    pub fn snapshot(&self) -> MembershipSnapshot {
        match &self.engine {
            Engine::Root(state) => state.converged.clone(),
            Engine::Member(state) => state.snapshot(),
        }
    }

    /// The last authority-confirmed snapshot, if this node has one.
    pub fn converged(&self) -> Option<&MembershipSnapshot> {
        match &self.engine {
            Engine::Root(state) => Some(&state.converged),
            Engine::Member(state) => state.converged.as_ref(),
        }
    }

    /// The configured authority public key.
    ///
    /// The root derives it from its signing key; the member holds the key
    /// provisioned as `mesh.pub`.  A route or capability verifier can use this
    /// without knowing which role this node has.
    pub fn authority_key(&self) -> NodeKey {
        match &self.engine {
            Engine::Root(state) => state.authority.public_key(),
            Engine::Member(state) => state.verifier.key(),
        }
    }

    /// Sign one canonical authority message, or return `None` on a member.
    ///
    /// This is the root-only seam used by the M2 route layer for capability
    /// issuance and the bounded home-move directive.  The seed never leaves
    /// the join state.
    pub fn sign_authority(&self, message: &[u8]) -> Option<[u8; 64]> {
        match &self.engine {
            Engine::Root(state) => Some(state.authority.sign(message)),
            Engine::Member(_) => None,
        }
    }

    /// The number of sends of the current request.
    pub fn attempts(&self) -> u32 {
        match &self.engine {
            Engine::Root(_) => 0,
            Engine::Member(state) => state.attempts,
        }
    }

    /// Start a new link.  A member begins a fresh join attempt; the root waits
    /// for a request.
    pub fn on_link_up(&mut self, now_ms: u64) -> Option<JoinRequest> {
        let self_id = self.self_id;
        let root = self.root;
        let sequence = self.sequence.wrapping_add(1);
        self.sequence = sequence;
        match &mut self.engine {
            Engine::Root(_) => None,
            Engine::Member(state) => {
                state.outstanding = None;
                state.attempts = 0;
                state.timed_out = false;
                Some(state.new_attempt(self_id, root, now_ms, sequence))
            }
        }
    }

    /// The link was torn down.  Keep the confirmed view; drop only the
    /// in-flight attempt so a new link starts clean.
    pub fn on_link_down(&mut self) {
        if let Engine::Member(state) = &mut self.engine {
            state.outstanding = None;
            state.attempts = 0;
            state.timed_out = false;
        }
    }

    /// Retry the outstanding request after the interval, up to the budget.
    pub fn poll(&mut self, now_ms: u64) -> Tick {
        match &mut self.engine {
            Engine::Root(_) => Tick::Idle,
            Engine::Member(state) => state.poll(now_ms),
        }
    }

    /// Handle one decoded join body from `src`.
    pub fn on_message(&mut self, src: NodeId, message: JoinMessage, now_ms: u64) -> Outcome {
        let self_id = self.self_id;
        let root = self.root;
        if src.is_zero() || src == self_id {
            return Outcome::rejected("bad-source");
        }
        match &mut self.engine {
            Engine::Root(state) => state.handle(root, src, message, now_ms),
            Engine::Member(state) => state.handle(self_id, src, message, now_ms),
        }
    }
}

impl RootState {
    fn handle(&mut self, root: NodeId, src: NodeId, message: JoinMessage, now_ms: u64) -> Outcome {
        match message {
            JoinMessage::Request(request) => {
                if request.node != src {
                    return Outcome::rejected("request-subject");
                }
                match self.authority.confirm(&request, now_ms) {
                    Ok(confirmation) => {
                        let snapshot = confirmation.snapshot();
                        self.converged = snapshot.clone();
                        Outcome {
                            reply: Some(JoinMessage::Confirm(confirmation)),
                            event: JoinEvent::Confirmed(snapshot),
                        }
                    }
                    Err(error) => {
                        let reason = refusal_reason(error);
                        let epoch = error
                            .epochs()
                            .map(|(local, _)| local)
                            .unwrap_or_else(|| self.authority.epoch());
                        Outcome {
                            reply: Some(JoinMessage::Refused(EpochRefusal {
                                root,
                                epoch,
                                reason,
                                request_nonce: request.nonce,
                            })),
                            event: JoinEvent::Refused { reason, epoch },
                        }
                    }
                }
            }
            JoinMessage::Ack(ack) => {
                if ack.node != src {
                    return Outcome::rejected("ack-subject");
                }
                if ack.epoch != self.converged.epoch || ack.digest != self.converged.digest() {
                    return Outcome::rejected("ack-mismatch");
                }
                Outcome {
                    reply: None,
                    event: JoinEvent::Acked {
                        node: ack.node,
                        epoch: ack.epoch,
                        digest: ack.digest,
                    },
                }
            }
            _ => Outcome::rejected("unexpected-message"),
        }
    }
}

impl MemberState {
    fn snapshot(&self) -> MembershipSnapshot {
        MembershipSnapshot::from_view(&self.table.snapshot())
            .expect("the member table always has a member")
    }

    fn poll(&mut self, now_ms: u64) -> Tick {
        let Some((request, sent_ms)) = self
            .outstanding
            .as_ref()
            .map(|outstanding| (outstanding.request, outstanding.sent_ms))
        else {
            return Tick::Idle;
        };
        if now_ms.saturating_sub(sent_ms) < RETRY_INTERVAL_MS {
            return Tick::Idle;
        }
        if self.attempts >= MAX_ATTEMPTS {
            self.outstanding = None;
            self.timed_out = true;
            return Tick::TimedOut;
        }
        self.attempts = self.attempts.saturating_add(1);
        if let Some(outstanding) = &mut self.outstanding {
            outstanding.sent_ms = now_ms;
        }
        Tick::Send(request)
    }

    fn handle(
        &mut self,
        self_id: NodeId,
        src: NodeId,
        message: JoinMessage,
        now_ms: u64,
    ) -> Outcome {
        match message {
            JoinMessage::Confirm(confirmation) => {
                if src != confirmation.root {
                    return Outcome::rejected("confirm-source");
                }
                let Some(outstanding) = &self.outstanding else {
                    return Outcome::rejected("no-outstanding-request");
                };
                let previous = self.snapshot();
                let expectation = Expectation {
                    subject: self_id,
                    nonce: outstanding.request.nonce,
                    incarnation: outstanding.request.incarnation,
                    minimum_epoch: self.table.epoch(),
                    previous: Some(previous.clone()),
                };
                let snapshot = match self.verifier.verify(&confirmation, &expectation) {
                    Ok(snapshot) => snapshot,
                    Err(error) => {
                        // The answer arrived; a bad signature, a wrong nonce or
                        // a stale epoch will not fix itself on a retry.  Stop
                        // the attempt and keep the previous view.
                        self.outstanding = None;
                        self.attempts = 0;
                        return Outcome::rejected(error.as_str());
                    }
                };
                let ack = JoinAck {
                    node: self_id,
                    epoch: snapshot.epoch,
                    digest: snapshot.digest(),
                    request_nonce: outstanding.request.nonce,
                };
                let duplicate =
                    snapshot.epoch == previous.epoch && snapshot.members == previous.members;
                if let Err(error) = self
                    .table
                    .install(snapshot.epoch, &snapshot.members, now_ms)
                {
                    return Outcome::rejected(membership_label(error));
                }
                if duplicate {
                    // The same epoch and list are already installed.  This is
                    // the terminal answer for a reconnect/rejoin; do not keep
                    // retrying the request.
                    self.outstanding = None;
                    self.attempts = 0;
                    self.timed_out = false;
                    return Outcome {
                        reply: Some(JoinMessage::Ack(ack)),
                        event: JoinEvent::Duplicate(snapshot),
                    };
                }
                self.outstanding = None;
                self.attempts = 0;
                self.timed_out = false;
                self.converged = Some(snapshot.clone());
                Outcome {
                    reply: Some(JoinMessage::Ack(ack)),
                    event: JoinEvent::Converged(snapshot),
                }
            }
            JoinMessage::Refused(refusal) => {
                if refusal.root != self.verifier.root() {
                    return Outcome::rejected("refusal-root");
                }
                let Some(outstanding) = &self.outstanding else {
                    return Outcome::rejected("stale-refusal");
                };
                if refusal.request_nonce != outstanding.request.nonce {
                    return Outcome::rejected("stale-refusal");
                }
                self.outstanding = None;
                self.attempts = 0;
                Outcome {
                    reply: None,
                    event: JoinEvent::Refused {
                        reason: refusal.reason,
                        epoch: refusal.epoch,
                    },
                }
            }
            _ => Outcome::rejected("unexpected-message"),
        }
    }

    fn new_attempt(
        &mut self,
        self_id: NodeId,
        root: NodeId,
        now_ms: u64,
        sequence: u64,
    ) -> JoinRequest {
        let (nonce, incarnation) = attempt_ids(self_id, root, now_ms, sequence);
        let snapshot = self.snapshot();
        let request = JoinRequest {
            node: self_id,
            incarnation,
            last_epoch: snapshot.epoch,
            last_digest: snapshot.digest(),
            nonce,
        };
        self.outstanding = Some(Outstanding {
            request,
            sent_ms: now_ms,
        });
        self.attempts = 1;
        self.timed_out = false;
        request
    }
}

/// Derive a boot-local nonce and incarnation for one attempt.
///
/// The nonce binds a confirmation to this attempt; it is not a secret and it
/// is not a cross-restart replay window (M2-ADR-0002 defers that to the TLS
/// transport).  Epoch regression and same-epoch equivocation are the
/// cross-attempt protections.
fn attempt_ids(self_id: NodeId, root: NodeId, now_ms: u64, sequence: u64) -> (u64, u64) {
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"IRIX7-M2-JOIN-NONCE-v1");
    hasher.update(self_id.as_bytes());
    hasher.update(root.as_bytes());
    hasher.update(&now_ms.to_le_bytes());
    hasher.update(&sequence.to_le_bytes());
    let hash = hasher.finalize();
    let bytes = hash.as_bytes();
    let nonce = u64::from_le_bytes([
        bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5], bytes[6], bytes[7],
    ]);
    let incarnation = u64::from_le_bytes([
        bytes[8], bytes[9], bytes[10], bytes[11], bytes[12], bytes[13], bytes[14], bytes[15],
    ])
    .max(1);
    (nonce, incarnation)
}

fn refusal_reason(error: AuthorityError) -> RefusalReason {
    match error {
        AuthorityError::EpochRegression { .. } => RefusalReason::EpochRegression,
        AuthorityError::WrongSubject | AuthorityError::ZeroRoot => RefusalReason::WrongSubject,
        AuthorityError::Membership(MembershipError::Capacity) => RefusalReason::Busy,
        AuthorityError::EpochConflict => RefusalReason::EpochConflict,
        _ => RefusalReason::Unsupported,
    }
}

fn membership_label(error: MembershipError) -> &'static str {
    match error {
        MembershipError::Capacity => "capacity",
        MembershipError::EpochOverflow => "epoch-overflow",
        MembershipError::UnknownNode => "unknown-node",
        MembershipError::EmptyView => "empty-view",
        MembershipError::ZeroNode => "zero-node",
        MembershipError::DuplicateNode => "duplicate-node",
        MembershipError::EpochRegression => "epoch-regression",
        MembershipError::EpochConflict => "epoch-conflict",
    }
}

#[cfg(test)]
mod tests;
