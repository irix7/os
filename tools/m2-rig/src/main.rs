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
mod homes;
mod join;
mod nvram;
mod probe;
mod route;
mod relay;
mod rig;
mod sha256;

use std::fs;
use std::net::Ipv4Addr;
use std::time::{Duration, Instant};

use evidence::{Evidence, MeshNodeProbe, MeshProbeEvidence, NodeEvidence, ProbeEvidence};
use fixture::Fixture;
use homes::HomesEvidence;
use join::{JoinEvidence, RestartInput};
use probe::{ProbeError, ProbeOutcome};
use relay::{MeshRelay, Relay};
use rig::{
    derive_nodes, git, mesh_environments, read_serial, run_ci, start_cpu, start_node, wait_boot,
    wait_ci_ready, write_nvram, Action, AuthorityKeys, BootFacts, Fault, Node, Options,
    RunningNode,
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
  --fault MODE     none (default), no-peer, wrong-authority or no-route
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
        wait_homes_serial(&nodes, Duration::from_secs(20));
        std::thread::sleep(Duration::from_secs(10));
    } else if options.fault == Fault::WrongAuthority {
        wait_join_reject(&nodes, Duration::from_secs(30));
        std::thread::sleep(Duration::from_secs(10));
    }
    let facts = snapshot_facts(&nodes);
    let keys = AuthorityKeys::lab();
    let join = join::evaluate(&nodes, &facts, &keys, options.fault, None);
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
        "probe",
        if passed { "pass" } else { "fail" },
        error.as_deref(),
    )?;
    println!("evidence: {}", directory.display());
    Ok(if passed { 0 } else { 1 })
}

