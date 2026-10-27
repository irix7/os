// SPDX-License-Identifier: GPL-3.0-only
//! The join state machine for the root and for a member.
//!
//! The module owns the socket and the frame loop; this type owns only the
//! membership policy.  It is deliberately synchronous and clock-injected so a
//! host test can drive timeouts, retries and replays without sleeping.
//!
//! ## Restart rules and the signed epoch record
//!
//! The module cannot write NVRAM, but ABI 11 exposes `block_read`/`block_write`.
//! A root can sign an [`EpochRecord`](crate::EpochRecord) and a member can
//! persist the signed confirmation it installed; both are verified again
//! before use.
//!
//! * **Member restart, root still up.** The member restores the signed
//!   confirmation, starts at the confirmed epoch and asks to join with that
//!   `last_epoch`.  The root recognises an existing member and re-confirms the
//!   same epoch and full list.
//! * **Root restart, member still up.** The root restores a signed
//!   [`EpochRecord`](crate::EpochRecord) and resumes the confirmed epoch.  A live member presents
//!   the same epoch; the root confirms it.  Without a valid record the old rule
//!   applies: the root starts at the seed, refuses a higher member epoch with
//!   `epoch-regression`, and the member keeps its last view.
//! * **Both restart.** Each verifies its own signed record and resumes the same
//!   epoch.  A missing store logs the absence and falls back to the seed; a
//!   corrupt or foreign record is refused rather than installed.
//!
//! [`EpochRecord`]: crate::wire::EpochRecord

use alloc::boxed::Box;
use alloc::collections::BTreeMap;
use alloc::vec::Vec;

use mesh_service::{FailureDetector, MembershipError, MembershipTable, NodeState};
use sys_primitives::{NodeId, NodeKey};

use crate::authority::{
    AuthorityError, AuthorityRoot, AuthorityVerifier, Expectation, SigningAuthority,
};
use crate::wire::{
    EpochConfirmation, EpochRecord, EpochRefusal, JoinAck, JoinMessage, JoinRequest,
    MembershipSnapshot, RefusalReason,
};
use crate::wire::{EpochSummary, MemberGrant, MemberSummary};

/// Interval between retries of the same join request.
pub const RETRY_INTERVAL_MS: u64 = 5_000;
/// Maximum sends of one join request, including the first.
pub const MAX_ATTEMPTS: u32 = 5;
/// Interval at which a converged member asks the authority for the current
/// epoch again.
///
/// M2 only asked on link-up; that is enough for two nodes, because a second
/// join advances the epoch and the first member would never hear about it.  M3
/// keeps the same signed confirmation and the same root-only epoch writer, but
/// a converged member re-asks on this bounded interval so a seven-member join
/// converges without a push protocol or a second authority.  The request is the
/// same `JoinRequest`; the root answers with the current epoch and full list.
pub const REFRESH_INTERVAL_MS: u64 = 5_000;

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
    /// Send a new signed request for the current epoch.
    ///
    /// A refresh is not a retry: it has a new nonce and incarnation, and it is
    /// only produced after a member has converged.  The authority still decides
    /// the epoch and the member list.
    Refresh(JoinRequest),
    /// Send one small liveness summary at the installed epoch.
    ///
    /// After the first confirmation a converged member stops sending signed
    /// join requests every interval.  It sends this summary instead, and the
    /// root signs at most one aggregate per change set rather than one full
    /// confirmation per member per refresh.
    Summary,
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
    /// A bounded refresh re-installed the same confirmed view.
    ///
    /// This is the routine M3 case after another member joined: unlike
    /// `Duplicate` it is not a rejoin event, so the module does not repeat the
    /// full home table in the serial log.
    Refreshed(MembershipSnapshot),
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
    /// A member has missed its heartbeat window; it is still routable.
    Suspect {
        /// The silent member.
        node: NodeId,
        /// The confirmed epoch at the transition.
        epoch: u32,
    },
    /// The failure detector confirmed a member unavailable.
    Failed {
        /// The failed member.
        node: NodeId,
        /// The confirmed epoch at the transition.
        epoch: u32,
    },
    /// A suspect member answered again before failure was confirmed.
    Recovered {
        /// The recovered member.
        node: NodeId,
        /// The confirmed epoch.
        epoch: u32,
    },
    /// The root removed a failed member and advanced the epoch exactly once.
    Removed {
        /// The removed member.
        node: NodeId,
        /// The new confirmed view.
        snapshot: MembershipSnapshot,
    },
    /// The root removed a whole failure set in one epoch.
    RemovedBatch {
        /// Every member removed in the set.
        nodes: Vec<NodeId>,
        /// The confirmed view after the set.
        snapshot: MembershipSnapshot,
    },
    /// A member lost contact with the authority.  It cannot confirm a new
    /// epoch and must not create a second authoritative home.
    AuthorityLost {
        /// The unreachable authority root.
        node: NodeId,
        /// The last confirmed epoch the member keeps.
        epoch: u32,
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
    /// Whether this attempt is a routine epoch refresh rather than a rejoin.
    refresh: bool,
}

