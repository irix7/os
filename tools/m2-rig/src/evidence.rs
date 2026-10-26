// SPDX-License-Identifier: GPL-3.0-only
//! The evidence bundle.
//!
//! Every run writes a self-contained directory: the two serial transcripts,
//! the guest MCP fetch log, the relay log, the probe transcripts, `run.json`
//! and a human-readable `summary.md`.  A reviewer can inspect the bundle
//! without the live processes or the host's shell history.

use std::path::{Path, PathBuf};

use crate::fixture::Fixture;
use crate::nvram;
use crate::relay::{lines as log_lines, SharedLog};
use crate::rig::{copy_if_present, write_text, BootFacts, Node};

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
        let nvram_env = std::fs::read(node.state_dir.join("nvram.bin"))
            .map(|image| nvram::parse(&image))
            .unwrap_or_default();
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
    /// PROM path.
    pub rom: &'a Path,
    /// PROM digest.
    pub rom_sha256: &'a str,
    /// Module path.
    pub module: &'a Path,
    /// Module digest.
    pub module_sha256: &'a str,
    /// Node facts.
    pub nodes: &'a [NodeEvidence],
    /// Probe results.
    pub probes: &'a [ProbeEvidence],
    /// Mesh transport probe, when one ran.
    pub mesh: Option<&'a MeshProbeEvidence>,
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

    let json = render_json(evidence);
    write_text(&directory.join("run.json"), &json)?;
    let summary = render_summary(evidence);
    write_text(&directory.join("summary.md"), &summary)?;
    Ok(directory)
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
        "  \"repo\": \"{}\",\n  \"branch\": \"{}\",\n  \"commit\": \"{}\",\n",
        json_escape(&evidence.repo.display().to_string()),
        json_escape(evidence.branch),
        json_escape(evidence.commit)
    ));
    out.push_str(&format!(
        "  \"rom\": {{\"path\": \"{}\", \"sha256\": \"{}\"}},\n",
        json_escape(&evidence.rom.display().to_string()),
        json_escape(evidence.rom_sha256)
    ));
    out.push_str(&format!(
        "  \"module\": {{\"path\": \"{}\", \"sha256\": \"{}\"}},\n",
        json_escape(&evidence.module.display().to_string()),
        json_escape(evidence.module_sha256)
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
            "\"boot_ready\": {}, \"mcp\": {}, \"mesh\": {}, \"mesh_transport_listen\": {}, \"mesh_transport_pass\": {}, \"network_mac\": {}, \"network_ip\": {}, \"nvram\": {}",
            json_option(&node.facts.boot_ready),
            json_option(&node.facts.mcp),
            json_option(&node.facts.mesh),
            json_option(&node.facts.mesh_transport_listen),
            json_option(&node.facts.mesh_transport_pass),
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
    out.push_str("\n}\n");
    out
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

fn json_escape(text: &str) -> String {
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

fn render_summary(evidence: &Evidence<'_>) -> String {
    let mut out = String::new();
    out.push_str("# M2 two-node rig evidence\n\n");
    out.push_str(&format!("**Verdict:** `{}`\n\n", evidence.verdict));
    out.push_str(&format!(
        "**Repository:** `{}`  \n**Branch:** `{}`  \n**Commit:** `{}`\n\n",
        evidence.repo.display(),
        evidence.branch,
        evidence.commit
    ));
    out.push_str(&format!(
        "**PROM:** `{}`  \n**PROM SHA-256:** `{}`\n\n**Module:** `{}`  \n**Module SHA-256:** `{}`\n\n",
        evidence.rom.display(),
        evidence.rom_sha256,
        evidence.module.display(),
        evidence.module_sha256
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
