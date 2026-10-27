// SPDX-License-Identifier: GPL-3.0-only
//! M3 first slice: eight-node shard evidence on real kernels.
//!
//! The rig starts `--nodes` independent iris instances.  Every guest keeps its
//! existing single mesh listener; a host-side link switch (`hub`) multiplexes
//! the member links onto the root's one stream.  The hub forwards framed
//! records by `MeshHeader.dst` and never computes a home or confirms an epoch.
//!
//! Evidence:
//!   * every kernel reports the same signed membership epoch and digest;
//!   * every kernel derives the same M3 fixture home table;
//!   * each kernel reports only its own assignments at `/mesh/shard`;
//!   * a stale `epoch=` is refused with no rows;
//!   * the M2 `/mesh/homes` surface is unchanged;
//!   * one capability route crosses from the root to a member that owns the
//!     object, with the member serving the stat.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use crate::check_prerequisites;
use crate::evidence::json_escape;
use crate::fixture::Fixture;
use crate::homes;
use crate::hub::MeshHub;
use crate::join;
use crate::relay::MeshRelay;
use crate::rig::{
    derive_nodes, git, read_serial, shard_environments, wait_boot, write_text, AuthorityKeys, Node,
    Options, RunningNode,
};

/// The M3 shard-evidence fixture count.  It is a test set, not a directory.
pub const SHARD_OBJECT_COUNT: u32 = 32;
const SHARD_OBJECT_MAX: u32 = 256;
const SHARD_DOMAIN: &[u8] = b"IRIX7-M3-SHARD-OBJECT-v1";
const HOME_DOMAIN: &[u8] = b"IRIX7-HOME-v1";
const SHARD_PREFIX: &str = "mesh-shard: ";
const OWNER_PREFIX: &str = "mesh-shard-owner: ";
const OWNED_PREFIX: &str = "mesh-shard-owned: ";

/// One parsed `mesh-shard-owned:` row (the same shape as an M2 home row).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ShardRow {
    /// Object ID, upper-case hex.
    pub object: String,
    /// Home NodeId, upper-case hex.
    pub home: String,
    /// Epoch carried by the row.
    pub epoch: u32,
    /// Membership digest, upper-case hex.
    pub digest: String,
}

/// One parsed `/mesh/shard` body or serial group.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ShardTable {
    /// `current`, `pending`, `stale`, `disabled`, `bad-request` or `error`.
    pub state: String,
    /// Node role: `root` or `member` when current.
    pub role: Option<String>,
    /// This node's NodeId, upper-case hex.
    pub node: Option<String>,
    /// The requested epoch on a stale query.
    pub requested_epoch: Option<u32>,
    /// Disabled or error reason.
    pub reason: Option<String>,
    /// Fixture set name.
    pub object_set: Option<String>,
    /// Confirmed membership epoch.
    pub epoch: Option<u32>,
    /// Confirmed member count.
    pub members: Option<usize>,
    /// Membership digest, upper-case hex.
    pub digest: Option<String>,
    /// Total object count.
    pub count: Option<usize>,
    /// Number of nodes owning at least one object.
    pub owners: Option<usize>,
    /// Number of objects owned by this node.
    pub owned: Option<usize>,
    /// Smallest non-empty shard.
    pub min: Option<usize>,
    /// Largest shard.
    pub max: Option<usize>,
    /// Per-member owner counts.
    pub owner_rows: Vec<(String, usize)>,
    /// This node's assignments.
    pub rows: Vec<ShardRow>,
    /// The exact body, for evidence.
    pub raw: String,
}