fn collect_route(
    options: &Options,
    nodes: &[Node],
    join: &JoinEvidence,
) -> Result<route::RouteEvidence, String> {
    match options.fault {
        Fault::None => {
            if join.epoch.is_none() || join.digest.is_none() || join.members.len() != 2 {
                return Err("join evidence is not a converged two-node view".into());
            }
            Ok(route::run(
                &nodes[0],
                &nodes[1],
                &join.members,
                join.epoch.unwrap_or(0),
                join.digest.as_deref().unwrap_or_default(),
                Duration::from_secs(180),
            ))
        }
        Fault::NoRoute | Fault::NoPeer | Fault::WrongAuthority => {
            route::fetch_disabled(nodes, options.fault, Duration::from_secs(30))
        }
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
    let mut route = None;
    let mut route_error = None;
    let mut restart = None;
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
        } else {
            let (value, relay) = probe_mesh(&nodes);
            mesh = Some(value);
            mesh_relay = relay;
        }
        if options.fault.joins() {
            wait_join(&nodes, Duration::from_secs(60));
            // The terminal acknowledgement must cross the relay before the
            // member is restarted; otherwise the restart can truncate it and
            // there is no evidence that B installed what A signed.
            wait_join_ack(&nodes, Duration::from_secs(20));
            wait_homes_serial(&nodes, Duration::from_secs(20));
            // The MCP HTTP slot is armed by the module tick after the join
            // and homes logging settles.  Give it the same 10 s the guest HTTP
            // probe uses before the first control-plane fetch.
            std::thread::sleep(Duration::from_secs(10));
        } else if options.fault == Fault::WrongAuthority {
            wait_join_reject(&nodes, Duration::from_secs(30));
            std::thread::sleep(Duration::from_secs(10));
        }
        facts = snapshot_facts(&nodes);

        let join_before = join::evaluate(&nodes, &facts, &keys, options.fault, None);
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

        // The guest-level HTTP probe still runs before the member restart, as
        // in M2-0/M2-1/M2-2.  It runs after the MCP homes fetch because the
        // iris host forward can hold a half-open guest connection after the
        // probe; the control-plane evidence is collected on the clean path.
        match probe_two_way(&nodes) {
            Ok((value, value_relays)) => {
                probes = value;
                relays = value_relays;
            }
            Err(error) => probe_error = Some(error),
        }

        if options.fault.joins() {
            match restart_member(options, &nodes, &mut running, &facts, &mut mesh_relay) {
                Ok(input) => {
                    restart = Some(input);
                    // The root's transcript gained a confirmation and the
                    // member's transcript is now the rejoin boot.
                    wait_homes_serial(&nodes, Duration::from_secs(20));
                    facts = snapshot_facts(&nodes);
                }
                Err(error) => join_error = Some(error),
            }
        }
    } else {
        // Keep the evidence useful even when a node never booted: record what
        // the serial logs show and do not invent a probe result.
        facts = snapshot_facts(&nodes);
    }

    let join = join::evaluate(&nodes, &facts, &keys, options.fault, restart.as_ref());
    if matches!(options.fault, Fault::None | Fault::NoRoute) && join_error.is_none() {
        if let (Some(epoch), Some(digest)) = (join.epoch, join.digest.as_deref()) {
            home_inputs.after =
                Some(homes::collect_current_serial(&nodes, &facts, epoch, digest, &join.members));
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
        true,
    );
    let mesh_ok = match options.fault {
        Fault::NoPeer => true,
        _ => mesh.as_ref().is_some_and(|mesh| mesh.ok),
    };
    let route_ok = route.as_ref().is_some_and(|route| route.ok);
    let passed = bring_up_error.is_none()
        && probe_error.is_none()
        && join_error.is_none()
        && homes_error.is_none()
        && route_error.is_none()
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

/// Wait until both transcripts carry a complete home group.
fn wait_homes_serial(nodes: &[Node], timeout: Duration) {
    let deadline = Instant::now() + timeout;
    loop {
        let facts = snapshot_facts(nodes);
        let ready = facts.iter().all(|fact| {
            fact.mesh_homes_lines
                .iter()
                .filter(|line| line.contains("mesh-home: "))
                .count()
                >= homes::OBJECT_COUNT as usize
        });
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
        Fault::None | Fault::NoRoute => {
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
        println!("  {}: {}", round.label, if round.ok { "pass" } else { "FAIL" });
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
    let disconnects_before = BootFacts::from_serial(&read_serial(&nodes[0]))
        .mesh_transport_disconnects
        .len();
    let relay_log = mesh_relay.take().map(|relay| {
        let log = relay.log.clone();
        drop(relay);
        log
    });
    std::thread::sleep(Duration::from_secs(1));
    rig::stop_node(options, &mut running[1])?;
    fs::copy(&node.serial_log, &boot1_log)
        .map_err(|error| format!("copy {}: {error}", node.serial_log.display()))?;
    // Wait until the root has left its old connection and re-armed its
    // listener.  The iris port forward does not reliably propagate the peer's
    // close to the guest, so the heartbeat timeout is the root's only signal;
    // connecting to its forward before that leaves half-open guest attempts in
    // the forward and the restart never converges.
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        let facts = BootFacts::from_serial(&read_serial(&nodes[0]));
        if facts.mesh_transport_disconnects.len() > disconnects_before {
            break;
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
    rig::wait_serial(
        options,
        &mut running[1],
        "mesh-transport: listening",
        Duration::from_secs(60),
    )?;
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
    let deadline = Instant::now() + Duration::from_secs(90);
    loop {
        let fact = BootFacts::from_serial(&read_serial(node));
        if fact.mesh_join_converged.is_some() {
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
    let rejoin = BootFacts::from_serial(&read_serial(node))
        .mesh_join_converged
        .as_deref()
        .and_then(join::JoinLine::parse)
        .ok_or_else(|| "n1 rejoin line is malformed".to_string())?;
    let fact = BootFacts::from_serial(&read_serial(node));
    let request_last_epoch = fact
        .mesh_join_request
        .as_deref()
        .and_then(join::request_last_epoch);
    let root_facts = BootFacts::from_serial(&read_serial(&nodes[0]));
    let confirm_epoch = root_facts
        .mesh_join_confirms
        .last()
        .and_then(|line| join::field_u32(line, "epoch="));
    Ok(RestartInput {
        node: node.name.clone(),
        boot1_log,
        first,
        rejoin,
        request_last_epoch,
        confirm_epoch,
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
            "  rejoin {}: epoch {} -> {} digest {} -> {} same={}",
            restart.node,
            restart.first_epoch,
            restart.rejoin_epoch,
            restart.first_digest,
            restart.rejoin_digest,
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
    let (environments, _authority, _wrong) = mesh_environments(nodes, options.fault);
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
    let (environments, authority, wrong) = mesh_environments(&nodes, options.fault);
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
        join,
        homes,
        route,
        fixture,
        relays: &relay_logs,
        verdict,
        error,
    };
    evidence::write(&bundle)
}
