// SPDX-License-Identifier: GPL-3.0-only
//! The receive-interrupt latency and idle gate.
//!
//! Boots two real kernels, joins them, then measures four things on the live
//! mesh:
//!
//! - a hot series, 20 ms between samples, inside the kernel's 50 ms activity
//!   window (`wait_short`);
//! - a cold series, 250 ms between samples, so every sample starts from a
//!   parked idle loop and its round trip includes the wake source;
//! - the guest's own idle load, from the kernel's idle-pass counters: the
//!   share of wall time the idle loop spends working instead of parked in
//!   `wait`.  iris spins a host thread on `wait`, so the host process's CPU
//!   percentage would measure the emulator, not the guest;
//! - the receive-interrupt counters before and after traffic, which prove the
//!   IP2 handler actually ran rather than the timer doing the work.
//!
//! The thresholds are deliberately stated in the output so a recorded run can
//! be read without this file.  The cold target is the point of #336: a first
//! request after idle must be substantially inside the 10 ms tick.

use std::fs;
use std::net::Ipv4Addr;
use std::time::Duration;

use crate::homes::fetch;
use crate::latency::{self, LatencyEvidence};
use crate::relay::MeshRelay;
use crate::rig::{self, Node, Options};

/// Cold p50 must be no more than this; the 10 ms tick is the number the
/// interrupt path has to beat, and 4 ms leaves the whole round trip inside
/// half a tick.
pub const COLD_P50_MAX_MS: u64 = 4;
/// Cold p95 is reported, not gated: between p50 and p99 the number is host
/// scheduling of the two emulated CPUs, and on a shared runner it moves by
/// more than the mechanism does.
pub const COLD_P95_REPORT_MS: u64 = 15;
/// Cold p99 allows one long host deschedule in a 64-sample series.
pub const COLD_P99_MAX_MS: u64 = 25;
/// Hot p50 must not regress from the readiness-poll path (recorded at 2 ms).
pub const HOT_P50_MAX_MS: u64 = 5;
/// Hot p95 is reported, not gated.  The readiness-poll path recorded 3 ms on
/// an unloaded host, but on a shared host the same kernel produced p95 values
/// from 2 ms to 19 ms across runs; the p50 is the regression signal.
pub const HOT_P95_REPORT_MS: u64 = 15;
/// Hot p99 is reported, not gated.  The readiness-poll path showed the same
/// host-scheduling tail, so a p99 bound here would test the host, not the
/// kernel path.
pub const HOT_P99_REPORT_MS: u64 = 30;
/// Guest idle load ceiling.  The idle loop runs once per tick; anything above
/// this means a poll loop is spinning instead of parking.
pub const IDLE_PERCENT_MAX: u32 = 8;
/// Boot-race tolerance for an interrupt whose pending bit dropped between the
/// exception and the handler's Cause read.
pub const UNEXPECTED_MAX: u32 = 8;

/// The idle window: long enough for the activity window to expire and for
/// several hundred ticks to accumulate.
const IDLE_WINDOW: Duration = Duration::from_secs(20);

/// Kernel counters read from `GET /`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct KernelCounters {
    /// Receive interrupts that found the Ethernet source.
    pub rx: u32,
    /// IP2 interrupts with no owned HPC3 source.
    pub spurious: u32,
    /// `EXC_INT` causes that named neither IP7 nor IP2.
    pub unexpected: u32,
    /// Times the handler parked `Status.IM2`.
    pub parked: u32,
    /// Whether the receive interrupt is unmasked.
    pub armed: bool,
    /// Idle-loop passes.
    pub passes: u32,
    /// Microseconds the loop body took.
    pub busy_us: u64,
    /// Microseconds of measured window.
    pub total_us: u64,
    /// Kernel's cumulative busy percentage.
    pub percent: u32,
}