impl ShardTable {
    /// Parse the last shard group in a body or serial transcript.
    pub fn parse(text: &str) -> Result<Self, String> {
        let lines: Vec<&str> = text.lines().collect();
        let header_index = lines
            .iter()
            .rposition(|line| line.trim_start().starts_with(SHARD_PREFIX))
            .ok_or_else(|| "no mesh-shard header".to_string())?;
        let header = lines[header_index].trim();
        let mut table = Self::default();
        for token in header[SHARD_PREFIX.len()..].split_whitespace() {
            let (key, value) = token
                .split_once('=')
                .ok_or_else(|| format!("malformed mesh-shard token {token:?}"))?;
            match key {
                "state" => table.state = value.to_string(),
                "role" => table.role = Some(value.to_string()),
                "node" => table.node = Some(value.to_ascii_uppercase()),
                "requested_epoch" => table.requested_epoch = Some(parse_u32(value)?),
                "reason" => table.reason = Some(value.to_string()),
                "object_set" => table.object_set = Some(value.to_string()),
                "epoch" => table.epoch = Some(parse_u32(value)?),
                "members" => table.members = Some(parse_usize(value)?),
                "digest" => table.digest = Some(value.to_ascii_uppercase()),
                "count" => table.count = Some(parse_usize(value)?),
                "owners" => table.owners = Some(parse_usize(value)?),
                "owned" => table.owned = Some(parse_usize(value)?),
                "min" => table.min = Some(parse_usize(value)?),
                "max" => table.max = Some(parse_usize(value)?),
                other => return Err(format!("unknown mesh-shard field {other:?}")),
            }
        }
        if table.state.is_empty() {
            return Err("mesh-shard header has no state".into());
        }
        table.raw = header.to_string();
        for line in &lines[header_index + 1..] {
            let line = line.trim();
            if line.starts_with(SHARD_PREFIX) {
                break;
            }
            if let Some(rest) = line.strip_prefix(OWNER_PREFIX) {
                let mut node = None;
                let mut count = None;
                for token in rest.split_whitespace() {
                    let (key, value) = token
                        .split_once('=')
                        .ok_or_else(|| format!("malformed shard-owner token {token:?}"))?;
                    match key {
                        "node" => node = Some(value.to_ascii_uppercase()),
                        "count" => count = Some(parse_usize(value)?),
                        other => return Err(format!("unknown shard-owner field {other:?}")),
                    }
                }
                table.owner_rows.push((
                    node.ok_or_else(|| "shard-owner row has no node".to_string())?,
                    count.ok_or_else(|| "shard-owner row has no count".to_string())?,
                ));
                table.raw.push('\n');
                table.raw.push_str(line);
            } else if let Some(rest) = line.strip_prefix(OWNED_PREFIX) {
                let mut row = ShardRow {
                    object: String::new(),
                    home: String::new(),
                    epoch: 0,
                    digest: String::new(),
                };
                let mut has_epoch = false;
                for token in rest.split_whitespace() {
                    let (key, value) = token
                        .split_once('=')
                        .ok_or_else(|| format!("malformed shard-owned token {token:?}"))?;
                    match key {
                        "object" => row.object = value.to_ascii_uppercase(),
                        "home" => row.home = value.to_ascii_uppercase(),
                        "epoch" => {
                            row.epoch = parse_u32(value)?;
                            has_epoch = true;
                        }
                        "digest" => row.digest = value.to_ascii_uppercase(),
                        other => return Err(format!("unknown shard-owned field {other:?}")),
                    }
                }
                if row.object.is_empty()
                    || row.home.is_empty()
                    || !has_epoch
                    || row.digest.is_empty()
                {
                    return Err("shard-owned row is incomplete".into());
                }
                table.rows.push(row);
                table.raw.push('\n');
                table.raw.push_str(line);
            }
        }
        if let Some(owners) = table.owners {
            if owners != table.owner_rows.len() {
                return Err(format!(
                    "mesh-shard owners={owners} does not match {} owner rows",
                    table.owner_rows.len()
                ));
            }
        }
        if let Some(owned) = table.owned {
            if owned != table.rows.len() {
                return Err(format!(
                    "mesh-shard owned={owned} does not match {} owned rows",
                    table.rows.len()
                ));
            }
        }
        Ok(table)
    }

    /// The header without its rows, for a compact evidence field.
    pub fn header(&self) -> String {
        self.raw.lines().next().unwrap_or_default().to_string()
    }
}

fn parse_u32(text: &str) -> Result<u32, String> {
    text.parse().map_err(|_| format!("{text:?} is not a u32"))
}

fn parse_usize(text: &str) -> Result<usize, String> {
    text.parse().map_err(|_| format!("{text:?} is not a count"))
}

