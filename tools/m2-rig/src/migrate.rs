// SPDX-License-Identifier: GPL-3.0-only
//! M2-5 L0 stackless-actor migration evidence.
//!
//! The MCP module exposes `GET /mesh/migrate` as a bounded in-memory report and
//! `GET /mesh/migrate?step=N` as the rig-driven lab control surface.  The rig
//! chooses the order; the policy chooses the actor, hosts and message ids from
//! the confirmed view.
//!
//! ```text
//! 1  seed at n0 and stop with message 11 queued and message 10 unacknowledged
//! 2  move n0 -> n1
//! 3  resume the queued message at n1
//! 4  refuse a stale delivery at the old n0 host
//! 5  duplicate-deliver message 11 at n1
//! 6  queue message 12 at n1
//! 7  move n1 -> n0
//! 8  resume the queued message at n0
//! 9  refuse a stale delivery at the old n1 host
//! 10 duplicate-deliver message 12 at n0
//! 11 refuse a tampered capability at n0
//! ```
//!
//! This module parses the reports, recomputes the actor identity, logical
//! message chain and canonical state hash independently of `mesh-migrate`, and
//! records the exact rows in the evidence bundle.

use std::fmt::Write as _;
use std::time::{Duration, Instant};

use crate::homes;
use crate::rig::{Fault, Node};

const PREFIX: &str = "mesh-migrate: ";
const COMMAND_SETTLE: Duration = Duration::from_secs(6);
const OBJECT_DOMAIN: &[u8] = b"IRIX7-M2-HOME-OBJECT-v1";
const ACTOR_DOMAIN: &[u8] = b"IRIX7-M2-ACTOR-v1";
const INIT_DOMAIN: &[u8] = b"IRIX7-M2-ACTOR-INIT-v1";
const STEP_DOMAIN: &[u8] = b"IRIX7-M2-ACTOR-STEP-v1";
const MESSAGE_DOMAIN: &[u8] = b"IRIX7-M2-ACTOR-MSG-v1";
const HASH_DOMAIN: &[u8] = b"IRIX7-M2-ACTOR-STATE-v1";
const HOME_DOMAIN: &[u8] = b"IRIX7-HOME-v1";
const SCHEMA: u32 = 1;

/// One queued or acknowledged message parsed from a report.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MessageRow {
    /// Message id.
    pub id: u64,
    /// Payload, upper-case hex.
    pub payload: String,
}

/// One serialised capability handle.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HandleRow {
    /// Object ID, upper-case hex.
    pub object: String,
    /// Rights bits.
    pub rights: u8,
    /// Home NodeId, upper-case hex.
    pub home: String,
    /// Home epoch.
    pub home_epoch: u32,
    /// Object epoch.
    pub object_epoch: u32,
}

/// One actor row from `/mesh/migrate`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ActorRow {
    /// Actor identity, upper-case hex.
    pub id: String,
    /// Rendezvous home, upper-case hex.
    pub home: String,
    /// `active`, `provisional` or `fenced`.
    pub state: String,
    /// Current host, upper-case hex.
    pub host: String,
    /// Placement epoch.
    pub epoch: u32,
    /// Applied-message counter.
    pub applied: u64,
    /// Highest applied message id.
    pub last: u64,
    /// Canonical state hash, upper-case hex.
    pub hash: String,
    /// Logical digest, upper-case hex.
    pub logical: String,
    /// Queued message.
    pub queued: Option<MessageRow>,
    /// Last acknowledgement.
    pub unacked: Option<MessageRow>,
    /// Capability handle.
    pub handle: Option<HandleRow>,
}

/// The latest placement change.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MoveRow {
    /// Actor identity, upper-case hex.
    pub actor: String,
    /// Old host, upper-case hex.
    pub old_host: String,
    /// New host, upper-case hex.
    pub new_host: String,
    /// Old placement epoch.
    pub old_epoch: u32,
    /// New placement epoch.
    pub new_epoch: u32,
    /// Canonical state hash.
    pub hash: String,
}

/// Bounded migration counters.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Counters {
    /// Completed migrations.
    pub migrations: u32,
    /// Migrations where this node was the destination.
    pub incoming: u32,
    /// Migrations where this node was the source.
    pub outgoing: u32,
    /// Queued messages resumed.
    pub resumes: u32,
    /// Idempotent duplicate deliveries.
    pub duplicates: u32,
    /// Stale-home or stale-epoch refusals.
    pub stale: u32,
    /// Delivery capability refusals.
    pub capability: u32,
    /// Other terminal refusals.
    pub rejects: u32,
    /// Aborted migrations.
    pub aborted: u32,
}

/// One parsed `/mesh/migrate` body.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MigrateReport {
    /// `enabled` or `disabled`.
    pub state: String,
    /// Disabled reason.
    pub reason: Option<String>,
    /// `root` or `member`.
    pub role: Option<String>,
    /// Confirmed view epoch.
    pub view_epoch: Option<u32>,
    /// Confirmed member count.
    pub members: Option<usize>,
    /// Confirmed membership digest, upper-case hex.
    pub digest: Option<String>,
    /// The actor row, when one exists.
    pub actor: Option<ActorRow>,
    /// The latest placement change.
    pub last_move: Option<MoveRow>,
    /// Counters.
    pub counters: Counters,
    /// Last protocol stage.
    pub stage: Option<String>,
    /// Last refusal reason.
    pub last_reason: Option<String>,
    /// The exact body.
    pub raw: String,
}

