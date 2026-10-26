// SPDX-License-Identifier: GPL-3.0-only
//! Two-node IRIX 7 rig.
//!
//! The rig starts two independent `iris` processes, each with its own state
//! directory, NVRAM, MAC, NAT subnet, CI socket, monitor and serial log.  It
//! serves a freshly built MCP module over a local HTTP fixture, waits for both
//! kernels to print `BOOT-READY`, and then drives each kernel's HTTP client
//! through a Rust TCP relay to the other kernel's MCP status page.  Every run
//! writes a self-contained evidence bundle under `<work>/evidence`.
//!
//! It does not implement join, membership, route cache, migration or
//! capabilities.  The NVRAM keys it seeds (`mesh.peer`, `mesh.authority`,
//! `mesh.epoch`) are the handoff to M2-1.

mod evidence;
mod fixture;
mod nvram;
mod probe;
mod relay;
mod rig;
mod sha256;

use std::fs;
use std::net::Ipv4Addr;
use std::time::{Duration, Instant};

use evidence::{Evidence, MeshNodeProbe, MeshProbeEvidence, NodeEvidence, ProbeEvidence};
use fixture::Fixture;
use probe::{ProbeError, ProbeOutcome};
use relay::{MeshRelay, Relay};
use rig::{
    derive_nodes, git, read_serial, run_ci, start_cpu, start_node, wait_boot, wait_ci_ready,
    write_nvram, Action, BootFacts, Node, Options, RunningNode,
};

const PROBE_TIMEOUT: Duration = Duration::from_secs(25);

/// Module bytes, PROM digest and module digest.
type Prerequisites = (Vec<u8>, [u8; 32], [u8; 32]);

/// Probe results and the relays that carried them.
type ProbeSet = (Vec<ProbeEvidence>, Vec<(String, Relay)>);

fn main() {
    match run() {
        Ok(code) => std::process::exit(code),
        Err(error) => {
            eprintln!("m2-rig: {error}");
            std::process::exit(1);
        }
    }
}

fn run() -> Result<i32, String> {
    let Some(options) = rig::parse_args(std::env::args())? else {
        print_help();
        return Ok(0);
    };

    match options.action {
        Action::Run => run_and_probe(&options),
        Action::Up => up(&options),
        Action::Down => down(&options),
        Action::Status => status(&options),
        Action::Probe => probe_only(&options),
        Action::PrintConfig => print_config(&options),
        Action::Clean => clean(&options),
    }
}

fn print_help() {
    println!(
        "m2-rig — two independent IRIX 7 kernels over a Rust host relay

Usage:
  m2-rig <command> [options]

Commands:
  run           start both nodes, probe HTTP and mesh framing, stop, write evidence
  up            start both nodes and leave them running
  down          stop both nodes through their CI sockets
  status        show lifecycle state and boot markers
  probe         probe HTTP and mesh framing against running nodes
  print-config  print the derived instance and NVRAM table
  clean         remove the rig work directory

Options:
  --repo DIR       repository root (default: discovered from cwd)
  --work DIR       work directory (default: <repo>/target/m2-rig)
  --iris PATH      iris binary (default: $IRIS or ../iris/target/release/iris)
  --iris-ci PATH   iris-ci binary (default: $IRIS_CI or ../iris/target/release/iris-ci)
  --rom PATH       PROM image (default: <work>/irix7-rom.bin)
  --module PATH    fresh MCP module (default: <work>/mcp.module)
  --base-port N    first monitor port block (default: 9100)
  --timeout SECS   boot deadline (default: 180)
  --keep           with run, leave the nodes running
  --verbose        print extra detail

Workflow:
  tools/m2-rig/build.sh
  cargo run --manifest-path tools/m2-rig/Cargo.toml -- run \\
    --iris <iris> --iris-ci <iris-ci>
"
    );
}

