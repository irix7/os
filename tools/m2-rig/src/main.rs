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

mod bulk;
mod evidence;
mod failure;
mod fixture;
mod homes;
mod hub;
mod join;
#[allow(dead_code)]
mod latency;
mod migrate;
mod nvram;
mod ports;
mod probe;
mod relay;
mod rig;
mod route;
mod sha256;
mod shard;

use std::fs;
use std::io::{Read, Write};
use std::net::{Ipv4Addr, TcpStream};
use std::time::{Duration, Instant};

use evidence::{Evidence, MeshNodeProbe, MeshProbeEvidence, NodeEvidence, ProbeEvidence};
use failure::FailureEvidence;
use fixture::Fixture;
use homes::HomesEvidence;
use join::{JoinEvidence, RestartInput, RootRestartInput};
use ports::PortsFixture;
use probe::{ProbeError, ProbeOutcome};
use relay::{MeshRelay, Relay};
use rig::{
    derive_nodes, git, mesh_environments, read_serial, run_ci, start_cpu, start_node, wait_boot,
    wait_ci_ready, write_nvram, Action, AuthorityKeys, BootFacts, Fault, MeshEnv, Node, Options,
    RunningNode, Transport,
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

    if !matches!(options.action, Action::Shard | Action::Ports) && options.nodes != 2 {
        return Err("--nodes applies to the shard/ports commands; the M2 run is two-node".into());
    }
    if options.transport != Transport::Relay
        && options.action == Action::Run
        && !matches!(options.fault, Fault::None | Fault::NoRoute)
    {
        return Err("--transport direct is supported for run --fault none/no-route".into());
    }
    match options.action {
        Action::Run => run_and_probe(&options),
        Action::Bulk => bulk_run(&options),
        Action::Shard => shard::run(&options),
        Action::Ports => ports_run(&options),
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
        "m2-rig — independent IRIX 7 kernels over a Rust host relay

Usage:
  m2-rig <command> [options]

Commands:
  run           start both M2 nodes, probe HTTP/mesh/join/route, write evidence
  bulk          drive the #247 64 KiB bulk measurement through the bounded hub
  shard         start the M3 shard slice (two real kernels by default)
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
  --nodes N        shard kernels, 2..=8; ports always uses one (default: 2)
  --hub            shard: use the experimental multi-node host link switch
  --transport M    relay (default), direct, or direct-fail (bulk/run)
  --timeout SECS   boot deadline (default: 180)
  --fault MODE     none (default), no-peer, wrong-authority, no-route,
                   kill-peer or partition
  --keep           with run or shard, leave the nodes running
  --verbose        print extra detail

Workflow:
  tools/m2-rig/build.sh
  cargo run --manifest-path tools/m2-rig/Cargo.toml -- run \\
    --iris <iris> --iris-ci <iris-ci>
  cargo run --manifest-path tools/m2-rig/Cargo.toml -- shard \\
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
    if module.len() > 524_288 {
        return Err(format!(
            "MCP module is {} bytes; slot A is 524288",
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
    let (mesh, mesh_relay) = if options.fault == Fault::NoPeer {
        (None, None)
    } else {
        let (value, relay) = probe_mesh(&nodes);
        (Some(value), relay)
    };
    if options.fault.joins() {
        wait_join(&nodes, Duration::from_secs(60));
        wait_join_ack(&nodes, Duration::from_secs(20));
        wait_homes_serial(&nodes, Duration::from_secs(60));
        std::thread::sleep(Duration::from_secs(10));
    } else if options.fault == Fault::WrongAuthority {
        wait_join_reject(&nodes, Duration::from_secs(30));
        std::thread::sleep(Duration::from_secs(10));
    }
    let facts = snapshot_facts(&nodes);
    let keys = AuthorityKeys::lab();
    let join = join::evaluate(&nodes, &facts, &keys, options.fault, None, None);
    let home_inputs = collect_homes(options, &nodes, &facts, &join);
    let route = match collect_route(options, &nodes, &join) {
        Ok(value) => Some(value),
        Err(error) => {
            eprintln!("m2-rig: route: {error}");
            None
        }
    };
    let homes = homes::evaluate(
        options.fault,
        home_inputs.before,
        None,
        home_inputs.stale,
        home_inputs.disabled,
        home_inputs.wrong_authority,
        false,
    );
    let (probes, relays) = probe_two_way(&nodes)?;
    let http_ok = probes.iter().all(|probe| probe.ok);
    let mesh_ok = mesh.as_ref().is_none_or(|mesh| mesh.ok);
    let route_ok = route.as_ref().is_some_and(|route| route.ok);
    let passed = http_ok && mesh_ok && join.ok && homes.ok && route_ok;
    print_probe_table(&probes);
    if let Some(mesh) = &mesh {
        print_mesh_result(mesh);
    }
    print_join_result(&join);
    print_homes_result(&homes);
    if let Some(route) = &route {
        print_route_result(route);
    }
    let error = if !http_ok {
        Some("one or both HTTP probes failed".to_string())
    } else if !mesh_ok {
        mesh.as_ref().map(|mesh| mesh.message.clone())
    } else if !join.ok {
        Some(join.message.clone())
    } else if !homes.ok {
        Some(homes.message.clone())
    } else if !route_ok {
        route.as_ref().map(|route| route.message.clone())
    } else {
        None
    };
    let directory = write_evidence(
        options,
        &nodes,
        &facts,
        &probes,
        &relays,
        mesh.as_ref(),
        mesh_relay.as_ref(),
        Some(&join),
        Some(&homes),
        route.as_ref(),
        None,
        None,
        None,
        None,
        None,
        "probe",
        if passed { "pass" } else { "fail" },
        error.as_deref(),
    )?;
    println!("evidence: {}", directory.display());
    Ok(if passed { 0 } else { 1 })
}

/// Boot one node running the module TCP slot probe.
///
/// The probe is the transport prerequisite for the multi-node membership
/// work: it opens one socket per kernel slot and requires all of them to be
/// connected at the same time.  The fixture records the echoed lines, so the
/// pass is not taken from the guest's own claim alone.
fn ports_run(options: &Options) -> Result<i32, String> {
    let (module, rom_sha, module_sha) = check_prerequisites(options)?;
    let module_sha_text = sha256::hex(&module_sha);
    let rom_sha_text = sha256::hex(&rom_sha);
    let fixture = Fixture::start(module).map_err(|error| format!("start fixture: {error}"))?;
    let base = options.base_port.saturating_add(2_000);
    let ports = PortsFixture::start(base, 8)
        .map_err(|error| format!("start ports fixture on {base}: {error}"))?;

    let mut single = options.clone();
    single.nodes = 1;
    let nodes = derive_nodes(&single)?;
    let environment = MeshEnv {
        extra: Some(vec![
            ("ports.base".into(), base.to_string()),
            ("ports.count".into(), "8".into()),
        ]),
        ..MeshEnv::default()
    };
    let mut running = Vec::new();
    let started = start_all_with(
        &single,
        &nodes,
        &[Some(environment)],
        fixture.port,
        &module_sha_text,
        &mut running,
    );
    if let Err(error) = started {
        let _ = rig::stop_nodes(&single, &mut running);
        return Err(error);
    }
    let boot = (|| -> Result<(), String> {
        rig::wait_serial(&single, &mut running[0], "BOOT-READY", single.timeout)?;
        let facts = BootFacts::from_serial(&read_serial(&nodes[0]));
        facts.verify_identity(&nodes[0])?;
        Ok(())
    })();
    if let Err(error) = boot {
        let _ = rig::stop_nodes(&single, &mut running);
        return Err(error);
    }

    let deadline = Instant::now() + Duration::from_secs(60);
    let mut marker = None;
    let mut failure = None;
    while Instant::now() < deadline {
        let text = read_serial(&nodes[0]);
        if let Some(line) = text
            .lines()
            .find(|line| line.contains("ports: pass sockets="))
        {
            marker = Some(line.trim().to_string());
            break;
        }
        if let Some(line) = text.lines().find(|line| line.contains("ports: fail ")) {
            failure = Some(line.trim().to_string());
            break;
        }
        std::thread::sleep(Duration::from_millis(250));
    }
    let stop_errors = if single.keep {
        println!(
            "ports: keeping {} up (monitor {}); stop with `m2-rig down`",
            nodes[0].name, nodes[0].monitor
        );
        Vec::new()
    } else {
        rig::stop_nodes(&single, &mut running)
    };
    let serial = read_serial(&nodes[0]);
    let received = ports.lines();
    let complete = ports.complete();
    let ok = marker.is_some() && failure.is_none() && complete && stop_errors.is_empty();
    let message = match (&marker, &failure) {
        (_, Some(failure)) => failure.clone(),
        (Some(marker), None) => marker.clone(),
        (None, None) => "the probe did not report a result".into(),
    };

    let evidence_dir = options.work.join("evidence");
    fs::create_dir_all(&evidence_dir)
        .map_err(|error| format!("create {}: {error}", evidence_dir.display()))?;
    let received_json = received
        .iter()
        .map(|line| format!("\"{line}\""))
        .collect::<Vec<_>>()
        .join(",");
    let document = format!(
        "{{\n  \"ok\": {ok},\n  \"base\": {base},\n  \"count\": {},\n  \"message\": \"{message}\",\n  \"module_sha256\": \"{module_sha_text}\",\n  \"prom_sha256\": \"{rom_sha_text}\",\n  \"received\": [{received_json}],\n  \"serial\": \"{}\"\n}}\n",
        8,
        nodes[0].serial_log.display()
    );
    fs::write(evidence_dir.join("ports.json"), document)
        .map_err(|error| format!("write ports evidence: {error}"))?;
    fs::write(evidence_dir.join("serial.log"), &serial)
        .map_err(|error| format!("write ports serial: {error}"))?;
    for error in stop_errors {
        eprintln!("m2-rig: {error}");
    }
    println!(
        "ports [{}]: {message} ({} listeners at {})",
        if ok { "pass" } else { "FAIL" },
        ports.count,
        ports.base
    );
    println!("{}", rig::node_table(&nodes));
    println!("evidence: {}", evidence_dir.display());
    Ok(if ok { 0 } else { 1 })
}

/// Run the real-kernel failure or partition scenario.
///
/// The base M2-4 sequence has already left a cached route and a lease at the
/// member, so the failure has something real to fence.  The rig does not
/// inject a home, an epoch or a lease: it stops the process or the seam and
/// then asserts the policy's own transition.
fn run_failure_scenario(
    options: &Options,
    nodes: &[Node],
    running: &mut [RunningNode],
    mesh_relay: &mut Option<MeshRelay>,
    join: &JoinEvidence,
    stale_object: Option<&str>,
) -> Result<FailureEvidence, String> {
    let (Some(epoch), Some(digest)) = (join.epoch, join.digest.clone()) else {
        return Err("the failure scenario needs a converged join".into());
    };
    if join.members.len() != 2 {
        return Err(format!(
            "the failure scenario needs two members, got {}",
            join.members.len()
        ));
    }
    let before = failure::expected_view(epoch, &join.members)?;
    if before.digest != digest {
        return Err(format!(
            "the join digest {digest} does not match the recomputed {}",
            before.digest
        ));
    }
    let stale_object =
        stale_object.ok_or_else(|| "the M2-4 route left no stale object to fence".to_string())?;
    match options.fault {
        Fault::KillPeer => {
            let mut kill = || rig::kill_node(&mut running[1]);
            Ok(failure::kill_peer(
                &nodes[0],
                &nodes[1],
                &before,
                stale_object,
                &mut kill,
            ))
        }
        Fault::Partition => {
            let relay = mesh_relay
                .take()
                .ok_or_else(|| "the partition fault needs a live mesh relay".to_string())?;
            let relay_log = relay.log.clone();
            // Drop frames, keep the sessions while the failure detector
            // converges.  The iris port forward cannot reliably reopen a
            // half-open guest connection, so tearing the relay down at the
            // start would test the forward, not the partition rule.
            relay.pause();
            let mut relay = Some(relay);
            let mut reseamed = false;
            let evidence = {
                let mut restore = || {
                    // A cut longer than the module's receive deadline tears a
                    // session mid-record.  Once the seam returns, reset both
                    // guest links through the existing link-only control and
                    // start a fresh relay; this is the same recovery the
                    // member-restart path uses.  Membership, epoch, route and
                    // lease state stay in memory.
                    drop(relay.take());
                    mcp_tool(&nodes[0], "mesh.reconnect")?;
                    mcp_tool(&nodes[1], "mesh.reconnect")?;
                    std::thread::sleep(Duration::from_secs(1));
                    // Let the restart settle before the relay connects; the
                    // guest listeners are armed, but the host forward can
                    // otherwise accept the relay before its guest-side path
                    // is ready.
                    std::thread::sleep(Duration::from_secs(2));
                    let restarted = MeshRelay::start_with_log(
                        (Ipv4Addr::LOCALHOST, nodes[0].mesh_port).into(),
                        (Ipv4Addr::LOCALHOST, nodes[1].mesh_port).into(),
                        relay_log.clone(),
                    )
                    .map_err(|error| format!("restart mesh relay: {error}"))?;
                    *mesh_relay = Some(restarted);
                    reseamed = true;
                    Ok(())
                };
                failure::partition(&nodes[0], &nodes[1], &before, stale_object, &mut restore)
            };
            if !reseamed {
                // The restore step failed before a fresh relay was installed;
                // leave the seam down rather than resuming a stale session.
                let _ = relay.take();
            }
            Ok(evidence)
        }
        _ => Err("not a failure mode".into()),
    }
}

fn collect_route(
    options: &Options,
    nodes: &[Node],
    join: &JoinEvidence,
) -> Result<route::RouteEvidence, String> {
    match options.fault {
        Fault::None | Fault::KillPeer | Fault::Partition => {
            if join.epoch.is_none() || join.digest.is_none() || join.members.len() != 2 {
                return Err("join evidence is not a converged two-node view".into());
            }
            let mut evidence = route::run(
                &nodes[0],
                &nodes[1],
                &join.members,
                join.epoch.unwrap_or(0),
                join.digest.as_deref().unwrap_or_default(),
                Duration::from_secs(180),
            );
            if options.fault.failures() {
                evidence.fault = options.fault.as_str().into();
            }
            Ok(evidence)
        }
        Fault::NoRoute | Fault::NoPeer | Fault::WrongAuthority => {
            route::fetch_disabled(nodes, options.fault, Duration::from_secs(30))
        }
    }
}

#[allow(dead_code)]
fn collect_migration(
    options: &Options,
    nodes: &[Node],
) -> Result<migrate::MigrateEvidence, String> {
    match options.fault {
        Fault::NoPeer | Fault::NoRoute | Fault::WrongAuthority => {
            migrate::fetch_disabled(nodes, options.fault, Duration::from_secs(30))
        }
        Fault::None | Fault::KillPeer | Fault::Partition => {
            Err("the migration scenario is driven by run".into())
        }
    }
}

fn print_failure_result(failure: &FailureEvidence) {
    println!(
        "failure [{}]: {} ({})",
        failure.fault,
        if failure.ok { "pass" } else { "FAIL" },
        failure.message
    );
    for check in &failure.checks {
        println!("  ok: {check}");
    }
    for line in &failure.serial {
        println!("  {line}");
    }
}

fn print_route_result(route: &route::RouteEvidence) {
    println!(
        "route [{}]: {} ({})",
        route.fault,
        if route.ok { "pass" } else { "FAIL" },
        route.message
    );
    for node in &route.nodes {
        match &node.report {
            Some(report) => println!(
                "  {} HTTP {} state={} epoch={:?} old={:?} new={:?}                  moved={}/{} retries={} completes={} rejects={} results={}",
                node.name,
                node.status
                    .map(|value| value.to_string())
                    .unwrap_or_else(|| "-".into()),
                report.state,
                report.view_epoch,
                report.move_old.as_deref(),
                report.move_new.as_deref(),
                report.moved_local,
                report.moved_remote,
                report.retries,
                report.completes,
                report.rejects,
                report.results.len()
            ),
            None => println!(
                "  {} error={}",
                node.name,
                node.error.as_deref().unwrap_or("no report")
            ),
        }
    }
}

fn print_migration_result(migration: &migrate::MigrateEvidence) {
    println!(
        "migration [{}]: {} ({})",
        migration.fault,
        if migration.ok { "pass" } else { "FAIL" },
        migration.message
    );
    for check in &migration.checks {
        println!("  ok: {check}");
    }
    for node in &migration.nodes {
        match &node.report {
            Some(report) => println!(
                "  {} HTTP {} state={} role={:?} epoch={:?} actor={:?} host={:?}",
                node.name,
                node.status
                    .map(|value| value.to_string())
                    .unwrap_or_else(|| "-".into()),
                report.state,
                report.role,
                report.view_epoch,
                report.actor.as_ref().map(|actor| actor.state.as_str()),
                report.actor.as_ref().map(|actor| actor.host.as_str())
            ),
            None => println!(
                "  {} error={}",
                node.name,
                node.error.as_deref().unwrap_or("no report")
            ),
        }
    }
}

fn bulk_run(options: &Options) -> Result<i32, String> {
    let (_module, rom_sha, module_sha) = check_prerequisites(options)?;
    let module = fs::read(&options.module).map_err(|error| format!("read module: {error}"))?;
    let fixture = Fixture::start(module).map_err(|error| format!("start fixture: {error}"))?;
    let nodes = derive_nodes(options)?;
    let mut running = Vec::new();
    let module_sha_text = sha256::hex(&module_sha);
    start_all(
        options,
        &nodes,
        fixture.port,
        &module_sha_text,
        &mut running,
    )?;
    let result = (|| -> Result<bulk::BulkEvidence, String> {
        let _facts = wait_boot(options, &mut running)?;
        // `BOOT-READY` is printed when the module entry returns; the mesh
        // listener is armed on a later module tick.  The iris forward accepts
        // a host connection before the guest socket exists, so start the hub
        // only after both listeners are armed.
        std::thread::sleep(Duration::from_secs(10));
        match options.transport {
            Transport::Relay => {
                let _hub = hub::MeshHub::start_with(&nodes, 1, hub::HubConfig::default())?;
                wait_join(&nodes, Duration::from_secs(60));
                wait_join_ack(&nodes, Duration::from_secs(20));
            }
            Transport::Direct => {
                wait_join(&nodes, Duration::from_secs(60));
                wait_join_ack(&nodes, Duration::from_secs(20));
            }
            Transport::DirectFail => {
                // The lower NodeId dials the dead port and must fall back to
                // its listener.  The higher NodeId is a listener throughout.
                let deadline = std::time::Instant::now() + Duration::from_secs(30);
                loop {
                    let texts: Vec<String> = nodes.iter().map(read_serial).collect();
                    let fallback = texts
                        .iter()
                        .any(|text| text.contains("mesh-transport: direct fallback listen"));
                    let joined = texts
                        .iter()
                        .any(|text| text.contains("mesh-join: converged"));
                    if fallback && !joined {
                        println!("direct-fail: fallback listener observed; no join, as expected");
                        return Ok(bulk::BulkEvidence {
                            ok: true,
                            message: "direct-fail: fallback listener observed; no join".into(),
                            baseline: None,
                            runs: Vec::new(),
                            control: None,
                            cancel: None,
                        });
                    }
                    if joined {
                        return Err("direct-fail joined a dead direct target".into());
                    }
                    if std::time::Instant::now() >= deadline {
                        return Err("direct-fail did not log the fallback listener".into());
                    }
                    std::thread::sleep(Duration::from_millis(500));
                }
            }
        }
        // Let the first home-table serial burst drain before measuring.
        std::thread::sleep(Duration::from_secs(10));
        bulk::run(&nodes[0], &nodes[1])
    })();
    let stop_errors = rig::stop_nodes(options, &mut running);
    let evidence = result?;
    for run in evidence.baseline.iter().chain(evidence.runs.iter()) {
        println!(
            "{} -> {}: {} B in {} x {} B chunks, {} ms, {:.3} MiB/s{}",
            run.from,
            run.to,
            run.bytes,
            run.chunks,
            run.chunk,
            run.elapsed_ms,
            run.mb_per_s,
            if run.chunk == bulk::BULK_BASELINE_CHUNK {
                " (baseline)"
            } else {
                ""
            }
        );
    }
    if let Some(control) = &evidence.control {
        println!(
            "control during bulk: {} B in {} chunks, bulk {} ms, p50 {} ms, p95 {} ms",
            control.bytes,
            control.chunks,
            control.bulk_ms,
            control.control_p50_ms,
            control.control_p95_ms
        );
    }
    if let Some(cancel) = &evidence.cancel {
        println!(
            "cancel: state={} error={} chunks={}",
            cancel.state,
            cancel.error.as_deref().unwrap_or("-"),
            cancel.chunks
        );
    }
    println!("bulk: {}", evidence.message);
    let commit = git(&options.repo, &["rev-parse", "HEAD"]);
    let branch = git(&options.repo, &["rev-parse", "--abbrev-ref", "HEAD"]);
    let mut json = String::new();
    json.push_str(&format!(
        "{{\n  \"action\": \"bulk\",\n  \"branch\": \"{}\",\n  \"commit\": \"{}\",\n  \"rom_sha256\": \"{}\",\n  \"module_sha256\": \"{}\",\n  \"ok\": {},\n  \"message\": \"{}\",\n  \"baseline\": ",
        evidence::json_escape(&branch),
        commit,
        sha256::hex(&rom_sha),
        module_sha_text,
        evidence.ok,
        evidence::json_escape(&evidence.message)
    ));
    match &evidence.baseline {
        Some(run) => json.push_str(&format!(
            "{{\"bytes\": {}, \"chunk\": {}, \"chunks\": {}, \"elapsed_ms\": {}, \"mib_per_s\": {:.4}, \"raw\": \"{}\"}}",
            run.bytes,
            run.chunk,
            run.chunks,
            run.elapsed_ms,
            run.mb_per_s,
            evidence::json_escape(&run.raw)
        )),
        None => json.push_str("null"),
    }
    json.push_str(",\n  \"control\": ");
    match &evidence.control {
        Some(control) => json.push_str(&format!(
            "{{\"bytes\": {}, \"chunks\": {}, \"bulk_ms\": {}, \"control_p50_ms\": {}, \"control_p95_ms\": {}}}",
            control.bytes, control.chunks, control.bulk_ms, control.control_p50_ms, control.control_p95_ms
        )),
        None => json.push_str("null"),
    }
    json.push_str(",\n  \"cancel\": ");
    match &evidence.cancel {
        Some(cancel) => json.push_str(&format!(
            "{{\"state\": \"{}\", \"error\": {}, \"chunks\": {}}}",
            evidence::json_escape(&cancel.state),
            cancel
                .error
                .as_deref()
                .map(|value| format!("\"{}\"", evidence::json_escape(value)))
                .unwrap_or_else(|| "null".into()),
            cancel.chunks
        )),
        None => json.push_str("null"),
    }
    json.push_str(",\n  \"runs\": [");
    for (index, run) in evidence.runs.iter().enumerate() {
        json.push_str(&format!(
            "\n    {{\"from\": \"{}\", \"to\": \"{}\", \"bytes\": {}, \"chunk\": {}, \"chunks\": {}, \"elapsed_ms\": {}, \"mib_per_s\": {:.4}, \"raw\": \"{}\"}}",
            run.from,
            run.to,
            run.bytes,
            run.chunk,
            run.chunks,
            run.elapsed_ms,
            run.mb_per_s,
            evidence::json_escape(&run.raw)
        ));
        if index + 1 < evidence.runs.len() {
            json.push(',');
        }
    }
    json.push_str("\n  ]\n}\n");
    let path = options.work.join("bulk-evidence.json");
    fs::write(&path, json).map_err(|error| format!("write {}: {error}", path.display()))?;
    for error in stop_errors {
        eprintln!("m2-rig: stop: {error}");
    }
    println!("evidence: {}", path.display());
    Ok(if evidence.ok { 0 } else { 1 })
}

fn run_and_probe(options: &Options) -> Result<i32, String> {
    let (module, _rom_sha, module_sha) = check_prerequisites(options)?;
    let module_sha_text = sha256::hex(&module_sha);
    let fixture = Fixture::start(module).map_err(|error| format!("start fixture: {error}"))?;
    let nodes = derive_nodes(options)?;

    let mut running = Vec::new();
    let mut facts;
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
        if let Err(error) = wait_boot(options, &mut running) {
            bring_up_error = Some(error);
        }
    }

    let mut probes = Vec::new();
    let mut relays = Vec::new();
    let mut mesh = None;
    let mut mesh_relay = None;
    let mut probe_error = None;
    let mut join_error = None;
    let mut homes_error = None;
    let latency_evidence: Option<latency::LatencyEvidence> = None;
    let latency_error: Option<String> = None;
    let bulk_evidence: Option<bulk::BulkEvidence> = None;
    let bulk_error: Option<String> = None;
    let mut route = None;
    let mut route_error = None;
    let mut migration = None;
    let mut migration_error = None;
    let mut restart = None;
    let mut root_restart = None;
    let mut failure_evidence: Option<FailureEvidence> = None;
    let mut failure_error = None;
    let mut join_before_evidence: Option<JoinEvidence> = None;
    let mut home_inputs = HomeInputs::default();
    let keys = AuthorityKeys::lab();
    if bring_up_error.is_none() {
        // The guest-level HTTP probe runs after the join and homes collection.
        // The MCP server has one HTTP slot while the mesh owns the other, and
        // the iris forward can hold a half-open guest connection after that
        // probe; collecting homes on the clean control plane first keeps the
        // evidence independent of the forward's close behaviour.
        if options.fault == Fault::NoPeer {
            // No mesh listener and no join: the M2-1 probe is skipped by
            // design and the evidence says so.
        } else if options.transport == Transport::Relay {
            let (value, relay) = probe_mesh(&nodes);
            mesh = Some(value);
            mesh_relay = relay;
        }
        if options.fault.joins() {
            wait_join(&nodes, Duration::from_secs(60));
            // The terminal acknowledgement must cross the link before the
            // member is restarted; otherwise the restart can truncate it and
            // there is no evidence that B installed what A signed.
            wait_join_ack(&nodes, Duration::from_secs(20));
            if options.transport == Transport::Direct {
                mesh = Some(direct_probe(&nodes));
            }
            wait_homes_serial(&nodes, Duration::from_secs(60));
            // The MCP HTTP slot is armed by the module tick after the join
            // and homes logging settles.  Give it the same 10 s the guest HTTP
            // probe uses before the first control-plane fetch.
            std::thread::sleep(Duration::from_secs(10));
        } else if options.fault == Fault::WrongAuthority {
            wait_join_reject(&nodes, Duration::from_secs(30));
            std::thread::sleep(Duration::from_secs(10));
        }
        facts = snapshot_facts(&nodes);

        let join_before = join::evaluate(&nodes, &facts, &keys, options.fault, None, None);
        home_inputs = collect_homes(options, &nodes, &facts, &join_before);
        if let Some(error) = home_inputs.error.take() {
            homes_error = Some(error);
        }
        match collect_route(options, &nodes, &join_before) {
            Ok(value) => {
                if !value.ok {
                    route_error = Some(value.message.clone());
                }
                route = Some(value);
            }
            Err(error) => route_error = Some(error),
        }

        if !options.fault.migrates() && !options.fault.failures() {
            match collect_migration(options, &nodes) {
                Ok(value) => {
                    if !value.ok {
                        migration_error = Some(value.message.clone());
                    }
                    migration = Some(value);
                }
                Err(error) => migration_error = Some(error),
            }
        }

        // The guest-level HTTP probe still runs before the member restart, as
        // in M2-0/M2-1/M2-2.  It runs after the MCP homes fetch because the
        // iris host forward can hold a half-open guest connection after the
        // probe; the control-plane evidence is collected on the clean path.
        // Failure modes must probe while both guests are still alive.  The
        // clean modes probe after the restart gates: the guest HTTP client
        // can leave the single MCP HTTP slot in a close handshake, and the
        // restart path needs that slot for `mesh.reconnect`.
        if options.fault.failures() {
            match settle_and_probe(&nodes) {
                Ok((value, value_relays)) => {
                    probes = value;
                    relays = value_relays;
                }
                Err(error) => probe_error = Some(error),
            }
        }

        // Run the migration on the stable initial pair.  Driving it after a
        // member restart puts a stateful 11-step exchange on a just-reopened
        // iris forward; the restart gates below do not depend on migration
        // state, so this order is both valid and more reliable.
        if options.fault.migrates() && join_error.is_none() {
            if let (Some(epoch), Some(digest)) = (join_before.epoch, join_before.digest.clone()) {
                if join_before.members.len() == 2 {
                    let evidence = migrate::run(
                        &nodes[0],
                        &nodes[1],
                        &join_before.members,
                        epoch,
                        &digest,
                        Duration::from_secs(240),
                    );
                    if !evidence.ok {
                        migration_error = Some(evidence.message.clone());
                    }
                    migration = Some(evidence);
                    facts = snapshot_facts(&nodes);
                } else {
                    migration_error = Some(format!(
                        "the migration scenario needs two members, got {}",
                        join_before.members.len()
                    ));
                }
            } else {
                migration_error =
                    Some("the migration scenario needs a converged epoch/digest".into());
            }
        }
        if options.fault.failures() {
            // The base M2-4 steps have already left a cached route and a
            // lease at the member.  Kill it (or cut the seam) and let the
            // bounded failure detector, not the socket close, drive the
            // transition.
            let stale = route.as_ref().and_then(|route| route.local.clone());
            match run_failure_scenario(
                options,
                &nodes,
                &mut running,
                &mut mesh_relay,
                &join_before,
                stale.as_deref(),
            ) {
                Ok(evidence) => {
                    if !evidence.ok {
                        failure_error = Some(evidence.message.clone());
                    }
                    failure_evidence = Some(evidence);
                }
                Err(error) => failure_error = Some(error),
            }
            join_before_evidence = Some(join_before.clone());
            // The failure/recovery path logs a fresh home group on both
            // nodes.  Wait for the last group to be complete before the
            // snapshot; a partial mirror group parses as a count mismatch.
            wait_homes_serial(&nodes, Duration::from_secs(60));
            facts = snapshot_facts(&nodes);
        } else if options.fault.joins() {
            match restart_member(options, &nodes, &mut running, &facts, &mut mesh_relay) {
                Ok(input) => {
                    restart = Some(input);
                    // The root's transcript gained a confirmation and the
                    // member's transcript is now the rejoin boot.
                    wait_homes_serial(&nodes, Duration::from_secs(60));
                    facts = snapshot_facts(&nodes);
                }
                Err(error) => join_error = Some(error),
            }
        }
        // The root restart is the M2-2 persistence gate.  It runs after the
        // migration so the migration evidence is collected on the original
        // pair; the member stays up while the root resumes from its store.
        if options.fault == Fault::None && join_error.is_none() && migration_error.is_none() {
            match restart_root(options, &nodes, &mut running, &facts, &mut mesh_relay) {
                Ok(input) => {
                    root_restart = Some(input);
                    facts = snapshot_facts(&nodes);
                }
                Err(error) => join_error = Some(error),
            }
        }

        if !options.fault.failures() {
            match settle_and_probe(&nodes) {
                Ok((value, value_relays)) => {
                    probes = value;
                    relays = value_relays;
                }
                Err(error) => probe_error = Some(error),
            }
        }
    } else {
        // Keep the evidence useful even when a node never booted: record what
        // the serial logs show and do not invent a probe result.
        facts = snapshot_facts(&nodes);
    }

    let join = if options.fault.failures() {
        // The pre-failure join is the last two-node convergence in kill-peer;
        // in partition mode the recovery rejoin is asserted by the failure
        // evidence, not by the M2-2 restart oracle.
        join_before_evidence
            .clone()
            .unwrap_or_else(|| join::evaluate(&nodes, &facts, &keys, options.fault, None, None))
    } else {
        join::evaluate(
            &nodes,
            &facts,
            &keys,
            options.fault,
            restart.as_ref(),
            root_restart.as_ref(),
        )
    };
    if matches!(options.fault, Fault::None | Fault::NoRoute) && join_error.is_none() {
        if let (Some(epoch), Some(digest)) = (join.epoch, join.digest.as_deref()) {
            home_inputs.after = Some(homes::collect_current_serial(
                &nodes,
                &facts,
                epoch,
                digest,
                &join.members,
            ));
        } else {
            homes_error = Some("join evidence has no epoch/digest for the rejoin homes".into());
        }
    }
    let homes = homes::evaluate(
        options.fault,
        home_inputs.before,
        home_inputs.after,
        home_inputs.stale,
        home_inputs.disabled,
        home_inputs.wrong_authority,
        !options.fault.failures(),
    );
    let mesh_ok = match options.fault {
        Fault::NoPeer => true,
        _ => mesh.as_ref().is_some_and(|mesh| mesh.ok),
    };
    let route_ok = route.as_ref().is_some_and(|route| route.ok);
    let migration_ok = migration.as_ref().is_some_and(|migration| migration.ok);
    let failure_ok = failure_error.is_none()
        && failure_evidence
            .as_ref()
            .map(|evidence| evidence.ok)
            .unwrap_or(!options.fault.failures());
    let passed = bring_up_error.is_none()
        && probe_error.is_none()
        && join_error.is_none()
        && homes_error.is_none()
        && route_error.is_none()
        && latency_error.is_none()
        && bulk_error.is_none()
        && migration_error.is_none()
        && (options.fault != Fault::None || migration_ok)
        && failure_ok
        && probes.len() == 2
        && probes.iter().all(|probe| probe.ok)
        && mesh_ok
        && join.ok
        && homes.ok
        && route_ok;
    let verdict = if passed { "pass" } else { "fail" };
    let error = bring_up_error
        .as_deref()
        .or(probe_error.as_deref())
        .or(join_error.as_deref())
        .or(homes_error.as_deref())
        .or(route_error.as_deref())
        .or(migration_error.as_deref())
        .or(failure_error.as_deref())
        .or_else(|| {
            mesh.as_ref()
                .filter(|mesh| !mesh.ok)
                .map(|mesh| mesh.message.as_str())
        })
        .or_else(|| (!join.ok).then_some(join.message.as_str()))
        .or_else(|| (!homes.ok).then_some(homes.message.as_str()))
        .or_else(|| {
            route
                .as_ref()
                .filter(|route| !route.ok)
                .map(|route| route.message.as_str())
        })
        .or_else(|| {
            migration
                .as_ref()
                .filter(|migration| !migration.ok)
                .map(|migration| migration.message.as_str())
        })
        .map(str::to_string);

    // Let the emulated UART drain the last pass/reject line before the
    // processes are stopped; a quit can otherwise truncate the final serial
    // line and the evidence reader sees a partial counter list.
    std::thread::sleep(Duration::from_secs(2));
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
    print_join_result(&join);
    print_homes_result(&homes);
    if let Some(route) = &route {
        print_route_result(route);
    }
    if let Some(latency) = &latency_evidence {
        println!("latency: {}", latency.message);
    }
    if let Some(error) = &latency_error {
        println!("latency [FAIL]: {error}");
    }
    if let Some(bulk) = &bulk_evidence {
        println!(
            "bulk [{}]: {}",
            if bulk.ok { "pass" } else { "FAIL" },
            bulk.message
        );
        for run in &bulk.runs {
            println!(
                "  {} -> {}: {} B in {} chunks, {} ms, {:.3} MiB/s",
                run.from, run.to, run.bytes, run.chunks, run.elapsed_ms, run.mb_per_s
            );
        }
    }
    if let Some(error) = &bulk_error {
        println!("bulk [FAIL]: {error}");
    }
    if let Some(migration) = &migration {
        print_migration_result(migration);
    }
    if let Some(failure) = &failure_evidence {
        print_failure_result(failure);
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
        Some(&join),
        Some(&homes),
        route.as_ref(),
        migration.as_ref(),
        latency_evidence.as_ref(),
        bulk_evidence.as_ref(),
        failure_evidence.as_ref(),
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

/// The last raw line containing `needle`, including a final partial line.
///
/// `str::lines()` yields the last line even when the UART has not written its
/// newline yet; `BootFacts` deliberately ignores such a partial tail.
fn raw_line<'a>(text: &'a str, needle: &str) -> Option<&'a str> {
    text.lines().rev().find(|line| line.contains(needle))
}

/// Re-read both transcripts.
fn snapshot_facts(nodes: &[Node]) -> Vec<BootFacts> {
    nodes
        .iter()
        .map(|node| BootFacts::from_serial(&read_serial(node)))
        .collect()
}

/// Wait until both modules have armed their mesh listener.
fn wait_mesh_listen(nodes: &[Node], timeout: Duration) {
    let deadline = Instant::now() + timeout;
    loop {
        let facts = snapshot_facts(nodes);
        if facts
            .iter()
            .all(|fact| fact.mesh_transport_listen.is_some())
        {
            return;
        }
        if Instant::now() >= deadline {
            return;
        }
        std::thread::sleep(Duration::from_millis(250));
    }
}

/// Wait until both nodes print a converged join view, or the deadline passes.
/// Build the mesh-probe evidence for a direct link from the serial transcript.
fn direct_probe(nodes: &[Node]) -> MeshProbeEvidence {
    let mut rows = Vec::new();
    let mut ok = true;
    for node in nodes {
        let text = read_serial(node);
        let listen = text
            .lines()
            .find(|line| line.contains("mesh-transport: direct role="))
            .map(str::to_string);
        let pass = text
            .lines()
            .find(|line| line.contains("mesh-transport: pass peer="))
            .map(str::to_string);
        ok &= listen.is_some() && pass.is_some();
        rows.push(MeshNodeProbe {
            name: node.name.clone(),
            listen,
            pass,
        });
    }
    MeshProbeEvidence {
        ok,
        message: if ok {
            "both kernels connected directly, with no host relay in the path".into()
        } else {
            "a direct link did not reach the pass marker".into()
        },
        relay: "direct: guest-initiated TCP through each NAT peer forward".into(),
        nodes: rows,
    }
}

fn wait_join(nodes: &[Node], timeout: Duration) {
    let deadline = Instant::now() + timeout;
    loop {
        let facts = snapshot_facts(nodes);
        if facts.iter().all(|fact| fact.mesh_join_converged.is_some()) {
            return;
        }
        if Instant::now() >= deadline {
            return;
        }
        std::thread::sleep(Duration::from_millis(250));
    }
}

/// Wait until the root logs the member's join acknowledgement.
fn wait_join_ack(nodes: &[Node], timeout: Duration) {
    let deadline = Instant::now() + timeout;
    loop {
        let facts = snapshot_facts(nodes);
        if !facts[0].mesh_join_acks.is_empty() {
            return;
        }
        if Instant::now() >= deadline {
            return;
        }
        std::thread::sleep(Duration::from_millis(250));
    }
}

/// Count raw lines containing `needle`.
fn raw_count(text: &str, needle: &str) -> usize {
    text.lines().filter(|line| line.contains(needle)).count()
}

/// Wait until both transcripts carry a complete home group.
fn wait_homes_serial(nodes: &[Node], timeout: Duration) {
    let deadline = Instant::now() + timeout;
    loop {
        let facts = snapshot_facts(nodes);
        // Require the *last* table group to declare and carry the same row
        // count.  Earlier groups may already supply eight rows while the new
        // group is still being mirrored; counting all rows would accept the
        // partial tail and the homes parser would then reject it.
        let ready = facts
            .iter()
            .all(|fact| homes::HomeTable::last_group_complete(&fact.mesh_homes_lines.join("\n")));
        if ready || Instant::now() >= deadline {
            return;
        }
        std::thread::sleep(Duration::from_millis(250));
    }
}

/// Wait until the member logs a join rejection (the wrong-authority fault).
fn wait_join_reject(nodes: &[Node], timeout: Duration) {
    let deadline = Instant::now() + timeout;
    loop {
        let facts = snapshot_facts(nodes);
        if facts[1].mesh_join_reject.is_some() {
            return;
        }
        if Instant::now() >= deadline {
            return;
        }
        std::thread::sleep(Duration::from_millis(250));
    }
}

#[derive(Default)]
struct HomeInputs {
    before: Option<homes::HomeRound>,
    after: Option<homes::HomeRound>,
    stale: Option<homes::HomeRound>,
    disabled: Option<homes::HomeRound>,
    wrong_authority: Option<homes::HomeRound>,
    error: Option<String>,
}

/// Collect the homes surface appropriate to the fault mode.
fn collect_homes(
    options: &Options,
    nodes: &[Node],
    facts: &[BootFacts],
    join: &JoinEvidence,
) -> HomeInputs {
    let mut inputs = HomeInputs::default();
    match options.fault {
        Fault::None | Fault::NoRoute | Fault::KillPeer | Fault::Partition => {
            let (Some(epoch), Some(digest)) = (join.epoch, join.digest.as_deref()) else {
                inputs.error = Some("join evidence has no epoch/digest for homes".into());
                return inputs;
            };
            inputs.before = Some(homes::collect_current(
                nodes,
                facts,
                epoch,
                digest,
                &join.members,
                Duration::from_secs(30),
            ));
            inputs.stale = Some(homes::collect_stale(
                nodes,
                1,
                epoch,
                digest,
                Duration::from_secs(30),
            ));
        }
        Fault::NoPeer => {
            inputs.disabled = Some(homes::collect_disabled(nodes, Duration::from_secs(30)));
        }
        Fault::WrongAuthority => {
            inputs.wrong_authority = Some(homes::collect_wrong_authority(
                nodes,
                facts,
                Duration::from_secs(30),
            ));
        }
    }
    inputs
}

fn print_homes_result(homes: &HomesEvidence) {
    println!(
        "homes [{} count={}]: {} ({})",
        homes.object_set,
        homes.count,
        if homes.ok { "pass" } else { "FAIL" },
        homes.message
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
        println!(
            "  {}: {}",
            round.label,
            if round.ok { "pass" } else { "FAIL" }
        );
        for node in &round.nodes {
            match &node.table {
                Some(table) => println!(
                    "    {} {} state={} epoch={:?} count={:?} digest={:?}",
                    node.name, node.path, table.state, table.epoch, table.count, table.digest
                ),
                None => println!(
                    "    {} {} error={}",
                    node.name,
                    node.path,
                    node.error.as_deref().unwrap_or("no response")
                ),
            }
        }
    }
}

/// Call one MCP tool on a guest through its host HTTP forward.
fn mcp_tool(node: &Node, name: &str) -> Result<(), String> {
    let body = format!(
        "{{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"tools/call\",\"params\":{{\"name\":\"{name}\"}}}}"
    );
    let request = format!(
        "POST /mcp HTTP/1.0\r\nHost: localhost\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n",
        body.len()
    );
    let mut last = String::new();
    for attempt in 1..=6 {
        let result = (|| -> Result<(), String> {
            let mut stream = TcpStream::connect((Ipv4Addr::LOCALHOST, node.http_port))
                .map_err(|error| format!("connect {}:{}: {error}", node.name, node.http_port))?;
            stream
                .set_read_timeout(Some(Duration::from_secs(10)))
                .map_err(|error| format!("timeout {}: {error}", node.name))?;
            stream
                .set_write_timeout(Some(Duration::from_secs(10)))
                .map_err(|error| format!("timeout {}: {error}", node.name))?;
            stream
                .write_all(request.as_bytes())
                .and_then(|_| stream.write_all(body.as_bytes()))
                .map_err(|error| format!("write {}: {error}", node.name))?;
            let mut response = String::new();
            let _ = stream.read_to_string(&mut response);
            if response.contains(" 200 ") && response.contains("\"result\"") {
                Ok(())
            } else {
                Err(format!(
                    "{} {name} response was not 200/result: {}",
                    node.name,
                    response.lines().next().unwrap_or("-")
                ))
            }
        })();
        match result {
            Ok(()) => return Ok(()),
            Err(error) => {
                last = error;
                if attempt < 6 {
                    std::thread::sleep(Duration::from_secs(2));
                }
            }
        }
    }
    Err(last)
}

/// Stop and restart the member, then wait for it to rejoin.
///
/// The root's listener and the host mesh relay stay up, so this is the
/// "member restart, root still authoritative" case: the same epoch must come
/// back.  The first transcript is copied before the restart because
/// `start_node` owns the serial log from its first byte.
fn restart_member(
    options: &Options,
    nodes: &[Node],
    running: &mut [RunningNode],
    facts_before: &[BootFacts],
    mesh_relay: &mut Option<MeshRelay>,
) -> Result<RestartInput, String> {
    let node = &nodes[1];
    let first = facts_before[1]
        .mesh_join_converged
        .as_deref()
        .and_then(join::JoinLine::parse)
        .ok_or_else(|| "n1 did not converge before the restart".to_string())?;
    let boot1_log = options.work.join("serial-n1-boot1.log");
    // Pause the byte pump while the member is down.  Otherwise the supervisor
    // repeatedly connects to the live root's forward and drops the connection,
    // which leaves the iris forward holding half-open guest attempts.  The log
    // is reused so the evidence keeps the pre-restart history.
    // Count raw lines, not `BootFacts`: the heartbeat disconnect is often the
    // last line in the transcript and has no trailing newline yet.  A fact
    // parser that ignores a partial tail would never see it.
    let root_before = read_serial(&nodes[0]);
    let disconnects_before = raw_count(&root_before, "mesh-transport: disconnected");
    let connected_before = raw_count(&root_before, "mesh-transport: connected");
    let already_down = connected_before <= disconnects_before;
    let relay_log = mesh_relay.take().map(|relay| {
        let log = relay.log.clone();
        drop(relay);
        log
    });
    std::thread::sleep(Duration::from_secs(1));
    let stopped_at = Instant::now();
    rig::stop_node(options, &mut running[1])?;
    if options.verbose {
        eprintln!(
            "m2-rig: n1 stopped at {:?}; waiting for n0 to leave the link",
            stopped_at.elapsed()
        );
    }
    fs::copy(&node.serial_log, &boot1_log)
        .map_err(|error| format!("copy {}: {error}", node.serial_log.display()))?;
    // Wait until the root has left its old connection and re-armed its
    // listener.  The iris port forward does not reliably propagate the peer's
    // close to the guest, so the heartbeat timeout is the root's only signal;
    // connecting to its forward before that leaves half-open guest attempts in
    // the forward and the restart never converges.
    let deadline = Instant::now() + Duration::from_secs(60);
    let mut last_note = Instant::now();
    if already_down && options.verbose {
        eprintln!(
            "m2-rig: n0 was already between links before n1 stopped (connected={connected_before}, disconnected={disconnects_before})"
        );
    }
    loop {
        let text = read_serial(&nodes[0]);
        let disconnects = raw_count(&text, "mesh-transport: disconnected");
        // If the root was already between links when the member stopped, a
        // further disconnect will never be logged; its listener is already
        // re-armed and ready for the new relay.
        if already_down || disconnects > disconnects_before {
            if options.verbose {
                eprintln!(
                    "m2-rig: n0 left the old link at {:?} (disconnects {})",
                    stopped_at.elapsed(),
                    disconnects
                );
            }
            break;
        }
        if options.verbose && last_note.elapsed() >= Duration::from_secs(5) {
            last_note = Instant::now();
            eprintln!(
                "m2-rig: waiting for n0 disconnect at {:?} (lines={}, disconnects={})",
                stopped_at.elapsed(),
                text.lines().count(),
                disconnects
            );
        }
        if Instant::now() >= deadline {
            return Err(format!(
                "n0 did not leave the old mesh link before the restart\n--- serial ---\n{}",
                rig::tail(&read_serial(&nodes[0]), 40)
            ));
        }
        std::thread::sleep(Duration::from_millis(250));
    }
    std::thread::sleep(Duration::from_secs(1));
    // A clean quit normally unlinks the CI socket; remove any stale path
    // before the new process binds it.
    let _ = fs::remove_file(&node.ci_socket);
    let child = start_node(options, node)?;
    running[1] = child;
    wait_ci_ready(options, node, Duration::from_secs(20))?;
    start_cpu(options, node)?;
    rig::wait_serial(options, &mut running[1], "BOOT-READY", options.timeout)?;
    let marker = if options.transport == Transport::Direct {
        "mesh-transport: direct role="
    } else {
        "mesh-transport: listening"
    };
    rig::wait_serial(options, &mut running[1], marker, Duration::from_secs(60))?;
    if options.transport == Transport::Relay {
        // Reset the live root's old guest-side socket; the forward can keep
        // delivering new requests to it, so the new relay would never reach
        // the listener.
        mcp_tool(&nodes[0], "mesh.reconnect")?;
        std::thread::sleep(Duration::from_secs(1));
        std::thread::sleep(Duration::from_secs(2));
        if let Some(log) = relay_log {
            let relay = MeshRelay::start_with_log(
                (Ipv4Addr::LOCALHOST, nodes[0].mesh_port).into(),
                (Ipv4Addr::LOCALHOST, nodes[1].mesh_port).into(),
                log,
            )
            .map_err(|error| format!("restart mesh relay: {error}"))?;
            *mesh_relay = Some(relay);
        } else {
            let relay = MeshRelay::start(
                (Ipv4Addr::LOCALHOST, nodes[0].mesh_port).into(),
                (Ipv4Addr::LOCALHOST, nodes[1].mesh_port).into(),
            )
            .map_err(|error| format!("restart mesh relay: {error}"))?;
            *mesh_relay = Some(relay);
        }
    }
    let deadline = Instant::now() + Duration::from_secs(90);
    loop {
        let text = read_serial(node);
        if text
            .lines()
            .any(|line| line.contains("mesh-join: converged "))
            || text
                .lines()
                .any(|line| line.contains("mesh-join: duplicate "))
            || text
                .lines()
                .any(|line| line.contains("mesh-join: refreshed "))
        {
            break;
        }
        if Instant::now() >= deadline {
            return Err(format!(
                "n1 did not rejoin before the deadline\n--- serial ---\n{}",
                rig::tail(&read_serial(node), 40)
            ));
        }
        std::thread::sleep(Duration::from_millis(250));
    }
    let text = read_serial(node);
    let rejoin = raw_line(&text, "mesh-join: converged ")
        .and_then(join::JoinLine::parse)
        .or_else(|| {
            raw_line(&text, "mesh-join: duplicate ")
                .and_then(|line| join::JoinLine::parse_with_role(line, "member"))
        })
        .or_else(|| {
            raw_line(&text, "mesh-join: local ")
                .and_then(|line| join::JoinLine::parse_with_role(line, "member"))
        })
        .or_else(|| {
            raw_line(&text, "mesh-join: refreshed ")
                .and_then(|line| join::JoinLine::parse_with_role(line, "member"))
        })
        .ok_or_else(|| "n1 rejoin line is malformed".to_string())?;
    let request_last_epoch =
        raw_line(&text, "mesh-join: request ").and_then(join::request_last_epoch);
    let store_epoch = raw_line(&text, "epoch-store: restored")
        .filter(|line| line.contains("source=confirmation"))
        .and_then(|line| join::field_u32(line, "epoch="));
    let root_text = read_serial(&nodes[0]);
    let confirm_epoch = raw_line(&root_text, "mesh-join: confirm ")
        .and_then(|line| join::field_u32(line, "epoch="));
    Ok(RestartInput {
        node: node.name.clone(),
        boot1_log,
        first,
        rejoin,
        request_last_epoch,
        confirm_epoch,
        store_restored: store_epoch.is_some(),
        store_epoch,
    })
}

/// Stop and restart the root while the member stays up.
///
/// The root must recover the signed confirmed epoch from its scratch store,
/// and the live member must present that epoch rather than the seed.  This is
/// the case the M2-ADR-0005 seed fallback could not survive.
fn restart_root(
    options: &Options,
    nodes: &[Node],
    running: &mut [RunningNode],
    facts_before: &[BootFacts],
    mesh_relay: &mut Option<MeshRelay>,
) -> Result<RootRestartInput, String> {
    let root = &nodes[0];
    let first = join::node_line(&facts_before[0], "root")
        .ok_or_else(|| "n0 did not converge before the root restart".to_string())?;
    let boot1_log = options.work.join("serial-n0-boot1.log");
    let member_before = read_serial(&nodes[1]);
    let member_offset = member_before.len();
    // Raw line count: the final disconnect may not have a trailing newline.
    let disconnects_before = raw_count(&member_before, "mesh-transport: disconnected");
    let connected_before = raw_count(&member_before, "mesh-transport: connected");
    let member_already_down = connected_before <= disconnects_before;
    let relay_log = mesh_relay.take().map(|relay| {
        let log = relay.log.clone();
        drop(relay);
        log
    });
    std::thread::sleep(Duration::from_secs(1));
    rig::stop_node(options, &mut running[0])?;
    fs::copy(&root.serial_log, &boot1_log)
        .map_err(|error| format!("copy {}: {error}", root.serial_log.display()))?;

    // Wait until the live member has observed the old link close and re-armed
    // its session.  The iris forward does not reliably propagate a peer close;
    // the heartbeat timeout is the member's only signal.
    let deadline = Instant::now() + Duration::from_secs(90);
    loop {
        let text = read_serial(&nodes[1]);
        let tail = text.get(member_offset..).unwrap_or("");
        if member_already_down
            || raw_count(tail, "mesh-transport: disconnected") > disconnects_before
        {
            break;
        }
        if Instant::now() >= deadline {
            return Err(format!(
                "n1 did not leave the old mesh link before the root restart\n--- serial tail ---\n{}",
                rig::tail(&text, 40)
            ));
        }
        std::thread::sleep(Duration::from_millis(250));
    }
    std::thread::sleep(Duration::from_secs(1));
    let _ = fs::remove_file(&root.ci_socket);
    let child = start_node(options, root)?;
    running[0] = child;
    wait_ci_ready(options, root, Duration::from_secs(20))?;
    start_cpu(options, root)?;
    rig::wait_serial(options, &mut running[0], "BOOT-READY", options.timeout)?;
    rig::wait_serial(
        options,
        &mut running[0],
        "mesh-transport: listening",
        Duration::from_secs(60),
    )?;
    // The member kept its old socket while the root was down; reset it for the
    // same reason as the member-restart path.
    mcp_tool(&nodes[1], "mesh.reconnect")?;
    std::thread::sleep(Duration::from_secs(1));
    std::thread::sleep(Duration::from_secs(2));
    if let Some(log) = relay_log {
        let relay = MeshRelay::start_with_log(
            (Ipv4Addr::LOCALHOST, nodes[0].mesh_port).into(),
            (Ipv4Addr::LOCALHOST, nodes[1].mesh_port).into(),
            log,
        )
        .map_err(|error| format!("restart mesh relay: {error}"))?;
        *mesh_relay = Some(relay);
    } else {
        let relay = MeshRelay::start(
            (Ipv4Addr::LOCALHOST, nodes[0].mesh_port).into(),
            (Ipv4Addr::LOCALHOST, nodes[1].mesh_port).into(),
        )
        .map_err(|error| format!("restart mesh relay: {error}"))?;
        *mesh_relay = Some(relay);
    }

    // Wait for the restored root view, the member's re-presentation of the
    // confirmed epoch, and a fresh confirmation.  A seed fallback shows up as
    // a missing restore line or as `epoch-regression`.
    let deadline = Instant::now() + Duration::from_secs(120);
    loop {
        let root_text = read_serial(root);
        let member_text = read_serial(&nodes[1]);
        let member_tail = member_text.get(member_offset..).unwrap_or("");
        let restored = raw_line(&root_text, "epoch-store: restored")
            .is_some_and(|line| line.contains("source=root-record"));
        let presented = member_tail
            .lines()
            .filter(|line| line.contains("mesh-join: request "))
            .filter_map(join::request_last_epoch)
            .any(|epoch| epoch == first.epoch);
        if restored
            && presented
            && raw_line(&root_text, "mesh-join: confirm ").is_some()
            && raw_line(&root_text, "mesh-join: ack from").is_some()
        {
            break;
        }
        if Instant::now() >= deadline {
            return Err(format!(
                "n0 did not restore epoch {} before the deadline\n--- n0 serial ---\n{}\n--- n1 tail ---\n{}",
                first.epoch,
                rig::tail(&root_text, 40),
                rig::tail(member_tail, 40)
            ));
        }
        std::thread::sleep(Duration::from_millis(250));
    }

    let root_text = read_serial(root);
    let local_epoch =
        raw_line(&root_text, "mesh-join: local ").and_then(|line| join::field_u32(line, "epoch="));
    let store_epoch = raw_line(&root_text, "epoch-store: restored")
        .filter(|line| line.contains("source=root-record"))
        .and_then(|line| join::field_u32(line, "epoch="));
    let confirm_epoch = raw_line(&root_text, "mesh-join: confirm ")
        .and_then(|line| join::field_u32(line, "epoch="));
    let member_text = read_serial(&nodes[1]);
    let member_tail = member_text.get(member_offset..).unwrap_or("");
    let member_last_epoch = member_tail
        .lines()
        .filter(|line| line.contains("mesh-join: request "))
        .filter_map(join::request_last_epoch)
        .next_back();
    let member_epoch = raw_line(member_tail, "mesh-join: duplicate ")
        .and_then(|line| join::field_u32(line, "epoch="))
        .or_else(|| {
            raw_line(member_tail, "mesh-join: local ")
                .and_then(|line| join::field_u32(line, "epoch="))
        });
    let epoch_regression =
        root_text.contains("epoch-regression") || member_tail.contains("epoch-regression");
    Ok(RootRestartInput {
        boot1_log,
        first,
        local_epoch,
        store_epoch,
        member_last_epoch,
        confirm_epoch,
        member_epoch,
        epoch_regression,
    })
}
fn print_join_result(join: &JoinEvidence) {
    println!(
        "join [{}]: {} ({})",
        join.fault,
        if join.ok { "pass" } else { "FAIL" },
        join.message
    );
    for node in &join.nodes {
        if let Some(line) = &node.converged {
            println!("  {line}");
        }
        if let Some(line) = &node.reject {
            println!("  {line}");
        }
    }
    if let Some(restart) = &join.restart {
        println!(
            "  rejoin {}: epoch {} -> {} digest {} -> {} store_restored={} same={}",
            restart.node,
            restart.first_epoch,
            restart.rejoin_epoch,
            restart.first_digest,
            restart.rejoin_digest,
            restart.store_restored,
            restart.same_epoch
        );
    }
    if let Some(restart) = &join.root_restart {
        println!(
            "  root restart {}: epoch {} -> store {} local {} confirm {} member_last {} same={}",
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
                .confirm_epoch
                .map(|value| value.to_string())
                .unwrap_or_else(|| "-".into()),
            restart
                .member_last_epoch
                .map(|value| value.to_string())
                .unwrap_or_else(|| "-".into()),
            restart.same_epoch
        );
    }
}

fn start_all(
    options: &Options,
    nodes: &[Node],
    fixture_port: u16,
    module_sha256: &str,
    running: &mut Vec<RunningNode>,
) -> Result<(), String> {
    let (environments, _authority, _wrong) =
        mesh_environments(nodes, options.fault, options.transport);
    start_all_with(
        options,
        nodes,
        &environments,
        fixture_port,
        module_sha256,
        running,
    )
}

fn start_all_with(
    options: &Options,
    nodes: &[Node],
    environments: &[Option<MeshEnv>],
    fixture_port: u16,
    module_sha256: &str,
    running: &mut Vec<RunningNode>,
) -> Result<(), String> {
    if environments.len() != nodes.len() {
        return Err("environment count does not match the node count".into());
    }
    if options.action != rig::Action::Shard {
        rig::reset_scratch(nodes)?;
    }
    for (node, environment) in nodes.iter().zip(environments.iter()) {
        write_nvram(node, fixture_port, module_sha256, environment.as_ref())?;
    }

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

/// Let the last control-plane connection close, then run the two-way probe.
///
/// The MCP server has one HTTP slot while the mesh is enabled.  A single retry
/// absorbs the iris forward's half-open connection without weakening the
/// check.
fn settle_and_probe(nodes: &[Node]) -> Result<ProbeSet, String> {
    std::thread::sleep(Duration::from_secs(12));
    let mut attempt = probe_two_way(nodes);
    if !attempt
        .as_ref()
        .map(|(value, _)| value.iter().all(|probe| probe.ok))
        .unwrap_or(false)
    {
        std::thread::sleep(Duration::from_secs(12));
        attempt = probe_two_way(nodes);
    }
    attempt
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
    // `BOOT-READY` precedes the module's first listener tick.  The relay's
    // cold session uses a first-byte guard and does not reconnect quickly, so
    // wait for both guest listeners before connecting to either forward.
    wait_mesh_listen(nodes, Duration::from_secs(60));
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
    println!("fault={}", options.fault.as_str());
    let module_sha = fs::read(&options.module)
        .map(|bytes| sha256::hex(&sha256::digest(&bytes)))
        .unwrap_or_else(|_| "<module sha256>".into());
    let (environments, authority, wrong) =
        mesh_environments(&nodes, options.fault, options.transport);
    println!("authority={}", nodes[0].node_id_string());
    println!("authority.pub={}", authority.public_hex);
    if options.fault == Fault::WrongAuthority {
        println!("n1.mesh.pub={} (wrong key)", wrong.public_hex);
    }
    for (node, environment) in nodes.iter().zip(environments.iter()) {
        println!("env {}:", node.name);
        let mut env = node.environment(0, environment.as_ref());
        for (name, value) in &mut env {
            if name == "mcp.sha256" {
                *value = module_sha.clone();
            }
            let shown = match name.as_str() {
                "mcp.url" => format!("http://{}:<fixture>/mcp.module", node.gateway),
                "mesh.key" => "<32-byte lab seed, root only>".into(),
                _ => value.clone(),
            };
            println!("  {name}={shown}");
        }
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

/// Read one artifact and return its size and SHA-256.
fn artifact(path: &std::path::Path, label: &str) -> Result<(u64, String), String> {
    let bytes =
        fs::read(path).map_err(|error| format!("read {label} {}: {error}", path.display()))?;
    Ok((bytes.len() as u64, sha256::hex(&sha256::digest(&bytes))))
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
    join: Option<&JoinEvidence>,
    homes: Option<&HomesEvidence>,
    route: Option<&route::RouteEvidence>,
    migration: Option<&migrate::MigrateEvidence>,
    latency: Option<&latency::LatencyEvidence>,
    bulk: Option<&bulk::BulkEvidence>,
    failure: Option<&FailureEvidence>,
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
    let (rom_size, rom_sha) = artifact(&options.rom, "PROM")?;
    let (module_size, module_sha) = artifact(&options.module, "module")?;
    let (iris_size, iris_sha) = artifact(&options.iris, "iris")?;
    let (iris_ci_size, iris_ci_sha) = artifact(&options.iris_ci, "iris-ci")?;
    let branch = git(&options.repo, &["rev-parse", "--abbrev-ref", "HEAD"]);
    let commit = git(&options.repo, &["rev-parse", "HEAD"]);
    let describe = git(
        &options.repo,
        &["describe", "--always", "--dirty", "--tags"],
    );
    // `git status --porcelain` is empty only for a clean tree.  A git failure
    // returns "unknown"; treat anything but an empty clean result as dirty so
    // an unattributable run cannot be presented as a clean commit.
    let status = git(&options.repo, &["status", "--porcelain"]);
    let dirty = !status.is_empty();
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
        describe: &describe,
        dirty,
        rom: &options.rom,
        rom_sha256: &rom_sha,
        rom_size,
        module: &options.module,
        module_sha256: &module_sha,
        module_size,
        iris: &options.iris,
        iris_sha256: &iris_sha,
        iris_size,
        iris_ci: &options.iris_ci,
        iris_ci_sha256: &iris_ci_sha,
        iris_ci_size,
        nodes: &node_evidence,
        probes,
        mesh,
        join,
        homes,
        route,
        migration,
        latency,
        bulk,
        failure,
        fixture,
        relays: &relay_logs,
        verdict,
        error,
    };
    evidence::write(&bundle)
}
