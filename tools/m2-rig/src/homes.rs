// SPDX-License-Identifier: GPL-3.0-only
//! M2-3 rendezvous-home evidence.
//!
//! Both kernels compute the same object-to-home mapping from their confirmed
//! membership snapshot.  The MCP module serves it at `GET /mesh/homes` and
//! logs the same bounded lines at convergence:
//!
//! ```text
//! mesh-homes: state=current object_set=m2-v1 epoch=2 members=2 digest=... count=8
//! mesh-home: object=... home=... epoch=2 digest=...
//! ```
//!
//! This module parses those lines, recomputes the fixture object IDs and the
//! rendezvous winners independently of `mesh-core`, and checks the two nodes
//! against each other.  It also drives the stale-epoch negative: a query for
//! an older epoch must return `409` with no object lines, never the current
//! table relabelled as the old one.

use std::fmt::Write as _;
use std::io::{Read, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpStream};
use std::time::{Duration, Instant};

use crate::join::JoinLine;
use crate::rig::{BootFacts, Fault, Node};

/// Stable name of the M2 home-evidence fixture set.
pub const OBJECT_SET: &str = "m2-v1";
/// Number of fixture objects the rig configures through `mesh.obj`.
pub const OBJECT_COUNT: u32 = 8;
/// Hard bound shared with `mesh-core::HOME_FIXTURE_MAX`.
const OBJECT_MAX: u32 = 32;
const OBJECT_DOMAIN: &[u8] = b"IRIX7-M2-HOME-OBJECT-v1";
const HOME_DOMAIN: &[u8] = b"IRIX7-HOME-v1";
const HEADER_PREFIX: &str = "mesh-homes: ";
/// A real table header.  The boot line `mesh-homes: configured ...` is not a
/// table and must not be selected as the last group.
const TABLE_PREFIX: &str = "mesh-homes: state=";
const ROW_PREFIX: &str = "mesh-home: ";

/// One parsed `mesh-home:` row.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HomeRow {
    /// Object ID, upper-case hex.
    pub object: String,
    /// Home NodeId, upper-case hex.
    pub home: String,
    /// Epoch carried by the row.
    pub epoch: u32,
    /// Membership digest carried by the row, upper-case hex.
    pub digest: String,
}

/// One parsed `/mesh/homes` body or serial group.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HomeTable {
    /// `current`, `pending`, `stale`, `disabled`, `bad-request` or `error`.
    pub state: String,
    /// The epoch named by a stale query.
    pub requested_epoch: Option<u32>,
    /// Why an endpoint is disabled or a query was refused.
    pub reason: Option<String>,
    /// Fixture set name.
    pub object_set: Option<String>,
    /// Membership epoch.
    pub epoch: Option<u32>,
    /// Member count.
    pub members: Option<usize>,
    /// Membership digest, upper-case hex.
    pub digest: Option<String>,
    /// Declared object count.
    pub count: Option<usize>,
    /// Parsed object rows.
    pub rows: Vec<HomeRow>,
    /// The exact header and rows, for the evidence bundle.
    pub raw: String,
}

impl HomeTable {
    fn empty() -> Self {
        Self {
            state: String::new(),
            requested_epoch: None,
            reason: None,
            object_set: None,
            epoch: None,
            members: None,
            digest: None,
            count: None,
            rows: Vec::new(),
            raw: String::new(),
        }
    }

    /// Parse the last complete `mesh-homes:` group.
    ///
    /// A transcript can contain more than one group (the root and the member
    /// log a fresh one on a rejoin or refresh), and the serial mirror can be
    /// caught after the header but before all rows.  A trailing partial group
    /// is not a table: skip it and use the last complete one.  A body with a
    /// single group still fails closed when its declared count is wrong.
    pub fn parse(text: &str) -> Result<Self, String> {
        let lines: Vec<&str> = text.lines().collect();
        let headers: Vec<usize> = lines
            .iter()
            .enumerate()
            .filter(|(_, line)| line.contains(TABLE_PREFIX))
            .map(|(index, _)| index)
            .collect();
        if headers.is_empty() {
            return Err("no mesh-homes header".to_string());
        }
        let mut last_error = None;
        for (position, &header_index) in headers.iter().enumerate().rev() {
            let end = headers.get(position + 1).copied().unwrap_or(lines.len());
            match Self::parse_group(&lines, header_index, end) {
                Ok(table) => return Ok(table),
                Err(error) => last_error = Some(error),
            }
        }
        Err(last_error.unwrap_or_else(|| "no mesh-homes header".to_string()))
    }