fn check_prerequisites(options: &Options) -> Result<Prerequisites, String> {
    for (label, option, path) in [
        ("iris", "--iris", &options.iris),
        ("iris-ci", "--iris-ci", &options.iris_ci),
        ("PROM", "--rom", &options.rom),
        ("MCP module", "--module", &options.module),
    ] {
        if !path.is_file() {
            return Err(format!(
                "{label} is missing at {}; run tools/m2-rig/build.sh and pass {option} if needed",
                path.display()
            ));
        }
    }
    let rom = fs::read(&options.rom).map_err(|error| format!("read PROM: {error}"))?;
    if rom.len() > 524_288 {
        return Err(format!("PROM is {} bytes; the window is 524288", rom.len()));
    }
    let module = fs::read(&options.module).map_err(|error| format!("read MCP module: {error}"))?;
    if module.len() > 2 * 1024 * 1024 {
        return Err(format!(
            "MCP module is {} bytes; the slot is 2 MiB",
            module.len()
        ));
    }
    let rom_sha = sha256::digest(&rom);
    let module_sha = sha256::digest(&module);
    Ok((module, rom_sha, module_sha))
}

fn up(options: &Options) -> Result<i32, String> {
    let (fixture, running, facts) = bring_up(options)?;
    println!("two nodes are up");
    println!("{}", rig::node_table(&nodes_of(&running)));
    for (node, fact) in running.iter().zip(facts.iter()) {
        println!(
            "{}: {}",
            node.node.name,
            fact.boot_ready.as_deref().unwrap_or("no BOOT-READY")
        );
    }
    println!(
        "module was served during boot from 127.0.0.1:{}; serial logs under {}",
        fixture.port,
        options.work.display()
    );
    println!("status: m2-rig status --work {}", options.work.display());
    println!("stop with: m2-rig down --work {}", options.work.display());
    Ok(0)
}

fn down(options: &Options) -> Result<i32, String> {
    let nodes = derive_nodes(options)?;
    let mut failed = false;
    for node in &nodes {
        if node.ci_socket.exists() {
            match run_ci(options, node, &["quit"]) {
                Ok(_) => println!("{}: stopped", node.name),
                Err(error) => {
                    eprintln!("{}: {error}", node.name);
                    failed = true;
                }
            }
        } else {
            println!("{}: not running", node.name);
        }
    }
    Ok(if failed { 1 } else { 0 })
}

fn status(options: &Options) -> Result<i32, String> {
    let nodes = derive_nodes(options)?;
    let mut failed = false;
    for node in &nodes {
        println!("== {} (instance {})", node.name, node.instance);
        match run_ci(options, node, &["status"]) {
            Ok(text) => {
                for line in text.lines() {
                    println!("  {line}");
                }
            }
            Err(error) => {
                println!("  {error}");
                failed = true;
            }
        }
        let facts = BootFacts::from_serial(&read_serial(node));
        if let Some(line) = &facts.boot_ready {
            println!("  {line}");
        } else {
            println!("  no BOOT-READY");
            failed = true;
        }
        if let Some(line) = &facts.mesh {
            println!("  {line}");
        }
        if let Some(line) = &facts.network_ip {
            println!("  {line}");
        }
        if let Err(error) = facts.verify(node) {
            println!("  identity check failed: {error}");
            failed = true;
        }
    }
    Ok(if failed { 1 } else { 0 })
}

fn probe_only(options: &Options) -> Result<i32, String> {
    let nodes = derive_nodes(options)?;
    let (probes, relays) = probe_two_way(&nodes)?;
    let (mesh, mesh_relay) = probe_mesh(&nodes);
    let http_ok = probes.iter().all(|probe| probe.ok);
    let passed = http_ok && mesh.ok;
    print_probe_table(&probes);
    print_mesh_result(&mesh);
    let error = if !http_ok {
        Some("one or both HTTP probes failed")
    } else if !mesh.ok {
        Some(mesh.message.as_str())
    } else {
        None
    };
    let directory = write_evidence(
        options,
        &nodes,
        &[],
        &probes,
        &relays,
        Some(&mesh),
        mesh_relay.as_ref(),
        None,
        "probe",
        if passed { "pass" } else { "fail" },
        error,
    )?;
    println!("evidence: {}", directory.display());
    Ok(if passed { 0 } else { 1 })
}

