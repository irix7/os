// SPDX-License-Identifier: GPL-3.0-only
//! M2-4 remote capability-route evidence.
//!
//! The MCP module exposes `GET /mesh/route` as a bounded in-memory report and
//! `GET /mesh/route?step=N` as the rig-driven lab control surface.  The rig
//! chooses the order; the route policy chooses the objects from the confirmed
//! rendezvous view.
//!
//! ```text
//! step 1  A -> B remote stat on the first peer-homed object
//! step 2  move that object from B to A
//! step 3  repeat the stat; B answers MOVED and A retries locally
//! step 4  move the first A-homed object to B
//! step 5  repeat the stat; A answers MOVED locally and retries at B
//! ```
//!
//! This module parses the reports, drives the steps, recomputes the fixture
//! object IDs, homes and values independently of `mesh-route`, and records
//! the exact rows in the evidence bundle.

use std::fmt::Write as _;
use std::time::{Duration, Instant};

use crate::homes;
use crate::rig::{Fault, Node};

const PREFIX: &str = "mesh-route: ";
/// The MCP HTTP server keeps its single slot in the close handshake for up to
/// 5 s.  A command is not idempotent, so the rig lets the slot re-arm rather
/// than replaying a request whose response was lost.
const COMMAND_SETTLE: Duration = Duration::from_secs(6);
const OBJECT_DOMAIN: &[u8] = b"IRIX7-M2-HOME-OBJECT-v1";
const OBJECT_MAX: u32 = 32;
const HOME_DOMAIN: &[u8] = b"IRIX7-HOME-v1";

/// One `result` row.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RouteResult {
    /// Object ID, upper-case hex.
    pub object: String,
    /// Home NodeId, upper-case hex.
    pub home: String,
    /// Home assignment epoch.
    pub home_epoch: u32,
    /// Operation: `stat` or `commit`.
    pub operation: String,
    /// Deterministic value, upper-case hex.
    pub value: String,
    /// Attempts used.
    pub attempts: u8,
    /// Whether a `MOVED` was consumed.
    pub moved: bool,
}

/// One fenced stale route or lease row from `/mesh/route`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FenceRow {
    /// Object ID, upper-case hex.
    pub object: String,
    /// The removed home, upper-case hex.
    pub stale_home: String,
    /// The stale home assignment epoch.
    pub stale_home_epoch: u32,
    /// The recomputed home, upper-case hex.
    pub home: String,
    /// The current membership epoch.
    pub home_epoch: u32,
    /// `left-view` or `lease-left-view`.
    pub reason: String,
}

/// One parsed `/mesh/route` body.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RouteReport {
    /// `enabled` or `disabled`.
    pub state: String,
    /// Disabled reason.
    pub reason: Option<String>,
    /// Confirmed view epoch.
    pub view_epoch: Option<u32>,
    /// Confirmed member count.
    pub members: Option<usize>,
    /// Confirmed membership digest, upper-case hex.
    pub digest: Option<String>,
    /// Object moved away from this node.
    pub move_old: Option<String>,
    /// Object accepted as the new home.
    pub move_new: Option<String>,
    /// `MOVED` answers sent for a remote old home.
    pub moved_remote: usize,
    /// `MOVED` answers produced by a local old home.
    pub moved_local: usize,
    /// Retries after `MOVED`.
    pub retries: usize,
    /// Home completions.
    pub completes: usize,
    /// Terminal refusals.
    pub rejects: usize,
    /// Last refusal reason.
    pub last_reject: Option<String>,
    /// Deadlines that expired.
    pub timeouts: usize,
    /// Authoritative-view state: `current` or `lost`.
    pub authority: String,
    /// The epoch named by the authority line.
    pub authority_epoch: Option<u32>,
    /// Whether a mutable commit is available.
    pub mutable: bool,
    /// Remaining authority write lease in milliseconds.
    pub write_lease_ms: Option<u32>,
    /// Fenced routes and leases.
    pub fences: usize,
    /// Fence rows.
    pub fence_rows: Vec<FenceRow>,
    /// Commits completed or served.
    pub commits: usize,
    /// Replayed stale frames.
    pub replays: usize,
    /// Last replay refusal.
    pub last_replay: Option<String>,
    /// Declared result count.
    pub result_count: usize,
    /// Result rows.
    pub results: Vec<RouteResult>,
    /// The exact body, for the evidence bundle.
    pub raw: String,
}