struct RootState {
    authority: AuthorityRoot,
    converged: MembershipSnapshot,
    /// The incarnation the root confirmed for each member.
    incarnations: BTreeMap<NodeId, u64>,
    /// Highest summary counter accepted per member in the current epoch.
    counters: BTreeMap<NodeId, u64>,
    /// The last aggregate this root signed, if any.
    published: Option<EpochSummary>,
}

struct MemberState {
    table: MembershipTable,
    verifier: AuthorityVerifier,
    outstanding: Option<Outstanding>,
    attempts: u32,
    timed_out: bool,
    converged: Option<MembershipSnapshot>,
    detector: FailureDetector,
    /// The root's current liveness state in this member's local view.
    root_state: NodeState,
    /// Monotonic time of the last accepted frame from the root.
    root_seen_ms: u64,
    /// The incarnation the root most recently confirmed for this member.
    incarnation: u64,
    /// Monotonic summary counter (one per refresh interval).
    summary_counter: u64,
    /// Whether the root's liveness is tracked (after the first confirmation).
    root_tracked: bool,
    /// Whether this member currently has an authoritative view.  A lost
    /// authority is restored only by a fresh signed confirmation.
    authority_current: bool,
    /// When the next bounded epoch refresh is due.
    ///
    /// `None` means no refresh is scheduled: before the first confirmation,
    /// after a rejection, and after a link loss.  A refresh is still a signed
    /// join request; it never advances the epoch by itself.
    refresh_at_ms: Option<u64>,
}

enum Engine {
    /// Boxed so the enum stays compact: both variants hold the potentially
    /// large state tables behind a pointer, so `Engine` itself is a single
    /// pointer and there is no large size difference between variants.
    Root(Box<RootState>),
    Member(Box<MemberState>),
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
            engine: Engine::Root(Box::new(RootState {
                authority,
                converged: converged.clone(),
                incarnations: {
                    let mut incarnations = BTreeMap::new();
                    incarnations.insert(self_id, 0);
                    incarnations
                },
                counters: BTreeMap::new(),
                published: None,
            })),
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
            engine: Engine::Member(Box::new(MemberState {
                table,
                verifier,
                outstanding: None,
                attempts: 0,
                timed_out: false,
                converged: None,
                detector: FailureDetector::m2(),
                root_state: NodeState::Alive,
                root_seen_ms: now_ms,
                incarnation: 0,
                summary_counter: 0,
                root_tracked: false,
                authority_current: false,
                refresh_at_ms: None,
            })),
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

    /// Sign the current confirmed view as a persistent epoch record.
    ///
    /// Only the authority root can produce this record; a member returns
    /// [`AuthorityError::NotRoot`].  The caller writes the record to a
    /// persistent store and verifies it through [`Self::restore_record`] on the
    /// next boot.
    pub fn epoch_record(&self) -> Result<EpochRecord, AuthorityError> {
        match &self.engine {
            Engine::Root(state) => state.authority.record(),
            Engine::Member(_) => Err(AuthorityError::NotRoot),
        }
    }