fn run_and_probe(options: &Options) -> Result<i32, String> {
    let (module, _rom_sha, module_sha) = check_prerequisites(options)?;
    let module_sha_text = sha256::hex(&module_sha);
    let fixture = Fixture::start(module).map_err(|error| format!("start fixture: {error}"))?;
    let nodes = derive_nodes(options)?;

    let mut running = Vec::new();
    let mut facts = Vec::new();
    let mut bring_up_error = None;
    if let Err(error) = start_all(
        options,
        &nodes,
        fixture.port,
        &module_sha_text,
        &mut running,
    ) {
        bring_up_error = Some(error);
    }
    if bring_up_error.is_none() {
        match wait_boot(options, &mut running) {
            Ok(value) => facts = value,
            Err(error) => bring_up_error = Some(error),
        }
    }

    let mut probes = Vec::new();
    let mut relays = Vec::new();
    let mut mesh = None;
    let mut mesh_relay = None;
    let mut probe_error = None;
    if bring_up_error.is_none() {
        match probe_two_way(&nodes) {
            Ok((value, value_relays)) => {
                probes = value;
                relays = value_relays;
            }
            Err(error) => probe_error = Some(error),
        }
        let (value, relay) = probe_mesh(&nodes);
        mesh = Some(value);
        mesh_relay = relay;
        // The mesh probe appends lines after the boot facts were captured;
        // re-read the transcripts so the evidence bundle carries them.
        facts = nodes
            .iter()
            .map(|node| BootFacts::from_serial(&read_serial(node)))
            .collect();
    } else {
        // Keep the evidence useful even when a node never booted: record what
        // the serial logs show and do not invent a probe result.
        for node in &nodes {
            facts.push(BootFacts::from_serial(&read_serial(node)));
        }
    }

    let mesh_ok = mesh.as_ref().is_some_and(|mesh| mesh.ok);
    let passed = bring_up_error.is_none()
        && probe_error.is_none()
        && probes.len() == 2
        && probes.iter().all(|probe| probe.ok)
        && mesh_ok;
    let verdict = if passed { "pass" } else { "fail" };
    let error = bring_up_error
        .as_deref()
        .or(probe_error.as_deref())
        .or_else(|| {
            mesh.as_ref()
                .filter(|mesh| !mesh.ok)
                .map(|mesh| mesh.message.as_str())
        })
        .map(str::to_string);

    if !options.keep {
        let errors = rig::stop_nodes(options, &mut running);
        for error in errors {
            eprintln!("m2-rig: {error}");
        }
    }

    print_probe_table(&probes);
    if let Some(mesh) = &mesh {
        print_mesh_result(mesh);
    }
    for (node, fact) in running.iter().zip(facts.iter()) {
        println!(
            "{}: {}",
            node.node.name,
            fact.boot_ready.as_deref().unwrap_or("no BOOT-READY")
        );
    }

    let directory = write_evidence(
        options,
        &nodes,
        &facts,
        &probes,
        &relays,
        mesh.as_ref(),
        mesh_relay.as_ref(),
        Some(&fixture),
        "run",
        verdict,
        error.as_deref(),
    )?;
    println!("evidence: {}", directory.display());
    if passed {
        Ok(0)
    } else {
        if let Some(error) = &error {
            eprintln!("m2-rig: {error}");
        }
        Ok(1)
    }
}

fn start_all(
    options: &Options,
    nodes: &[Node],
    fixture_port: u16,
    module_sha256: &str,
    running: &mut Vec<RunningNode>,
) -> Result<(), String> {
    let authority = nodes[0].mac_string();
    let peer0 = format!("{}:{}", nodes[0].gateway, nodes[1].mesh_port);
    let peer1 = format!("{}:{}", nodes[1].gateway, nodes[0].mesh_port);
    write_nvram(&nodes[0], fixture_port, module_sha256, &peer0, &authority)?;
    write_nvram(&nodes[1], fixture_port, module_sha256, &peer1, &authority)?;

    for node in nodes {
        match start_node(options, node) {
            Ok(child) => running.push(child),
            Err(error) => {
                let _ = rig::stop_nodes(options, running);
                return Err(error);
            }
        }
    }
    for child in running.iter() {
        if let Err(error) = wait_ci_ready(options, &child.node, Duration::from_secs(20)) {
            let _ = rig::stop_nodes(options, running);
            return Err(error);
        }
        if let Err(error) = start_cpu(options, &child.node) {
            let _ = rig::stop_nodes(options, running);
            return Err(error);
        }
    }
    Ok(())
}

fn bring_up(options: &Options) -> Result<(Fixture, Vec<RunningNode>, Vec<BootFacts>), String> {
    let (module, _rom_sha, module_sha) = check_prerequisites(options)?;
    let module_sha_text = sha256::hex(&module_sha);
    let fixture = Fixture::start(module).map_err(|error| format!("start fixture: {error}"))?;
    let nodes = derive_nodes(options)?;
    let mut running = Vec::new();
    start_all(
        options,
        &nodes,
        fixture.port,
        &module_sha_text,
        &mut running,
    )?;
    let facts = match wait_boot(options, &mut running) {
        Ok(facts) => facts,
        Err(error) => {
            let _ = rig::stop_nodes(options, &mut running);
            return Err(error);
        }
    };
    Ok((fixture, running, facts))
}