impl RouteReport {
    /// Parse a `/mesh/route` body.
    pub fn parse(text: &str) -> Result<Self, String> {
        let mut report = Self::default();
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
                            "view_epoch" => report.view_epoch = Some(parse_u32(value)?),
                            "members" => report.members = Some(parse_usize(value)?),
                            "digest" => report.digest = Some(value.to_ascii_uppercase()),
                            other => return Err(format!("unknown route field {other:?}")),
                        }
                    }
                }
                "state=disabled" => {
                    report.state = "disabled".into();
                    for token in tokens {
                        let (key, value) = split(token)?;
                        match key {
                            "reason" => report.reason = Some(value.to_string()),
                            other => return Err(format!("unknown route field {other:?}")),
                        }
                    }
                }
                "move" => {
                    for token in tokens {
                        let (key, value) = split(token)?;
                        match key {
                            "old" => report.move_old = parse_object(value)?,
                            "new" => report.move_new = parse_object(value)?,
                            other => return Err(format!("unknown route move field {other:?}")),
                        }
                    }
                }
                "result" => {
                    let mut row = RouteResult {
                        object: String::new(),
                        home: String::new(),
                        home_epoch: 0,
                        operation: String::new(),
                        value: String::new(),
                        attempts: 0,
                        moved: false,
                    };
                    let mut has_moved = false;
                    for token in tokens {
                        let (key, value) = split(token)?;
                        match key {
                            "object" => row.object = value.to_ascii_uppercase(),
                            "home" => row.home = value.to_ascii_uppercase(),
                            "home_epoch" => row.home_epoch = parse_u32(value)?,
                            "op" => row.operation = value.to_string(),
                            "value" => row.value = value.to_ascii_uppercase(),
                            "attempts" => row.attempts = parse_u8(value)?,
                            "moved" => {
                                row.moved = value == "1";
                                has_moved = true;
                            }
                            other => return Err(format!("unknown route result field {other:?}")),
                        }
                    }
                    if row.object.is_empty()
                        || row.home.is_empty()
                        || row.operation.is_empty()
                        || row.value.is_empty()
                        || !has_moved
                    {
                        return Err("route result row is incomplete".into());
                    }
                    report.results.push(row);
                }
                "authority" => {
                    for token in tokens {
                        let (key, value) = split(token)?;
                        match key {
                            "state" => report.authority = value.to_string(),
                            "epoch" => report.authority_epoch = Some(parse_u32(value)?),
                            "mutable" => report.mutable = value == "1",
                            "write_lease_ms" => report.write_lease_ms = Some(parse_u32(value)?),
                            other => return Err(format!("unknown authority field {other:?}")),
                        }
                    }
                    if report.authority.is_empty() {
                        return Err("authority row has no state".into());
                    }
                }
                "fence" => {
                    let mut row = FenceRow {
                        object: String::new(),
                        stale_home: String::new(),
                        stale_home_epoch: 0,
                        home: String::new(),
                        home_epoch: 0,
                        reason: String::new(),
                    };
                    for token in tokens {
                        let (key, value) = split(token)?;
                        match key {
                            "object" => row.object = value.to_ascii_uppercase(),
                            "stale_home" => row.stale_home = value.to_ascii_uppercase(),
                            "stale_home_epoch" => row.stale_home_epoch = parse_u32(value)?,
                            "home" => row.home = value.to_ascii_uppercase(),
                            "home_epoch" => row.home_epoch = parse_u32(value)?,
                            "reason" => row.reason = value.to_string(),
                            other => return Err(format!("unknown fence field {other:?}")),
                        }
                    }
                    if row.object.is_empty()
                        || row.stale_home.is_empty()
                        || row.home.is_empty()
                        || row.reason.is_empty()
                    {
                        return Err("fence row is incomplete".into());
                    }
                    report.fence_rows.push(row);
                }
                "counters" => {
                    for token in tokens {
                        let (key, value) = split(token)?;
                        match key {
                            "completes" => report.completes = parse_usize(value)?,
                            "retries" => report.retries = parse_usize(value)?,
                            "rejects" => report.rejects = parse_usize(value)?,
                            "timeouts" => report.timeouts = parse_usize(value)?,
                            "results" => report.result_count = parse_usize(value)?,
                            "moved_local" => report.moved_local = parse_usize(value)?,
                            "moved_remote" => report.moved_remote = parse_usize(value)?,
                            "fences" => report.fences = parse_usize(value)?,
                            "commits" => report.commits = parse_usize(value)?,
                            "replays" => report.replays = parse_usize(value)?,
                            "last_replay" => report.last_replay = Some(value.to_string()),
                            "last_reject" => report.last_reject = Some(value.to_string()),
                            other => return Err(format!("unknown route counter {other:?}")),
                        }
                    }
                }
                _ => return Err(format!("unknown route line {first:?}")),
            }
        }
        if report.state.is_empty() {
            return Err("no route state".into());
        }
        if report.state == "enabled" {
            if report.view_epoch.is_none() || report.members.is_none() || report.digest.is_none() {
                return Err("enabled route report lacks the view".into());
            }
        } else if report.reason.is_none() {
            return Err("disabled route report lacks a reason".into());
        }
        if report.result_count != report.results.len() {
            return Err(format!(
                "route result count {} does not match {} rows",
                report.result_count,
                report.results.len()
            ));
        }
        report.raw = text.trim().to_string();
        Ok(report)
    }
}