impl MigrateReport {
    /// Parse a `/mesh/migrate` body.
    pub fn parse(text: &str) -> Result<Self, String> {
        let mut report = Self::default();
        let mut actor = None;
        let mut counters_seen = false;
        for line in text.lines() {
            let line = line.trim();
            let Some(rest) = line.strip_prefix(PREFIX) else {
                continue;
            };
            let mut tokens = rest.split_whitespace();
            let Some(first) = tokens.next() else {
                continue;
            };
            match first {
                "state=enabled" => {
                    report.state = "enabled".into();
                    for token in tokens {
                        let (key, value) = split(token)?;
                        match key {
                            "role" => report.role = Some(value.to_string()),
                            "view_epoch" => report.view_epoch = Some(parse_u32(value)?),
                            "members" => report.members = Some(parse_usize(value)?),
                            "digest" => report.digest = Some(value.to_ascii_uppercase()),
                            other => return Err(format!("unknown migrate field {other:?}")),
                        }
                    }
                }
                "state=disabled" => {
                    report.state = "disabled".into();
                    for token in tokens {
                        let (key, value) = split(token)?;
                        match key {
                            "reason" => report.reason = Some(value.to_string()),
                            other => return Err(format!("unknown migrate field {other:?}")),
                        }
                    }
                }
                "actor" => {
                    let mut has_identity = false;
                    let mut row = actor.take().unwrap_or_else(empty_actor);
                    for token in tokens {
                        let (key, value) = split(token)?;
                        match key {
                            "id" => {
                                row.id = value.to_ascii_uppercase();
                                has_identity = true;
                            }
                            "home" => row.home = value.to_ascii_uppercase(),
                            "state" => {
                                if value == "none" {
                                    actor = None;
                                    has_identity = false;
                                    break;
                                }
                                row.state = value.to_string();
                            }
                            "host" => row.host = value.to_ascii_uppercase(),
                            "epoch" => row.epoch = parse_u32(value)?,
                            "applied" => row.applied = parse_u64(value)?,
                            "last" => row.last = parse_u64(value)?,
                            "hash" => row.hash = value.to_ascii_uppercase(),
                            "logical" => row.logical = value.to_ascii_uppercase(),
                            "queued" => row.queued = parse_message(value)?,
                            "unacked" => row.unacked = parse_message(value)?,
                            "handle" => row.handle = parse_handle(value)?,
                            other => return Err(format!("unknown actor field {other:?}")),
                        }
                    }
                    let _ = has_identity;
                    if !row.id.is_empty() {
                        actor = Some(row);
                    }
                }
                "move" => {
                    let mut row = MoveRow {
                        actor: String::new(),
                        old_host: String::new(),
                        new_host: String::new(),
                        old_epoch: 0,
                        new_epoch: 0,
                        hash: String::new(),
                    };
                    for token in tokens {
                        let (key, value) = split(token)?;
                        match key {
                            "actor" => row.actor = value.to_ascii_uppercase(),
                            "old" => row.old_host = value.to_ascii_uppercase(),
                            "new" => row.new_host = value.to_ascii_uppercase(),
                            "old_epoch" => row.old_epoch = parse_u32(value)?,
                            "new_epoch" => row.new_epoch = parse_u32(value)?,
                            "hash" => row.hash = value.to_ascii_uppercase(),
                            other => return Err(format!("unknown move field {other:?}")),
                        }
                    }
                    if row.actor.is_empty() || row.old_host.is_empty() || row.new_host.is_empty() {
                        return Err("migrate move row is incomplete".into());
                    }
                    report.last_move = Some(row);
                }
                "counters" => {
                    counters_seen = true;
                    for token in tokens {
                        let (key, value) = split(token)?;
                        match key {
                            "migrations" => report.counters.migrations = parse_u32(value)?,
                            "incoming" => report.counters.incoming = parse_u32(value)?,
                            "outgoing" => report.counters.outgoing = parse_u32(value)?,
                            "resumes" => report.counters.resumes = parse_u32(value)?,
                            "duplicates" => report.counters.duplicates = parse_u32(value)?,
                            "stale" => report.counters.stale = parse_u32(value)?,
                            "capability" => report.counters.capability = parse_u32(value)?,
                            "rejects" => report.counters.rejects = parse_u32(value)?,
                            "aborted" => report.counters.aborted = parse_u32(value)?,
                            other => return Err(format!("unknown migrate counter {other:?}")),
                        }
                    }
                }
                "last" => {
                    for token in tokens {
                        let (key, value) = split(token)?;
                        match key {
                            "stage" => report.stage = Some(value.to_string()),
                            "reason" => report.last_reason = Some(value.to_string()),
                            other => return Err(format!("unknown last field {other:?}")),
                        }
                    }
                }
                _ => return Err(format!("unknown migrate line {first:?}")),
            }
        }
        if report.state.is_empty() {
            return Err("no migrate state".into());
        }
        if report.state == "enabled" {
            if report.role.is_none() || report.view_epoch.is_none() || report.digest.is_none() {
                return Err("enabled migrate report lacks the view".into());
            }
            if !counters_seen {
                return Err("enabled migrate report lacks counters".into());
            }
        } else if report.reason.is_none() {
            return Err("disabled migrate report lacks a reason".into());
        }
        report.actor = actor;
        report.raw = text.trim().to_string();
        Ok(report)
    }
}

fn empty_actor() -> ActorRow {
    ActorRow {
        id: String::new(),
        home: String::new(),
        state: String::new(),
        host: String::new(),
        epoch: 0,
        applied: 0,
        last: 0,
        hash: String::new(),
        logical: String::new(),
        queued: None,
        unacked: None,
        handle: None,
    }
}

fn split(token: &str) -> Result<(&str, &str), String> {
    token
        .split_once('=')
        .ok_or_else(|| format!("malformed migrate token {token:?}"))
}

fn parse_u32(text: &str) -> Result<u32, String> {
    text.parse().map_err(|_| format!("{text:?} is not a u32"))
}

fn parse_u64(text: &str) -> Result<u64, String> {
    text.parse().map_err(|_| format!("{text:?} is not a u64"))
}

fn parse_usize(text: &str) -> Result<usize, String> {
    text.parse().map_err(|_| format!("{text:?} is not a count"))
}