/// Parse the `irq:` and `idle:` lines of the kernel status page.
pub fn parse_status(body: &str) -> Result<KernelCounters, String> {
    let mut counters = KernelCounters::default();
    let mut seen_irq = false;
    let mut seen_idle = false;
    for line in body.lines() {
        let line = line.trim_end_matches('\r').trim();
        let Some((tag, rest)) = line.split_once(':') else {
            continue;
        };
        match tag {
            "irq" => {
                seen_irq = true;
                for token in rest.split_whitespace() {
                    let Some((key, value)) = token.split_once('=') else {
                        continue;
                    };
                    let number = || {
                        value
                            .parse::<u64>()
                            .map_err(|_| format!("irq {key}={value:?} is not a number"))
                    };
                    match key {
                        "rx" => counters.rx = number()? as u32,
                        "spurious" => counters.spurious = number()? as u32,
                        "unexpected" => counters.unexpected = number()? as u32,
                        "parked" => counters.parked = number()? as u32,
                        "armed" => counters.armed = number()? != 0,
                        _ => {}
                    }
                }
            }
            "idle" => {
                seen_idle = true;
                for token in rest.split_whitespace() {
                    let Some((key, value)) = token.split_once('=') else {
                        continue;
                    };
                    let number = || {
                        value
                            .parse::<u64>()
                            .map_err(|_| format!("idle {key}={value:?} is not a number"))
                    };
                    match key {
                        "passes" => counters.passes = number()? as u32,
                        "busy_us" => counters.busy_us = number()?,
                        "total_us" => counters.total_us = number()?,
                        "pct" => counters.percent = number()? as u32,
                        _ => {}
                    }
                }
            }
            _ => {}
        }
    }
    if !seen_irq {
        return Err(format!("status page has no irq: line: {body:?}"));
    }
    if !seen_idle {
        return Err(format!("status page has no idle: line: {body:?}"));
    }
    Ok(counters)
}

/// One measured idle window.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct IdleWindow {
    /// Idle-loop passes in the window.
    pub passes: u32,
    /// Microseconds of loop work.
    pub busy_us: u64,
    /// Microseconds covered.
    pub total_us: u64,
    /// Work share in whole percent.
    pub percent: u32,
    /// Receive interrupts in the window.
    pub rx: u32,
}

impl IdleWindow {
    /// The window between two status samples.
    pub fn between(before: KernelCounters, after: KernelCounters) -> Self {
        let passes = after.passes.wrapping_sub(before.passes);
        let busy_us = after.busy_us.saturating_sub(before.busy_us);
        let total_us = after.total_us.saturating_sub(before.total_us);
        let percent = busy_us
            .saturating_mul(100)
            .checked_div(total_us)
            .unwrap_or(0) as u32;
        Self {
            passes,
            busy_us,
            total_us,
            percent,
            rx: after.rx.wrapping_sub(before.rx),
        }
    }
}

/// One direction's series for a spacing.
#[derive(Clone, Debug)]
pub struct Series {
    /// Node that sent the probe.
    pub node: String,
    /// Gap used, ms.
    pub gap_ms: u32,
    /// Repeats that were reduced to the median.
    pub runs: u32,
    /// Median evidence over the repeats.
    pub evidence: LatencyEvidence,
}

/// Repeats per series.  This is a shared host and an emulator: a single
/// 50-sample series can lose the CPU for tens of milliseconds and move its own
/// p99.  The gate reports the median of three, which is a property of the
/// path, and still records the spread in the per-run evidence.
pub const SERIES_RUNS: u32 = 3;

/// Median of the percentiles and extremes, by run.
fn median(mut runs: Vec<LatencyEvidence>) -> Result<LatencyEvidence, String> {
    if runs.is_empty() {
        return Err("no latency runs to reduce".into());
    }
    fn pick<F: Fn(&LatencyEvidence) -> u64>(runs: &[LatencyEvidence], field: F) -> u64 {
        let mut values: Vec<u64> = runs.iter().map(field).collect();
        values.sort_unstable();
        values[values.len() / 2]
    }
    let ok = runs.iter().all(|run| run.ok);
    let received = pick(&runs, |run| u64::from(run.received)) as u32;
    let sent = pick(&runs, |run| u64::from(run.sent)) as u32;
    let gap_ms = runs[0].gap_ms;
    let p50_ms = pick(&runs, |run| run.p50_ms);
    let p95_ms = pick(&runs, |run| run.p95_ms);
    let p99_ms = pick(&runs, |run| run.p99_ms);
    let min_ms = pick(&runs, |run| run.min_ms);
    let max_ms = pick(&runs, |run| run.max_ms);
    let raw = std::mem::take(&mut runs[0].raw);
    Ok(LatencyEvidence {
        ok,
        message: String::new(),
        sent,
        received,
        gap_ms,
        p50_ms,
        p95_ms,
        p99_ms,
        min_ms,
        max_ms,
        raw,
    })
}