fn probe_two_way(nodes: &[Node]) -> Result<ProbeSet, String> {
    // `BOOT-READY` is printed as soon as the module entry returns; the module
    // arms its first listener on a later idle tick, after the autoload
    // socket has been removed from the shared set.  Give both guests time to
    // settle before the first host connection so the forward never races the
    // listener and resets the connection.  The emulated MIPS CPU and the
    // module tick are slow enough that a couple of seconds is not reliable.
    std::thread::sleep(Duration::from_secs(10));
    let relay_n0_n1 = Relay::start((Ipv4Addr::LOCALHOST, nodes[1].http_port).into())
        .map_err(|error| format!("start relay n0->n1: {error}"))?;
    let relay_n1_n0 = Relay::start((Ipv4Addr::LOCALHOST, nodes[0].http_port).into())
        .map_err(|error| format!("start relay n1->n0: {error}"))?;

    let result_n0_n1 =
        probe::probe_http(&nodes[0], nodes[0].gateway, relay_n0_n1.port, PROBE_TIMEOUT);
    let url_n0_n1 = format!("http://{}:{}/", nodes[0].gateway, relay_n0_n1.port);
    let relay_text_n0_n1 = format!(
        "127.0.0.1:{} -> 127.0.0.1:{}",
        relay_n0_n1.port, nodes[1].http_port
    );
    let mut probes = vec![probe_evidence(
        &nodes[0],
        &nodes[1],
        url_n0_n1,
        relay_text_n0_n1,
        result_n0_n1,
    )];

    let result_n1_n0 =
        probe::probe_http(&nodes[1], nodes[1].gateway, relay_n1_n0.port, PROBE_TIMEOUT);
    let url_n1_n0 = format!("http://{}:{}/", nodes[1].gateway, relay_n1_n0.port);
    let relay_text_n1_n0 = format!(
        "127.0.0.1:{} -> 127.0.0.1:{}",
        relay_n1_n0.port, nodes[0].http_port
    );
    probes.push(probe_evidence(
        &nodes[1],
        &nodes[0],
        url_n1_n0,
        relay_text_n1_n0,
        result_n1_n0,
    ));

    Ok((
        probes,
        vec![
            ("n0-to-n1".into(), relay_n0_n1),
            ("n1-to-n0".into(), relay_n1_n0),
        ],
    ))
}