fn parse_message(text: &str) -> Result<Option<MessageRow>, String> {
    if text == "-" {
        return Ok(None);
    }
    let (id, payload) = text
        .split_once(':')
        .ok_or_else(|| format!("{text:?} is not a message"))?;
    let payload = payload.to_ascii_uppercase();
    if payload.len() != 64 || !payload.chars().all(|ch| ch.is_ascii_hexdigit()) {
        return Err(format!("{payload:?} is not a 32-byte payload"));
    }
    Ok(Some(MessageRow {
        id: parse_u64(id)?,
        payload,
    }))
}

fn parse_handle(text: &str) -> Result<Option<HandleRow>, String> {
    if text == "-" {
        return Ok(None);
    }
    let parts: Vec<&str> = text.split(':').collect();
    if parts.len() != 5 {
        return Err(format!("{text:?} is not a capability handle"));
    }
    let object = parts[0].to_ascii_uppercase();
    let home = parts[2].to_ascii_uppercase();
    if object.len() != 64 || home.len() != 32 {
        return Err(format!("{text:?} has the wrong identifier width"));
    }
    let rights: u8 = parts[1]
        .parse()
        .map_err(|_| format!("{:?} is not a rights mask", parts[1]))?;
    Ok(Some(HandleRow {
        object,
        rights,
        home,
        home_epoch: parse_u32(parts[3])?,
        object_epoch: parse_u32(parts[4])?,
    }))
}

/// Fetch `/mesh/migrate` from one node.
pub fn fetch(node: &Node, timeout: Duration) -> Result<(u16, MigrateReport), String> {
    let response = homes::fetch(node, "/mesh/migrate", timeout)?;
    let report =
        MigrateReport::parse(&response.body).map_err(|error| format!("{}: {error}", node.name))?;
    Ok((response.status, report))
}

/// Fetch until a parseable report arrives.
pub fn fetch_ready(node: &Node, timeout: Duration) -> Result<(u16, MigrateReport), String> {
    let deadline = Instant::now() + timeout;
    loop {
        let error = match fetch(node, Duration::from_secs(10)) {
            Ok(value) => return Ok(value),
            Err(error) => error,
        };
        if Instant::now() >= deadline {
            return Err(format!("{}: {error}", node.name));
        }
        std::thread::sleep(Duration::from_millis(250));
    }
}

/// Drive one migration step on the root node.
pub fn command(node: &Node, step: u8, timeout: Duration) -> Result<(u16, String), String> {
    let path = format!("/mesh/migrate?step={step}");
    let response = homes::fetch(node, &path, timeout)?;
    Ok((response.status, response.body))
}

/// One node's report in the evidence bundle.
#[derive(Clone, Debug)]
pub struct NodeMigrate {
    /// `n0` or `n1`.
    pub name: String,
    /// HTTP status.
    pub status: Option<u16>,
    /// Parsed report.
    pub report: Option<MigrateReport>,
    /// Fetch or parse error.
    pub error: Option<String>,
}

/// The migration section of the evidence bundle.
#[derive(Clone, Debug)]
pub struct MigrateEvidence {
    /// Whether every expected row and continuation was observed.
    pub ok: bool,
    /// Human-readable result.
    pub message: String,
    /// The fault mode.
    pub fault: String,
    /// Actor identity, upper-case hex.
    pub actor: Option<String>,
    /// The actor home, upper-case hex.
    pub home: Option<String>,
    /// The state hash at the first move boundary.
    pub boundary_hash: Option<String>,
    /// The state hash after the return move.
    pub return_hash: Option<String>,
    /// Per-node final reports.
    pub nodes: Vec<NodeMigrate>,
    /// Driven steps and their command responses.
    pub steps: Vec<String>,
    /// Independent checks that passed or failed.
    pub checks: Vec<String>,
}

fn hex(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        let _ = write!(out, "{byte:02X}");
    }
    out
}

fn decode_hex(text: &str) -> Option<Vec<u8>> {
    if !text.len().is_multiple_of(2) || !text.chars().all(|ch| ch.is_ascii_hexdigit()) {
        return None;
    }
    let mut out = Vec::with_capacity(text.len() / 2);
    for index in 0..text.len() / 2 {
        out.push(u8::from_str_radix(&text[index * 2..index * 2 + 2], 16).ok()?);
    }
    Some(out)
}

/// Derive the deterministic fixture actor identity.
pub fn actor_for(index: u32) -> [u8; 16] {
    let mut hasher = blake3::Hasher::new();
    hasher.update(ACTOR_DOMAIN);
    hasher.update(&index.to_le_bytes());
    let hash = hasher.finalize();
    let mut actor = [0u8; 16];
    actor.copy_from_slice(&hash.as_bytes()[..16]);
    actor
}

/// Derive the deterministic fixture message.
pub fn message_for(index: u32) -> (u64, [u8; 32]) {
    let mut hasher = blake3::Hasher::new();
    hasher.update(MESSAGE_DOMAIN);
    hasher.update(&index.to_le_bytes());
    let hash = hasher.finalize();
    let mut payload = [0u8; 32];
    payload.copy_from_slice(hash.as_bytes());
    (10 + u64::from(index), payload)
}

/// The initial logical digest.
pub fn initial_logical(actor: &[u8; 16]) -> [u8; 32] {
    let mut hasher = blake3::Hasher::new();
    hasher.update(INIT_DOMAIN);
    hasher.update(actor);
    *hasher.finalize().as_bytes()
}

/// One deterministic logical step.
pub fn step_logical(previous: &[u8; 32], id: u64, payload: &[u8; 32]) -> [u8; 32] {
    let mut hasher = blake3::Hasher::new();
    hasher.update(STEP_DOMAIN);
    hasher.update(previous);
    hasher.update(&id.to_le_bytes());
    hasher.update(payload);
    *hasher.finalize().as_bytes()
}