    /// Parse one group bounded by the next header or the end of the text.
    fn parse_group(lines: &[&str], header_index: usize, end: usize) -> Result<Self, String> {
        let header_line = lines[header_index].trim();
        let header_start = header_line
            .find(HEADER_PREFIX)
            .ok_or_else(|| "malformed mesh-homes header".to_string())?
            + HEADER_PREFIX.len();
        let mut table = Self::empty();
        for token in header_line[header_start..].split_whitespace() {
            let (key, value) = token
                .split_once('=')
                .ok_or_else(|| format!("malformed mesh-homes token {token:?}"))?;
            match key {
                "state" => table.state = value.to_string(),
                "requested_epoch" => table.requested_epoch = Some(parse_u32(value)?),
                "reason" => table.reason = Some(value.to_string()),
                "object_set" => table.object_set = Some(value.to_string()),
                "epoch" => table.epoch = Some(parse_u32(value)?),
                "members" => table.members = Some(parse_usize(value)?),
                "digest" => table.digest = Some(value.to_ascii_uppercase()),
                "count" => table.count = Some(parse_usize(value)?),
                other => return Err(format!("unknown mesh-homes field {other:?}")),
            }
        }
        if table.state.is_empty() {
            return Err("mesh-homes header has no state".into());
        }
        let mut raw = String::from(header_line);
        for line in &lines[header_index + 1..end] {
            let row_line = line.trim();
            let Some(start) = row_line.find(ROW_PREFIX) else {
                continue;
            };
            let mut row = HomeRow {
                object: String::new(),
                home: String::new(),
                epoch: 0,
                digest: String::new(),
            };
            let mut has_epoch = false;
            for token in row_line[start + ROW_PREFIX.len()..].split_whitespace() {
                let (key, value) = token
                    .split_once('=')
                    .ok_or_else(|| format!("malformed mesh-home token {token:?}"))?;
                match key {
                    "object" => row.object = value.to_ascii_uppercase(),
                    "home" => row.home = value.to_ascii_uppercase(),
                    "epoch" => {
                        row.epoch = parse_u32(value)?;
                        has_epoch = true;
                    }
                    "digest" => row.digest = value.to_ascii_uppercase(),
                    other => return Err(format!("unknown mesh-home field {other:?}")),
                }
            }
            if row.object.is_empty() || row.home.is_empty() || !has_epoch || row.digest.is_empty() {
                return Err("mesh-home row is incomplete".into());
            }
            table.rows.push(row);
            raw.push('\n');
            raw.push_str(row_line);
        }
        if let Some(count) = table.count {
            if count != table.rows.len() {
                return Err(format!(
                    "mesh-homes count {count} does not match {} rows",
                    table.rows.len()
                ));
            }
        }
        table.raw = raw;
        Ok(table)
    }

    /// Whether the last group in `text` declares and carries the same row
    /// count.  `parse` skips a partial trailing group; this raw check is what
    /// the rig waits on before taking a snapshot, so a header without its rows
    /// is not mistaken for a complete table.
    pub fn last_group_complete(text: &str) -> bool {
        let lines: Vec<&str> = text.lines().collect();
        let Some(header) = lines.iter().rposition(|line| line.contains(TABLE_PREFIX)) else {
            return false;
        };
        let header_line = lines[header].trim();
        let count = header_line
            .split_whitespace()
            .find_map(|token| token.strip_prefix("count="))
            .and_then(|value| value.parse::<usize>().ok());
        let Some(count) = count else {
            return true;
        };
        let rows = lines[header + 1..]
            .iter()
            .filter(|line| line.contains(ROW_PREFIX))
            .count();
        rows == count
    }

    fn mapping_eq(&self, other: &Self) -> bool {
        self.state == other.state
            && self.reason == other.reason
            && self.object_set == other.object_set
            && self.epoch == other.epoch
            && self.members == other.members
            && self.digest == other.digest
            && self.count == other.count
            && self.rows == other.rows
    }
}

fn parse_u32(text: &str) -> Result<u32, String> {
    text.parse().map_err(|_| format!("{text:?} is not a u32"))
}

fn parse_usize(text: &str) -> Result<usize, String> {
    text.parse().map_err(|_| format!("{text:?} is not a count"))
}

/// An HTTP response from the MCP server.
#[derive(Clone, Debug)]
pub struct HttpResponse {
    /// HTTP status code.
    pub status: u16,
    /// Response body as UTF-8 text.
    pub body: String,
}