/// Wait for the two guest mesh listeners to exchange framed envelopes through
/// the host mesh relay.
///
/// This is the M2-1 framing probe: the guests generate and decode the
/// `MeshEnvelope` records themselves; the relay only pumps bytes.  The pass
/// line includes the peer `NodeId`, frame counts and reject count, so the rig
/// can prove that each kernel decoded a header from the other real node.
fn probe_mesh(nodes: &[Node]) -> (MeshProbeEvidence, Option<MeshRelay>) {
    let relay_text = format!(
        "127.0.0.1:{} <-> 127.0.0.1:{}",
        nodes[0].mesh_port, nodes[1].mesh_port
    );
    let relay = match MeshRelay::start(
        (Ipv4Addr::LOCALHOST, nodes[0].mesh_port).into(),
        (Ipv4Addr::LOCALHOST, nodes[1].mesh_port).into(),
    ) {
        Ok(relay) => relay,
        Err(error) => {
            return (
                MeshProbeEvidence {
                    ok: false,
                    message: format!("start mesh relay: {error}"),
                    relay: relay_text,
                    nodes: nodes
                        .iter()
                        .map(|node| MeshNodeProbe {
                            name: node.name.clone(),
                            listen: None,
                            pass: None,
                        })
                        .collect(),
                },
                None,
            )
        }
    };

    let deadline = Instant::now() + Duration::from_secs(45);
    loop {
        let facts: Vec<BootFacts> = nodes
            .iter()
            .map(|node| BootFacts::from_serial(&read_serial(node)))
            .collect();
        if facts.iter().all(|fact| fact.mesh_transport_pass.is_some()) {
            break;
        }
        if Instant::now() >= deadline {
            break;
        }
        std::thread::sleep(Duration::from_millis(250));
    }

    let facts: Vec<BootFacts> = nodes
        .iter()
        .map(|node| BootFacts::from_serial(&read_serial(node)))
        .collect();
    let mut errors = Vec::new();
    for (index, fact) in facts.iter().enumerate() {
        let node = &nodes[index];
        if fact.mesh_transport_listen.is_none() {
            errors.push(format!("{}: no mesh-transport listening line", node.name));
        }
        match &fact.mesh_transport_pass {
            None => errors.push(format!("{}: no mesh-transport pass line", node.name)),
            Some(line) => {
                let expected = nodes[1 - index].node_id_string();
                if !line
                    .to_ascii_uppercase()
                    .contains(&format!("PEER={expected}"))
                {
                    errors.push(format!(
                        "{}: peer NodeId is not {}: {line}",
                        node.name, expected
                    ));
                }
                if !line.contains("rejects=0") {
                    errors.push(format!(
                        "{}: transport reject counter is not zero",
                        node.name
                    ));
                }
                if counter(line, "rx_frames=").unwrap_or(0) == 0 {
                    errors.push(format!("{}: no received frames", node.name));
                }
                if counter(line, "tx_frames=").unwrap_or(0) == 0 {
                    errors.push(format!("{}: no sent frames", node.name));
                }
            }
        }
    }
    let relay_lines = relay::lines(&relay.log);
    if !relay_lines
        .iter()
        .any(|line| line.contains("established after first bytes both ways"))
    {
        errors.push("mesh relay did not establish first bytes in both directions".into());
    }

    let pass_lines = nodes
        .iter()
        .zip(facts.iter())
        .filter_map(|(node, fact)| {
            fact.mesh_transport_pass
                .as_ref()
                .map(|line| format!("{}: {line}", node.name))
        })
        .collect::<Vec<_>>();
    let message = if errors.is_empty() {
        format!(
            "both kernels exchanged framed MeshEnvelopes and decoded the peer NodeId ({})",
            pass_lines.join("; ")
        )
    } else {
        errors.join("; ")
    };
    (
        MeshProbeEvidence {
            ok: errors.is_empty(),
            message,
            relay: relay_text,
            nodes: nodes
                .iter()
                .zip(facts.iter())
                .map(|(node, fact)| MeshNodeProbe {
                    name: node.name.clone(),
                    listen: fact.mesh_transport_listen.clone(),
                    pass: fact.mesh_transport_pass.clone(),
                })
                .collect(),
        },
        Some(relay),
    )
}

fn counter(line: &str, key: &str) -> Option<u64> {
    let start = line.find(key)? + key.len();
    let digits: String = line[start..]
        .chars()
        .take_while(|character| character.is_ascii_digit())
        .collect();
    digits.parse().ok()
}

fn print_mesh_result(mesh: &MeshProbeEvidence) {
    println!(
        "mesh {}: {} ({})",
        mesh.relay,
        if mesh.ok { "pass" } else { "FAIL" },
        mesh.message
    );
    for node in &mesh.nodes {
        if let Some(line) = &node.pass {
            println!("  {line}");
        }
    }
}

fn probe_evidence(
    from: &Node,
    to: &Node,
    url: String,
    relay: String,
    result: Result<ProbeOutcome, ProbeError>,
) -> ProbeEvidence {
    match result {
        Ok(outcome) => ProbeEvidence {
            from: from.name.clone(),
            to: to.name.clone(),
            url,
            relay,
            ok: true,
            message: format!(
                "HTTP {} from {}'s HTTP client, {} bytes",
                outcome.status, from.name, outcome.bytes
            ),
            status: Some(outcome.status),
            bytes: Some(outcome.bytes),
            sha256: outcome.sha256,
            text: outcome.text,
            raw: outcome.transcript,
        },
        Err(error) => ProbeEvidence {
            from: from.name.clone(),
            to: to.name.clone(),
            url,
            relay,
            ok: false,
            message: error.message,
            status: None,
            bytes: None,
            sha256: None,
            text: error.text,
            raw: error.transcript,
        },
    }
}

fn print_probe_table(probes: &[ProbeEvidence]) {
    for probe in probes {
        println!(
            "{} -> {}: {} ({})",
            probe.from,
            probe.to,
            if probe.ok { "pass" } else { "FAIL" },
            probe.message
        );
        if probe.ok {
            if let (Some(status), Some(bytes)) = (probe.status, probe.bytes) {
                println!("  http {status}, {bytes} bytes");
            }
            if let Some(sha) = &probe.sha256 {
                println!("  sha256 {sha}");
            }
        }
    }
}