/// The first fixture object.
pub fn fixture_object() -> [u8; 32] {
    let mut hasher = blake3::Hasher::new();
    hasher.update(OBJECT_DOMAIN);
    hasher.update(&0u32.to_le_bytes());
    *hasher.finalize().as_bytes()
}

fn rendezvous(identity: &[u8], node: &[u8; 16]) -> u64 {
    let mut hasher = blake3::Hasher::new();
    hasher.update(HOME_DOMAIN);
    hasher.update(identity);
    hasher.update(node);
    hasher.update(&0u32.to_le_bytes());
    let hash = hasher.finalize();
    let mut bytes = [0u8; 8];
    bytes.copy_from_slice(&hash.as_bytes()[..8]);
    u64::from_le_bytes(bytes)
}

/// The rendezvous home for the fixture actor.
pub fn actor_home(actor: &[u8; 16], members: &[String]) -> Option<String> {
    let mut best_score = 0u64;
    let mut best = None;
    for member in members {
        let node = decode_hex(member)?;
        if node.len() != 16 {
            return None;
        }
        let mut node_bytes = [0u8; 16];
        node_bytes.copy_from_slice(&node);
        let score = rendezvous(actor, &node_bytes);
        if best.is_none() || score > best_score {
            best_score = score;
            best = Some(member.to_ascii_uppercase());
        }
    }
    best
}

/// Recompute the canonical logical state hash from a parsed actor row.
///
/// This mirrors `mesh_migrate::hash_state`: schema, actor, home, applied, last,
/// logical, the queued flag/message, the unacked flag/ack and the handle flag.
/// Placement (`host`/`epoch`) is deliberately excluded.
pub fn expected_hash(row: &ActorRow) -> Option<String> {
    let actor = decode_hex(&row.id)?;
    let home = decode_hex(&row.home)?;
    let logical = decode_hex(&row.logical)?;
    if actor.len() != 16 || home.len() != 16 || logical.len() != 32 {
        return None;
    }
    let mut hasher = blake3::Hasher::new();
    hasher.update(HASH_DOMAIN);
    hasher.update(&SCHEMA.to_le_bytes());
    hasher.update(&actor);
    hasher.update(&home);
    hasher.update(&row.applied.to_le_bytes());
    hasher.update(&row.last.to_le_bytes());
    hasher.update(&logical);
    match &row.queued {
        Some(queued) => {
            let payload = decode_hex(&queued.payload)?;
            if payload.len() != 32 {
                return None;
            }
            hasher.update(&[1]);
            hasher.update(&queued.id.to_le_bytes());
            hasher.update(&payload);
        }
        None => {
            hasher.update(&[0]);
        }
    }
    match &row.unacked {
        Some(unacked) => {
            let state = decode_hex(&unacked.payload)?;
            if state.len() != 32 {
                return None;
            }
            hasher.update(&[1]);
            hasher.update(&unacked.id.to_le_bytes());
            hasher.update(&state);
        }
        None => {
            hasher.update(&[0]);
        }
    }
    match &row.handle {
        Some(handle) => {
            let object = decode_hex(&handle.object)?;
            let home = decode_hex(&handle.home)?;
            if object.len() != 32 || home.len() != 16 {
                return None;
            }
            hasher.update(&[1]);
            hasher.update(&object);
            hasher.update(&[handle.rights]);
            hasher.update(&home);
            hasher.update(&handle.home_epoch.to_le_bytes());
            hasher.update(&handle.object_epoch.to_le_bytes());
        }
        None => {
            hasher.update(&[0]);
        }
    }
    Some(hex(hasher.finalize().as_bytes()))
}

fn verify(checks: &mut Vec<String>, errors: &mut Vec<String>, condition: bool, message: &str) {
    if condition {
        checks.push(message.to_string());
    } else {
        errors.push(message.to_string());
    }
}

fn wait_for(
    root: &Node,
    member: &Node,
    timeout: Duration,
    mut done: impl FnMut(&MigrateReport, &MigrateReport) -> bool,
) -> Option<(MigrateReport, MigrateReport)> {
    let deadline = Instant::now() + timeout;
    loop {
        let root_now = fetch_ready(root, Duration::from_secs(10));
        let member_now = fetch_ready(member, Duration::from_secs(10));
        if let (Ok((_, root_report)), Ok((_, member_report))) = (root_now, member_now) {
            if done(&root_report, &member_report) {
                return Some((root_report, member_report));
            }
        }
        if Instant::now() >= deadline {
            return None;
        }
        std::thread::sleep(Duration::from_millis(250));
    }
}

fn drive_step(
    root: &Node,
    member: &Node,
    timeout: Duration,
    evidence: &mut MigrateEvidence,
    errors: &mut Vec<String>,
    number: u8,
    mut done: impl FnMut(&MigrateReport, &MigrateReport) -> bool,
) -> Option<(MigrateReport, MigrateReport)> {
    std::thread::sleep(COMMAND_SETTLE);
    match command(root, number, Duration::from_secs(10)) {
        Ok((status, body)) => {
            evidence
                .steps
                .push(format!("step {number}: HTTP {status} {}", body.trim()));
            if status != 202 {
                errors.push(format!(
                    "step {number} returned HTTP {status}: {}",
                    body.trim()
                ));
                return None;
            }
        }
        Err(error) => {
            errors.push(format!("step {number}: {error}"));
            return None;
        }
    }
    match wait_for(root, member, timeout, &mut done) {
        Some(value) => Some(value),
        None => {
            errors.push(format!("step {number}: timed out waiting for the reports"));
            None
        }
    }
}

fn actor_ready(report: &MigrateReport, epoch: u32, digest: &str) -> bool {
    report.state == "enabled"
        && report.view_epoch == Some(epoch)
        && report.members == Some(2)
        && report.digest.as_deref() == Some(digest)
}