/// Fetch one path from a node's host MCP forward.
pub fn fetch(node: &Node, target: &str, timeout: Duration) -> Result<HttpResponse, String> {
    let address: SocketAddr = (Ipv4Addr::LOCALHOST, node.http_port).into();
    let mut stream = TcpStream::connect_timeout(&address, timeout)
        .map_err(|error| format!("connect 127.0.0.1:{}: {error}", node.http_port))?;
    stream
        .set_read_timeout(Some(timeout))
        .map_err(|error| format!("set read timeout: {error}"))?;
    stream
        .set_write_timeout(Some(timeout))
        .map_err(|error| format!("set write timeout: {error}"))?;
    let request = format!(
        "GET {target} HTTP/1.0\r\nHost: 127.0.0.1:{}\r\nConnection: close\r\n\r\n",
        node.http_port
    );
    stream
        .write_all(request.as_bytes())
        .map_err(|error| format!("write request: {error}"))?;
    let mut bytes = Vec::new();
    // Read the header, then exactly Content-Length bytes of body.  The MCP
    // server keeps its side open until the close handshake; waiting for EOF
    // would either time out or race the module's slot-reclaim timeout.
    let mut buffer = [0u8; 1024];
    let header_end = loop {
        if let Some(index) = find(&bytes, b"\r\n\r\n") {
            break index;
        }
        let count = stream
            .read(&mut buffer)
            .map_err(|error| format!("read response: {error}"))?;
        if count == 0 {
            return Err("HTTP response ended before its headers".into());
        }
        bytes.extend_from_slice(&buffer[..count]);
        if bytes.len() > 64 * 1024 {
            return Err("HTTP response headers exceed 64 KiB".into());
        }
    };
    let head = String::from_utf8_lossy(&bytes[..header_end]);
    let status_line = head
        .lines()
        .next()
        .ok_or_else(|| "HTTP response has no status line".to_string())?;
    let status = status_line
        .split_whitespace()
        .nth(1)
        .ok_or_else(|| format!("malformed HTTP status line {status_line:?}"))?
        .parse()
        .map_err(|_| format!("malformed HTTP status line {status_line:?}"))?;
    let content_length = head.lines().find_map(|line| {
        let (name, value) = line.split_once(':')?;
        if name.eq_ignore_ascii_case("content-length") {
            value.trim().parse::<usize>().ok()
        } else {
            None
        }
    });
    let body_start = header_end + 4;
    if let Some(length) = content_length {
        while bytes.len() < body_start + length {
            let count = stream
                .read(&mut buffer)
                .map_err(|error| format!("read response body: {error}"))?;
            if count == 0 {
                break;
            }
            bytes.extend_from_slice(&buffer[..count]);
            if bytes.len() > body_start + length + 1024 {
                return Err("HTTP response body exceeds Content-Length".into());
            }
        }
        let end = (body_start + length).min(bytes.len());
        let body = String::from_utf8_lossy(&bytes[body_start..end]).into_owned();
        Ok(HttpResponse { status, body })
    } else {
        let body = String::from_utf8_lossy(&bytes[body_start..]).into_owned();
        Ok(HttpResponse { status, body })
    }
}