/// The whole gate result.
#[derive(Clone, Debug, Default)]
pub struct GateEvidence {
    /// Whether every threshold passed.
    pub ok: bool,
    /// Whether this arm booted with `net.rx=off` (the polled control).
    pub rx_poll: bool,
    /// Why it failed, if it did.
    pub error: Option<String>,
    /// Status before the idle window on the first node.
    pub counters_before: KernelCounters,
    /// Status after the idle window.
    pub counters_after: KernelCounters,
    /// Status after all traffic.
    pub counters_final: KernelCounters,
    /// The measured idle window.
    pub idle: IdleWindow,
    /// Hot series.
    pub hot: Vec<Series>,
    /// Cold series.
    pub cold: Vec<Series>,
}

fn status(node: &Node) -> Result<KernelCounters, String> {
    let mut last = String::new();
    for _ in 0..4 {
        match fetch(node, "/status", crate::timing::secs(10)) {
            Ok(response) => {
                if response.status != 200 {
                    last = format!("{} status page returned {}", node.name, response.status);
                } else {
                    return parse_status(&response.body);
                }
            }
            Err(error) => last = error,
        }
        std::thread::sleep(Duration::from_millis(500));
    }
    Err(last)
}

/// Wait until both modules report the mesh link connected.
fn wait_connected(nodes: &[Node], timeout: Duration) -> Result<(), String> {
    let deadline = std::time::Instant::now() + timeout;
    loop {
        if nodes
            .iter()
            .all(|node| crate::read_serial(node).contains("mesh-transport: connected"))
        {
            return Ok(());
        }
        if std::time::Instant::now() >= deadline {
            return Err(format!(
                "the mesh link did not report connected within {} s",
                timeout.as_secs()
            ));
        }
        std::thread::sleep(Duration::from_millis(250));
    }
}

fn series(nodes: &[Node], gap_ms: u32, count: u32) -> Result<Vec<Series>, String> {
    let mut out = Vec::new();
    for node in nodes {
        let mut runs = Vec::new();
        for _ in 0..SERIES_RUNS {
            runs.push(latency::run_with_gap(node, count, gap_ms)?);
            std::thread::sleep(Duration::from_millis(200));
        }
        out.push(Series {
            node: node.name.clone(),
            gap_ms,
            runs: SERIES_RUNS,
            evidence: median(runs)?,
        });
    }
    Ok(out)
}

fn worst(series: &[Series], field: fn(&LatencyEvidence) -> u64) -> u64 {
    series
        .iter()
        .map(|item| field(&item.evidence))
        .max()
        .unwrap_or(0)
}