fn actor_row_equal(a: &ActorRow, b: &ActorRow, checks: &mut Vec<String>, errors: &mut Vec<String>) {
    verify(
        checks,
        errors,
        a.id == b.id,
        "source and destination agree on the actor identity",
    );
    verify(
        checks,
        errors,
        a.home == b.home,
        "source and destination agree on the actor rendezvous home",
    );
    verify(
        checks,
        errors,
        a.hash == b.hash,
        &format!(
            "source and destination carry the same state hash {}",
            a.hash
        ),
    );
    verify(
        checks,
        errors,
        a.applied == b.applied && a.last == b.last,
        "source and destination carry the same message boundary",
    );
    verify(
        checks,
        errors,
        a.queued == b.queued,
        "the queued message moved with the actor",
    );
    verify(
        checks,
        errors,
        a.unacked == b.unacked,
        "the unacknowledged message moved with the actor",
    );
    verify(
        checks,
        errors,
        a.handle == b.handle,
        "the serialised capability handle moved with the actor",
    );
}

/// Drive the full two-way migration and return the evidence.
pub fn run(
    root: &Node,
    member: &Node,
    members: &[String],
    epoch: u32,
    digest: &str,
    timeout: Duration,
) -> MigrateEvidence {
    let mut evidence = MigrateEvidence {
        ok: false,
        message: String::new(),
        fault: Fault::None.as_str().into(),
        actor: None,
        home: None,
        boundary_hash: None,
        return_hash: None,
        nodes: vec![
            NodeMigrate {
                name: root.name.clone(),
                status: None,
                report: None,
                error: None,
            },
            NodeMigrate {
                name: member.name.clone(),
                status: None,
                report: None,
                error: None,
            },
        ],
        steps: Vec::new(),
        checks: Vec::new(),
    };
    let mut errors = Vec::new();
    let actor = actor_for(0);
    let actor_hex = hex(&actor);
    let home = actor_home(&actor, members);
    let Some(home) = home else {
        evidence.message = "the confirmed member set has no rendezvous actor home".into();
        return evidence;
    };
    evidence.actor = Some(actor_hex.clone());
    evidence.home = Some(home.clone());
    let (first_id, first_payload) = message_for(0);
    let (second_id, second_payload) = message_for(1);
    let (third_id, third_payload) = message_for(2);
    let logical1 = step_logical(&initial_logical(&actor), first_id, &first_payload);
    let logical2 = step_logical(&logical1, second_id, &second_payload);
    let logical3 = step_logical(&logical2, third_id, &third_payload);
    let handle_object = hex(&fixture_object());

    // 1. Seed at the root and stop at a message boundary.
    if let Some((root_report, member_report)) = drive_step(
        root,
        member,
        timeout,
        &mut evidence,
        &mut errors,
        1,
        |root_report, _| {
            root_report
                .actor
                .as_ref()
                .is_some_and(|actor| actor.applied == 1 && actor.queued.is_some())
        },
    ) {
        let Some(row) = root_report.actor.clone() else {
            evidence.message = "the seed left no actor row".into();
            return evidence;
        };
        verify(
            &mut evidence.checks,
            &mut errors,
            row.id == actor_hex,
            "the fixture actor identity is deterministic",
        );
        verify(
            &mut evidence.checks,
            &mut errors,
            row.home == home,
            "the actor home is the confirmed rendezvous winner",
        );
        verify(
            &mut evidence.checks,
            &mut errors,
            row.host == root.node_id_string() && row.epoch == 1 && row.state == "active",
            "the actor is active at the root at epoch 1",
        );
        verify(
            &mut evidence.checks,
            &mut errors,
            row.logical == hex(&logical1),
            "the first message produced the independent logical digest",
        );
        verify(
            &mut evidence.checks,
            &mut errors,
            row.unacked
                .as_ref()
                .is_some_and(|ack| ack.id == first_id && ack.payload == hex(&logical1)),
            "message 10 is unacknowledged at the boundary",
        );
        verify(
            &mut evidence.checks,
            &mut errors,
            row.queued.as_ref().is_some_and(|queued| {
                queued.id == second_id && queued.payload == hex(&second_payload)
            }),
            "message 11 is queued at the boundary",
        );
        verify(
            &mut evidence.checks,
            &mut errors,
            row.handle.as_ref().is_some_and(|handle| {
                handle.object == handle_object
                    && handle.rights == 9
                    && handle.home == root.node_id_string()
            }),
            "the capability handle is serialised as authority, not a pointer",
        );
        verify(
            &mut evidence.checks,
            &mut errors,
            expected_hash(&row).as_deref() == Some(row.hash.as_str()),
            "the stated hash is the independent canonical state hash",
        );
        evidence.nodes[0].report = Some(root_report);
        evidence.nodes[1].report = Some(member_report);
    } else {
        evidence.message = errors.join("; ");
        return evidence;
    }

    // 2. Move root -> member.  The state hash must match at both ends.
    if let Some((root_report, member_report)) = drive_step(
        root,
        member,
        timeout,
        &mut evidence,
        &mut errors,
        2,
        |root_report, member_report| {
            root_report
                .actor
                .as_ref()
                .is_some_and(|actor| actor.state == "fenced" && actor.epoch == 2)
                && member_report
                    .actor
                    .as_ref()
                    .is_some_and(|actor| actor.state == "active" && actor.epoch == 2)
        },
    ) {
        let (Some(source), Some(destination)) =
            (root_report.actor.clone(), member_report.actor.clone())
        else {
            evidence.message = "the first move left no actor rows".into();
            return evidence;
        };
        actor_row_equal(&source, &destination, &mut evidence.checks, &mut errors);
        verify(
            &mut evidence.checks,
            &mut errors,
            source.host == member.node_id_string() && destination.host == member.node_id_string(),
            "both rows name the member as the new host",
        );
        verify(
            &mut evidence.checks,
            &mut errors,
            source
                .queued
                .as_ref()
                .is_some_and(|queued| queued.id == second_id)
                && destination
                    .queued
                    .as_ref()
                    .is_some_and(|queued| queued.id == second_id),
            "the queued message crossed the message boundary",
        );
        verify(
            &mut evidence.checks,
            &mut errors,
            expected_hash(&destination).as_deref() == Some(destination.hash.as_str()),
            "the destination's hash is the independent canonical hash",
        );
        evidence.boundary_hash = Some(destination.hash.clone());
        evidence.nodes[0].report = Some(root_report);
        evidence.nodes[1].report = Some(member_report);
    } else {
        evidence.message = errors.join("; ");
        return evidence;
    }

    // 3. Resume the queued message at the destination.
    if let Some((root_report, member_report)) = drive_step(
        root,
        member,
        timeout,
        &mut evidence,
        &mut errors,
        3,
        |_, member_report| {
            member_report.actor.as_ref().is_some_and(|actor| {
                actor.applied == 2 && actor.last == second_id && actor.queued.is_none()
            })
        },
    ) {
        let Some(row) = member_report.actor.clone() else {
            evidence.message = "the destination lost its actor on resume".into();
            return evidence;
        };
        verify(
            &mut evidence.checks,
            &mut errors,
            row.logical == hex(&logical2),
            "the destination resumed the queued message exactly once",
        );
        verify(
            &mut evidence.checks,
            &mut errors,
            row.unacked
                .as_ref()
                .is_some_and(|ack| ack.id == second_id && ack.payload == hex(&logical2)),
            "message 11 is unacknowledged after the boundary continuation",
        );
        verify(
            &mut evidence.checks,
            &mut errors,
            expected_hash(&row).as_deref() == Some(row.hash.as_str()),
            "the resumed state hash is independently reproduced",
        );
        evidence.nodes[0].report = Some(root_report);
        evidence.nodes[1].report = Some(member_report);
    } else {
        evidence.message = errors.join("; ");
        return evidence;
    }

    // 4. A stale delivery at the old root host is refused locally.
    if let Some((root_report, member_report)) = drive_step(
        root,
        member,
        timeout,
        &mut evidence,
        &mut errors,
        4,
        |root_report, _| root_report.counters.stale >= 1,
    ) {
        verify(
            &mut evidence.checks,
            &mut errors,
            root_report.counters.stale == 1,
            "the old root home refused the stale delivery",
        );
        evidence.nodes[0].report = Some(root_report);
        evidence.nodes[1].report = Some(member_report);
    } else {
        evidence.message = errors.join("; ");
        return evidence;
    }

    // 5. A duplicate delivery at the member is idempotent.
    if let Some((root_report, member_report)) = drive_step(
        root,
        member,
        timeout,
        &mut evidence,
        &mut errors,
        5,
        |_, member_report| member_report.counters.duplicates >= 1,
    ) {
        let Some(row) = member_report.actor.clone() else {
            evidence.message = "the member lost its actor on duplicate delivery".into();
            return evidence;
        };
        verify(
            &mut evidence.checks,
            &mut errors,
            row.applied == 2,
            "the duplicate did not advance the actor",
        );
        verify(
            &mut evidence.checks,
            &mut errors,
            row.applied == 2 && row.logical == hex(&logical2),
            "the duplicate left the logical state unchanged",
        );
        verify(
            &mut evidence.checks,
            &mut errors,
            member_report.counters.duplicates == 1,
            "the member counted exactly one idempotent duplicate",
        );
        evidence.nodes[0].report = Some(root_report);
        evidence.nodes[1].report = Some(member_report);
    } else {
        evidence.message = errors.join("; ");
        return evidence;
    }

    // 6. Queue the next message at the member before the return move.
    if let Some((root_report, member_report)) = drive_step(
        root,
        member,
        timeout,
        &mut evidence,
        &mut errors,
        6,
        |_, member_report| {
            member_report.actor.as_ref().is_some_and(|actor| {
                actor
                    .queued
                    .as_ref()
                    .is_some_and(|queued| queued.id == third_id)
            })
        },
    ) {
        verify(
            &mut evidence.checks,
            &mut errors,
            member_report
                .actor
                .as_ref()
                .and_then(|actor| actor.queued.as_ref())
                .is_some_and(|queued| queued.payload == hex(&third_payload)),
            "message 12 is queued at the message boundary",
        );
        evidence.nodes[0].report = Some(root_report);
        evidence.nodes[1].report = Some(member_report);
    } else {
        evidence.message = errors.join("; ");
        return evidence;
    }

    // 7. Move member -> root (the return migration).
    if let Some((root_report, member_report)) = drive_step(
        root,
        member,
        timeout,
        &mut evidence,
        &mut errors,
        7,
        |root_report, member_report| {
            root_report
                .actor
                .as_ref()
                .is_some_and(|actor| actor.state == "active" && actor.epoch == 3)
                && member_report
                    .actor
                    .as_ref()
                    .is_some_and(|actor| actor.state == "fenced" && actor.epoch == 3)
        },
    ) {
        let (Some(destination), Some(source)) =
            (root_report.actor.clone(), member_report.actor.clone())
        else {
            evidence.message = "the return move left no actor rows".into();
            return evidence;
        };
        actor_row_equal(&source, &destination, &mut evidence.checks, &mut errors);
        verify(
            &mut evidence.checks,
            &mut errors,
            destination.host == root.node_id_string() && source.host == root.node_id_string(),
            "both rows name the root as the returned host",
        );
        verify(
            &mut evidence.checks,
            &mut errors,
            destination
                .queued
                .as_ref()
                .is_some_and(|queued| queued.id == third_id),
            "the queued message crossed the return boundary",
        );
        verify(
            &mut evidence.checks,
            &mut errors,
            expected_hash(&destination).as_deref() == Some(destination.hash.as_str()),
            "the returned state hash is independently reproduced",
        );
        evidence.return_hash = Some(destination.hash.clone());
        evidence.nodes[0].report = Some(root_report);
        evidence.nodes[1].report = Some(member_report);
    } else {
        evidence.message = errors.join("; ");
        return evidence;
    }

    // 8. Resume at the returned home.
    if let Some((root_report, member_report)) = drive_step(
        root,
        member,
        timeout,
        &mut evidence,
        &mut errors,
        8,
        |root_report, _| {
            root_report.actor.as_ref().is_some_and(|actor| {
                actor.applied == 3 && actor.last == third_id && actor.queued.is_none()
            })
        },
    ) {
        let Some(row) = root_report.actor.clone() else {
            evidence.message = "the returned actor disappeared on resume".into();
            return evidence;
        };
        verify(
            &mut evidence.checks,
            &mut errors,
            row.logical == hex(&logical3),
            "the returned home resumed the queued message exactly once",
        );
        verify(
            &mut evidence.checks,
            &mut errors,
            row.unacked
                .as_ref()
                .is_some_and(|ack| ack.id == third_id && ack.payload == hex(&logical3)),
            "message 12 is unacknowledged after the return continuation",
        );
        verify(
            &mut evidence.checks,
            &mut errors,
            expected_hash(&row).as_deref() == Some(row.hash.as_str()),
            "the final state hash is independently reproduced",
        );
        evidence.nodes[0].report = Some(root_report);
        evidence.nodes[1].report = Some(member_report);
    } else {
        evidence.message = errors.join("; ");
        return evidence;
    }

    // 9. A stale delivery at the old member host is refused on the wire.
    if let Some((root_report, member_report)) = drive_step(
        root,
        member,
        timeout,
        &mut evidence,
        &mut errors,
        9,
        |root_report, member_report| {
            root_report.counters.stale >= 2 && member_report.counters.stale >= 1
        },
    ) {
        verify(
            &mut evidence.checks,
            &mut errors,
            member_report.counters.stale == 1,
            "the old member home refused the stale wire delivery",
        );
        verify(
            &mut evidence.checks,
            &mut errors,
            root_report.counters.stale == 2,
            "the root recorded the local and wire stale refusals",
        );
        evidence.nodes[0].report = Some(root_report);
        evidence.nodes[1].report = Some(member_report);
    } else {
        evidence.message = errors.join("; ");
        return evidence;
    }

    // 10. A duplicate at the returned home is idempotent.
    if let Some((root_report, member_report)) = drive_step(
        root,
        member,
        timeout,
        &mut evidence,
        &mut errors,
        10,
        |root_report, _| root_report.counters.duplicates >= 1,
    ) {
        let Some(row) = root_report.actor.clone() else {
            evidence.message = "the returned actor disappeared on duplicate delivery".into();
            return evidence;
        };
        verify(
            &mut evidence.checks,
            &mut errors,
            row.applied == 3 && row.logical == hex(&logical3),
            "the returned-home duplicate left the logical state unchanged",
        );
        verify(
            &mut evidence.checks,
            &mut errors,
            root_report.counters.duplicates >= 1,
            "the returned home counted the idempotent duplicate",
        );
        evidence.nodes[0].report = Some(root_report);
        evidence.nodes[1].report = Some(member_report);
    } else {
        evidence.message = errors.join("; ");
        return evidence;
    }

    // 11. A tampered delivery capability is refused.
    if let Some((root_report, member_report)) = drive_step(
        root,
        member,
        timeout,
        &mut evidence,
        &mut errors,
        11,
        |root_report, _| root_report.counters.capability >= 1,
    ) {
        verify(
            &mut evidence.checks,
            &mut errors,
            root_report.counters.capability == 1,
            "the current host refused the tampered capability",
        );
        verify(
            &mut evidence.checks,
            &mut errors,
            root_report
                .actor
                .as_ref()
                .is_some_and(|actor| actor.applied == 3),
            "a refused capability did not advance the actor",
        );
        evidence.nodes[0].report = Some(root_report);
        evidence.nodes[1].report = Some(member_report);
    } else {
        evidence.message = errors.join("; ");
        return evidence;
    }

    let (Some(root_report), Some(member_report)) = (
        evidence.nodes[0].report.clone(),
        evidence.nodes[1].report.clone(),
    ) else {
        evidence.message = "final migration reports are missing".into();
        return evidence;
    };

    verify(
        &mut evidence.checks,
        &mut errors,
        actor_ready(&root_report, epoch, digest),
        "the root migration report carries the join-confirmed view",
    );
    verify(
        &mut evidence.checks,
        &mut errors,
        actor_ready(&member_report, epoch, digest),
        "the member migration report carries the join-confirmed view",
    );
    verify(
        &mut evidence.checks,
        &mut errors,
        root_report.counters.migrations == 2
            && root_report.counters.outgoing == 1
            && root_report.counters.incoming == 1,
        "the root completed one outgoing and one incoming migration",
    );
    verify(
        &mut evidence.checks,
        &mut errors,
        member_report.counters.migrations == 2
            && member_report.counters.incoming == 1
            && member_report.counters.outgoing == 1,
        "the member completed one incoming and one outgoing migration",
    );
    verify(
        &mut evidence.checks,
        &mut errors,
        root_report.counters.aborted == 0 && member_report.counters.aborted == 0,
        "neither node aborted a migration",
    );
    verify(
        &mut evidence.checks,
        &mut errors,
        evidence.boundary_hash.is_some() && evidence.return_hash.is_some(),
        "the first and return move boundaries were both recorded",
    );

    evidence.ok = errors.is_empty();
    evidence.message = if evidence.ok {
        format!(
            "actor {} moved {} -> {} -> {}; boundary hash {} returned as {}",
            actor_hex,
            root.node_id_string(),
            member.node_id_string(),
            root.node_id_string(),
            evidence.boundary_hash.as_deref().unwrap_or("-"),
            evidence.return_hash.as_deref().unwrap_or("-")
        )
    } else {
        errors.join("; ")
    };
    evidence
}

