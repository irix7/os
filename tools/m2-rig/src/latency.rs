// SPDX-License-Identifier: GPL-3.0-only
//! Mesh round-trip latency evidence (`GET /mesh/latency`).
//!
//! The module sends sequential `Request/Latency` records on the live mesh
//! link and records the send-to-response interval.  The MCP HTTP phase machine
//! is outside the measured interval, so the number is the mesh path latency,
//! not the control-plane cost.
//!
//! Two spacings matter.  At 20 ms the probe keeps the node inside the kernel's
//! 50 ms activity window and measures the hot path.  At 250 ms the node parks
//! between samples, so the round trip includes whatever wake source the kernel
//! has: the 10 ms timer tick alone, or the Seeq receive interrupt.

use std::time::{Duration, Instant};

use crate::homes::{fetch, HttpResponse};
use crate::rig::Node;

/// Samples requested per hot run.  Long enough that one host hiccup moves a
/// percentile by a couple of points, not twenty.
pub const COUNT: u32 = 64;
/// Samples requested per cold run.
pub const COLD_COUNT: u32 = 48;
/// Spacing of a hot series: inside the 50 ms activity window.
pub const HOT_GAP_MS: u32 = 20;
/// Spacing of a cold series: beyond the activity window and the 10 ms tick.
pub const COLD_GAP_MS: u32 = 250;

/// One parsed latency report.
#[derive(Clone, Debug, Default)]
pub struct LatencyEvidence {
    /// Whether all requested samples completed.
    pub ok: bool,
    /// Human-readable result.
    pub message: String,
    /// Sent samples.
    pub sent: u32,
    /// Received samples.
    pub received: u32,
    /// Spacing the module used, from the report.
    pub gap_ms: u32,
    /// p50 in milliseconds.
    pub p50_ms: u64,
    /// p95 in milliseconds.
    pub p95_ms: u64,
    /// p99 in milliseconds.
    pub p99_ms: u64,
    /// Minimum in milliseconds.
    pub min_ms: u64,
    /// Maximum in milliseconds.
    pub max_ms: u64,
    /// Raw module line.
    pub raw: String,
}

fn parse(body: &str) -> Result<LatencyEvidence, String> {
    let line = body
        .lines()
        .find(|line| line.contains("mesh-latency:"))
        .ok_or_else(|| format!("no mesh-latency line in {body:?}"))?;
    let mut evidence = LatencyEvidence::default();
    for token in line
        .split_once("mesh-latency:")
        .map(|(_, rest)| rest)
        .unwrap_or("")
        .split_whitespace()
    {
        let (key, value) = token
            .split_once('=')
            .ok_or_else(|| format!("malformed mesh-latency token {token:?}"))?;
        let number = || {
            value
                .parse::<u64>()
                .map_err(|_| format!("malformed number {value:?}"))
        };
        match key {
            "state" => evidence.ok = value == "done",
            "sent" => evidence.sent = number()? as u32,
            "received" => evidence.received = number()? as u32,
            "gap" => evidence.gap_ms = number()? as u32,
            "p50" => evidence.p50_ms = number()?,
            "p95" => evidence.p95_ms = number()?,
            "p99" => evidence.p99_ms = number()?,
            "min" => evidence.min_ms = number()?,
            "max" => evidence.max_ms = number()?,
            _ => {}
        }
    }
    evidence.raw = body.trim().to_string();
    Ok(evidence)
}

fn fetch_retry(node: &Node, path: &str) -> Result<HttpResponse, String> {
    let mut last = String::new();
    for attempt in 0..8 {
        match fetch(node, path, crate::timing::secs(20)) {
            Ok(response) => return Ok(response),
            Err(error) => {
                last = error;
                std::thread::sleep(Duration::from_millis(750 + attempt * 250));
            }
        }
    }
    Err(last)
}

/// Read the current latency report without starting a new series.
pub fn read_status(node: &Node) -> Result<LatencyEvidence, String> {
    let response = fetch_retry(node, "/mesh/latency?status=1")?;
    let mut evidence = parse(&response.body)?;
    if evidence.ok && evidence.received >= 1 {
        evidence.message = format!(
            "{} samples gap {} ms: p50 {} ms, p95 {} ms, p99 {} ms, min {} ms, max {} ms",
            evidence.received,
            evidence.gap_ms,
            evidence.p50_ms,
            evidence.p95_ms,
            evidence.p99_ms,
            evidence.min_ms,
            evidence.max_ms
        );
    }
    Ok(evidence)
}

