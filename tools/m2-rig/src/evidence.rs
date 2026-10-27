// SPDX-License-Identifier: GPL-3.0-only
//! The evidence bundle.
//!
//! Every run writes a self-contained directory: the two serial transcripts,
//! the guest MCP fetch log, the relay log, the probe transcripts, `run.json`
//! and a human-readable `summary.md`.  A reviewer can inspect the bundle
//! without the live processes or the host's shell history.

use std::path::{Path, PathBuf};

use crate::fixture::Fixture;
use crate::homes::HomesEvidence;
use crate::join::JoinEvidence;
use crate::migrate::MigrateEvidence;
use crate::nvram;
use crate::relay::{lines as log_lines, SharedLog};
use crate::rig::{copy_if_present, write_text, BootFacts, Node};
use crate::route::RouteEvidence;

/// One node as recorded in the bundle.
#[derive(Clone, Debug)]
pub struct NodeEvidence {
    /// `n0` or `n1`.
    pub name: String,
    /// iris instance id.
    pub instance: u32,
    /// State directory.
    pub state_dir: PathBuf,
    /// CI socket.
    pub ci_socket: PathBuf,
    /// Serial transcript.
    pub serial_log: PathBuf,
    /// Monitor port.
    pub monitor: u16,
    /// MAC text.
    pub mac: String,
    /// NodeId text.
    pub node_id: String,
    /// NAT CIDR.
    pub nat_subnet: String,
    /// Guest address.
    pub guest: String,
    /// Gateway address.
    pub gateway: String,
    /// Host telnet forward.
    pub telnet_port: u16,
    /// Host MCP forward.
    pub http_port: u16,
    /// Host mesh forward.
    pub mesh_port: u16,
    /// Boot facts.
    pub facts: BootFacts,
    /// The DS1386 environment the guest booted with.
    pub nvram_env: Vec<(String, String)>,
}

impl NodeEvidence {
    /// Capture a node and its boot facts.
    pub fn capture(node: &Node, facts: BootFacts) -> Self {
        let mut nvram_env = std::fs::read(node.state_dir.join("nvram.bin"))
            .map(|image| nvram::parse(&image))
            .unwrap_or_default();
        // The authority seed is a lab fixture, not production key material,
        // but a published evidence bundle should not carry a signing key.
        for (name, value) in &mut nvram_env {
            if name == "mesh.key" {
                *value = "<redacted lab seed>".into();
            }
        }
        Self {
            name: node.name.clone(),
            instance: node.instance,
            state_dir: node.state_dir.clone(),
            ci_socket: node.ci_socket.clone(),
            serial_log: node.serial_log.clone(),
            monitor: node.monitor,
            mac: node.mac_string(),
            node_id: node.node_id_string(),
            nat_subnet: node.nat_subnet.clone(),
            guest: node.guest.to_string(),
            gateway: node.gateway.to_string(),
            telnet_port: node.telnet_port,
            http_port: node.http_port,
            mesh_port: node.mesh_port,
            facts,
            nvram_env,
        }
    }
}

/// One direction of the two-way probe.
#[derive(Clone, Debug)]
pub struct ProbeEvidence {
    /// Source node.
    pub from: String,
    /// Target node.
    pub to: String,
    /// URL the source guest fetched.
    pub url: String,
    /// Relay mapping.
    pub relay: String,
    /// Whether the probe passed.
    pub ok: bool,
    /// Human-readable result or error.
    pub message: String,
    /// HTTP status.
    pub status: Option<u16>,
    /// Body length.
    pub bytes: Option<u64>,
    /// Body digest.
    pub sha256: Option<String>,
    /// Printable transcript.
    pub text: String,
    /// Raw transcript.
    pub raw: Vec<u8>,
}

/// One node's mesh transport probe result.
#[derive(Clone, Debug)]
pub struct MeshNodeProbe {
    /// `n0` or `n1`.
    pub name: String,
    /// The `mesh-transport: listening` line, if seen.
    pub listen: Option<String>,
    /// The `mesh-transport: pass` line, if seen.
    pub pass: Option<String>,
}

/// The two-node mesh framing probe.
#[derive(Clone, Debug)]
pub struct MeshProbeEvidence {
    /// Whether both nodes decoded a frame from the other and passed.
    pub ok: bool,
    /// Human-readable result or failure.
    pub message: String,
    /// The host forward pair the relay connected to.
    pub relay: String,
    /// Per-node transport lines.
    pub nodes: Vec<MeshNodeProbe>,
}

/// Everything the bundle needs.
pub struct Evidence<'a> {
    /// Rig work directory.
    pub work: &'a Path,
    /// Action name (`run` or `probe`).
    pub action: &'a str,
    /// Repository root.
    pub repo: &'a Path,
    /// Branch name.
    pub branch: &'a str,
    /// Commit.
    pub commit: &'a str,
    /// `git describe --always --dirty --tags`.
    pub describe: &'a str,
    /// Whether the working tree was dirty when the run was recorded.
    pub dirty: bool,
    /// PROM path.
    pub rom: &'a Path,
    /// PROM digest.
    pub rom_sha256: &'a str,
    /// PROM size in bytes.
    pub rom_size: u64,
    /// Module path.
    pub module: &'a Path,
    /// Module digest.
    pub module_sha256: &'a str,
    /// Module size in bytes.
    pub module_size: u64,
    /// `iris` binary path.
    pub iris: &'a Path,
    /// `iris` binary digest.
    pub iris_sha256: &'a str,
    /// `iris` binary size.
    pub iris_size: u64,
    /// `iris-ci` binary path.
    pub iris_ci: &'a Path,
    /// `iris-ci` binary digest.
    pub iris_ci_sha256: &'a str,
    /// `iris-ci` binary size.
    pub iris_ci_size: u64,
    /// Node facts.
    pub nodes: &'a [NodeEvidence],
    /// Probe results.
    pub probes: &'a [ProbeEvidence],
    /// Mesh transport probe, when one ran.
    pub mesh: Option<&'a MeshProbeEvidence>,
    /// Join and membership-epoch evidence, when one ran.
    pub join: Option<&'a JoinEvidence>,
    /// Rendezvous-home evidence, when one ran.
    pub homes: Option<&'a HomesEvidence>,
    /// Remote capability-route evidence, when one ran.
    pub route: Option<&'a RouteEvidence>,
    /// L0 actor-migration evidence, when that scenario ran.
    pub migration: Option<&'a MigrateEvidence>,
    /// Bulk-transfer evidence, when that scenario ran.
    pub bulk: Option<&'a crate::bulk::BulkEvidence>,
    /// Failure/partition evidence, when that fault mode ran.
    pub failure: Option<&'a crate::failure::FailureEvidence>,
    /// Module fixture, when the run started one.
    pub fixture: Option<&'a Fixture>,
    /// Named relay logs.
    pub relays: &'a [(String, SharedLog)],
    /// `pass` or `fail`.
    pub verdict: &'a str,
    /// Top-level error, if any.
    pub error: Option<&'a str>,
}