/// Evaluate the disabled modes without driving a step.
pub fn evaluate_disabled(
    nodes: &[Node],
    fault: Fault,
    reports: &[(u16, MigrateReport)],
) -> MigrateEvidence {
    let mut evidence = MigrateEvidence {
        ok: false,
        message: String::new(),
        fault: fault.as_str().into(),
        actor: None,
        home: None,
        boundary_hash: None,
        return_hash: None,
        nodes: Vec::new(),
        steps: Vec::new(),
        checks: Vec::new(),
    };
    let mut errors = Vec::new();
    let expected_reason = match fault {
        Fault::NoPeer => "peer",
        Fault::NoRoute | Fault::WrongAuthority => "config",
        Fault::None | Fault::KillPeer | Fault::Partition | Fault::Udp => {
            unreachable!("the migration mode is evaluated by run")
        }
    };
    for (node, (status, report)) in nodes.iter().zip(reports) {
        evidence.nodes.push(NodeMigrate {
            name: node.name.clone(),
            status: Some(*status),
            report: Some(report.clone()),
            error: None,
        });
        if *status != 404 {
            errors.push(format!("{}: HTTP {status}, expected 404", node.name));
        }
        if report.state != "disabled" {
            errors.push(format!(
                "{}: state is {:?}, expected disabled",
                node.name, report.state
            ));
        }
        if report.reason.as_deref() != Some(expected_reason) {
            errors.push(format!(
                "{}: reason is {:?}, expected {expected_reason}",
                node.name, report.reason
            ));
        }
    }
    evidence.ok = errors.is_empty();
    evidence.message = if evidence.ok {
        format!("migration disabled with reason={expected_reason}; earlier M2 markers preserved")
    } else {
        errors.join("; ")
    };
    evidence
}