/// Render the gate as a JSON evidence document.
fn to_json(evidence: &GateEvidence) -> String {
    let mut fields: Vec<String> = Vec::new();
    fields.push(format!("\"ok\": {}", evidence.ok));
    fields.push(format!(
        "\"arm\": \"{}\"",
        if evidence.rx_poll { "poll" } else { "irq" }
    ));
    if let Some(error) = &evidence.error {
        fields.push(format!("\"error\": \"{}\"", error.replace('"', "'")));
    }
    fields.push(format!(
        "\"thresholds\": {{\"cold_p50_ms\": {}, \"cold_p95_report_ms\": {}, \"cold_p99_ms\": {}, \"hot_p50_ms\": {}, \"hot_p95_report_ms\": {}, \"hot_p99_report_ms\": {}, \"idle_percent\": {}}}",
        COLD_P50_MAX_MS,
        COLD_P95_REPORT_MS,
        COLD_P99_MAX_MS,
        HOT_P50_MAX_MS,
        HOT_P95_REPORT_MS,
        HOT_P99_REPORT_MS,
        IDLE_PERCENT_MAX
    ));
    fields.push(format!(
        "\"idle\": {{\"passes\": {}, \"busy_us\": {}, \"total_us\": {}, \"percent\": {}, \"rx\": {}}}",
        evidence.idle.passes,
        evidence.idle.busy_us,
        evidence.idle.total_us,
        evidence.idle.percent,
        evidence.idle.rx
    ));
    fields.push(format!(
        "\"irq\": {{\"before\": {}, \"after\": {}, \"final\": {}, \"spurious\": {}, \"unexpected\": {}, \"parked\": {}, \"armed\": {}}}",
        evidence.counters_before.rx,
        evidence.counters_after.rx,
        evidence.counters_final.rx,
        evidence.counters_final.spurious,
        evidence.counters_final.unexpected,
        evidence.counters_final.parked,
        evidence.counters_final.armed
    ));
    for (name, series) in [("hot", &evidence.hot), ("cold", &evidence.cold)] {
        let mut rows: Vec<String> = Vec::new();
        for item in series.iter() {
            rows.push(format!(
                "{{\"node\": \"{}\", \"gap_ms\": {}, \"runs\": {}, \"sent\": {}, \"received\": {}, \"p50_ms\": {}, \"p95_ms\": {}, \"p99_ms\": {}, \"min_ms\": {}, \"max_ms\": {}}}",
                item.node,
                item.gap_ms,
                item.runs,
                item.evidence.sent,
                item.evidence.received,
                item.evidence.p50_ms,
                item.evidence.p95_ms,
                item.evidence.p99_ms,
                item.evidence.min_ms,
                item.evidence.max_ms
            ));
        }
        fields.push(format!("\"{name}\": [\n    {}\n  ]", rows.join(",\n    ")));
    }
    format!("{{\n  {}\n}}\n", fields.join(",\n  "))
}

/// Run the gate.
pub fn run(options: &Options) -> Result<i32, String> {
    crate::check_prerequisites(options)?;
    let nodes = rig::derive_nodes(options)?;

    let mut running = Vec::new();
    crate::start_all(options, &nodes, &mut running)?;
    let facts = match rig::wait_boot(options, &mut running) {
        Ok(facts) => facts,
        Err(error) => {
            let _ = rig::stop_nodes(options, &mut running);
            return Err(error);
        }
    };

    let result = measure(options, &nodes);
    let errors = rig::stop_nodes(options, &mut running);
    for error in errors {
        eprintln!("m2-rig: {error}");
    }

    let evidence = match result {
        Ok(evidence) => evidence,
        Err(error) => {
            println!("mesh-irq-gate: verdict=fail error={error}");
            return Err(error);
        }
    };

    println!(
        "mesh-irq-gate: arm={}",
        if evidence.rx_poll { "poll" } else { "irq" }
    );
    println!(
        "mesh-irq-gate: thresholds cold_p50<={COLD_P50_MAX_MS}ms cold_p95_report<={COLD_P95_REPORT_MS}ms cold_p99<={COLD_P99_MAX_MS}ms hot_p50<={HOT_P50_MAX_MS}ms hot_p95_report<={HOT_P95_REPORT_MS}ms hot_p99_report<={HOT_P99_REPORT_MS}ms idle<={IDLE_PERCENT_MAX}%"
    );
    for item in evidence.hot.iter().chain(evidence.cold.iter()) {
        println!(
            "mesh-irq-gate: {} {} gap={}ms runs={} sent={} received={} p50={} p95={} p99={} min={} max={}",
            item.node,
            if item.gap_ms <= latency::HOT_GAP_MS {
                "hot "
            } else {
                "cold"
            },
            item.gap_ms,
            item.runs,
            item.evidence.sent,
            item.evidence.received,
            item.evidence.p50_ms,
            item.evidence.p95_ms,
            item.evidence.p99_ms,
            item.evidence.min_ms,
            item.evidence.max_ms
        );
    }
    println!(
        "mesh-irq-gate: idle passes={} busy_us={} total_us={} pct={} rx={}",
        evidence.idle.passes,
        evidence.idle.busy_us,
        evidence.idle.total_us,
        evidence.idle.percent,
        evidence.idle.rx
    );
    println!(
        "mesh-irq-gate: irq rx_before={} rx_after_idle={} rx_final={} spurious={} unexpected={} parked={} armed={}",
        evidence.counters_before.rx,
        evidence.counters_after.rx,
        evidence.counters_final.rx,
        evidence.counters_final.spurious,
        evidence.counters_final.unexpected,
        evidence.counters_final.parked,
        evidence.counters_final.armed
    );

    let directory = options.work.join("evidence");
    fs::create_dir_all(&directory)
        .map_err(|error| format!("create {}: {error}", directory.display()))?;
    let path = directory.join("irq-gate.json");
    fs::write(&path, to_json(&evidence))
        .map_err(|error| format!("write {}: {error}", path.display()))?;
    println!("evidence: {}", path.display());

    for (node, fact) in running.iter().zip(facts.iter()) {
        println!(
            "{}: {}",
            node.node.name,
            fact.boot_ready.as_deref().unwrap_or("no BOOT-READY")
        );
    }

    let verdict = if evidence.ok { "pass" } else { "fail" };
    println!(
        "mesh-irq-gate: verdict={verdict}{}",
        evidence
            .error
            .as_ref()
            .map(|error| format!(" error={error}"))
            .unwrap_or_default()
    );
    Ok(if evidence.ok { 0 } else { 1 })
}