/// Write the bundle and return its directory.
pub fn write(evidence: &Evidence<'_>) -> Result<PathBuf, String> {
    let directory = evidence.work.join("evidence");
    // A bundle is owned by exactly one run: never leave a stale relay log or
    // first-boot transcript from a previous mode in the directory.
    if directory.exists() {
        std::fs::remove_dir_all(&directory)
            .map_err(|error| format!("clear {}: {error}", directory.display()))?;
    }
    std::fs::create_dir_all(&directory)
        .map_err(|error| format!("create {}: {error}", directory.display()))?;

    for node in evidence.nodes {
        copy_if_present(
            &node.serial_log,
            &directory.join(format!("serial-{}.log", node.name)),
        )
        .map_err(|error| format!("copy serial log: {error}"))?;
        copy_if_present(
            &node.state_dir.join("stdout"),
            &directory.join(format!("stdout-{}.log", node.name)),
        )
        .map_err(|error| format!("copy stdout: {error}"))?;
        copy_if_present(
            &node.state_dir.join("stderr"),
            &directory.join(format!("stderr-{}.log", node.name)),
        )
        .map_err(|error| format!("copy stderr: {error}"))?;
    }

    for probe in evidence.probes {
        let stem = format!("probe-{}-to-{}", probe.from, probe.to);
        write_text(&directory.join(format!("{stem}.txt")), &probe.text)?;
        std::fs::write(directory.join(format!("{stem}.raw")), &probe.raw)
            .map_err(|error| format!("write {stem}.raw: {error}"))?;
    }

    if let Some(fixture) = evidence.fixture {
        let fixture_log = fixture.lines().join("\n");
        write_text(&directory.join("fixture.log"), &format!("{fixture_log}\n"))?;
    }
    for (name, relay) in evidence.relays {
        let text = log_lines(relay).join("\n");
        write_text(
            &directory.join(format!("relay-{name}.log")),
            &format!("{text}\n"),
        )?;
    }
    if let Some(restart) = evidence.join.and_then(|join| join.restart.as_ref()) {
        copy_if_present(
            &restart.boot1_log,
            &directory.join(format!("serial-{}-boot1.log", restart.node)),
        )
        .map_err(|error| format!("copy first-boot serial log: {error}"))?;
    }
    if let Some(restart) = evidence.join.and_then(|join| join.root_restart.as_ref()) {
        copy_if_present(
            &restart.boot1_log,
            &directory.join(format!("serial-{}-boot1.log", restart.node)),
        )
        .map_err(|error| format!("copy root first-boot serial log: {error}"))?;
    }

    let json = render_json(evidence);
    write_text(&directory.join("run.json"), &json)?;
    let summary = render_summary(evidence);
    write_text(&directory.join("summary.md"), &summary)?;
    let manifest = render_manifest(evidence);
    write_text(&directory.join("manifest.json"), &manifest)?;
    Ok(directory)
}

/// A deterministic source/artefact manifest.
///
/// No timestamp or host name is included, so two clean runs of the same tree
/// with the same toolchain produce the same manifest.  The dirty flag and the
/// binary digests make an uncommitted or locally patched run visible instead
/// of silently attributing it to `HEAD`.
fn render_manifest(evidence: &Evidence<'_>) -> String {
    format!(
        "{{\n  \"schema\": \"irix7-m2-artifact-manifest-v1\",\n  \"repo\": \"{}\",\n  \"branch\": \"{}\",\n  \"commit\": \"{}\",\n  \"describe\": \"{}\",\n  \"dirty\": {},\n  \"artifacts\": {{\n    \"rom\": {{\"path\": \"{}\", \"size\": {}, \"sha256\": \"{}\"}},\n    \"module\": {{\"path\": \"{}\", \"size\": {}, \"sha256\": \"{}\"}},\n    \"iris\": {{\"path\": \"{}\", \"size\": {}, \"sha256\": \"{}\"}},\n    \"iris_ci\": {{\"path\": \"{}\", \"size\": {}, \"sha256\": \"{}\"}}\n  }}\n}}\n",
        json_escape(&evidence.repo.display().to_string()),
        json_escape(evidence.branch),
        json_escape(evidence.commit),
        json_escape(evidence.describe),
        evidence.dirty,
        json_escape(&evidence.rom.display().to_string()),
        evidence.rom_size,
        json_escape(evidence.rom_sha256),
        json_escape(&evidence.module.display().to_string()),
        evidence.module_size,
        json_escape(evidence.module_sha256),
        json_escape(&evidence.iris.display().to_string()),
        evidence.iris_size,
        json_escape(evidence.iris_sha256),
        json_escape(&evidence.iris_ci.display().to_string()),
        evidence.iris_ci_size,
        json_escape(evidence.iris_ci_sha256)
    )
}