    /// Restore a root's confirmed view from a signed epoch record.
    ///
    /// The signature is checked by [`AuthorityRoot::restore_record`] before
    /// any membership field is used, and a lower epoch is refused.  This is
    /// the root restart path: the root resumes the epoch it had confirmed
    /// instead of starting from the NVRAM seed.
    pub fn restore_record(
        &mut self,
        record: &EpochRecord,
        now_ms: u64,
    ) -> Result<MembershipSnapshot, AuthorityError> {
        match &mut self.engine {
            Engine::Root(state) => {
                let snapshot = state.authority.restore_record(record, now_ms)?;
                state.converged = snapshot.clone();
                Ok(snapshot)
            }
            Engine::Member(_) => Err(AuthorityError::NotRoot),
        }
    }

    /// Restore a member's confirmed view from a signed confirmation.
    ///
    /// The member stores the last confirmation it accepted, so a restart can
    /// present `last_epoch` = the confirmed epoch rather than falling back to
    /// the seed.  The signature, root, subject and epoch are verified with the
    /// same [`AuthorityVerifier`] used on the live path.  The restored view is
    /// **not** authority-current: reads may use the existing lease, but a
    /// mutable commit and a membership change still need a fresh signed
    /// confirmation from the root.
    pub fn restore_confirmation(
        &mut self,
        confirmation: &EpochConfirmation,
        now_ms: u64,
    ) -> Result<MembershipSnapshot, AuthorityError> {
        match &mut self.engine {
            Engine::Member(state) => {
                let expectation = Expectation {
                    subject: self.self_id,
                    nonce: confirmation.request_nonce,
                    incarnation: confirmation.incarnation,
                    minimum_epoch: self.seed_epoch,
                    previous: None,
                };
                let snapshot = state.verifier.verify(confirmation, &expectation)?;
                state
                    .table
                    .install(snapshot.epoch, &snapshot.members, now_ms)?;
                state.converged = Some(snapshot.clone());
                state.outstanding = None;
                state.attempts = 0;
                state.timed_out = false;
                state.authority_current = false;
                state.refresh_at_ms = None;
                Ok(snapshot)
            }
            Engine::Root(_) => Err(AuthorityError::NotRoot),
        }
    }

    /// The number of sends of the current request.
    pub fn attempts(&self) -> u32 {
        match &self.engine {
            Engine::Root(_) => 0,
            Engine::Member(state) => state.attempts,
        }
    }

    /// Whether this node holds a view it can still treat as current.
    ///
    /// The authority root is the 1-of-1 authority, so its confirmed view is
    /// current by construction.  A member is current only while it is in
    /// contact with the authority and has installed a confirmation.  A member
    /// that cannot reach the root keeps its last confirmed view for reads but
    /// must not create a new home or accept a mutable commit.
    pub fn authority_current(&self) -> bool {
        match &self.engine {
            Engine::Root(_) => true,
            Engine::Member(state) => state.authority_current,
        }
    }

    /// Whether a mutable object may be committed on this node right now.
    ///
    /// This is the M2 availability rule: a mutable commit needs a current
    /// authoritative view *and* the home lease checked by the route layer.
    /// The side of a partition without the authority fails closed.
    pub fn mutable_available(&self) -> bool {
        self.authority_current() && self.converged().is_some()
    }