/// Collect the migration reports for a non-migration mode without driving steps.
pub fn fetch_disabled(
    nodes: &[Node],
    fault: Fault,
    timeout: Duration,
) -> Result<MigrateEvidence, String> {
    let mut reports = Vec::new();
    for node in nodes {
        reports.push(fetch_ready(node, timeout)?);
    }
    Ok(evaluate_disabled(nodes, fault, &reports))
}

#[cfg(test)]
mod tests {
    use super::*;

    const MEMBERS: [&str; 2] = [
        "08006969B60100000000000000000000",
        "08006995F50000000000000000000000",
    ];

    #[test]
    fn parses_an_enabled_report_with_a_boundary() {
        let body = "mesh-migrate: state=enabled role=root view_epoch=2 members=2 digest=2EDA13C5AC6F726132E0479DFE6045B7\n\
                    mesh-migrate: actor id=759FD61E30F4DFA28311542283B188BF home=08006969B60100000000000000000000 state=active host=08006969B60100000000000000000000 epoch=1 applied=1 last=10\n\
                    mesh-migrate: actor hash=4A4F0AA23C46957E27B0127ADF94EA39712A0C1ACB7E4DF02637EAA70A958173 logical=C2982E2808BFE5127A05987122465101A494B442DFACDD9BE9AB9CC7CF9D51CE queued=11:6B6C3D353AC3F0CB422D919958191787903243CA9FE827D7F802F517EAAE96CB unacked=10:C2982E2808BFE5127A05987122465101A494B442DFACDD9BE9AB9CC7CF9D51CE handle=BEB6D1C7D3D6204050933044057A4B31417ACE296D6666B5EAB98F42A538D2A6:9:08006969B60100000000000000000000:1:1\n\
                    mesh-migrate: counters migrations=0 incoming=0 outgoing=0 resumes=0 duplicates=0 stale=0 capability=0 rejects=0 aborted=0\n\
                    mesh-migrate: last stage=seeded\n";
        let report = MigrateReport::parse(body).expect("report");
        assert_eq!(report.state, "enabled");
        assert_eq!(report.role.as_deref(), Some("root"));
        assert_eq!(report.view_epoch, Some(2));
        let actor = report.actor.expect("actor");
        assert_eq!(actor.applied, 1);
        assert_eq!(actor.queued.as_ref().map(|row| row.id), Some(11));
        assert_eq!(actor.unacked.as_ref().map(|row| row.id), Some(10));
        assert_eq!(actor.handle.as_ref().map(|row| row.rights), Some(9));
        assert_eq!(
            expected_hash(&actor).as_deref(),
            Some("F18347DA24CAFF0DA270D3BDC87F34425B740D75576FB522D663C1D2AE51ADC2")
        );
    }

    #[test]
    fn parses_a_disabled_report() {
        let report =
            MigrateReport::parse("mesh-migrate: state=disabled reason=config\n").expect("report");
        assert_eq!(report.state, "disabled");
        assert_eq!(report.reason.as_deref(), Some("config"));
    }

    #[test]
    fn actor_identity_and_home_are_deterministic() {
        let actor = actor_for(0);
        assert_eq!(hex(&actor), "759FD61E30F4DFA28311542283B188BF");
        let members: Vec<String> = MEMBERS.iter().map(|value| value.to_string()).collect();
        let home = actor_home(&actor, &members).expect("home");
        assert!(home == MEMBERS[0] || home == MEMBERS[1]);
        assert_eq!(home, actor_home(&actor, &members).expect("home"));
    }

    #[test]
    fn message_chain_is_pinned() {
        let actor = actor_for(0);
        let (id, payload) = message_for(0);
        assert_eq!(id, 10);
        let first = step_logical(&initial_logical(&actor), id, &payload);
        let (id, payload) = message_for(1);
        assert_eq!(id, 11);
        let second = step_logical(&first, id, &payload);
        assert_ne!(first, second);
    }
}