fn render_json(evidence: &Evidence<'_>) -> String {
    let mut out = String::new();
    out.push_str("{\n");
    out.push_str(&format!(
        "  \"action\": \"{}\",\n",
        json_escape(evidence.action)
    ));
    out.push_str(&format!(
        "  \"verdict\": \"{}\",\n",
        json_escape(evidence.verdict)
    ));
    if let Some(error) = evidence.error {
        out.push_str(&format!("  \"error\": \"{}\",\n", json_escape(error)));
    }
    out.push_str(&format!(
        "  \"repo\": \"{}\",\n  \"branch\": \"{}\",\n  \"commit\": \"{}\",\n  \"describe\": \"{}\",\n  \"dirty\": {},\n",
        json_escape(&evidence.repo.display().to_string()),
        json_escape(evidence.branch),
        json_escape(evidence.commit),
        json_escape(evidence.describe),
        evidence.dirty
    ));
    out.push_str(&format!(
        "  \"rom\": {{\"path\": \"{}\", \"size\": {}, \"sha256\": \"{}\"}},\n",
        json_escape(&evidence.rom.display().to_string()),
        evidence.rom_size,
        json_escape(evidence.rom_sha256)
    ));
    out.push_str(&format!(
        "  \"module\": {{\"path\": \"{}\", \"size\": {}, \"sha256\": \"{}\"}},\n",
        json_escape(&evidence.module.display().to_string()),
        evidence.module_size,
        json_escape(evidence.module_sha256)
    ));
    out.push_str(&format!(
        "  \"iris\": {{\"path\": \"{}\", \"size\": {}, \"sha256\": \"{}\"}},\n",
        json_escape(&evidence.iris.display().to_string()),
        evidence.iris_size,
        json_escape(evidence.iris_sha256)
    ));
    out.push_str(&format!(
        "  \"iris_ci\": {{\"path\": \"{}\", \"size\": {}, \"sha256\": \"{}\"}},\n",
        json_escape(&evidence.iris_ci.display().to_string()),
        evidence.iris_ci_size,
        json_escape(evidence.iris_ci_sha256)
    ));
    out.push_str("  \"nodes\": [\n");
    for (index, node) in evidence.nodes.iter().enumerate() {
        out.push_str("    {");
        out.push_str(&format!(
            "\"name\": \"{}\", \"instance\": {}, \"state_dir\": \"{}\", \"ci_socket\": \"{}\", \"serial_log\": \"{}\", ",
            json_escape(&node.name),
            node.instance,
            json_escape(&node.state_dir.display().to_string()),
            json_escape(&node.ci_socket.display().to_string()),
            json_escape(&node.serial_log.display().to_string())
        ));
        out.push_str(&format!(
            "\"monitor\": {}, \"mac\": \"{}\", \"node_id\": \"{}\", \"nat_subnet\": \"{}\", \"guest\": \"{}\", \"gateway\": \"{}\", ",
            node.monitor,
            json_escape(&node.mac),
            json_escape(&node.node_id),
            json_escape(&node.nat_subnet),
            json_escape(&node.guest),
            json_escape(&node.gateway)
        ));
        out.push_str(&format!(
            "\"telnet_port\": {}, \"http_port\": {}, \"mesh_port\": {}, ",
            node.telnet_port, node.http_port, node.mesh_port
        ));
        out.push_str(&format!(
            "\"boot_ready\": {}, \"mcp\": {}, \"mesh\": {}, \"mesh_transport_listen\": {}, \"mesh_transport_pass\": {}, \"mesh_transport_disconnects\": {}, \"mesh_join_enabled\": {}, \"mesh_join_disabled\": {}, \"mesh_join_local\": {}, \"mesh_join_request\": {}, \"mesh_join_confirms\": {}, \"mesh_join_converged\": {}, \"mesh_join_acks\": {}, \"mesh_join_acks_sent\": {}, \"mesh_join_duplicate\": {}, \"mesh_join_refused\": {}, \"mesh_join_timeout\": {}, \"mesh_join_reject\": {}, \"mesh_route_enabled\": {}, \"mesh_route_disabled\": {}, \"mesh_migrate_enabled\": {}, \"mesh_migrate_disabled\": {}, \"mesh_migrate_lines\": {}, \"network_mac\": {}, \"network_ip\": {}, \"nvram\": {}",
            json_option(&node.facts.boot_ready),
            json_option(&node.facts.mcp),
            json_option(&node.facts.mesh),
            json_option(&node.facts.mesh_transport_listen),
            json_option(&node.facts.mesh_transport_pass),
            json_strings(&node.facts.mesh_transport_disconnects),
            json_option(&node.facts.mesh_join_enabled),
            json_option(&node.facts.mesh_join_disabled),
            json_option(&node.facts.mesh_join_local),
            json_option(&node.facts.mesh_join_request),
            json_strings(&node.facts.mesh_join_confirms),
            json_option(&node.facts.mesh_join_converged),
            json_strings(&node.facts.mesh_join_acks),
            json_strings(&node.facts.mesh_join_acks_sent),
            json_option(&node.facts.mesh_join_duplicate),
            json_option(&node.facts.mesh_join_refused),
            json_option(&node.facts.mesh_join_timeout),
            json_option(&node.facts.mesh_join_reject),
            json_option(&node.facts.mesh_route_enabled),
            json_option(&node.facts.mesh_route_disabled),
            json_option(&node.facts.mesh_migrate_enabled),
            json_option(&node.facts.mesh_migrate_disabled),
            json_strings(&node.facts.mesh_migrate_lines),
            json_option(&node.facts.network_mac),
            json_option(&node.facts.network_ip),
            json_env(&node.nvram_env)
        ));
        out.push('}');
        if index + 1 < evidence.nodes.len() {
            out.push(',');
        }
        out.push('\n');
    }
    out.push_str("  ],\n  \"probes\": [\n");
    for (index, probe) in evidence.probes.iter().enumerate() {
        out.push_str(&format!(
            "    {{\"from\": \"{}\", \"to\": \"{}\", \"url\": \"{}\", \"relay\": \"{}\", \"ok\": {}, \"message\": \"{}\", \"status\": {}, \"bytes\": {}, \"sha256\": {}, \"transcript\": \"probe-{}-to-{}.txt\"}}",
            json_escape(&probe.from),
            json_escape(&probe.to),
            json_escape(&probe.url),
            json_escape(&probe.relay),
            probe.ok,
            json_escape(&probe.message),
            probe.status.map(|value| value.to_string()).unwrap_or_else(|| "null".into()),
            probe
                .bytes
                .map(|value| value.to_string())
                .unwrap_or_else(|| "null".into()),
            probe
                .sha256
                .as_deref()
                .map(|value| format!("\"{}\"", json_escape(value)))
                .unwrap_or_else(|| "null".into()),
            json_escape(&probe.from),
            json_escape(&probe.to)
        ));
        if index + 1 < evidence.probes.len() {
            out.push(',');
        }
        out.push('\n');
    }
    out.push_str("  ],\n  \"mesh\": ");
    match evidence.mesh {
        Some(mesh) => {
            out.push_str(&format!(
                "{{\"ok\": {}, \"message\": \"{}\", \"relay\": \"{}\", \"nodes\": [",
                mesh.ok,
                json_escape(&mesh.message),
                json_escape(&mesh.relay)
            ));
            for (index, node) in mesh.nodes.iter().enumerate() {
                out.push_str(&format!(
                    "{{\"name\": \"{}\", \"listen\": {}, \"pass\": {}}}",
                    json_escape(&node.name),
                    json_option(&node.listen),
                    json_option(&node.pass)
                ));
                if index + 1 < mesh.nodes.len() {
                    out.push(',');
                }
            }
            out.push_str("]}");
        }
        None => out.push_str("null"),
    }
    out.push_str(",\n  \"join\": ");
    match evidence.join {
        Some(join) => out.push_str(&render_join_json(join)),
        None => out.push_str("null"),
    }
    out.push_str(",\n  \"homes\": ");
    match evidence.homes {
        Some(homes) => out.push_str(&render_homes_json(homes)),
        None => out.push_str("null"),
    }
    out.push_str(",\n  \"route\": ");
    match evidence.route {
        Some(route) => out.push_str(&render_route_json(route)),
        None => out.push_str("null"),
    }
    out.push_str(",\n  \"migration\": ");
    match evidence.migration {
        Some(migration) => out.push_str(&render_migration_json(migration)),
        None => out.push_str("null"),
    }
    out.push_str(",\n  \"bulk\": ");
    match evidence.bulk {
        Some(bulk) => out.push_str(&render_bulk_json(bulk)),
        None => out.push_str("null"),
    }
    out.push_str(",\n  \"failure\": ");
    match evidence.failure {
        Some(failure) => out.push_str(&render_failure_json(failure)),
        None => out.push_str("null"),
    }
    out.push_str("\n}\n");
    out
}

fn render_expected_view(view: &Option<crate::failure::ExpectedView>) -> String {
    let Some(view) = view else {
        return "null".into();
    };
    let mut out = String::new();
    out.push_str(&format!(
        "{{\"epoch\": {}, \"digest\": \"{}\", \"members\": {}, \"homes\": [",
        view.epoch,
        json_escape(&view.digest),
        json_strings(&view.members)
    ));
    for (index, row) in view.table.rows.iter().enumerate() {
        out.push_str(&format!(
            "{{\"object\": \"{}\", \"home\": \"{}\"}}",
            json_escape(&row.object),
            json_escape(&row.home)
        ));
        if index + 1 < view.table.rows.len() {
            out.push(',');
        }
    }
    out.push_str("]}");
    out
}

fn render_bulk_json(bulk: &crate::bulk::BulkEvidence) -> String {
    let mut out = String::new();
    out.push_str(&format!(
        "{{\"ok\": {}, \"message\": \"{}\", ",
        bulk.ok,
        json_escape(&bulk.message)
    ));
    out.push_str("\"baseline\": ");
    match &bulk.baseline {
        Some(run) => out.push_str(&format!(
            "{{\"from\": \"{}\", \"to\": \"{}\", \"bytes\": {}, \"chunk\": {}, \"chunks\": {}, \"elapsed_ms\": {}, \"mib_per_s\": {:.4}, \"raw\": \"{}\"}}",
            json_escape(&run.from),
            json_escape(&run.to),
            run.bytes,
            run.chunk,
            run.chunks,
            run.elapsed_ms,
            run.mb_per_s,
            json_escape(&run.raw)
        )),
        None => out.push_str("null"),
    }
    out.push_str(", \"runs\": [");
    for (index, run) in bulk.runs.iter().enumerate() {
        out.push_str(&format!(
            "{{\"from\": \"{}\", \"to\": \"{}\", \"bytes\": {}, \"chunk\": {}, \"chunks\": {}, \"elapsed_ms\": {}, \"mib_per_s\": {:.4}, \"raw\": \"{}\"}}",
            json_escape(&run.from),
            json_escape(&run.to),
            run.bytes,
            run.chunk,
            run.chunks,
            run.elapsed_ms,
            run.mb_per_s,
            json_escape(&run.raw)
        ));
        if index + 1 < bulk.runs.len() {
            out.push(',');
        }
    }
    out.push_str("]}");
    out
}