fn split(token: &str) -> Result<(&str, &str), String> {
    token
        .split_once('=')
        .ok_or_else(|| format!("malformed route token {token:?}"))
}

fn parse_u32(text: &str) -> Result<u32, String> {
    text.parse().map_err(|_| format!("{text:?} is not a u32"))
}

fn parse_u8(text: &str) -> Result<u8, String> {
    text.parse().map_err(|_| format!("{text:?} is not a u8"))
}

fn parse_usize(text: &str) -> Result<usize, String> {
    text.parse().map_err(|_| format!("{text:?} is not a count"))
}

fn parse_object(text: &str) -> Result<Option<String>, String> {
    if text == "-" {
        return Ok(None);
    }
    if text.len() != 64 {
        return Err(format!("{text:?} is not a 32-byte object ID"));
    }
    Ok(Some(text.to_ascii_uppercase()))
}

/// Fetch `/mesh/route` from one node.
pub fn fetch(node: &Node, timeout: Duration) -> Result<(u16, RouteReport), String> {
    let response = homes::fetch(node, "/mesh/route", timeout)?;
    let report =
        RouteReport::parse(&response.body).map_err(|error| format!("{}: {error}", node.name))?;
    Ok((response.status, report))
}

/// Fetch `/mesh/route` until a parseable body arrives.
pub fn fetch_ready(node: &Node, timeout: Duration) -> Result<(u16, RouteReport), String> {
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

/// Drive one route step on the root node.
pub fn command(node: &Node, step: u8, timeout: Duration) -> Result<(u16, String), String> {
    let path = format!("/mesh/route?step={step}");
    let response = homes::fetch(node, &path, timeout)?;
    Ok((response.status, response.body))
}

/// One node's report in the evidence bundle.
#[derive(Clone, Debug)]
pub struct NodeRoute {
    /// `n0` or `n1`.
    pub name: String,
    /// HTTP status.
    pub status: Option<u16>,
    /// Parsed report.
    pub report: Option<RouteReport>,
    /// Fetch or parse error.
    pub error: Option<String>,
}

/// The route section of the evidence bundle.
#[derive(Clone, Debug)]
pub struct RouteEvidence {
    /// Whether every expected row was observed.
    pub ok: bool,
    /// Human-readable result.
    pub message: String,
    /// The fault mode.
    pub fault: String,
    /// The object moved from the peer to this node (and used for the wire
    /// MOVED).
    pub wire: Option<String>,
    /// The object moved from this node to the peer (and used for the local
    /// MOVED).
    pub local: Option<String>,
    /// Per-node final reports.
    pub nodes: Vec<NodeRoute>,
    /// The driven steps and their command responses.
    pub steps: Vec<String>,
    /// Independent checks that passed or failed.
    pub checks: Vec<String>,
}

/// Derive one fixture object independently of `mesh-core`.
pub fn fixture_object(index: u32) -> [u8; 32] {
    let mut hasher = blake3::Hasher::new();
    hasher.update(OBJECT_DOMAIN);
    hasher.update(&index.to_le_bytes());
    *hasher.finalize().as_bytes()
}

fn fixture_objects(count: u32) -> Vec<[u8; 32]> {
    (0..count.min(OBJECT_MAX)).map(fixture_object).collect()
}

fn rendezvous(object: &[u8; 32], node: &[u8; 16]) -> u64 {
    let mut hasher = blake3::Hasher::new();
    hasher.update(HOME_DOMAIN);
    hasher.update(object);
    hasher.update(node);
    hasher.update(&0u32.to_le_bytes());
    let hash = hasher.finalize();
    u64::from_le_bytes([
        hash.as_bytes()[0],
        hash.as_bytes()[1],
        hash.as_bytes()[2],
        hash.as_bytes()[3],
        hash.as_bytes()[4],
        hash.as_bytes()[5],
        hash.as_bytes()[6],
        hash.as_bytes()[7],
    ])
}

fn expected_home(object: &[u8; 32], members: &[String]) -> Option<String> {
    let mut best_score = 0u64;
    let mut best = None;
    for member in members {
        let node = decode_hex_16(member)?;
        let score = rendezvous(object, &node);
        if best.is_none() || score > best_score {
            best_score = score;
            best = Some(member.to_ascii_uppercase());
        }
    }
    best
}

/// The first fixture object whose rendezvous home is `home`.
pub fn first_object_for(members: &[String], home: &str) -> Option<String> {
    fixture_objects(homes::OBJECT_COUNT)
        .into_iter()
        .find(|object| expected_home(object, members).as_deref() == Some(home))
        .map(|object| hex(&object))
}

/// The deterministic fixture value a home returns.
pub fn commit_value(object: &str, home_epoch: u32, object_epoch: u32) -> Option<String> {
    fixture_value(
        b"IRIX7-M2-ROUTE-COMMIT-v1",
        object,
        home_epoch,
        object_epoch,
    )
}

pub fn route_value(object: &str, home_epoch: u32, object_epoch: u32) -> Option<String> {
    fixture_value(b"IRIX7-M2-ROUTE-VALUE-v1", object, home_epoch, object_epoch)
}

fn fixture_value(
    domain: &[u8],
    object: &str,
    home_epoch: u32,
    object_epoch: u32,
) -> Option<String> {
    let object = decode_hex_32(object)?;
    let mut hasher = blake3::Hasher::new();
    hasher.update(domain);
    hasher.update(&object);
    hasher.update(&home_epoch.to_le_bytes());
    hasher.update(&object_epoch.to_le_bytes());
    Some(hex(hasher.finalize().as_bytes()))
}

fn decode_hex_16(text: &str) -> Option<[u8; 16]> {
    if text.len() != 32 {
        return None;
    }
    let mut out = [0u8; 16];
    for (index, byte) in out.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&text[index * 2..index * 2 + 2], 16).ok()?;
    }
    Some(out)
}