fn nodes_of(running: &[RunningNode]) -> Vec<Node> {
    running.iter().map(|child| child.node.clone()).collect()
}

fn print_config(options: &Options) -> Result<i32, String> {
    let nodes = derive_nodes(options)?;
    println!("{}", rig::node_table(&nodes));
    let authority = nodes[0].mac_string();
    let module_sha = fs::read(&options.module)
        .map(|bytes| sha256::hex(&sha256::digest(&bytes)))
        .unwrap_or_else(|_| "<module sha256>".into());
    for (index, node) in nodes.iter().enumerate() {
        let peer = &nodes[1 - index];
        let mesh_peer = format!("{}:{}", node.gateway, peer.mesh_port);
        println!("env {}:", node.name);
        println!("  mcp.url=http://{}:<fixture>/mcp.module", node.gateway);
        println!("  mcp.sha256={module_sha}");
        println!("  mesh.peer={mesh_peer}");
        println!("  mesh.authority={authority}");
        println!("  mesh.epoch=1");
        println!("  console=serial0");
    }
    println!("HTTP probe relays are allocated on ephemeral loopback ports at probe time");
    println!("n0 -> n1 HTTP target: 127.0.0.1:{}", nodes[1].http_port);
    println!("n1 -> n0 HTTP target: 127.0.0.1:{}", nodes[0].http_port);
    println!(
        "mesh relay: 127.0.0.1:{} <-> 127.0.0.1:{}",
        nodes[0].mesh_port, nodes[1].mesh_port
    );
    Ok(0)
}

fn clean(options: &Options) -> Result<i32, String> {
    for instance in 0..2 {
        let socket = options.node_dir(instance).join("iris.sock");
        if socket.exists() {
            return Err(format!(
                "{} exists; run `m2-rig down` before clean",
                socket.display()
            ));
        }
    }
    if options.work.exists() {
        fs::remove_dir_all(&options.work)
            .map_err(|error| format!("remove {}: {error}", options.work.display()))?;
        println!("removed {}", options.work.display());
    } else {
        println!("{} does not exist", options.work.display());
    }
    Ok(0)
}

/// Build and write the evidence bundle.
#[allow(clippy::too_many_arguments)]
fn write_evidence(
    options: &Options,
    nodes: &[Node],
    facts: &[BootFacts],
    probes: &[ProbeEvidence],
    relays: &[(String, Relay)],
    mesh: Option<&MeshProbeEvidence>,
    mesh_relay: Option<&MeshRelay>,
    fixture: Option<&Fixture>,
    action: &str,
    verdict: &str,
    error: Option<&str>,
) -> Result<std::path::PathBuf, String> {
    let node_evidence: Vec<NodeEvidence> = nodes
        .iter()
        .enumerate()
        .map(|(index, node)| {
            let facts = facts
                .get(index)
                .cloned()
                .unwrap_or_else(|| BootFacts::from_serial(&read_serial(node)));
            NodeEvidence::capture(node, facts)
        })
        .collect();
    let rom_sha = sha256::hex(&sha256::digest(
        &fs::read(&options.rom).map_err(|error| format!("read PROM: {error}"))?,
    ));
    let module_sha = sha256::hex(&sha256::digest(
        &fs::read(&options.module).map_err(|error| format!("read module: {error}"))?,
    ));
    let branch = git(&options.repo, &["rev-parse", "--abbrev-ref", "HEAD"]);
    let commit = git(&options.repo, &["rev-parse", "HEAD"]);
    let mut relay_logs: Vec<(String, relay::SharedLog)> = relays
        .iter()
        .map(|(name, relay)| (name.clone(), relay.log.clone()))
        .collect();
    if let Some(relay) = mesh_relay {
        relay_logs.push(("mesh".into(), relay.log.clone()));
    }
    let bundle = Evidence {
        work: &options.work,
        action,
        repo: &options.repo,
        branch: &branch,
        commit: &commit,
        rom: &options.rom,
        rom_sha256: &rom_sha,
        module: &options.module,
        module_sha256: &module_sha,
        nodes: &node_evidence,
        probes,
        mesh,
        fixture,
        relays: &relay_logs,
        verdict,
        error,
    };
    evidence::write(&bundle)
}