fn render_migration_json(migration: &MigrateEvidence) -> String {
    let mut out = String::new();
    out.push('{');
    out.push_str(&format!(
        "\"ok\": {}, \"fault\": \"{}\", \"actor\": {}, \"home\": {}, \"boundary_hash\": {}, \"return_hash\": {}, \"message\": \"{}\", ",
        migration.ok,
        json_escape(&migration.fault),
        json_option(&migration.actor),
        json_option(&migration.home),
        json_option(&migration.boundary_hash),
        json_option(&migration.return_hash),
        json_escape(&migration.message)
    ));
    out.push_str(&format!("\"steps\": {}, ", json_strings(&migration.steps)));
    out.push_str(&format!(
        "\"checks\": {}, ",
        json_strings(&migration.checks)
    ));
    out.push_str("\"nodes\": [");
    for (index, node) in migration.nodes.iter().enumerate() {
        out.push_str(&format!(
            "{{\"name\": \"{}\", \"status\": {}, \"report\": {}, \"error\": {}}}",
            json_escape(&node.name),
            node.status
                .map(|value| value.to_string())
                .unwrap_or_else(|| "null".into()),
            node.report
                .as_ref()
                .map(|report| format!("\"{}\"", json_escape(&report.raw)))
                .unwrap_or_else(|| "null".into()),
            json_option(&node.error)
        ));
        if index + 1 < migration.nodes.len() {
            out.push(',');
        }
    }
    out.push_str("]}");
    out
}

fn render_failure_json(failure: &crate::failure::FailureEvidence) -> String {
    let mut out = String::new();
    out.push('{');
    out.push_str(&format!(
        "\"fault\": \"{}\", \"stage\": \"{}\", \"ok\": {}, \"message\": \"{}\", ",
        json_escape(&failure.fault),
        json_escape(&failure.stage),
        failure.ok,
        json_escape(&failure.message)
    ));
    out.push_str(&format!(
        "\"before\": {}, ",
        render_expected_view(&failure.before)
    ));
    out.push_str(&format!(
        "\"after\": {}, ",
        render_expected_view(&failure.after)
    ));
    out.push_str(&format!(
        "\"recovered\": {}, ",
        render_expected_view(&failure.recovered)
    ));
    out.push_str(&format!("\"serial\": {}, ", json_strings(&failure.serial)));
    out.push_str(&format!("\"steps\": {}, ", json_strings(&failure.steps)));
    out.push_str(&format!("\"checks\": {}, ", json_strings(&failure.checks)));
    out.push_str("\"nodes\": [");
    for (index, node) in failure.nodes.iter().enumerate() {
        out.push_str(&format!(
            "{{\"name\": \"{}\", \"report\": {}, \"error\": {}}}",
            json_escape(&node.name),
            node.report
                .as_ref()
                .map(|report| format!("\"{}\"", json_escape(&report.raw)))
                .unwrap_or_else(|| "null".into()),
            json_option(&node.error)
        ));
        if index + 1 < failure.nodes.len() {
            out.push(',');
        }
    }
    out.push_str("]}");
    out
}

fn render_homes_json(homes: &HomesEvidence) -> String {
    let mut out = String::new();
    out.push('{');
    out.push_str(&format!(
        "\"ok\": {}, \"object_set\": \"{}\", \"count\": {}, \"message\": \"{}\", ",
        homes.ok,
        json_escape(&homes.object_set),
        homes.count,
        json_escape(&homes.message)
    ));
    for (index, (name, round)) in [
        ("before", homes.before.as_ref()),
        ("after", homes.after.as_ref()),
        ("stale", homes.stale.as_ref()),
        ("disabled", homes.disabled.as_ref()),
        ("wrong_authority", homes.wrong_authority.as_ref()),
    ]
    .into_iter()
    .enumerate()
    {
        if index > 0 {
            out.push_str(", ");
        }
        out.push_str(&format!("\"{name}\": "));
        match round {
            Some(round) => out.push_str(&render_home_round(round)),
            None => out.push_str("null"),
        }
    }
    out.push('}');
    out
}

fn render_home_round(round: &crate::homes::HomeRound) -> String {
    let mut out = String::new();
    out.push('{');
    out.push_str(&format!(
        "\"label\": \"{}\", \"ok\": {}, \"message\": \"{}\", \"expected\": ",
        json_escape(&round.label),
        round.ok,
        json_escape(&round.message)
    ));
    match &round.expected {
        Some(table) => out.push_str(&render_home_table(table)),
        None => out.push_str("null"),
    }
    out.push_str(", \"nodes\": [");
    for (index, node) in round.nodes.iter().enumerate() {
        if index > 0 {
            out.push(',');
        }
        out.push_str(&format!(
            "{{\"name\": \"{}\", \"path\": \"{}\", \"status\": {}, \"error\": {}, \"table\": ",
            json_escape(&node.name),
            json_escape(&node.path),
            node.status
                .map(|value| value.to_string())
                .unwrap_or_else(|| "null".into()),
            json_option(&node.error),
        ));
        match &node.table {
            Some(table) => out.push_str(&render_home_table(table)),
            None => out.push_str("null"),
        }
        out.push('}');
    }
    out.push_str("]}");
    out
}

fn render_home_table(table: &crate::homes::HomeTable) -> String {
    let mut out = String::new();
    out.push('{');
    out.push_str(&format!(
        "\"state\": \"{}\", \"requested_epoch\": {}, \"reason\": {}, \"object_set\": {}, \"epoch\": {}, \"members\": {}, \"digest\": {}, \"count\": {}, \"raw\": \"{}\", \"rows\": [",
        json_escape(&table.state),
        table
            .requested_epoch
            .map(|value| value.to_string())
            .unwrap_or_else(|| "null".into()),
        json_option(&table.reason),
        json_option(&table.object_set),
        table
            .epoch
            .map(|value| value.to_string())
            .unwrap_or_else(|| "null".into()),
        table
            .members
            .map(|value| value.to_string())
            .unwrap_or_else(|| "null".into()),
        json_option(&table.digest),
        table
            .count
            .map(|value| value.to_string())
            .unwrap_or_else(|| "null".into()),
        json_escape(&table.raw)
    ));
    for (index, row) in table.rows.iter().enumerate() {
        if index > 0 {
            out.push(',');
        }
        out.push_str(&format!(
            "{{\"object\": \"{}\", \"home\": \"{}\", \"epoch\": {}, \"digest\": \"{}\"}}",
            json_escape(&row.object),
            json_escape(&row.home),
            row.epoch,
            json_escape(&row.digest)
        ));
    }
    out.push_str("]}");
    out
}