fn decode_hex_32(text: &str) -> Option<[u8; 32]> {
    if text.len() != 64 {
        return None;
    }
    let mut out = [0u8; 32];
    for (index, byte) in out.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&text[index * 2..index * 2 + 2], 16).ok()?;
    }
    Some(out)
}

fn hex(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        let _ = write!(out, "{byte:02X}");
    }
    out
}

fn verify(checks: &mut Vec<String>, errors: &mut Vec<String>, condition: bool, message: &str) {
    if condition {
        checks.push(message.to_string());
    } else {
        errors.push(message.to_string());
    }
}

fn view_ready(report: &RouteReport, epoch: u32, digest: &str) -> bool {
    report.state == "enabled"
        && report.view_epoch == Some(epoch)
        && report.members == Some(2)
        && report.digest.as_deref() == Some(digest)
}

fn drive_step(
    root: &Node,
    member: &Node,
    timeout: Duration,
    evidence: &mut RouteEvidence,
    errors: &mut Vec<String>,
    number: u8,
    done: &mut dyn FnMut(&RouteReport, &RouteReport) -> bool,
) -> Option<(RouteReport, RouteReport)> {
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
    let deadline = Instant::now() + timeout;
    loop {
        let root_now = fetch_ready(root, Duration::from_secs(10));
        let member_now = fetch_ready(member, Duration::from_secs(10));
        match (root_now, member_now) {
            (Ok((_, root_report)), Ok((_, member_report))) => {
                if done(&root_report, &member_report) {
                    return Some((root_report, member_report));
                }
            }
            (Err(error), _) | (_, Err(error)) => {
                errors.push(format!("step {number}: {error}"));
            }
        }
        if Instant::now() >= deadline {
            errors.push(format!("step {number}: timed out waiting for the reports"));
            return None;
        }
        std::thread::sleep(Duration::from_millis(250));
    }
}