/// Fetch and parse one table, retrying until the module tick arms the
/// listener and answers.
pub fn fetch_table(
    node: &Node,
    target: &str,
    timeout: Duration,
) -> Result<(u16, HomeTable), String> {
    let deadline = Instant::now() + timeout;
    loop {
        let error = match fetch(node, target, Duration::from_secs(10)).and_then(|response| {
            HomeTable::parse(&response.body).map(|table| (response.status, table))
        }) {
            Ok(value) => return Ok(value),
            Err(error) => error,
        };
        if Instant::now() >= deadline {
            return Err(format!("{}: {error}", node.name));
        }
        std::thread::sleep(Duration::from_millis(1000));
    }
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

/// Derive one fixture object independently of `mesh-core`.
pub fn fixture_object(index: u32) -> [u8; 32] {
    let mut hasher = blake3::Hasher::new();
    hasher.update(OBJECT_DOMAIN);
    hasher.update(&index.to_le_bytes());
    *hasher.finalize().as_bytes()
}

/// The fixture object IDs in configured order.
pub fn fixture_objects(count: u32) -> Vec<[u8; 32]> {
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

/// Compute the independent rendezvous winner for one object.
pub fn expected_home(object: &[u8; 32], members: &[String]) -> Option<String> {
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

/// Build the table the two kernels must agree on.
pub fn expected_table(
    epoch: u32,
    digest: &str,
    members: &[String],
    count: u32,
) -> Result<HomeTable, String> {
    if members.is_empty() {
        return Err("cannot compute homes for an empty membership".into());
    }
    if count > OBJECT_MAX {
        return Err(format!("object count {count} exceeds {OBJECT_MAX}"));
    }
    let mut members = members.to_vec();
    members.sort();
    members.dedup();
    let mut rows = Vec::new();
    for (index, object) in fixture_objects(count).iter().enumerate() {
        let home = expected_home(object, &members)
            .ok_or_else(|| format!("no home for fixture object {index}"))?;
        rows.push(HomeRow {
            object: hex(object),
            home,
            epoch,
            digest: digest.to_ascii_uppercase(),
        });
    }
    Ok(HomeTable {
        state: "current".into(),
        requested_epoch: None,
        reason: None,
        object_set: Some(OBJECT_SET.into()),
        epoch: Some(epoch),
        members: Some(members.len()),
        digest: Some(digest.to_ascii_uppercase()),
        count: Some(count as usize),
        rows,
        raw: String::new(),
    })
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

fn hex(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        let _ = write!(out, "{byte:02X}");
    }
    out
}

/// One node's `/mesh/homes` result.
#[derive(Clone, Debug)]
pub struct NodeHomes {
    /// `n0` or `n1`.
    pub name: String,
    /// The path that was fetched.
    pub path: String,
    /// HTTP status, when a response arrived.
    pub status: Option<u16>,
    /// Parsed table, when the body was valid.
    pub table: Option<HomeTable>,
    /// Fetch or validation error.
    pub error: Option<String>,
}

/// One collection round (`before`, `after` or `stale`).
#[derive(Clone, Debug)]
pub struct HomeRound {
    /// Round label.
    pub label: String,
    /// Whether every node answered and agreed.
    pub ok: bool,
    /// Human-readable result.
    pub message: String,
    /// The independent expected table.
    pub expected: Option<HomeTable>,
    /// Per-node results.
    pub nodes: Vec<NodeHomes>,
}

impl HomeRound {
    fn new(label: &str, expected: Option<HomeTable>) -> Self {
        Self {
            label: label.into(),
            ok: false,
            message: String::new(),
            expected,
            nodes: Vec::new(),
        }
    }

    fn finish(mut self, errors: Vec<String>) -> Self {
        self.ok = errors.is_empty();
        self.message = if self.ok {
            format!(
                "{}: both kernels reported the same {} object homes",
                self.label,
                self.expected.as_ref().map_or(0, |table| table.rows.len())
            )
        } else {
            errors.join("; ")
        };
        self
    }
}

fn serial_table(facts: &BootFacts) -> Result<HomeTable, String> {
    if facts.mesh_homes_lines.is_empty() {
        return Err("no mesh-homes serial lines".into());
    }
    HomeTable::parse(&facts.mesh_homes_lines.join("\n"))
}

fn check_current(table: &HomeTable, expected: &HomeTable) -> Vec<String> {
    let mut errors = Vec::new();
    if table.state != "current" {
        errors.push(format!("state is {:?}, expected current", table.state));
    }
    if table.object_set.as_deref() != Some(OBJECT_SET) {
        errors.push(format!(
            "object set is {:?}, expected {OBJECT_SET}",
            table.object_set
        ));
    }
    if table.epoch != expected.epoch {
        errors.push(format!(
            "epoch is {:?}, expected {:?}",
            table.epoch, expected.epoch
        ));
    }
    if table.members != expected.members {
        errors.push(format!(
            "member count is {:?}, expected {:?}",
            table.members, expected.members
        ));
    }
    if table.digest != expected.digest {
        errors.push(format!(
            "digest is {:?}, expected {:?}",
            table.digest, expected.digest
        ));
    }
    if table.count != expected.count {
        errors.push(format!(
            "count is {:?}, expected {:?}",
            table.count, expected.count
        ));
    }
    if table.rows.len() != expected.rows.len() {
        errors.push(format!(
            "{} rows, expected {}",
            table.rows.len(),
            expected.rows.len()
        ));
    }
    for (index, (row, want)) in table.rows.iter().zip(&expected.rows).enumerate() {
        if row != want {
            errors.push(format!(
                "row {index} is object={} home={} epoch={} digest={}, \
                 expected object={} home={} epoch={} digest={}",
                row.object,
                row.home,
                row.epoch,
                row.digest,
                want.object,
                want.home,
                want.epoch,
                want.digest
            ));
        }
    }
    errors
}

/// Fetch `/mesh/homes` from both nodes and check the mapping.
pub fn collect_current(
    nodes: &[Node],
    facts: &[BootFacts],
    epoch: u32,
    digest: &str,
    members: &[String],
    timeout: Duration,
) -> HomeRound {
    let expected = expected_table(epoch, digest, members, OBJECT_COUNT);
    let mut round = HomeRound::new("current", expected.clone().ok());
    let mut errors = Vec::new();
    let mut tables = Vec::new();
    let expected = match expected {
        Ok(table) => table,
        Err(error) => {
            round.nodes = nodes
                .iter()
                .map(|node| NodeHomes {
                    name: node.name.clone(),
                    path: "/mesh/homes".into(),
                    status: None,
                    table: None,
                    error: Some(error.clone()),
                })
                .collect();
            return round.finish(vec![error]);
        }
    };
    for (node, facts) in nodes.iter().zip(facts) {
        let mut evidence = NodeHomes {
            name: node.name.clone(),
            path: "/mesh/homes".into(),
            status: None,
            table: None,
            error: None,
        };
        match fetch_table(node, "/mesh/homes", timeout) {
            Ok((status, table)) => {
                evidence.status = Some(status);
                if status != 200 {
                    errors.push(format!("{}: HTTP {status}, expected 200", node.name));
                }
                let mut node_errors = check_current(&table, &expected);
                errors.extend(
                    node_errors
                        .drain(..)
                        .map(|error| format!("{}: {error}", node.name)),
                );
                match serial_table(facts) {
                    Ok(serial) => {
                        if !serial.mapping_eq(&table) {
                            errors.push(format!(
                                "{}: serial mesh-homes group does not match the MCP body",
                                node.name
                            ));
                        }
                    }
                    Err(error) => errors.push(format!("{}: serial {error}", node.name)),
                }
                tables.push((node.name.clone(), table.clone()));
                evidence.table = Some(table);
            }
            Err(error) => {
                errors.push(format!("{}: {error}", node.name));
                evidence.error = Some(error);
            }
        }
        round.nodes.push(evidence);
    }
    if tables.len() == nodes.len() {
        let (first_name, first) = &tables[0];
        for (name, table) in &tables[1..] {
            if !first.mapping_eq(table) {
                errors.push(format!(
                    "{first_name} and {name} reported different home mappings"
                ));
            }
        }
    }
    round.finish(errors)
}

/// Collect the table from the serial groups instead of the MCP endpoint.
///
/// The module logs the same header and rows from the same `HomeTable` on
/// convergence.  The after-rejoin check uses this path because the iris host
/// forward can retain a half-open guest connection across the member restart;
/// the serial transcript is produced by both kernels and is part of the gate
/// evidence, so it does not weaken the equality check.
pub fn collect_current_serial(
    nodes: &[Node],
    facts: &[BootFacts],
    epoch: u32,
    digest: &str,
    members: &[String],
) -> HomeRound {
    let expected = expected_table(epoch, digest, members, OBJECT_COUNT);
    let mut round = HomeRound::new("current (serial)", expected.clone().ok());
    let mut errors = Vec::new();
    let expected = match expected {
        Ok(table) => table,
        Err(error) => {
            round.nodes = nodes
                .iter()
                .map(|node| NodeHomes {
                    name: node.name.clone(),
                    path: "serial".into(),
                    status: None,
                    table: None,
                    error: Some(error.clone()),
                })
                .collect();
            return round.finish(vec![error]);
        }
    };
    let mut tables = Vec::new();
    for (node, facts) in nodes.iter().zip(facts) {
        let mut evidence = NodeHomes {
            name: node.name.clone(),
            path: "serial".into(),
            status: None,
            table: None,
            error: None,
        };
        match serial_table(facts) {
            Ok(table) => {
                errors.extend(
                    check_current(&table, &expected)
                        .into_iter()
                        .map(|error| format!("{}: {error}", node.name)),
                );
                tables.push((node.name.clone(), table.clone()));
                evidence.table = Some(table);
            }
            Err(error) => {
                errors.push(format!("{}: serial {error}", node.name));
                evidence.error = Some(error);
            }
        }
        round.nodes.push(evidence);
    }
    if tables.len() == nodes.len() {
        let (first_name, first) = &tables[0];
        for (name, table) in &tables[1..] {
            if !first.mapping_eq(table) {
                errors.push(format!(
                    "{first_name} and {name} logged different home mappings"
                ));
            }
        }
    }
    round.finish(errors)
}

/// Fetch `/mesh/homes?epoch=<stale>` and require a fail-closed `409`.
pub fn collect_stale(
    nodes: &[Node],
    stale_epoch: u32,
    current_epoch: u32,
    current_digest: &str,
    timeout: Duration,
) -> HomeRound {
    let mut round = HomeRound::new("stale", None);
    let mut errors = Vec::new();
    if stale_epoch >= current_epoch {
        errors.push(format!(
            "stale epoch {stale_epoch} is not below the current epoch {current_epoch}"
        ));
    }
    let path = format!("/mesh/homes?epoch={stale_epoch}");
    for node in nodes {
        let mut evidence = NodeHomes {
            name: node.name.clone(),
            path: path.clone(),
            status: None,
            table: None,
            error: None,
        };
        match fetch_table(node, &path, timeout) {
            Ok((status, table)) => {
                evidence.status = Some(status);
                if status != 409 {
                    errors.push(format!("{}: HTTP {status}, expected 409", node.name));
                }
                if table.state != "stale" {
                    errors.push(format!(
                        "{}: state is {:?}, expected stale",
                        node.name, table.state
                    ));
                }
                if table.requested_epoch != Some(stale_epoch) {
                    errors.push(format!(
                        "{}: requested_epoch is {:?}, expected {stale_epoch}",
                        node.name, table.requested_epoch
                    ));
                }
                if table.epoch != Some(current_epoch) {
                    errors.push(format!(
                        "{}: current epoch is {:?}, expected {current_epoch}",
                        node.name, table.epoch
                    ));
                }
                if table.digest.as_deref() != Some(current_digest) {
                    errors.push(format!(
                        "{}: digest is {:?}, expected {current_digest}",
                        node.name, table.digest
                    ));
                }
                if !table.rows.is_empty() {
                    errors.push(format!(
                        "{}: stale response carried {} object rows",
                        node.name,
                        table.rows.len()
                    ));
                }
                if table.count.unwrap_or(0) != 0 {
                    errors.push(format!(
                        "{}: stale response declared count {:?}, expected 0",
                        node.name, table.count
                    ));
                }
                evidence.table = Some(table);
            }
            Err(error) => {
                errors.push(format!("{}: {error}", node.name));
                evidence.error = Some(error);
            }
        }
        round.nodes.push(evidence);
    }
    round.finish(errors)
}

/// Fetch `/mesh/homes` in the `no-peer` mode: the mesh is disabled, so the
/// endpoint must answer `404` and must not invent a one-node table.
pub fn collect_disabled(nodes: &[Node], timeout: Duration) -> HomeRound {
    let mut round = HomeRound::new("disabled", None);
    let mut errors = Vec::new();
    for node in nodes {
        let mut evidence = NodeHomes {
            name: node.name.clone(),
            path: "/mesh/homes".into(),
            status: None,
            table: None,
            error: None,
        };
        match fetch_table(node, "/mesh/homes", timeout) {
            Ok((status, table)) => {
                evidence.status = Some(status);
                if status != 404 {
                    errors.push(format!("{}: HTTP {status}, expected 404", node.name));
                }
                if table.state != "disabled" {
                    errors.push(format!(
                        "{}: state is {:?}, expected disabled",
                        node.name, table.state
                    ));
                }
                if !table.rows.is_empty() {
                    errors.push(format!(
                        "{}: disabled response carried {} object rows",
                        node.name,
                        table.rows.len()
                    ));
                }
                evidence.table = Some(table);
            }
            Err(error) => {
                errors.push(format!("{}: {error}", node.name));
                evidence.error = Some(error);
            }
        }
        round.nodes.push(evidence);
    }
    round.finish(errors)
}

/// Check the fail-closed joins: the root keeps its confirmed seed view and
/// the member presents no homes while it has no confirmed snapshot.
pub fn collect_wrong_authority(
    nodes: &[Node],
    facts: &[BootFacts],
    timeout: Duration,
) -> HomeRound {
    let mut round = HomeRound::new("wrong-authority", None);
    let mut errors = Vec::new();
    // The root accepted the request and signed its own view; it is the member
    // that rejects the signature and stays pending.  Check the root against
    // its converged snapshot, not against the seed local line.
    let root_snapshot = facts[0]
        .mesh_join_converged
        .as_deref()
        .or(facts[0].mesh_join_local.as_deref())
        .and_then(JoinLine::parse);
    let expected = root_snapshot.as_ref().and_then(|line| {
        expected_table(line.epoch, &line.digest, &line.snapshot, OBJECT_COUNT).ok()
    });
    if let Some(expected) = &expected {
        let mut evidence = NodeHomes {
            name: nodes[0].name.clone(),
            path: "/mesh/homes".into(),
            status: None,
            table: None,
            error: None,
        };
        match fetch_table(&nodes[0], "/mesh/homes", timeout) {
            Ok((status, table)) => {
                evidence.status = Some(status);
                if status != 200 {
                    errors.push(format!("{}: HTTP {status}, expected 200", nodes[0].name));
                }
                errors.extend(
                    check_current(&table, expected)
                        .into_iter()
                        .map(|error| format!("{}: {error}", nodes[0].name)),
                );
                evidence.table = Some(table);
            }
            Err(error) => {
                errors.push(format!("{}: {error}", nodes[0].name));
                evidence.error = Some(error);
            }
        }
        round.nodes.push(evidence);
    } else {
        errors.push("n0: no confirmed local snapshot to check against".into());
    }

    let mut evidence = NodeHomes {
        name: nodes[1].name.clone(),
        path: "/mesh/homes".into(),
        status: None,
        table: None,
        error: None,
    };
    match fetch_table(&nodes[1], "/mesh/homes", timeout) {
        Ok((status, table)) => {
            evidence.status = Some(status);
            if status != 409 {
                errors.push(format!("{}: HTTP {status}, expected 409", nodes[1].name));
            }
            if table.state != "pending" {
                errors.push(format!(
                    "{}: state is {:?}, expected pending",
                    nodes[1].name, table.state
                ));
            }
            if table.epoch != Some(1) || table.members != Some(1) {
                errors.push(format!(
                    "{}: pending view is epoch {:?} members {:?}, expected the seed (1, 1)",
                    nodes[1].name, table.epoch, table.members
                ));
            }
            if !table.rows.is_empty() {
                errors.push(format!(
                    "{}: pending response carried {} object rows",
                    nodes[1].name,
                    table.rows.len()
                ));
            }
            evidence.table = Some(table);
        }
        Err(error) => {
            errors.push(format!("{}: {error}", nodes[1].name));
            evidence.error = Some(error);
        }
    }
    round.nodes.push(evidence);
    round.finish(errors)
}

/// The complete M2-3 homes evidence.
#[derive(Clone, Debug)]
pub struct HomesEvidence {
    /// Whether the expected home evidence was observed.
    pub ok: bool,
    /// Human-readable result or failure.
    pub message: String,
    /// The fixture set name.
    pub object_set: String,
    /// The configured fixture count.
    pub count: u32,
    /// Homes before the member restart.
    pub before: Option<HomeRound>,
    /// Homes after the rejoin.
    pub after: Option<HomeRound>,
    /// The stale-epoch negative.
    pub stale: Option<HomeRound>,
    /// The no-peer negative.
    pub disabled: Option<HomeRound>,
    /// The wrong-authority negative.
    pub wrong_authority: Option<HomeRound>,
}

/// Combine the rounds and require the before/after mappings to be identical.
pub fn evaluate(
    fault: Fault,
    before: Option<HomeRound>,
    after: Option<HomeRound>,
    stale: Option<HomeRound>,
    disabled: Option<HomeRound>,
    wrong_authority: Option<HomeRound>,
    require_rejoin: bool,
) -> HomesEvidence {
    let mut errors = Vec::new();
    let mut evidence = HomesEvidence {
        ok: false,
        message: String::new(),
        object_set: OBJECT_SET.into(),
        count: OBJECT_COUNT,
        before,
        after,
        stale,
        disabled,
        wrong_authority,
    };
    match fault {
        Fault::None | Fault::NoRoute | Fault::KillPeer | Fault::Partition => {
            match &evidence.before {
                Some(before) if before.ok => {}
                Some(before) => errors.push(before.message.clone()),
                None => errors.push("no homes were collected".into()),
            }
            if require_rejoin {
                match (&evidence.before, &evidence.after) {
                    (Some(before), Some(after)) => {
                        if !after.ok {
                            errors.push(after.message.clone());
                        }
                        if before.nodes.len() != after.nodes.len() {
                            errors.push("before/after node counts differ".into());
                        } else {
                            for (first, second) in before.nodes.iter().zip(&after.nodes) {
                                match (&first.table, &second.table) {
                                    (Some(first), Some(second)) if first.mapping_eq(second) => {}
                                    (Some(_), Some(_)) => errors.push(format!(
                                        "{}: homes changed across the member rejoin",
                                        first.name
                                    )),
                                    _ => errors.push(format!(
                                        "{}: no table on one side of the member rejoin",
                                        first.name
                                    )),
                                }
                            }
                        }
                    }
                    (None, _) => errors.push("no homes before the member restart".into()),
                    (_, None) => errors.push("no homes after the member rejoin".into()),
                }
            }
            match &evidence.stale {
                Some(stale) if stale.ok => {}
                Some(stale) => errors.push(stale.message.clone()),
                None => errors.push("the stale-epoch negative did not run".into()),
            }
        }
        Fault::NoPeer => match &evidence.disabled {
            Some(round) if round.ok => {}
            Some(round) => errors.push(round.message.clone()),
            None => errors.push("the no-peer homes check did not run".into()),
        },
        Fault::WrongAuthority => match &evidence.wrong_authority {
            Some(round) if round.ok => {}
            Some(round) => errors.push(round.message.clone()),
            None => errors.push("the wrong-authority homes check did not run".into()),
        },
    }
    evidence.ok = errors.is_empty();
    evidence.message = if evidence.ok {
        match fault {
            Fault::None | Fault::NoRoute | Fault::KillPeer | Fault::Partition => {
                let suffix = if require_rejoin {
                    "before and after the member rejoin"
                } else {
                    "on both kernels"
                };
                format!(
                    "both kernels matched the independent rendezvous table at epoch {} \
                     {suffix}; stale epoch refused",
                    evidence
                        .before
                        .as_ref()
                        .and_then(|round| round.expected.as_ref())
                        .and_then(|table| table.epoch)
                        .unwrap_or(0)
                )
            }
            Fault::NoPeer => "no-peer: /mesh/homes is disabled and presents no homes".into(),
            Fault::WrongAuthority => {
                "wrong-authority: the root's confirmed view is checked, and the member presents \
                 no homes while it rejects the signature and stays pending"
                    .into()
            }
        }
    } else {
        errors.join("; ")
    };
    evidence
}

/// Parse the epoch from a `?epoch=` path, for evidence tests.
#[cfg(test)]
fn path_epoch(path: &str) -> Option<u32> {
    let query = path.split_once('?')?.1;
    query.split('&').find_map(|part| {
        part.strip_prefix("epoch=")
            .and_then(|value| value.parse().ok())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(byte: u8) -> String {
        format!("{byte:02X}").repeat(16)
    }

    fn sample(epoch: u32, digest: &str, members: &[String]) -> HomeTable {
        expected_table(epoch, digest, members, 4).expect("table")
    }

    #[test]
    fn fixture_objects_are_deterministic_and_bounded() {
        let first = fixture_objects(8);
        assert_eq!(first, fixture_objects(8));
        assert_eq!(first.len(), 8);
        let mut distinct = first.clone();
        distinct.sort_unstable();
        distinct.dedup();
        assert_eq!(distinct.len(), 8);
        assert_eq!(fixture_objects(OBJECT_MAX + 1).len(), OBJECT_MAX as usize);
        // This is the same vector the mesh-core test pins; the two
        // implementations must not drift.
        assert_eq!(
            hex(&fixture_object(0)),
            "BEB6D1C7D3D6204050933044057A4B31417ACE296D6666B5EAB98F42A538D2A6"
        );
    }

    #[test]
    fn expected_homes_are_stable_and_member_local() {
        let members = vec![id(2), id(1)];
        let table = sample(2, "AABB", &members);
        assert_eq!(table, sample(2, "AABB", &members));
        assert_eq!(table.rows.len(), 4);
        for row in &table.rows {
            assert!(row.home == id(1) || row.home == id(2));
            let object = decode_hex_32(&row.object).expect("object");
            assert_eq!(
                expected_home(&object, &members).as_deref(),
                Some(row.home.as_str())
            );
        }
    }

    #[test]
    fn a_single_node_owns_every_fixture_object() {
        let members = vec![id(7)];
        let table = sample(2, "AABB", &members);
        assert!(table.rows.iter().all(|row| row.home == id(7)));
    }

    #[test]
    fn parse_round_trips_the_evidence_lines() {
        let table = sample(2, "AABB", &[id(1), id(2)]);
        let mut text = format!(
            "mesh-homes: state=current object_set={} epoch={} members={} digest={} count={}\n",
            OBJECT_SET,
            table.epoch.unwrap(),
            table.members.unwrap(),
            table.digest.as_deref().unwrap(),
            table.count.unwrap()
        );
        for row in &table.rows {
            let _ = writeln!(
                text,
                "mesh-home: object={} home={} epoch={} digest={}",
                row.object, row.home, row.epoch, row.digest
            );
        }
        let parsed = HomeTable::parse(&text).expect("parse");
        assert!(parsed.mapping_eq(&table));
    }

    #[test]
    fn parse_takes_the_last_serial_group() {
        let first = sample(2, "AABB", &[id(1), id(2)]);
        let mut text = String::new();
        for table in [&first, &first] {
            let _ = writeln!(
                text,
                "mesh-homes: state=current object_set={} epoch={} members={} digest={} count={}",
                OBJECT_SET,
                table.epoch.unwrap(),
                table.members.unwrap(),
                table.digest.as_deref().unwrap(),
                table.count.unwrap()
            );
            for row in &table.rows {
                let _ = writeln!(
                    text,
                    "sash> mesh-home: object={} home={} epoch={} digest={}",
                    row.object, row.home, row.epoch, row.digest
                );
            }
        }
        assert!(HomeTable::parse(&text).expect("parse").mapping_eq(&first));
    }

    #[test]
    fn last_group_completeness_is_raw() {
        let table = sample(2, "AABB", &[id(1), id(2)]);
        let mut text = String::new();
        let _ = writeln!(
            text,
            "mesh-homes: state=current object_set={} epoch={} members={} digest={} count={}",
            OBJECT_SET,
            table.epoch.unwrap(),
            table.members.unwrap(),
            table.digest.as_deref().unwrap(),
            table.count.unwrap()
        );
        for row in &table.rows {
            let _ = writeln!(
                text,
                "mesh-home: object={} home={} epoch={} digest={}",
                row.object, row.home, row.epoch, row.digest
            );
        }
        assert!(HomeTable::last_group_complete(&text));
        let _ = writeln!(
            text,
            "mesh-homes: state=current object_set={} epoch={} members={} digest={} count={}",
            OBJECT_SET,
            table.epoch.unwrap(),
            table.members.unwrap(),
            table.digest.as_deref().unwrap(),
            table.count.unwrap()
        );
        assert!(!HomeTable::last_group_complete(&text));
    }

    #[test]
    fn a_partial_trailing_group_is_skipped() {
        // The serial mirror can leave the last group with a header and a
        // couple of rows.  It is not a table; the previous complete group is
        // the current view.
        let table = sample(2, "AABB", &[id(1), id(2)]);
        let mut text = String::new();
        for group in 0..2 {
            let _ = writeln!(
                text,
                "mesh-homes: state=current object_set={} epoch={} members={} digest={} count={}",
                OBJECT_SET,
                table.epoch.unwrap(),
                table.members.unwrap(),
                table.digest.as_deref().unwrap(),
                table.count.unwrap()
            );
            let rows = if group == 0 { table.rows.len() } else { 2 };
            for row in table.rows.iter().take(rows) {
                let _ = writeln!(
                    text,
                    "sash> mesh-home: object={} home={} epoch={} digest={}",
                    row.object, row.home, row.epoch, row.digest
                );
            }
        }
        assert!(HomeTable::parse(&text).expect("parse").mapping_eq(&table));
    }

    #[test]
    fn stale_and_pending_bodies_have_no_rows() {
        let stale = HomeTable::parse(
            "mesh-homes: state=stale requested_epoch=1 epoch=2 members=2 \
             digest=2EDA13C5AC6F726132E0479DFE6045B7 count=0\n",
        )
        .expect("stale");
        assert_eq!(stale.state, "stale");
        assert_eq!(stale.requested_epoch, Some(1));
        assert!(stale.rows.is_empty());

        let pending = HomeTable::parse(
            "mesh-homes: state=pending epoch=1 members=1 \
             digest=00112233445566778899AABBCCDDEEFF count=0\n",
        )
        .expect("pending");
        assert_eq!(pending.state, "pending");
        assert!(pending.rows.is_empty());

        let disabled =
            HomeTable::parse("mesh-homes: state=disabled reason=peer\n").expect("disabled");
        assert_eq!(disabled.state, "disabled");
        assert_eq!(disabled.reason.as_deref(), Some("peer"));
        assert!(disabled.rows.is_empty());
    }

    #[test]
    fn mismatch_is_detected_between_tables() {
        let first = sample(2, "AABB", &[id(1), id(2)]);
        let mut second = first.clone();
        second.rows[0].home = if second.rows[0].home == id(1) {
            id(2)
        } else {
            id(1)
        };
        assert!(!first.mapping_eq(&second));
        let mut third = first.clone();
        third.epoch = Some(3);
        assert!(!first.mapping_eq(&third));
    }

    #[test]
    fn epoch_query_is_parsed() {
        assert_eq!(path_epoch("/mesh/homes"), None);
        assert_eq!(path_epoch("/mesh/homes?epoch=1"), Some(1));
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
}