fn render_join_json(join: &JoinEvidence) -> String {
    let mut out = String::new();
    out.push('{');
    out.push_str(&format!(
        "\"ok\": {}, \"fault\": \"{}\", \"message\": \"{}\", \"authority\": \"{}\", \"authority_key\": \"{}\", ",
        join.ok,
        json_escape(&join.fault),
        json_escape(&join.message),
        json_escape(&join.authority),
        json_escape(&join.authority_key)
    ));
    out.push_str(&format!(
        "\"epoch\": {}, \"digest\": {}, \"expected_digest\": {}, \"members\": {}, ",
        join.epoch
            .map(|value| value.to_string())
            .unwrap_or_else(|| "null".into()),
        json_option(&join.digest),
        json_option(&join.expected_digest),
        json_strings(&join.members)
    ));
    out.push_str("\"nodes\": [");
    for (index, node) in join.nodes.iter().enumerate() {
        if index > 0 {
            out.push(',');
        }
        out.push_str(&format!(
            "{{\"name\": \"{}\", \"role\": {}, \"enabled\": {}, \"disabled\": {}, \"local\": {}, \"request\": {}, \"confirms\": {}, \"converged\": {}, \"acks\": {}, \"acks_sent\": {}, \"duplicate\": {}, \"refused\": {}, \"timeout\": {}, \"reject\": {}}}",
            json_escape(&node.name),
            json_option(&node.role),
            json_option(&node.enabled),
            json_option(&node.disabled),
            json_option(&node.local),
            json_option(&node.request),
            json_strings(&node.confirms),
            json_option(&node.converged),
            json_strings(&node.acks),
            json_strings(&node.acks_sent),
            json_option(&node.duplicate),
            json_option(&node.refused),
            json_option(&node.timeout),
            json_option(&node.reject)
        ));
    }
    out.push_str("], \"restart\": ");
    match &join.restart {
        Some(restart) => out.push_str(&format!(
            "{{\"node\": \"{}\", \"boot1_log\": \"serial-{}-boot1.log\", \"first_epoch\": {}, \"rejoin_epoch\": {}, \"first_digest\": \"{}\", \"rejoin_digest\": \"{}\", \"request_last_epoch\": {}, \"confirm_epoch\": {}, \"store_restored\": {}, \"store_epoch\": {}, \"same_epoch\": {}}}",
            json_escape(&restart.node),
            json_escape(&restart.node),
            restart.first_epoch,
            restart.rejoin_epoch,
            json_escape(&restart.first_digest),
            json_escape(&restart.rejoin_digest),
            restart
                .request_last_epoch
                .map(|value| value.to_string())
                .unwrap_or_else(|| "null".into()),
            restart
                .confirm_epoch
                .map(|value| value.to_string())
                .unwrap_or_else(|| "null".into()),
            restart.store_restored,
            restart
                .store_epoch
                .map(|value| value.to_string())
                .unwrap_or_else(|| "null".into()),
            restart.same_epoch
        )),
        None => out.push_str("null"),
    }
    out.push_str(", \"root_restart\": ");
    match &join.root_restart {
        Some(restart) => out.push_str(&format!(
            "{{\"node\": \"{}\", \"boot1_log\": \"serial-{}-boot1.log\", \"first_epoch\": {}, \"first_digest\": \"{}\", \"store_epoch\": {}, \"local_epoch\": {}, \"member_last_epoch\": {}, \"confirm_epoch\": {}, \"member_epoch\": {}, \"epoch_regression\": {}, \"same_epoch\": {}}}",
            json_escape(&restart.node),
            json_escape(&restart.node),
            restart.first_epoch,
            json_escape(&restart.first_digest),
            restart
                .store_epoch
                .map(|value| value.to_string())
                .unwrap_or_else(|| "null".into()),
            restart
                .local_epoch
                .map(|value| value.to_string())
                .unwrap_or_else(|| "null".into()),
            restart
                .member_last_epoch
                .map(|value| value.to_string())
                .unwrap_or_else(|| "null".into()),
            restart
                .confirm_epoch
                .map(|value| value.to_string())
                .unwrap_or_else(|| "null".into()),
            restart
                .member_epoch
                .map(|value| value.to_string())
                .unwrap_or_else(|| "null".into()),
            restart.epoch_regression,
            restart.same_epoch
        )),
        None => out.push_str("null"),
    }
    out.push('}');
    out
}

fn json_strings(values: &[String]) -> String {
    let entries: Vec<String> = values
        .iter()
        .map(|value| format!("\"{}\"", json_escape(value)))
        .collect();
    format!("[{}]", entries.join(", "))
}

fn json_env(env: &[(String, String)]) -> String {
    let entries: Vec<String> = env
        .iter()
        .map(|(name, value)| format!("\"{}\": \"{}\"", json_escape(name), json_escape(value)))
        .collect();
    format!("{{{}}}", entries.join(", "))
}

fn json_option(value: &Option<String>) -> String {
    value
        .as_deref()
        .map(|text| format!("\"{}\"", json_escape(text)))
        .unwrap_or_else(|| "null".into())
}

pub(crate) fn json_escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for character in text.chars() {
        match character {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            character if (character as u32) < 0x20 => {
                out.push_str(&format!("\\u{:04x}", character as u32));
            }
            character => out.push(character),
        }
    }
    out
}

fn render_route_json(route: &RouteEvidence) -> String {
    let mut out = String::new();
    out.push('{');
    out.push_str(&format!(
        "\"ok\": {}, \"fault\": \"{}\", \"wire\": {}, \"local\": {}, \"message\": \"{}\", ",
        route.ok,
        json_escape(&route.fault),
        json_option(&route.wire),
        json_option(&route.local),
        json_escape(&route.message)
    ));
    out.push_str("\"steps\": ");
    out.push_str(&json_strings(&route.steps));
    out.push_str(", \"checks\": ");
    out.push_str(&json_strings(&route.checks));
    out.push_str(", \"nodes\": [");
    for (index, node) in route.nodes.iter().enumerate() {
        out.push_str(&format!(
            "{{\"name\": \"{}\", \"status\": {}, \"error\": {}, \"report\": ",
            json_escape(&node.name),
            node.status
                .map(|value| value.to_string())
                .unwrap_or_else(|| "null".into()),
            json_option(&node.error)
        ));
        match &node.report {
            Some(report) => {
                out.push_str(&format!(
                    "{{\"state\": \"{}\", \"reason\": {}, \"view_epoch\": {}, \"members\": {}, \"digest\": {}, \"move_old\": {}, \"move_new\": {}, \"moved_remote\": {}, \"moved_local\": {}, \"retries\": {}, \"completes\": {}, \"rejects\": {}, \"timeouts\": {}, \"last_reject\": {}, \"results\": [",
                    json_escape(&report.state),
                    json_option(&report.reason),
                    report
                        .view_epoch
                        .map(|value| value.to_string())
                        .unwrap_or_else(|| "null".into()),
                    report
                        .members
                        .map(|value| value.to_string())
                        .unwrap_or_else(|| "null".into()),
                    json_option(&report.digest),
                    json_option(&report.move_old),
                    json_option(&report.move_new),
                    report.moved_remote,
                    report.moved_local,
                    report.retries,
                    report.completes,
                    report.rejects,
                    report.timeouts,
                    json_option(&report.last_reject)
                ));
                for (row_index, row) in report.results.iter().enumerate() {
                    out.push_str(&format!(
                        "{{\"object\": \"{}\", \"home\": \"{}\", \"home_epoch\": {}, \"value\": \"{}\", \"attempts\": {}, \"moved\": {}}}",
                        json_escape(&row.object),
                        json_escape(&row.home),
                        row.home_epoch,
                        json_escape(&row.value),
                        row.attempts,
                        row.moved
                    ));
                    if row_index + 1 < report.results.len() {
                        out.push(',');
                    }
                }
                out.push_str("]}");
            }
            None => out.push_str("null"),
        }
        out.push('}');
        if index + 1 < route.nodes.len() {
            out.push(',');
        }
    }
    out.push_str("]}");
    out
}