/// Drive the full route sequence and collect the final reports.
///
/// This is only called for the default fault mode.  The steps are issued one
/// at a time and each one is confirmed in the reports before the next is
/// issued, so a pending operation can never be mistaken for a completed one.
pub fn run(
    root: &Node,
    member: &Node,
    members: &[String],
    epoch: u32,
    digest: &str,
    timeout: Duration,
) -> RouteEvidence {
    let mut evidence = RouteEvidence {
        ok: false,
        message: String::new(),
        fault: Fault::None.as_str().into(),
        wire: None,
        local: None,
        nodes: vec![
            NodeRoute {
                name: root.name.clone(),
                status: None,
                report: None,
                error: None,
            },
            NodeRoute {
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

    let wire = first_object_for(members, &member.node_id_string());
    let local = first_object_for(members, &root.node_id_string());
    evidence.wire = wire.clone();
    evidence.local = local.clone();
    let (Some(wire), Some(local)) = (wire, local) else {
        evidence.message = "the fixture set has no object for one of the two homes".into();
        evidence.nodes[0].error = Some("missing wire object".into());
        evidence.nodes[1].error = Some("missing local object".into());
        return evidence;
    };

    let view_deadline = Instant::now() + timeout;
    let (root_status, root_report, member_status, member_report) = loop {
        let root_now = fetch_ready(root, Duration::from_secs(10));
        let member_now = fetch_ready(member, Duration::from_secs(10));
        match (root_now, member_now) {
            (Ok((root_status, root_report)), Ok((member_status, member_report)))
                if view_ready(&root_report, epoch, digest)
                    && view_ready(&member_report, epoch, digest) =>
            {
                break (root_status, root_report, member_status, member_report);
            }
            (Err(error), _) | (_, Err(error)) if Instant::now() >= view_deadline => {
                evidence.message = error;
                return evidence;
            }
            _ => {}
        }
        if Instant::now() >= view_deadline {
            evidence.message = "the route report did not reach the confirmed view".into();
            return evidence;
        }
        std::thread::sleep(Duration::from_millis(250));
    };
    evidence.nodes[0].status = Some(root_status);
    evidence.nodes[0].report = Some(root_report.clone());
    evidence.nodes[1].status = Some(member_status);
    evidence.nodes[1].report = Some(member_report);

    // 1. Remote stat at B.
    if let Some((root_report, member_report)) = drive_step(
        root,
        member,
        timeout,
        &mut evidence,
        &mut errors,
        1,
        &mut |root_report, member_report| {
            root_report
                .results
                .iter()
                .any(|row| row.object == wire && row.attempts == 1 && !row.moved)
                && member_report.completes >= 1
        },
    ) {
        evidence
            .checks
            .push("step 1: A -> B remote stat completed".into());
        evidence.nodes[0].report = Some(root_report);
        evidence.nodes[1].report = Some(member_report);
    } else {
        evidence.message = errors.join("; ");
        return evidence;
    }

    // 2. Move the remote object from B to A.
    if let Some((root_report, member_report)) = drive_step(
        root,
        member,
        timeout,
        &mut evidence,
        &mut errors,
        2,
        &mut |root_report, member_report| {
            root_report.move_new.as_deref() == Some(wire.as_str())
                && member_report.move_old.as_deref() == Some(wire.as_str())
        },
    ) {
        evidence
            .checks
            .push("step 2: B -> A home lease moved on both nodes".into());
        evidence.nodes[0].report = Some(root_report);
        evidence.nodes[1].report = Some(member_report);
    } else {
        evidence.message = errors.join("; ");
        return evidence;
    }

    // 3. The stale route answers MOVED and the sender retries once locally.
    if let Some((root_report, member_report)) = drive_step(
        root,
        member,
        timeout,
        &mut evidence,
        &mut errors,
        3,
        &mut |root_report, member_report| {
            root_report.results.iter().any(|row| {
                row.object == wire
                    && row.attempts == 2
                    && row.moved
                    && row.home == root.node_id_string()
            }) && member_report.moved_remote >= 1
        },
    ) {
        evidence
            .checks
            .push("step 3: remote MOVED consumed with one local retry".into());
        evidence.nodes[0].report = Some(root_report);
        evidence.nodes[1].report = Some(member_report);
    } else {
        evidence.message = errors.join("; ");
        return evidence;
    }

    // 4. Move the local object from A to B.
    if let Some((root_report, member_report)) = drive_step(
        root,
        member,
        timeout,
        &mut evidence,
        &mut errors,
        4,
        &mut |root_report, member_report| {
            root_report.move_old.as_deref() == Some(local.as_str())
                && member_report.move_new.as_deref() == Some(local.as_str())
        },
    ) {
        evidence
            .checks
            .push("step 4: A -> B home lease moved on both nodes".into());
        evidence.nodes[0].report = Some(root_report);
        evidence.nodes[1].report = Some(member_report);
    } else {
        evidence.message = errors.join("; ");
        return evidence;
    }

    // 5. The local old home answers MOVED and the sender retries at B.
    if let Some((root_report, member_report)) = drive_step(
        root,
        member,
        timeout,
        &mut evidence,
        &mut errors,
        5,
        &mut |root_report, member_report| {
            root_report.results.iter().any(|row| {
                row.object == local
                    && row.attempts == 2
                    && row.moved
                    && row.home == member.node_id_string()
            }) && member_report.completes >= 2
        },
    ) {
        evidence
            .checks
            .push("step 5: local MOVED consumed with one remote retry".into());
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
        evidence.message = "final route reports are missing".into();
        return evidence;
    };

    verify(
        &mut evidence.checks,
        &mut errors,
        root_report.state == "enabled",
        "root route state is enabled",
    );
    verify(
        &mut evidence.checks,
        &mut errors,
        member_report.state == "enabled",
        "member route state is enabled",
    );
    verify(
        &mut evidence.checks,
        &mut errors,
        root_report.view_epoch == Some(epoch) && member_report.view_epoch == Some(epoch),
        "both nodes report the join-confirmed view epoch",
    );
    verify(
        &mut evidence.checks,
        &mut errors,
        root_report.members == Some(2) && member_report.members == Some(2),
        "both nodes report two members",
    );
    verify(
        &mut evidence.checks,
        &mut errors,
        root_report.digest.as_deref() == Some(digest)
            && member_report.digest.as_deref() == Some(digest),
        "both nodes report the join-confirmed membership digest",
    );
    verify(
        &mut evidence.checks,
        &mut errors,
        root_report.move_new.as_deref() == Some(wire.as_str()),
        "root accepted the wire object as the new home",
    );
    verify(
        &mut evidence.checks,
        &mut errors,
        root_report.move_old.as_deref() == Some(local.as_str()),
        "root moved the local object away",
    );
    verify(
        &mut evidence.checks,
        &mut errors,
        member_report.move_old.as_deref() == Some(wire.as_str()),
        "member moved the wire object away",
    );
    verify(
        &mut evidence.checks,
        &mut errors,
        member_report.move_new.as_deref() == Some(local.as_str()),
        "member accepted the local object as the new home",
    );
    verify(
        &mut evidence.checks,
        &mut errors,
        root_report.moved_remote == 1,
        "root saw one remote MOVED",
    );
    verify(
        &mut evidence.checks,
        &mut errors,
        root_report.moved_local == 1,
        "root produced one local MOVED",
    );
    verify(
        &mut evidence.checks,
        &mut errors,
        root_report.retries == 2,
        "root retried exactly twice",
    );
    verify(
        &mut evidence.checks,
        &mut errors,
        root_report.completes == 1,
        "root served one home completion",
    );
    verify(
        &mut evidence.checks,
        &mut errors,
        member_report.moved_remote == 1,
        "member produced one MOVED",
    );
    verify(
        &mut evidence.checks,
        &mut errors,
        member_report.completes == 2,
        "member served two home completions",
    );
    verify(
        &mut evidence.checks,
        &mut errors,
        root_report.rejects == 0,
        "root refused no route frame",
    );
    verify(
        &mut evidence.checks,
        &mut errors,
        member_report.rejects == 0,
        "member refused no route frame",
    );
    verify(
        &mut evidence.checks,
        &mut errors,
        root_report.timeouts == 0,
        "root timed out no route operation",
    );
    verify(
        &mut evidence.checks,
        &mut errors,
        member_report.timeouts == 0,
        "member timed out no route operation",
    );
    verify(
        &mut evidence.checks,
        &mut errors,
        root_report.results.len() == 3,
        "root recorded three results",
    );
    verify(
        &mut evidence.checks,
        &mut errors,
        member_report.results.is_empty(),
        "member initiated no operation",
    );

    let expected = [
        (wire.as_str(), member.node_id_string(), 2u32, 1u8, false),
        (wire.as_str(), root.node_id_string(), 3u32, 2u8, true),
        (local.as_str(), member.node_id_string(), 3u32, 2u8, true),
    ];
    for (index, (object, home, home_epoch, attempts, moved)) in expected.iter().enumerate() {
        let Some(row) = root_report.results.get(index) else {
            errors.push(format!("root result {index} is missing"));
            continue;
        };
        let want_value = route_value(object, *home_epoch, 1).unwrap_or_default();
        verify(
            &mut evidence.checks,
            &mut errors,
            row.object == *object,
            &format!("result {index} names object {object}"),
        );
        verify(
            &mut evidence.checks,
            &mut errors,
            row.home == *home,
            &format!("result {index} names home {home}"),
        );
        verify(
            &mut evidence.checks,
            &mut errors,
            row.home_epoch == *home_epoch,
            &format!("result {index} uses home_epoch {home_epoch}"),
        );
        verify(
            &mut evidence.checks,
            &mut errors,
            row.attempts == *attempts,
            &format!("result {index} used {attempts} attempts"),
        );
        verify(
            &mut evidence.checks,
            &mut errors,
            row.moved == *moved,
            &format!("result {index} moved={moved}"),
        );
        verify(
            &mut evidence.checks,
            &mut errors,
            row.value == want_value,
            &format!("result {index} value matches the independent fixture"),
        );
    }

    evidence.ok = errors.is_empty();
    evidence.message = if evidence.ok {
        format!(
            "wire object {} moved B->A and retried locally; local object {} moved A->B and retried at B",
            wire, local
        )
    } else {
        errors.join("; ")
    };
    evidence
}

/// Evaluate the disabled modes without driving any route step.
pub fn evaluate_disabled(
    nodes: &[Node],
    fault: Fault,
    reports: &[(u16, RouteReport)],
) -> RouteEvidence {
    let mut evidence = RouteEvidence {
        ok: false,
        message: String::new(),
        fault: fault.as_str().into(),
        wire: None,
        local: None,
        nodes: Vec::new(),
        steps: Vec::new(),
        checks: Vec::new(),
    };
    let mut errors = Vec::new();
    let expected_reason = match fault {
        Fault::NoPeer => "peer",
        Fault::NoRoute | Fault::WrongAuthority => "config",
        Fault::None | Fault::KillPeer | Fault::Partition | Fault::Udp => {
            unreachable!("the route mode is evaluated by run")
        }
    };
    for (node, (status, report)) in nodes.iter().zip(reports) {
        evidence.nodes.push(NodeRoute {
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
        format!("route disabled with reason={expected_reason}; M1/M2-1/M2-2/M2-3 markers preserved")
    } else {
        errors.join("; ")
    };
    evidence
}

/// Collect the route reports for a non-route mode without driving steps.
pub fn fetch_disabled(
    nodes: &[Node],
    fault: Fault,
    timeout: Duration,
) -> Result<RouteEvidence, String> {
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
    fn parses_an_enabled_report_with_a_result() {
        let body = "mesh-route: state=enabled view_epoch=2 members=2 digest=2EDA13C5AC6F726132E0479DFE6045B7\n\
                    mesh-route: authority state=current epoch=2 mutable=1 write_lease_ms=4294967295\n\
                    mesh-route: fence object=BEB6D1C7D3D6204050933044057A4B31417ACE296D6666B5EAB98F42A538D2A6 stale_home=08006995F50000000000000000000000 stale_home_epoch=2 home=08006969B60100000000000000000000 home_epoch=3 reason=left-view\n\
                    mesh-route: move old=- new=BEB6D1C7D3D6204050933044057A4B31417ACE296D6666B5EAB98F42A538D2A6\n\
                    mesh-route: result object=BEB6D1C7D3D6204050933044057A4B31417ACE296D6666B5EAB98F42A538D2A6 home=08006995F50000000000000000000000 home_epoch=3 op=stat value=692A42AA61F7193B1397E73A00A73336EC90792C6CED0B2BECF83C4231A3D258 attempts=2 moved=1\n\
                    mesh-route: counters completes=1 retries=2 rejects=0 timeouts=0 results=1 moved_local=1 moved_remote=1 fences=1 commits=0 replays=0\n";
        let report = RouteReport::parse(body).expect("report");
        assert_eq!(report.state, "enabled");
        assert_eq!(report.view_epoch, Some(2));
        assert_eq!(report.members, Some(2));
        assert_eq!(
            report.move_new.as_deref(),
            Some("BEB6D1C7D3D6204050933044057A4B31417ACE296D6666B5EAB98F42A538D2A6")
        );
        assert_eq!(report.results.len(), 1);
        assert_eq!(report.results[0].attempts, 2);
        assert_eq!(report.results[0].operation, "stat");
        assert!(report.results[0].moved);
        assert_eq!(report.retries, 2);
        assert_eq!(report.authority, "current");
        assert!(report.mutable);
        assert_eq!(report.write_lease_ms, Some(u32::MAX));
        assert_eq!(report.fences, 1);
        assert_eq!(report.fence_rows.len(), 1);
        assert_eq!(report.fence_rows[0].reason, "left-view");
        assert_eq!(report.fence_rows[0].stale_home_epoch, 2);
        assert_eq!(report.fence_rows[0].home_epoch, 3);
    }

    #[test]
    fn parses_a_disabled_report() {
        let report =
            RouteReport::parse("mesh-route: state=disabled reason=config\n").expect("report");
        assert_eq!(report.state, "disabled");
        assert_eq!(report.reason.as_deref(), Some("config"));
    }

    #[test]
    fn value_and_object_selection_match_the_protocol_vectors() {
        let members: Vec<String> = MEMBERS.iter().map(|value| value.to_string()).collect();
        let wire = first_object_for(&members, MEMBERS[0]).expect("wire object");
        let local = first_object_for(&members, MEMBERS[1]).expect("local object");
        assert_eq!(
            wire,
            "BEB6D1C7D3D6204050933044057A4B31417ACE296D6666B5EAB98F42A538D2A6"
        );
        assert_eq!(
            local,
            "652803CCAD037968360931DBB5EC548A16893D3B8573D76E880B5AF67EFA32F1"
        );
        // The first fixture object at epoch 2 is the value `mesh-route` pinned
        // in its own host test.
        assert_eq!(
            route_value(&wire, 2, 1).as_deref(),
            Some("FD0B0CC8BD9B18A184A1A2D7AEE6F36BCDCCD6CE64CBBA3EB857C892C72B6001")
        );
    }
}