    /// Note one accepted frame from a confirmed peer.
    ///
    /// This is the membership heartbeat path: any well-formed frame from the
    /// peer is proof of life, and the join/route frames already cross the link
    /// every tick.  A stale or failed record is not revived by a bare frame.
    pub fn observe_peer(&mut self, node: NodeId, now_ms: u64) -> Option<JoinEvent> {
        if node.is_zero() || node == self.self_id {
            return None;
        }
        match &mut self.engine {
            Engine::Root(state) => {
                if !state.authority.is_member(node) {
                    return None;
                }
                let observe = state.authority.observe(node, now_ms);
                let epoch = state.authority.epoch();
                if matches!(observe, mesh_service::Observe::Revived) {
                    return Some(JoinEvent::Recovered { node, epoch });
                }
                None
            }
            Engine::Member(state) => {
                if node != self.root || !state.root_tracked {
                    return None;
                }
                let epoch = state.table.epoch();
                state.root_seen_ms = now_ms;
                if state.root_state == NodeState::Suspect {
                    state.root_state = NodeState::Alive;
                    if state.authority_current {
                        return Some(JoinEvent::Recovered { node, epoch });
                    }
                }
                None
            }
        }
    }

    /// Ask for a fresh join confirmation after authority was lost.
    ///
    /// The member never confirms itself.  When a frame from the authority
    /// arrives on a live link, this creates a new signed request (new nonce
    /// and incarnation) so the root can re-admit or re-confirm the member.
    /// Returns `None` while a request is already outstanding.
    pub fn request_rejoin(&mut self, now_ms: u64) -> Option<JoinRequest> {
        let self_id = self.self_id;
        let root = self.root;
        let sequence = self.sequence.wrapping_add(1);
        match &mut self.engine {
            Engine::Member(state)
                if state.root_tracked
                    && !state.authority_current
                    && state.outstanding.is_none() =>
            {
                self.sequence = sequence;
                Some(state.new_attempt(self_id, root, now_ms, sequence, false))
            }
            _ => None,
        }
    }