/// Run the measurement against two joined nodes.
fn measure(options: &Options, nodes: &[Node]) -> Result<GateEvidence, String> {
    if options.fault != rig::Fault::None {
        return Err("the receive-interrupt gate needs --fault none".into());
    }
    // The M2-1 relay path: each guest connects to its own gateway's mesh
    // forward port and the host pumps bytes between the two.  The listeners
    // exist a tick after BOOT-READY, so arm the relay only after both have
    // logged it.
    crate::wait_mesh_listen(nodes, crate::timing::secs(20));
    let _relay = MeshRelay::start(
        (Ipv4Addr::LOCALHOST, nodes[0].mesh_port).into(),
        (Ipv4Addr::LOCALHOST, nodes[1].mesh_port).into(),
    )
    .map_err(|error| format!("start mesh relay: {error}"))?;

    crate::wait_join(nodes, crate::timing::secs(60));
    crate::wait_join_ack(nodes, crate::timing::secs(20));
    // The module arms its HTTP listener on a later tick and the mesh link
    // settles after the join ack; both must be up before `/status` and
    // `/mesh/latency` can answer.
    wait_connected(nodes, crate::timing::secs(60))?;
    std::thread::sleep(crate::timing::secs(10));

    let counters_before = status(&nodes[0])?;
    if !counters_before.armed && !options.rx_poll {
        return Err(format!(
            "{} booted with the receive interrupt masked; the handler self-test failed",
            nodes[0].name
        ));
    }
    if counters_before.armed && options.rx_poll {
        return Err("--rx-poll booted with the interrupt armed".into());
    }

    // Idle window: no rig traffic at all.  The module's own refresh and
    // heartbeat continue; that is the idle system this measures.
    std::thread::sleep(IDLE_WINDOW);
    let counters_after = status(&nodes[0])?;
    let idle = IdleWindow::between(counters_before, counters_after);
    if idle.total_us == 0 {
        return Err("the idle window measured no Count time".into());
    }

    let hot = series(nodes, latency::HOT_GAP_MS, latency::COUNT)?;
    let cold = series(nodes, latency::COLD_GAP_MS, latency::COLD_COUNT)?;
    let counters_final = status(&nodes[0])?;

    let mut evidence = GateEvidence {
        ok: true,
        rx_poll: options.rx_poll,
        error: None,
        counters_before,
        counters_after,
        counters_final,
        idle,
        hot,
        cold,
    };

    let mut failures = Vec::new();
    let hot_p50 = worst(&evidence.hot, |item| item.p50_ms);
    let cold_p50 = worst(&evidence.cold, |item| item.p50_ms);
    let cold_p99 = worst(&evidence.cold, |item| item.p99_ms);
    if evidence.hot.len() != nodes.len() || evidence.cold.len() != nodes.len() {
        failures.push("a direction did not produce a series".to_string());
    }
    if evidence
        .hot
        .iter()
        .chain(evidence.cold.iter())
        .any(|item| !item.evidence.ok || item.evidence.received < 1)
    {
        failures.push("a series did not complete".to_string());
    }
    if hot_p50 > HOT_P50_MAX_MS {
        failures.push(format!("hot p50 {hot_p50} ms > {HOT_P50_MAX_MS} ms"));
    }
    if cold_p50 > COLD_P50_MAX_MS {
        failures.push(format!("cold p50 {cold_p50} ms > {COLD_P50_MAX_MS} ms"));
    }
    if cold_p99 > COLD_P99_MAX_MS {
        failures.push(format!("cold p99 {cold_p99} ms > {COLD_P99_MAX_MS} ms"));
    }
    if evidence.idle.percent > IDLE_PERCENT_MAX {
        failures.push(format!(
            "idle {}% > {IDLE_PERCENT_MAX}%",
            evidence.idle.percent
        ));
    }
    if !options.rx_poll && counters_final.rx == 0 {
        failures.push("the handler counted no receive interrupt".to_string());
    }
    // `spurious` means IP2 was asserted and the HPC3 status named no owned
    // source, which is the shape of a stuck or wrongly acknowledged line: never allowed.
    // `unexpected` is one `EXC_INT` with neither IP7 nor IP2 still pending;
    // a level line can drop between the exception and the handler's Cause
    // read, so a handful are a benign race, not a fault.
    if !options.rx_poll && counters_final.spurious != 0 {
        failures.push(format!(
            "spurious interrupts: spurious={}",
            counters_final.spurious
        ));
    }
    if counters_final.unexpected > UNEXPECTED_MAX {
        failures.push(format!(
            "unexpected interrupt causes {} > {UNEXPECTED_MAX}",
            counters_final.unexpected
        ));
    }
    if !failures.is_empty() {
        evidence.ok = false;
        evidence.error = Some(failures.join("; "));
    }
    Ok(evidence)
}