/// Wait for a series that was started by another request.
pub fn wait_done(node: &Node, count: u32) -> Result<LatencyEvidence, String> {
    let deadline = Instant::now() + crate::timing::secs(30);
    loop {
        let evidence = read_status(node)?;
        if evidence.ok && evidence.received >= count {
            return Ok(evidence);
        }
        if Instant::now() >= deadline {
            return Err(format!(
                "{} latency probe did not finish: received={} count={}",
                node.name, evidence.received, count
            ));
        }
        std::thread::sleep(Duration::from_millis(250));
    }
}

/// Run one latency series against the live mesh on `node`.
pub fn run(node: &Node) -> Result<LatencyEvidence, String> {
    run_with_gap(node, COUNT, HOT_GAP_MS)
}

/// Run `count` samples at the default hot spacing.
pub fn run_with(node: &Node, count: u32) -> Result<LatencyEvidence, String> {
    run_with_gap(node, count, HOT_GAP_MS)
}

/// Run one cold series: beyond the kernel's activity window.
pub fn run_cold(node: &Node) -> Result<LatencyEvidence, String> {
    run_with_gap(node, COLD_COUNT, COLD_GAP_MS)
}

/// Run `count` samples spaced `gap_ms` apart.
pub fn run_with_gap(node: &Node, count: u32, gap_ms: u32) -> Result<LatencyEvidence, String> {
    let start = format!("/mesh/latency?start=1&count={count}&gap={gap_ms}");
    let response = fetch_retry(node, &start)?;
    if response.status != 202 {
        return Err(format!(
            "{} refused latency start with {}: {}",
            node.name, response.status, response.body
        ));
    }
    // A cold series costs count x gap; leave 15 s of headroom over the series
    // itself so a slow status read cannot fail a healthy run.
    let series = Duration::from_millis(u64::from(count) * u64::from(gap_ms));
    let deadline = Instant::now() + series + crate::timing::secs(15);
    loop {
        std::thread::sleep(Duration::from_millis(250));
        let response = fetch_retry(node, "/mesh/latency?status=1")?;
        let mut evidence = parse(&response.body)?;
        if evidence.ok && evidence.received >= count {
            evidence.message = format!(
                "{} samples gap {} ms: p50 {} ms, p95 {} ms, p99 {} ms, min {} ms, max {} ms",
                evidence.received,
                evidence.gap_ms,
                evidence.p50_ms,
                evidence.p95_ms,
                evidence.p99_ms,
                evidence.min_ms,
                evidence.max_ms
            );
            return Ok(evidence);
        }
        if Instant::now() >= deadline {
            return Err(format!(
                "{} latency probe did not finish: {}",
                node.name, response.body
            ));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_complete_report() {
        let evidence = parse(
            "mesh-latency: state=done sent=50 received=50 count=50 gap=20 p50=11 p95=21 p99=23 min=10 max=31\n",
        )
        .expect("parse");
        assert!(evidence.ok);
        assert_eq!(evidence.sent, 50);
        assert_eq!(evidence.received, 50);
        assert_eq!(evidence.gap_ms, 20);
        assert_eq!(evidence.p50_ms, 11);
        assert_eq!(evidence.p95_ms, 21);
        assert_eq!(evidence.p99_ms, 23);
        assert_eq!(evidence.min_ms, 10);
        assert_eq!(evidence.max_ms, 31);
    }

    #[test]
    fn parses_a_cold_report() {
        let evidence = parse(
            "mesh-latency: state=done sent=32 received=32 count=32 gap=250 p50=2 p95=3 p99=4 min=1 max=6\n",
        )
        .expect("parse");
        assert_eq!(evidence.gap_ms, 250);
        assert_eq!(evidence.p99_ms, 4);
    }

    #[test]
    fn a_running_report_is_not_ok() {
        let evidence =
            parse("mesh-latency: state=running sent=7 received=5 count=50\n").expect("parse");
        assert!(!evidence.ok);
        assert_eq!(evidence.received, 5);
    }

    #[test]
    fn an_old_report_without_gap_or_p99_still_parses() {
        let evidence =
            parse("mesh-latency: state=done sent=1 received=1 count=1 p50=2 p95=2 min=2 max=2\n")
                .expect("parse");
        assert_eq!(evidence.gap_ms, 0);
        assert_eq!(evidence.p99_ms, 0);
    }
}