    /// Advance the failure detector and return the state changes to log.
    ///
    /// The root removes members whose failure was confirmed and advances the
    /// membership epoch exactly once per removal.  A member never removes the
    /// root: losing the authority is not a membership change, so it records
    /// `AuthorityLost` and keeps the last confirmed view.
    pub fn poll_failures(&mut self, now_ms: u64) -> alloc::vec::Vec<JoinEvent> {
        let mut events = alloc::vec::Vec::new();
        match &mut self.engine {
            Engine::Root(state) => {
                let epoch = state.authority.epoch();
                for transition in state.authority.expire(now_ms) {
                    let event = match transition.state {
                        NodeState::Suspect => JoinEvent::Suspect {
                            node: transition.node,
                            epoch,
                        },
                        NodeState::Failed => JoinEvent::Failed {
                            node: transition.node,
                            epoch,
                        },
                        NodeState::Alive => continue,
                    };
                    events.push(event);
                }
                if let Some(removal) = state.authority.remove_failed() {
                    state.converged = removal.snapshot.clone();
                    events.push(JoinEvent::RemovedBatch {
                        nodes: removal.nodes,
                        snapshot: removal.snapshot,
                    });
                }
            }
            Engine::Member(state) => {
                if !state.root_tracked {
                    return events;
                }
                let epoch = state.table.epoch();
                let next = state
                    .detector
                    .next_state(state.root_state, state.root_seen_ms, now_ms);
                match next {
                    NodeState::Suspect if state.root_state == NodeState::Alive => {
                        state.root_state = NodeState::Suspect;
                        events.push(JoinEvent::Suspect {
                            node: state.verifier.root(),
                            epoch,
                        });
                    }
                    NodeState::Failed if state.root_state != NodeState::Failed => {
                        state.root_state = NodeState::Failed;
                        events.push(JoinEvent::Failed {
                            node: state.verifier.root(),
                            epoch,
                        });
                        if state.authority_current {
                            state.authority_current = false;
                            events.push(JoinEvent::AuthorityLost {
                                node: state.verifier.root(),
                                epoch,
                            });
                        }
                    }
                    _ => {}
                }
            }
        }
        events
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
                state.refresh_at_ms = None;
                // A new link is a fresh chance to hear the authority.  It does
                // not restore authority by itself: only a signed confirmation
                // can do that.
                state.root_seen_ms = now_ms;
                if state.root_state != NodeState::Failed {
                    state.root_state = NodeState::Alive;
                }
                Some(state.new_attempt(self_id, root, now_ms, sequence, false))
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
            state.refresh_at_ms = None;
        }
    }

    /// Retry the outstanding request after the interval, up to the budget, and
    /// start one bounded epoch refresh after a converged member's interval.
    pub fn poll(&mut self, now_ms: u64) -> Tick {
        match &mut self.engine {
            Engine::Root(_) => Tick::Idle,
            Engine::Member(state) => {
                let tick = state.poll(now_ms);
                if !matches!(tick, Tick::Idle) {
                    return tick;
                }
                if !state.refresh_due(now_ms) {
                    return Tick::Idle;
                }
                state.refresh_at_ms = Some(now_ms.saturating_add(REFRESH_INTERVAL_MS));
                Tick::Summary
            }
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
    /// Build the member's periodic liveness summary at the installed epoch.
    ///
    /// Member only.  Returns `None` before the first confirmation or when the
    /// authority view is lost.  The counter advances on every call so a replay
    /// of an old summary is detectable.
    pub fn member_summary(&mut self) -> Option<MemberSummary> {
        let Engine::Member(state) = &mut self.engine else {
            return None;
        };
        if !state.authority_current || state.converged.is_none() {
            return None;
        }
        let snapshot = state.snapshot();
        let counter = {
            state.summary_counter = state.summary_counter.saturating_add(1);
            state.summary_counter
        };
        Some(MemberSummary {
            node: self.self_id,
            epoch: snapshot.epoch,
            digest: snapshot.digest(),
            incarnation: state.incarnation,
            counter,
        })
    }

    /// Root: sign one aggregate for the current confirmed view and publish it.
    ///
    /// Returns `None` when not a root.  The signed aggregate binds every
    /// member `(node, incarnation)` to `epoch + digest`; it is the single
    /// signature per change set.
    pub fn publish_aggregate(&mut self) -> Option<EpochSummary> {
        let Engine::Root(state) = &mut self.engine else {
            return None;
        };
        let snapshot = state.converged.clone();
        let mut grants: Vec<MemberGrant> = snapshot
            .members
            .iter()
            .map(|&node| MemberGrant {
                node,
                incarnation: state.incarnations.get(&node).copied().unwrap_or(0),
            })
            .collect();
        grants.sort_by_key(|grant| grant.node);
        let mut summary = EpochSummary::new(self.root, snapshot.epoch, &grants).ok()?;
        summary.signature = state.authority.sign(&summary.signed_bytes());
        state.published = Some(summary.clone());
        Some(summary)
    }

    /// Member: install a root-signed aggregate.
    ///
    /// The signature is verified before any field is trusted.  The aggregate
    /// must name this member and must not move the local epoch backwards.
    pub fn install_aggregate(
        &mut self,
        summary: &EpochSummary,
        now_ms: u64,
    ) -> Result<JoinEvent, &'static str> {
        let Engine::Member(state) = &mut self.engine else {
            return Err("not-member");
        };
        state.install_aggregate(self.self_id, summary, now_ms)
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
                        self.incarnations
                            .insert(confirmation.subject, confirmation.incarnation);
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
                if ack.epoch < self.converged.epoch {
                    return Outcome {
                        reply: None,
                        event: JoinEvent::Rejected("ack-stale"),
                    };
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
            JoinMessage::Summary(summary) => {
                if summary.epoch != self.converged.epoch
                    || summary.digest != self.converged.digest()
                {
                    return Outcome::rejected("epoch-digest-mismatch");
                }
                let known = self.incarnations.get(&src).copied().unwrap_or(0);
                if known != 0 && summary.incarnation != known {
                    return Outcome::rejected("incarnation-mismatch");
                }
                let prev = self.counters.get(&src).copied().unwrap_or(0);
                if summary.counter <= prev {
                    return Outcome::rejected("stale-counter");
                }
                self.counters.insert(src, summary.counter);
                self.authority
                    .observe_with_incarnation(src, summary.incarnation, now_ms);
                Outcome {
                    reply: None,
                    event: JoinEvent::None,
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
                let refresh = outstanding.refresh;
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
                // A verified confirmation is the only path that makes a
                // member's view authoritative again.  It also clears the
                // failure clock for the link that carried it.
                self.root_tracked = true;
                self.root_seen_ms = now_ms;
                self.root_state = NodeState::Alive;
                self.authority_current = true;
                self.refresh_at_ms = Some(now_ms.saturating_add(REFRESH_INTERVAL_MS));
                if duplicate {
                    // The same epoch and list are already installed.  This is
                    // the terminal answer for a reconnect/rejoin; do not keep
                    // retrying the request.  After a restart the restored epoch
                    // may not yet be marked converged, so install it now.
                    if self.converged.is_none() {
                        self.converged = Some(snapshot.clone());
                    }
                    self.outstanding = None;
                    self.attempts = 0;
                    self.timed_out = false;
                    return Outcome {
                        reply: Some(JoinMessage::Ack(ack)),
                        event: if refresh {
                            JoinEvent::Refreshed(snapshot)
                        } else {
                            JoinEvent::Duplicate(snapshot)
                        },
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
            JoinMessage::Epoch(summary) => {
                match self.install_aggregate(self_id, &summary, now_ms) {
                    Ok(event) => Outcome { reply: None, event },
                    Err(reason) => Outcome::rejected(reason),
                }
            }
            _ => Outcome::rejected("unexpected-message"),
        }
    }

    fn install_aggregate(
        &mut self,
        self_id: NodeId,
        summary: &EpochSummary,
        now_ms: u64,
    ) -> Result<JoinEvent, &'static str> {
        self.verifier
            .verify_signature(&summary.signed_bytes(), &summary.signature)
            .map_err(|_| "bad-signature")?;
        let snapshot = summary.snapshot().map_err(|_| "digest-mismatch")?;
        if !snapshot.members.contains(&self_id) {
            return Err("not-a-member");
        }
        if snapshot.epoch < self.table.epoch() {
            return Err("epoch-regression");
        }
        self.table
            .install(snapshot.epoch, &snapshot.members, now_ms)
            .map_err(|_| "install-failed")?;
        self.incarnation = summary.grant(self_id).unwrap_or(0);
        self.root_tracked = true;
        self.root_seen_ms = now_ms;
        self.root_state = NodeState::Alive;
        self.authority_current = true;
        self.refresh_at_ms = Some(now_ms.saturating_add(REFRESH_INTERVAL_MS));
        let was_converged = self.converged.is_some();
        self.converged = Some(snapshot.clone());
        if was_converged {
            Ok(JoinEvent::Refreshed(snapshot))
        } else {
            Ok(JoinEvent::Converged(snapshot))
        }
    }

    fn new_attempt(
        &mut self,
        self_id: NodeId,
        root: NodeId,
        now_ms: u64,
        sequence: u64,
        refresh: bool,
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
            refresh,
        });
        self.attempts = 1;
        self.timed_out = false;
        self.refresh_at_ms = None;
        request
    }

    /// Whether a converged, authoritative member should ask for the current
    /// epoch again.
    ///
    /// The first call after a confirmation schedules the interval; later calls
    /// are due only at the boundary.  A member that has lost the authority or
    /// has never converged never starts a refresh on its own; the rejoin path
    /// handles that case.
    fn refresh_due(&mut self, now_ms: u64) -> bool {
        if !self.authority_current || self.converged.is_none() || self.outstanding.is_some() {
            return false;
        }
        match self.refresh_at_ms {
            None => {
                self.refresh_at_ms = Some(now_ms.saturating_add(REFRESH_INTERVAL_MS));
                false
            }
            Some(at) => now_ms >= at,
        }
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