#[cfg(test)]
mod tests {
    use super::*;

    const PAGE: &str = "stage: kernel\r\nip: 127.0.0.1\r\nirq: rx=7 spurious=0 unexpected=0 parked=0 armed=1\r\nidle: passes=2000 busy_us=30000 total_us=20000000 pct=0\r\n";

    #[test]
    fn parses_the_kernel_counters() {
        let counters = parse_status(PAGE).expect("parse");
        assert_eq!(counters.rx, 7);
        assert!(counters.armed);
        assert_eq!(counters.spurious, 0);
        assert_eq!(counters.passes, 2000);
        assert_eq!(counters.busy_us, 30_000);
        assert_eq!(counters.total_us, 20_000_000);
    }

    #[test]
    fn a_page_without_counters_is_an_error() {
        assert!(parse_status("stage: kernel\r\n").is_err());
        assert!(parse_status("irq: rx=1\r\n").is_err());
    }

    #[test]
    fn the_idle_window_is_a_delta() {
        let before = KernelCounters {
            passes: 100,
            busy_us: 1_000,
            total_us: 1_000_000,
            rx: 3,
            ..Default::default()
        };
        let after = KernelCounters {
            passes: 2_100,
            busy_us: 21_000,
            total_us: 21_000_000,
            rx: 9,
            ..Default::default()
        };
        let window = IdleWindow::between(before, after);
        assert_eq!(window.passes, 2_000);
        assert_eq!(window.busy_us, 20_000);
        assert_eq!(window.total_us, 20_000_000);
        assert_eq!(window.percent, 0);
        assert_eq!(window.rx, 6);
    }

    #[test]
    fn the_evidence_document_has_no_trailing_comma() {
        let json = to_json(&GateEvidence::default());
        assert!(!json.contains(",\n}"), "{json}");
        assert!(json.contains("\"arm\": \"irq\""), "{json}");
        assert!(json.contains("\"cold\": ["), "{json}");
    }

    #[test]
    fn an_idle_percent_is_rounded_down_from_the_delta() {
        let before = KernelCounters::default();
        let after = KernelCounters {
            busy_us: 250_000,
            total_us: 10_000_000,
            ..Default::default()
        };
        assert_eq!(IdleWindow::between(before, after).percent, 2);
    }
}