fn shard_fixture_object(index: u32) -> [u8; 32] {
    let mut hasher = blake3::Hasher::new();
    hasher.update(SHARD_DOMAIN);
    hasher.update(&index.to_le_bytes());
    *hasher.finalize().as_bytes()
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

/// The independently computed shard table.
struct Expected {
    epoch: u32,
    digest: String,
    members: Vec<String>,
    rows: Vec<ShardRow>,
    counts: BTreeMap<String, usize>,
    min: usize,
    max: usize,
}

impl Expected {
    fn build(nodes: &[Node], epoch: u32) -> Result<Self, String> {
        let mut members: Vec<String> = nodes.iter().map(Node::node_id_string).collect();
        members.sort();
        let digest = join::snapshot_digest(epoch, &members)
            .ok_or_else(|| "cannot compute the membership digest".to_string())?;
        let mut rows = Vec::new();
        for index in 0..SHARD_OBJECT_COUNT.min(SHARD_OBJECT_MAX) {
            let object = shard_fixture_object(index);
            let home = expected_home(&object, &members)
                .ok_or_else(|| format!("no home for shard object {index}"))?;
            rows.push(ShardRow {
                object: hex(&object),
                home,
                epoch,
                digest: digest.clone(),
            });
        }
        let mut counts: BTreeMap<String, usize> = members
            .iter()
            .map(|member| (member.clone(), 0usize))
            .collect();
        for row in &rows {
            *counts
                .get_mut(&row.home)
                .ok_or_else(|| format!("home {} is not a member", row.home))? += 1;
        }
        let min = counts
            .values()
            .filter(|count| **count > 0)
            .min()
            .copied()
            .unwrap_or(0);
        let max = counts.values().max().copied().unwrap_or(0);
        Ok(Self {
            epoch,
            digest,
            members,
            rows,
            counts,
            min,
            max,
        })
    }

    fn owned(&self, node: &str) -> Vec<ShardRow> {
        self.rows
            .iter()
            .filter(|row| row.home == node)
            .cloned()
            .collect()
    }
}

fn hex(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        let _ = write!(out, "{byte:02X}");
    }
    out
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

/// Fetch a shard body, retrying while the module's HTTP slot re-arms.
fn fetch_shard(node: &Node, target: &str, timeout: Duration) -> Result<(u16, ShardTable), String> {
    let deadline = Instant::now() + timeout;
    loop {
        let error = match homes::fetch(node, target, Duration::from_secs(10)).and_then(|response| {
            ShardTable::parse(&response.body).map(|table| (response.status, table))
        }) {
            Ok(value) => return Ok(value),
            Err(error) => error,
        };
        if Instant::now() >= deadline {
            return Err(format!("{}: {error}", node.name));
        }
        std::thread::sleep(Duration::from_millis(500));
    }
}

/// The root's last logged confirmed epoch.
fn root_epoch(node: &Node) -> Option<u32> {
    read_serial(node)
        .lines()
        .rfind(|line| {
            line.contains("mesh-join: confirm ") || line.contains("mesh-join: converged role=root")
        })
        .and_then(|line| join::field_u32(line, "epoch="))
}

/// Wait until the root has confirmed at least `wanted` members.
fn wait_root_epoch(node: &Node, wanted: u32, timeout: Duration) -> Result<u32, String> {
    let deadline = Instant::now() + timeout;
    loop {
        if let Some(epoch) = root_epoch(node) {
            if epoch >= wanted {
                return Ok(epoch);
            }
        }
        if Instant::now() >= deadline {
            return Err(format!(
                "{} did not confirm epoch {wanted}; last root epoch {:?}",
                node.name,
                root_epoch(node)
            ));
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// Wait until every node has confirmed the expected epoch and member list.
fn wait_converged(
    nodes: &[Node],
    expected: &Expected,
    timeout: Duration,
) -> Result<Vec<String>, String> {
    let deadline = Instant::now() + timeout;
    loop {
        let mut lines = Vec::new();
        let mut missing = Vec::new();
        for node in nodes {
            let text = read_serial(node);
            let line = text
                .lines()
                .filter(|line| line.contains("mesh-join: converged role="))
                .filter_map(join::JoinLine::parse)
                .rfind(|line| {
                    line.epoch == expected.epoch && line.members == expected.members.len()
                });
            match line {
                Some(line) => lines.push(line.raw),
                None => missing.push(node.name.clone()),
            }
        }
        if missing.is_empty() {
            return Ok(lines);
        }
        if Instant::now() >= deadline {
            return Err(format!(
                "{} did not converge on epoch {} members {}: {}",
                missing.join(", "),
                expected.epoch,
                expected.members.len(),
                nodes
                    .iter()
                    .map(|node| {
                        let text = read_serial(node);
                        let last = text
                            .lines()
                            .rfind(|line| line.contains("mesh-join:"))
                            .unwrap_or("no mesh-join line");
                        format!("{}: {last}", node.name)
                    })
                    .collect::<Vec<_>>()
                    .join("; ")
            ));
        }
        std::thread::sleep(Duration::from_millis(500));
    }
}

fn verify_current(
    node: &Node,
    table: &ShardTable,
    status: u16,
    expected: &Expected,
    role: &str,
) -> Result<Vec<String>, String> {
    let mut checks = Vec::new();
    if status != 200 {
        return Err(format!("{}: /mesh/shard returned HTTP {status}", node.name));
    }
    if table.state != "current" {
        return Err(format!(
            "{}: /mesh/shard state {} is not current",
            node.name, table.state
        ));
    }
    if table.object_set.as_deref() != Some("m3-v1") {
        return Err(format!(
            "{}: object_set {:?} is not m3-v1",
            node.name, table.object_set
        ));
    }
    if table.role.as_deref() != Some(role) {
        return Err(format!(
            "{}: role {:?} is not {role}",
            node.name, table.role
        ));
    }
    let id = node.node_id_string();
    if table.node.as_deref() != Some(id.as_str()) {
        return Err(format!(
            "{}: node {:?} is not {}",
            node.name, table.node, id
        ));
    }
    if table.epoch != Some(expected.epoch) {
        return Err(format!(
            "{}: epoch {:?} is not {}",
            node.name, table.epoch, expected.epoch
        ));
    }
    if table.members != Some(expected.members.len()) {
        return Err(format!(
            "{}: members {:?} is not {}",
            node.name,
            table.members,
            expected.members.len()
        ));
    }
    if table.digest.as_deref() != Some(expected.digest.as_str()) {
        return Err(format!(
            "{}: digest {:?} is not {}",
            node.name, table.digest, expected.digest
        ));
    }
    if table.count != Some(expected.rows.len()) {
        return Err(format!(
            "{}: count {:?} is not {}",
            node.name,
            table.count,
            expected.rows.len()
        ));
    }
    if table.owners != Some(expected.counts.values().filter(|count| **count > 0).count()) {
        return Err(format!("{}: owners {:?} is wrong", node.name, table.owners));
    }
    let owned = expected.owned(&id);
    if table.rows != owned {
        return Err(format!(
            "{}: owned rows differ from the independent shard (want {}, got {})",
            node.name,
            owned.len(),
            table.rows.len()
        ));
    }
    if table.owned != Some(owned.len()) {
        return Err(format!(
            "{}: owned={:?} but {} rows",
            node.name,
            table.owned,
            owned.len()
        ));
    }
    if table.min != Some(expected.min) || table.max != Some(expected.max) {
        return Err(format!(
            "{}: balance min/max {:?}/{:?} is not {}/{}",
            node.name, table.min, table.max, expected.min, expected.max
        ));
    }
    let mut seen = BTreeMap::new();
    for (member, count) in &table.owner_rows {
        if seen.insert(member.clone(), *count).is_some() {
            return Err(format!("{}: duplicate owner row for {member}", node.name));
        }
    }
    if seen != expected.counts {
        return Err(format!(
            "{}: owner counts differ: want {:?}, got {:?}",
            node.name, expected.counts, seen
        ));
    }
    checks.push(format!(
        "{}: role={role} epoch={} members={} digest={} owned={} min={} max={}",
        node.name,
        expected.epoch,
        expected.members.len(),
        expected.digest,
        owned.len(),
        expected.min,
        expected.max
    ));
    Ok(checks)
}

fn verify_m2_homes(
    node: &Node,
    status: u16,
    table: &homes::HomeTable,
    expected: &Expected,
) -> Result<(), String> {
    if status != 200 || table.state != "current" {
        return Err(format!(
            "{}: M2 /mesh/homes is not current (HTTP {status}, state {})",
            node.name, table.state
        ));
    }
    if table.epoch != Some(expected.epoch)
        || table.members != Some(expected.members.len())
        || table.digest.as_deref() != Some(expected.digest.as_str())
    {
        return Err(format!(
            "{}: M2 /mesh/homes view does not match the confirmed epoch",
            node.name
        ));
    }
    let want = homes::expected_table(
        expected.epoch,
        &expected.digest,
        &expected.members,
        homes::OBJECT_COUNT,
    )?;
    if table.rows != want.rows || table.count != want.count {
        return Err(format!(
            "{}: M2 /mesh/homes rows differ from the independent table",
            node.name
        ));
    }
    Ok(())
}

fn verify_stale(
    node: &Node,
    path: &str,
    status: u16,
    table: &ShardTable,
    requested: u32,
    expected: &Expected,
) -> Result<(), String> {
    if status != 409 || table.state != "stale" {
        return Err(format!(
            "{}: {path} did not refuse a stale epoch (HTTP {status}, state {})",
            node.name, table.state
        ));
    }
    if table.requested_epoch != Some(requested)
        || table.epoch != Some(expected.epoch)
        || table.digest.as_deref() != Some(expected.digest.as_str())
        || !table.rows.is_empty()
    {
        return Err(format!(
            "{}: stale {path} body is not a clean refusal",
            node.name
        ));
    }
    Ok(())
}

fn shard_dir(options: &Options) -> PathBuf {
    options.work.join("evidence/shard")
}

/// Run the M3 first slice.
pub fn run(options: &Options) -> Result<i32, String> {
    if options.nodes > 2 && !options.hub {
        return Err(
            "the real-kernel join slice is two nodes; use --nodes 2 for the gate, \
             or --hub for the experimental multi-node link switch"
                .into(),
        );
    }
    let (module, rom_sha, module_sha) = check_prerequisites(options)?;
    let module_sha_text = crate::sha256::hex(&module_sha);
    let fixture =
        Fixture::start(module).map_err(|error| format!("start module fixture: {error}"))?;
    let nodes = derive_nodes(options)?;
    let mut members: Vec<String> = nodes.iter().map(Node::node_id_string).collect();
    members.sort();
    let first_member = (1..nodes.len())
        .find(|index| {
            crate::route::first_object_for(&members, &nodes[*index].node_id_string()).is_some()
        })
        .ok_or_else(|| "no member owns an M2 route object".to_string())?;
    let (environments, authority) = shard_environments(&nodes, SHARD_OBJECT_COUNT, first_member);
    let environment_options: Vec<Option<_>> = environments.into_iter().map(Some).collect();
    let mut running = Vec::new();
    for (node, environment) in nodes.iter().zip(environment_options.iter()) {
        crate::rig::write_nvram(node, fixture.port, &module_sha_text, environment.as_ref())?;
    }
    // Boot one kernel at a time.  The emulated serial mirror is not lossless
    // when eight guests boot and log concurrently; this is a harness pacing
    // choice, not a policy rule, and it keeps the boot facts trustworthy.
    for node in &nodes {
        match crate::rig::start_node(options, node) {
            Ok(child) => running.push(child),
            Err(error) => {
                let _ = crate::rig::stop_nodes(options, &mut running);
                return Err(error);
            }
        }
        if let Err(error) = crate::rig::wait_ci_ready(options, node, Duration::from_secs(20)) {
            let _ = crate::rig::stop_nodes(options, &mut running);
            return Err(error);
        }
        if let Err(error) = crate::rig::start_cpu(options, node) {
            let _ = crate::rig::stop_nodes(options, &mut running);
            return Err(error);
        }
        if let Err(error) = crate::rig::wait_serial(
            options,
            running.last_mut().expect("just pushed"),
            "BOOT-READY",
            options.timeout,
        ) {
            let _ = crate::rig::stop_nodes(options, &mut running);
            return Err(error);
        }
    }
    let result = run_started(
        options,
        &nodes,
        &mut running,
        &authority,
        &rom_sha,
        &module_sha_text,
        first_member,
    );
    let stop_errors = if options.keep {
        Vec::new()
    } else {
        crate::rig::stop_nodes(options, &mut running)
    };
    let code = result?;
    if !stop_errors.is_empty() {
        return Err(format!(
            "the shard evidence passed, but stopping the nodes failed: {}",
            stop_errors.join("; ")
        ));
    }
    Ok(code)
}

fn run_started(
    options: &Options,
    nodes: &[Node],
    running: &mut [RunningNode],
    authority: &AuthorityKeys,
    rom_sha: &[u8; 32],
    module_sha: &str,
    first_member: usize,
) -> Result<i32, String> {
    let facts = wait_boot(options, running)?;
    crate::wait_mesh_listen(nodes, Duration::from_secs(90));
    let expected_epoch = nodes.len() as u32;
    let expected = Expected::build(nodes, expected_epoch)?;

    // The M2 route policy still uses its own fixture; the M3 shard set is
    // evidence for the same homes, not a second placement model.
    let mut hub = None;
    let mut relay = None;
    let mut epoch;
    if nodes.len() == 2 {
        // The M2 relay is the proven two-kernel seam: one TCP connection to
        // each guest mesh forward, pumped byte for byte.
        relay = Some(
            MeshRelay::start(
                (std::net::Ipv4Addr::LOCALHOST, nodes[0].mesh_port).into(),
                (std::net::Ipv4Addr::LOCALHOST, nodes[1].mesh_port).into(),
            )
            .map_err(|error| format!("start mesh relay: {error}"))?,
        );
        epoch = wait_root_epoch(&nodes[0], 2, Duration::from_secs(30))?;
    } else {
        let mut host_hub = MeshHub::start(nodes, first_member)?;
        epoch = wait_root_epoch(&nodes[0], 2, Duration::from_secs(30))?;
        for index in (1..nodes.len()).filter(|index| *index != first_member) {
            host_hub.add_link(&nodes[index])?;
            epoch = wait_root_epoch(&nodes[0], epoch + 1, Duration::from_secs(30))?;
        }
        hub = Some(host_hub);
    }
    let converged = match wait_converged(nodes, &expected, Duration::from_secs(90)) {
        Ok(lines) => lines,
        Err(error) => {
            let detail = hub
                .as_ref()
                .map(MeshHub::summary)
                .unwrap_or_else(|| "topology=direct-two-node".into());
            return Err(format!("{error}\n{detail}"));
        }
    };
    let topology = if relay.is_some() {
        "direct-two-node"
    } else {
        "host-hub"
    };

    let mut checks = Vec::new();
    checks.push(format!(
        "membership: {} kernels converged on epoch {} digest {} ({} members); root confirmed epoch {epoch}",
        nodes.len(),
        expected.epoch,
        expected.digest,
        expected.members.len()
    ));
    checks.push(format!(
        "topology: {topology} root={} first_member={}",
        nodes[0].name, nodes[first_member].name
    ));
    if let Some(hub) = hub.as_ref() {
        checks.push(format!("hub: {}", hub.summary()));
    }

    // Fetch and verify every node's shard view.  The round trips are spaced so
    // the module's single HTTP slot re-arms between nodes.
    std::thread::sleep(Duration::from_secs(2));
    let mut node_evidence = Vec::new();
    for (index, node) in nodes.iter().enumerate() {
        std::thread::sleep(Duration::from_millis(200));
        let (status, table) = fetch_shard(node, "/mesh/shard", Duration::from_secs(60))?;
        let role = if index == 0 { "root" } else { "member" };
        checks.extend(verify_current(node, &table, status, &expected, role)?);

        let (homes_status, homes_table) =
            homes::fetch_table(node, "/mesh/homes", Duration::from_secs(30))?;
        verify_m2_homes(node, homes_status, &homes_table, &expected)?;

        let stale_epoch = expected.epoch.saturating_sub(1).max(1);
        let stale_path = format!("/mesh/shard?epoch={stale_epoch}");
        let (stale_status, stale_table) = fetch_shard(node, &stale_path, Duration::from_secs(10))?;
        verify_stale(
            node,
            &stale_path,
            stale_status,
            &stale_table,
            stale_epoch,
            &expected,
        )?;

        let homes_stale_path = format!("/mesh/homes?epoch={stale_epoch}");
        let (homes_stale_status, homes_stale_table) =
            homes::fetch_table(node, &homes_stale_path, Duration::from_secs(20))?;
        if homes_stale_status != 409
            || homes_stale_table.state != "stale"
            || !homes_stale_table.rows.is_empty()
        {
            return Err(format!(
                "{}: M2 /mesh/homes stale check is not a clean 409",
                node.name
            ));
        }

        node_evidence.push((table, homes_table, stale_table));
    }
    checks.push("shard: every node reported the current epoch and its own assignments".into());
    checks.push("stale: every node refused an older epoch with no rows".into());
    checks.push("M2: /mesh/homes remains current on every node".into());

    // One remote route across the shard boundary.  The member is the home;
    // the root is a different node that only knows the confirmed view.
    let wire_object =
        crate::route::first_object_for(&expected.members, &nodes[first_member].node_id_string())
            .ok_or_else(|| "no route object for the chosen member".to_string())?;
    std::thread::sleep(Duration::from_secs(6));
    let (command_status, command_body) =
        crate::route::command(&nodes[0], 1, Duration::from_secs(10))?;
    if command_status != 202 {
        return Err(format!(
            "route step 1 returned HTTP {command_status}: {}",
            command_body.trim()
        ));
    }
    let deadline = Instant::now() + Duration::from_secs(30);
    let (root_report, member_report) = loop {
        let root = crate::route::fetch_ready(&nodes[0], Duration::from_secs(10));
        let member = crate::route::fetch_ready(&nodes[first_member], Duration::from_secs(10));
        if let (Ok((_, root_report)), Ok((_, member_report))) = (root, member) {
            let done = root_report.results.iter().any(|row| {
                row.object == wire_object
                    && row.home == nodes[first_member].node_id_string()
                    && row.home_epoch == expected.epoch
                    && row.operation == "stat"
                    && row.attempts == 1
                    && !row.moved
            }) && member_report.completes >= 1;
            if done {
                break (root_report, member_report);
            }
        }
        if Instant::now() >= deadline {
            return Err("the remote route did not complete".into());
        }
        std::thread::sleep(Duration::from_millis(250));
    };
    let row = root_report
        .results
        .iter()
        .find(|row| row.object == wire_object)
        .ok_or_else(|| "the root report has no route result".to_string())?;
    let want_value = crate::route::route_value(&wire_object, expected.epoch, 1)
        .ok_or_else(|| "cannot compute the route value".to_string())?;
    if row.value != want_value {
        return Err(format!(
            "route value {} is not the independent value {}",
            row.value, want_value
        ));
    }
    checks.push(format!(
        "route: {} -> {} object={} home_epoch={} attempts=1 moved=0 value={}",
        nodes[0].name, nodes[first_member].name, wire_object, expected.epoch, row.value
    ));
    checks.push(format!(
        "route: member {} served the stat (completes={})",
        nodes[first_member].name, member_report.completes
    ));

    // Collect the hub counters after the route, then write the bundle.
    let hub_stats = hub.as_ref().map(MeshHub::stats).unwrap_or_default();
    let dir = shard_dir(options);
    std::fs::create_dir_all(&dir).map_err(|error| format!("create {}: {error}", dir.display()))?;
    for node in nodes {
        let _ = crate::rig::copy_if_present(
            &node.serial_log,
            &dir.join(format!("serial-{}.log", node.name)),
        );
    }
    let commit = git(&options.repo, &["rev-parse", "HEAD"]);
    let branch = git(&options.repo, &["rev-parse", "--abbrev-ref", "HEAD"]);
    let mut json = String::new();
    let _ = writeln!(json, "{{");
    let _ = writeln!(json, "  \"schema\": \"irix7-m3-shard-1\",");
    let _ = writeln!(json, "  \"mode\": \"m3-first-shard-slice\",");
    let _ = writeln!(json, "  \"topology\": \"{}\",", topology);
    let _ = writeln!(json, "  \"commit\": \"{}\",", json_escape(&commit));
    let _ = writeln!(json, "  \"branch\": \"{}\",", json_escape(&branch));
    let _ = writeln!(json, "  \"nodes\": {},", nodes.len());
    let _ = writeln!(json, "  \"epoch\": {},", expected.epoch);
    let _ = writeln!(json, "  \"digest\": \"{}\",", expected.digest);
    let _ = writeln!(json, "  \"object_set\": \"m3-v1\",");
    let _ = writeln!(json, "  \"object_count\": {},", expected.rows.len());
    let _ = writeln!(json, "  \"real_kernel_nodes\": {},", nodes.len());
    let _ = writeln!(json, "  \"host_policy_nodes\": [1, 2, 8, 128],");
    let _ = writeln!(
        json,
        "  \"real_kernel_limit\": \"two joined kernels; the emulated smoltcp receive path truncated multi-member frames at the root under the host hub, so the eight-node scale is the host policy gate\","
    );
    let _ = writeln!(
        json,
        "  \"host_policy\": \"cargo test -p mesh-core -p mesh-join -p mesh-sim\","
    );
    let _ = writeln!(
        json,
        "  \"rom_sha256\": \"{}\",",
        crate::sha256::hex(rom_sha)
    );
    let _ = writeln!(json, "  \"module_sha256\": \"{}\",", module_sha);
    let _ = writeln!(
        json,
        "  \"authority_public\": \"{}\",",
        authority.public_hex
    );
    let _ = writeln!(json, "  \"checks\": [");
    for (index, check) in checks.iter().enumerate() {
        let comma = if index + 1 == checks.len() { "" } else { "," };
        let _ = writeln!(json, "    \"{}\"{}", json_escape(check), comma);
    }
    let _ = writeln!(json, "  ],");
    let _ = writeln!(json, "  \"nodes_detail\": [");
    for (index, (node, fact)) in nodes.iter().zip(facts.iter()).enumerate() {
        let (table, homes_table, stale_table) = &node_evidence[index];
        let comma = if index + 1 == nodes.len() { "" } else { "," };
        let _ = writeln!(json, "    {{");
        let _ = writeln!(json, "      \"name\": \"{}\",", node.name);
        let _ = writeln!(json, "      \"instance\": {},", node.instance);
        let _ = writeln!(json, "      \"mac\": \"{}\",", node.mac_string());
        let _ = writeln!(json, "      \"node_id\": \"{}\",", node.node_id_string());
        let _ = writeln!(json, "      \"http_port\": {},", node.http_port);
        let _ = writeln!(json, "      \"mesh_port\": {},", node.mesh_port);
        let _ = writeln!(
            json,
            "      \"boot_ready\": \"{}\",",
            json_escape(fact.boot_ready.as_deref().unwrap_or(""))
        );
        let _ = writeln!(
            json,
            "      \"shard\": \"{}\",",
            json_escape(&table.header())
        );
        let _ = writeln!(
            json,
            "      \"homes\": \"{}\",",
            json_escape(homes_table.raw.lines().next().unwrap_or(""))
        );
        let _ = writeln!(
            json,
            "      \"stale\": \"{}\",",
            json_escape(&stale_table.header())
        );
        let _ = writeln!(json, "      \"owned\": {}", table.rows.len());
        let _ = writeln!(json, "    }}{comma}");
    }
    let _ = writeln!(json, "  ],");
    let _ = writeln!(
        json,
        "  \"converged_lines\": [{}],",
        converged
            .iter()
            .map(|line| format!("\"{}\"", json_escape(line)))
            .collect::<Vec<_>>()
            .join(", ")
    );
    let _ = writeln!(json, "  \"route\": {{");
    let _ = writeln!(json, "    \"object\": \"{}\",", wire_object);
    let _ = writeln!(json, "    \"root\": \"{}\",", nodes[0].node_id_string());
    let _ = writeln!(
        json,
        "    \"home\": \"{}\",",
        nodes[first_member].node_id_string()
    );
    let _ = writeln!(
        json,
        "    \"command\": \"{}\",",
        json_escape(command_body.trim())
    );
    let _ = writeln!(
        json,
        "    \"root_report\": \"{}\",",
        json_escape(&root_report.raw)
    );
    let _ = writeln!(
        json,
        "    \"member_report\": \"{}\"",
        json_escape(&member_report.raw)
    );
    let _ = writeln!(json, "  }},");
    let _ = writeln!(json, "  \"hub\": {{");
    let _ = writeln!(json, "    \"frames\": {},", hub_stats.frames);
    let _ = writeln!(json, "    \"bytes\": {},", hub_stats.bytes);
    let _ = writeln!(json, "    \"forwarded\": {},", hub_stats.forwarded);
    let _ = writeln!(json, "    \"self_drops\": {},", hub_stats.self_drops);
    let _ = writeln!(json, "    \"unknown\": {},", hub_stats.unknown);
    let _ = writeln!(json, "    \"malformed\": {},", hub_stats.malformed);
    let _ = writeln!(json, "    \"sources\": [");
    for (index, (name, stats)) in hub_stats.per_source.iter().enumerate() {
        let comma = if index + 1 == hub_stats.per_source.len() {
            ""
        } else {
            ","
        };
        let _ = writeln!(
            json,
            "      {{\"name\": \"{}\", \"received\": {}, \"forwarded\": {}, \"bytes\": {}}}{comma}",
            json_escape(name),
            stats.received,
            stats.forwarded,
            stats.bytes
        );
    }
    let _ = writeln!(json, "    ]");
    let _ = writeln!(json, "  }},");
    let _ = writeln!(json, "  \"verdict\": \"pass\"");
    let _ = writeln!(json, "}}");
    write_text(&dir.join("run.json"), &json)?;

    let mut summary = String::new();
    let _ = writeln!(summary, "# M3 first slice — {} kernels\n", nodes.len());
    let _ = writeln!(
        summary,
        "**Evidence:** `run.json`, `serial-n*.log`.  Branch `{}` at `{}`.\n",
        branch, commit
    );
    let _ = writeln!(
        summary,
        "All {} kernels confirmed epoch **{}** with membership digest **{}**.  \
         The `/mesh/shard` surface returned current, deterministic assignments on every \
         node, each node reported only its own rows, and every stale `epoch=` was a \
         `409` with no rows.  The M2 `/mesh/homes` surface stayed current.  One \
         capability stat crossed from the root to `{}` and was served by the member.\n",
        nodes.len(),
        expected.epoch,
        expected.digest,
        nodes[first_member].name
    );
    let _ = writeln!(summary, "## Checks\n");
    for check in &checks {
        let _ = writeln!(summary, "- {check}");
    }
    let _ = writeln!(summary, "\n## Scope and remaining M3 work\n");
    let _ = writeln!(
        summary,
        "- **Real-kernel limit: two joined kernels.**  The 1/2/8/128-node shard policy \
         gate is host-side (`crates/mesh-sim/tests/shard.rs`, `crates/mesh-join`).  Eight \
         iris kernels were launched and booted during the slice, but the emulated smoltcp \
         receive path truncated multi-member frames at the root when a host link switch \
         multiplexed the members onto its single module TCP slot.  The eight-node gate is \
         therefore policy-only; it is not claimed as real-kernel convergence."
    );
    let _ = writeln!(
        summary,
        "- The host hub remains an explicitly experimental `--hub` path.  It is a link \
         switch, not a kernel or module component and not an object directory."
    );
    let _ = writeln!(
        summary,
        "- This is the first shard slice: no GPU placement, two-node simultaneous failure, \
         full migration churn, persistent directory, CAS fetch-by-OID or whole-M3 gate."
    );
    let _ = writeln!(
        summary,
        "- Membership churn, duplicate/stale epochs, shard balance and 128-node policy are \
         host tests; they are not claimed as real-kernel churn."
    );
    write_text(&dir.join("summary.md"), &summary)?;

    println!("{}", summary);
    println!("evidence: {}", dir.display());
    Ok(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(byte: u8) -> String {
        format!("{byte:02X}").repeat(16)
    }

    #[test]
    fn shard_body_parses_owned_rows_and_counts() {
        let body = "\
mesh-shard: state=current object_set=m3-v1 role=member node=02020202020202020202020202020202 epoch=8 members=8 digest=0123456789ABCDEF0123456789ABCDEF count=128 owners=8 owned=16 min=12 max=20
mesh-shard-owner: node=01010101010101010101010101010101 count=15
mesh-shard-owner: node=02020202020202020202020202020202 count=16
mesh-shard-owned: object=AA unexpected=1 home=02020202020202020202020202020202 epoch=8 digest=0123456789ABCDEF0123456789ABCDEF
";
        // The parser rejects an unknown token rather than guessing.
        assert!(ShardTable::parse(body).is_err());

        let body = "\
mesh-shard: state=current object_set=m3-v1 role=member node=02020202020202020202020202020202 epoch=8 members=8 digest=0123456789ABCDEF0123456789ABCDEF count=128 owners=2 owned=1 min=1 max=1
mesh-shard-owner: node=01010101010101010101010101010101 count=1
mesh-shard-owner: node=02020202020202020202020202020202 count=1
mesh-shard-owned: object=AA home=02020202020202020202020202020202 epoch=8 digest=0123456789ABCDEF0123456789ABCDEF
";
        let table = ShardTable::parse(body).expect("shard body");
        assert_eq!(table.state, "current");
        assert_eq!(table.owned, Some(1));
        assert_eq!(table.owner_rows.len(), 2);
        assert_eq!(table.rows.len(), 1);
        assert_eq!(table.rows[0].home, id(2));
        assert_eq!(table.header(), "mesh-shard: state=current object_set=m3-v1 role=member node=02020202020202020202020202020202 epoch=8 members=8 digest=0123456789ABCDEF0123456789ABCDEF count=128 owners=2 owned=1 min=1 max=1");
    }

    #[test]
    fn stale_and_pending_bodies_have_no_rows() {
        let stale = "mesh-shard: state=stale requested_epoch=7 epoch=8 members=8 digest=AA count=0 owned=0\n";
        let table = ShardTable::parse(stale).expect("stale");
        assert_eq!(table.state, "stale");
        assert_eq!(table.requested_epoch, Some(7));
        assert!(table.rows.is_empty());

        let pending = "mesh-shard: state=pending epoch=1 members=1 digest=BB count=0 owned=0\n";
        let table = ShardTable::parse(pending).expect("pending");
        assert_eq!(table.state, "pending");
        assert!(table.rows.is_empty());
    }

    #[test]
    fn the_fixture_and_expected_homes_are_independent_of_order() {
        let members = vec![id(1), id(2), id(3)];
        let mut reversed = members.clone();
        reversed.reverse();
        for index in 0..8 {
            let object = shard_fixture_object(index);
            assert_eq!(
                expected_home(&object, &members),
                expected_home(&object, &reversed)
            );
        }
        assert_ne!(shard_fixture_object(0), homes::fixture_object(0));
    }
}