fn render_summary(evidence: &Evidence<'_>) -> String {
    let mut out = String::new();
    out.push_str("# M2 two-node rig evidence\n\n");
    out.push_str(&format!("**Verdict:** `{}`\n\n", evidence.verdict));
    out.push_str(&format!(
        "**Repository:** `{}`  \n**Branch:** `{}`  \n**Commit:** `{}`  \n**Describe:** `{}`  \n**Dirty:** `{}`\n\n",
        evidence.repo.display(),
        evidence.branch,
        evidence.commit,
        evidence.describe,
        evidence.dirty
    ));
    out.push_str(&format!(
        "**PROM:** `{}`  \n**PROM bytes:** {}  \n**PROM SHA-256:** `{}`\n\n**Module:** `{}`  \n**Module bytes:** {}  \n**Module SHA-256:** `{}`\n\n**iris:** `{}`  \n**iris SHA-256:** `{}`  \n**iris-ci:** `{}`  \n**iris-ci SHA-256:** `{}`\n\n**Artifact manifest:** `manifest.json`\n\n",
        evidence.rom.display(),
        evidence.rom_size,
        evidence.rom_sha256,
        evidence.module.display(),
        evidence.module_size,
        evidence.module_sha256,
        evidence.iris.display(),
        evidence.iris_sha256,
        evidence.iris_ci.display(),
        evidence.iris_ci_sha256
    ));
    if let Some(error) = evidence.error {
        out.push_str(&format!("**Error:** {error}\n\n"));
    }

    out.push_str("## Nodes\n\n");
    out.push_str(
        "| Node | Instance | MAC | NodeId | Guest IP | Gateway | NAT | Telnet | HTTP | Mesh |\n",
    );
    out.push_str("|---|---|---|---|---|---|---|---|---|---|\n");
    for node in evidence.nodes {
        out.push_str(&format!(
            "| {} | {} | `{}` | `{}` | {} | {} | {} | {} | {} | {} |\n",
            node.name,
            node.instance,
            node.mac,
            node.node_id,
            node.guest,
            node.gateway,
            node.nat_subnet,
            node.telnet_port,
            node.http_port,
            node.mesh_port
        ));
    }
    out.push('\n');

    out.push_str("## Boot markers\n\n");
    for node in evidence.nodes {
        out.push_str(&format!("### {}\n\n", node.name));
        if let Some(line) = &node.facts.boot_ready {
            out.push_str(&format!("```text\n{line}\n```\n\n"));
        } else {
            out.push_str("No `BOOT-READY` line.\n\n");
        }
        if let Some(line) = &node.facts.mesh {
            out.push_str(&format!("- `{line}`\n"));
        }
        if let Some(line) = &node.facts.mesh_transport_listen {
            out.push_str(&format!("- `{line}`\n"));
        }
        if let Some(line) = &node.facts.mesh_transport_pass {
            out.push_str(&format!("- `{line}`\n"));
        }
        if let Some(line) = &node.facts.mesh_route_enabled {
            out.push_str(&format!("- `{line}`\n"));
        }
        if let Some(line) = &node.facts.mesh_route_disabled {
            out.push_str(&format!("- `{line}`\n"));
        }
        if let Some(line) = &node.facts.network_mac {
            out.push_str(&format!("- `{line}`\n"));
        }
        if let Some(line) = &node.facts.network_ip {
            out.push_str(&format!("- `{line}`\n"));
        }
        out.push('\n');
    }

    out.push_str("## NVRAM environment\n\n");
    for node in evidence.nodes {
        out.push_str(&format!("### {}\n\n```text\n", node.name));
        for (name, value) in &node.nvram_env {
            out.push_str(&format!("{name}={value}\n"));
        }
        out.push_str("```\n\n");
    }

    out.push_str("## Two-way probe\n\n");
    out.push_str("| From | To | URL | Relay | Result | Status | Bytes | SHA-256 |\n");
    out.push_str("|---|---|---|---|---|---|---|---|\n");
    for probe in evidence.probes {
        out.push_str(&format!(
            "| {} | {} | `{}` | `{}` | {} | {} | {} | `{}` |\n",
            probe.from,
            probe.to,
            probe.url,
            probe.relay,
            if probe.ok { "pass" } else { "FAIL" },
            probe
                .status
                .map(|value| value.to_string())
                .unwrap_or_else(|| "-".into()),
            probe
                .bytes
                .map(|value| value.to_string())
                .unwrap_or_else(|| "-".into()),
            probe.sha256.as_deref().unwrap_or("-")
        ));
    }
    out.push('\n');

    for probe in evidence.probes {
        out.push_str(&format!(
            "### {} -> {} transcript\n\n```text\n{}\n```\n\n",
            probe.from,
            probe.to,
            probe.text.trim()
        ));
    }

    if let Some(mesh) = evidence.mesh {
        out.push_str("## Mesh transport probe\n\n");
        out.push_str(&format!(
            "**Result:** {}  \n**Relay:** `{}`\n\n",
            if mesh.ok { "pass" } else { "FAIL" },
            mesh.relay
        ));
        out.push_str(&format!("{}\n\n", mesh.message));
        out.push_str("| Node | Listening | Pass line |\n|---|---|---|\n");
        for node in &mesh.nodes {
            out.push_str(&format!(
                "| {} | {} | {} |\n",
                node.name,
                node.listen.as_deref().unwrap_or("-"),
                node.pass.as_deref().unwrap_or("-")
            ));
        }
        out.push('\n');
    }

    if let Some(bulk) = evidence.bulk {
        out.push_str("## Bulk transfer (#247)\n\n");
        out.push_str(&format!(
            "**Result:** {}  \n**Logical record:** {} bytes in {} byte chunks\n\n",
            if bulk.ok { "pass" } else { "FAIL" },
            crate::bulk::BULK_BYTES,
            crate::bulk::BULK_CHUNK
        ));
        out.push_str(&format!("{}\n\n", bulk.message));
        out.push_str("| From | To | Bytes | Chunks | Mesh ms | MiB/s | Raw |\n|---|---|---|---|---|---|---|\n");
        if let Some(run) = &bulk.baseline {
            out.push_str(&format!(
                "| {} (baseline) | {} | {} | {} | {} | {:.3} | `{}` |\n",
                run.from, run.to, run.bytes, run.chunks, run.elapsed_ms, run.mb_per_s, run.raw.trim()
            ));
        }
        for run in &bulk.runs {
            out.push_str(&format!(
                "| {} | {} | {} | {} | {} | {:.3} | `{}` |\n",
                run.from, run.to, run.bytes, run.chunks, run.elapsed_ms, run.mb_per_s, run.raw.trim()
            ));
        }
        out.push('\n');
    }

    if let Some(join) = evidence.join {
        out.push_str("## Join and membership epoch\n\n");
        out.push_str(&format!(
            "**Result:** {}  \n**Fault mode:** `{}`  \n**Authority:** `{}`  \n**Authority key:** `{}`\n\n",
            if join.ok { "pass" } else { "FAIL" },
            join.fault,
            join.authority,
            join.authority_key
        ));
        out.push_str(&format!("{}\n\n", join.message));
        out.push_str(
            "| Node | Role | Local line | Converged | Reject / refusal |\n|---|---|---|---|---|\n",
        );
        for node in &join.nodes {
            out.push_str(&format!(
                "| {} | {} | {} | {} | {} |\n",
                node.name,
                node.role.as_deref().unwrap_or("-"),
                node.local.as_deref().unwrap_or("-"),
                node.converged.as_deref().unwrap_or("-"),
                node.reject
                    .as_deref()
                    .or(node.refused.as_deref())
                    .or(node.timeout.as_deref())
                    .unwrap_or("-")
            ));
        }
        out.push('\n');
        if let (Some(epoch), Some(digest), Some(expected)) =
            (join.epoch, &join.digest, &join.expected_digest)
        {
            out.push_str(&format!(
                "Converged epoch `{epoch}`, members `{}`, digest `{digest}`, rig-recomputed digest `{expected}`.\n\n",
                join.members.join(", ")
            ));
        }
        if let Some(restart) = &join.restart {
            out.push_str("### Member restart and rejoin\n\n");
            out.push_str(
                "| Node | First epoch | Rejoin epoch | First digest | Rejoin digest | Request last epoch | Root confirm epoch | Store restored | Store epoch | Same |\n|---|---|---|---|---|---|---|---|---|---|\n",
            );
            out.push_str(&format!(
                "| {} | {} | {} | `{}` | `{}` | {} | {} | {} | {} | {} |\n\n",
                restart.node,
                restart.first_epoch,
                restart.rejoin_epoch,
                restart.first_digest,
                restart.rejoin_digest,
                restart
                    .request_last_epoch
                    .map(|value| value.to_string())
                    .unwrap_or_else(|| "-".into()),
                restart
                    .confirm_epoch
                    .map(|value| value.to_string())
                    .unwrap_or_else(|| "-".into()),
                restart.store_restored,
                restart
                    .store_epoch
                    .map(|value| value.to_string())
                    .unwrap_or_else(|| "-".into()),
                restart.same_epoch
            ));
            out.push_str(&format!(
                "The first transcript is `serial-{}-boot1.log`; the current transcript is the rejoin boot. The member restored the signed confirmation from its scratch epoch store and presented the confirmed `last_epoch`.\n\n",
                restart.node
            ));
        }
        if let Some(restart) = &join.root_restart {
            out.push_str("### Root restart and epoch recovery\n\n");
            out.push_str(
                "| Node | First epoch | Store epoch | Local epoch | Member last epoch | Confirm epoch | Member epoch | Epoch regression | Same |\n|---|---|---|---|---|---|---|---|---|\n",
            );
            out.push_str(&format!(
                "| {} | {} | {} | {} | {} | {} | {} | {} | {} |\n\n",
                restart.node,
                restart.first_epoch,
                restart
                    .store_epoch
                    .map(|value| value.to_string())
                    .unwrap_or_else(|| "-".into()),
                restart
                    .local_epoch
                    .map(|value| value.to_string())
                    .unwrap_or_else(|| "-".into()),
                restart
                    .member_last_epoch
                    .map(|value| value.to_string())
                    .unwrap_or_else(|| "-".into()),
                restart
                    .confirm_epoch
                    .map(|value| value.to_string())
                    .unwrap_or_else(|| "-".into()),
                restart
                    .member_epoch
                    .map(|value| value.to_string())
                    .unwrap_or_else(|| "-".into()),
                restart.epoch_regression,
                restart.same_epoch
            ));
            out.push_str(&format!(
                "The root transcript before the restart is `serial-{}-boot1.log`. The restarted root verified the signed epoch record from its scratch store, and the live member presented the same confirmed epoch instead of the seed.\n\n",
                restart.node
            ));
        }
        out.push_str("```text\n");
        for node in &join.nodes {
            for line in node
                .enabled
                .iter()
                .chain(node.disabled.iter())
                .chain(node.local.iter())
                .chain(node.request.iter())
                .chain(node.confirms.iter())
                .chain(node.converged.iter())
                .chain(node.acks.iter())
                .chain(node.acks_sent.iter())
                .chain(node.duplicate.iter())
                .chain(node.refused.iter())
                .chain(node.timeout.iter())
                .chain(node.reject.iter())
            {
                out.push_str(line);
                out.push('\n');
            }
        }
        out.push_str("```\n\n");
    }

    if let Some(homes) = evidence.homes {
        out.push_str("## Rendezvous homes\n\n");
        out.push_str(&format!(
            "**Result:** {}  \n**Object set:** `{}`  \n**Configured count:** {}  \n",
            if homes.ok { "pass" } else { "FAIL" },
            homes.object_set,
            homes.count
        ));
        out.push_str(&format!("{}\n\n", homes.message));
        out.push_str(
            "| Round | Node | Path | HTTP | State | Epoch | Count | Digest | Result |\n|---|---|---|---|---|---|---|---|---|\n",
        );
        for round in [
            homes.before.as_ref(),
            homes.after.as_ref(),
            homes.stale.as_ref(),
            homes.disabled.as_ref(),
            homes.wrong_authority.as_ref(),
        ]
        .into_iter()
        .flatten()
        {
            for node in &round.nodes {
                out.push_str(&format!(
                    "| {} | {} | `{}` | {} | {} | {} | {} | {} | {} |\n",
                    round.label,
                    node.name,
                    node.path,
                    node.status
                        .map(|value| value.to_string())
                        .unwrap_or_else(|| "-".into()),
                    node.table
                        .as_ref()
                        .map(|table| table.state.as_str())
                        .unwrap_or("-"),
                    node.table
                        .as_ref()
                        .and_then(|table| table.epoch)
                        .map(|value| value.to_string())
                        .unwrap_or_else(|| "-".into()),
                    node.table
                        .as_ref()
                        .and_then(|table| table.count)
                        .map(|value| value.to_string())
                        .unwrap_or_else(|| "-".into()),
                    node.table
                        .as_ref()
                        .and_then(|table| table.digest.as_deref())
                        .unwrap_or("-"),
                    if round.ok { "pass" } else { "FAIL" }
                ));
            }
        }
        out.push('\n');
        for (label, round) in [
            ("before the member restart", homes.before.as_ref()),
            ("after the member rejoin", homes.after.as_ref()),
            ("stale-epoch query", homes.stale.as_ref()),
        ] {
            let Some(round) = round else {
                continue;
            };
            out.push_str(&format!("### Homes {label}\n\n```text\n"));
            for node in &round.nodes {
                if let Some(table) = &node.table {
                    out.push_str(&table.raw);
                    out.push('\n');
                }
            }
            out.push_str("```\n\n");
        }
    }

    if let Some(route) = evidence.route {
        out.push_str("## Remote capability route\n\n");
        out.push_str(&format!(
            "**Result:** {}  \n**Fault mode:** `{}`  \n**Wire object:** {}  \n**Local object:** {}\n\n",
            if route.ok { "pass" } else { "FAIL" },
            route.fault,
            route.wire.as_deref().unwrap_or("-"),
            route.local.as_deref().unwrap_or("-")
        ));
        out.push_str(&format!("{}\n\n", route.message));
        if !route.steps.is_empty() {
            out.push_str("### Steps\n\n```text\n");
            for step in &route.steps {
                out.push_str(step);
                out.push('\n');
            }
            out.push_str("```\n\n");
        }
        out.push_str("### Checks\n\n");
        for check in &route.checks {
            out.push_str(&format!("- {check}\n"));
        }
        out.push('\n');
        out.push_str(
            "### Node reports\n\n| Node | HTTP | State | View | Move old | Move new | MOVED local/remote | Retries | Completes | Rejects |\n|---|---|---|---|---|---|---|---|---|---|\n",
        );
        for node in &route.nodes {
            let report = node.report.as_ref();
            out.push_str(&format!(
                "| {} | {} | {} | {} | {} | {} | {}/{} | {} | {} | {} |\n",
                node.name,
                node.status
                    .map(|value| value.to_string())
                    .unwrap_or_else(|| "-".into()),
                report.map(|report| report.state.as_str()).unwrap_or("-"),
                report
                    .map(|report| format!(
                        "{}/{}",
                        report
                            .view_epoch
                            .map(|value| value.to_string())
                            .unwrap_or_else(|| "-".into()),
                        report
                            .members
                            .map(|value| value.to_string())
                            .unwrap_or_else(|| "-".into())
                    ))
                    .unwrap_or_else(|| "-".into()),
                report
                    .and_then(|report| report.move_old.as_deref())
                    .unwrap_or("-"),
                report
                    .and_then(|report| report.move_new.as_deref())
                    .unwrap_or("-"),
                report.map(|report| report.moved_local).unwrap_or(0),
                report.map(|report| report.moved_remote).unwrap_or(0),
                report.map(|report| report.retries).unwrap_or(0),
                report.map(|report| report.completes).unwrap_or(0),
                report.map(|report| report.rejects).unwrap_or(0)
            ));
        }
        out.push('\n');
        let mut wrote_results = false;
        for node in &route.nodes {
            let Some(report) = &node.report else {
                continue;
            };
            for row in &report.results {
                if !wrote_results {
                    out.push_str("### Results\n\n| Node | Object | Home | Home epoch | Attempts | MOVED | Value |\n|---|---|---|---|---|---|---|\n");
                    wrote_results = true;
                }
                out.push_str(&format!(
                    "| {} | {} | {} | {} | {} | {} | {} |\n",
                    node.name,
                    row.object,
                    row.home,
                    row.home_epoch,
                    row.attempts,
                    row.moved,
                    row.value
                ));
            }
        }
        if wrote_results {
            out.push('\n');
        }
        out.push_str("### Raw reports\n\n```text\n");
        for node in &route.nodes {
            if let Some(report) = &node.report {
                out.push_str(&format!("== {} ==\n{}\n", node.name, report.raw));
            } else if let Some(error) = &node.error {
                out.push_str(&format!("== {} ==\n{error}\n", node.name));
            }
        }
        out.push_str("```\n\n");
    }

    if let Some(migration) = evidence.migration {
        out.push_str("## L0 actor migration\n\n");
        out.push_str(&format!(
            "**{}** (fault `{}`). Actor `{}`, home `{}`.\n\n",
            if migration.ok { "pass" } else { "FAIL" },
            migration.fault,
            migration.actor.as_deref().unwrap_or("-"),
            migration.home.as_deref().unwrap_or("-")
        ));
        out.push_str(&format!("{}\n\n", migration.message));
        out.push_str(&format!(
            "Boundary hash `{}`, return hash `{}`.\n\n",
            migration.boundary_hash.as_deref().unwrap_or("-"),
            migration.return_hash.as_deref().unwrap_or("-")
        ));
        if !migration.steps.is_empty() {
            out.push_str("### Steps\n\n```text\n");
            for step in &migration.steps {
                out.push_str(step);
                out.push('\n');
            }
            out.push_str("```\n\n");
        }
        out.push_str("### Checks\n\n");
        for check in &migration.checks {
            out.push_str(&format!("- {check}\n"));
        }
        out.push('\n');
        out.push_str("### Node reports\n\n");
        for node in &migration.nodes {
            out.push_str(&format!(
                "#### {} (HTTP {})\n\n```text\n{}\n```\n\n",
                node.name,
                node.status
                    .map(|value| value.to_string())
                    .unwrap_or_else(|| "-".into()),
                node.report
                    .as_ref()
                    .map(|report| report.raw.as_str())
                    .or(node.error.as_deref())
                    .unwrap_or("no report")
            ));
        }
    }
    if let Some(failure) = evidence.failure {
        out.push_str("## Failure, fencing and partition\n\n");
        out.push_str(&format!(
            "**Result:** {}  \n**Fault mode:** `{}`  \n**Stage:** `{}`\n\n",
            if failure.ok { "pass" } else { "FAIL" },
            failure.fault,
            failure.stage
        ));
        out.push_str(&format!("{}\n\n", failure.message));
        if let Some(before) = &failure.before {
            out.push_str(&format!(
                "**Before:** epoch {} members {} digest `{}`\n\n",
                before.epoch,
                before.members.len(),
                before.digest
            ));
        }
        if let Some(after) = &failure.after {
            out.push_str(&format!(
                "**Survivor view:** epoch {} members {} digest `{}`\n\n",
                after.epoch,
                after.members.len(),
                after.digest
            ));
        }
        if let Some(recovered) = &failure.recovered {
            out.push_str(&format!(
                "**Recovered view:** epoch {} members {} digest `{}`\n\n",
                recovered.epoch,
                recovered.members.len(),
                recovered.digest
            ));
        }
        if !failure.serial.is_empty() {
            out.push_str("### Failure lines\n\n```text\n");
            for line in &failure.serial {
                out.push_str(line);
                out.push('\n');
            }
            out.push_str("```\n\n");
        }
        if !failure.steps.is_empty() {
            out.push_str("### Steps\n\n```text\n");
            for step in &failure.steps {
                out.push_str(step);
                out.push('\n');
            }
            out.push_str("```\n\n");
        }
        out.push_str("### Checks\n\n");
        for check in &failure.checks {
            out.push_str(&format!("- {check}\n"));
        }
        out.push('\n');
        out.push_str("### Node reports\n\n```text\n");
        for node in &failure.nodes {
            out.push_str(&format!("== {} ==\n", node.name));
            match &node.report {
                Some(report) => {
                    out.push_str(&report.raw);
                    out.push('\n');
                }
                None => {
                    out.push_str(node.error.as_deref().unwrap_or("no report"));
                    out.push('\n');
                }
            }
        }
        out.push_str("```\n\n");
    }

    if let Some(fixture) = evidence.fixture {
        out.push_str("## Module fixture\n\n```text\n");
        for line in fixture.lines() {
            out.push_str(&line);
            out.push('\n');
        }
        out.push_str("```\n\n");
    }

    out.push_str("## Relays\n\n");
    for (name, relay) in evidence.relays {
        out.push_str(&format!("### {name}\n\n```text\n"));
        for line in log_lines(relay) {
            out.push_str(&line);
            out.push('\n');
        }
        out.push_str("```\n\n");
    }

    out.push_str("## Reproduce\n\n");
    out.push_str("```sh\n");
    out.push_str("tools/m2-rig/build.sh\n");
    out.push_str("cargo run --manifest-path tools/m2-rig/Cargo.toml -- run \\\n");
    out.push_str("  --iris <path-to-iris> --iris-ci <path-to-iris-ci>\n");
    out.push_str("```\n");
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json_escapes_quotes_and_controls() {
        assert_eq!(json_escape("a\"b\\c\n"), "a\\\"b\\\\c\\n");
    }
}
