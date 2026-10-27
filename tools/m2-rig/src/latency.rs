// SPDX-License-Identifier: GPL-3.0-only
//! Mesh round-trip latency evidence (`GET /mesh/latency`).
//!
//! The module sends sequential `Request/Latency` records on the live mesh
//! link and records the send-to-response interval.  The MCP HTTP phase machine
//! is outside the measured interval, so the number is the mesh path latency,
//! not the control-plane cost.

use std::time::{Duration, Instant};

use crate::homes::{fetch, HttpResponse};
use crate::rig::Node;

/// Samples requested per run.
pub const COUNT: u32 = 50;

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
    /// p50 in milliseconds.
    pub p50_ms: u64,
    /// p95 in milliseconds.
    pub p95_ms: u64,
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
            "p50" => evidence.p50_ms = number()?,
            "p95" => evidence.p95_ms = number()?,
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
        match fetch(node, path, Duration::from_secs(20)) {
            Ok(response) => return Ok(response),
            Err(error) => {
                last = error;
                std::thread::sleep(Duration::from_millis(750 + attempt * 250));
            }
        }
    }
    Err(last)
}

/// Run one latency series against the live mesh on `node`.
pub fn run(node: &Node) -> Result<LatencyEvidence, String> {
    let start = format!("/mesh/latency?start=1&count={COUNT}");
    let response = fetch_retry(node, &start)?;
    if response.status != 202 {
        return Err(format!(
            "{} refused latency start with {}: {}",
            node.name, response.status, response.body
        ));
    }
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        std::thread::sleep(Duration::from_millis(250));
        let response = fetch_retry(node, "/mesh/latency?status=1")?;
        let mut evidence = parse(&response.body)?;
        if evidence.ok && evidence.received >= COUNT {
            evidence.message = format!(
                "{} samples: p50 {} ms, p95 {} ms, min {} ms, max {} ms",
                evidence.received, evidence.p50_ms, evidence.p95_ms, evidence.min_ms, evidence.max_ms
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
            "mesh-latency: state=done sent=50 received=50 count=50 p50=11 p95=21 min=10 max=31\n",
        )
        .expect("parse");
        assert!(evidence.ok);
        assert_eq!(evidence.sent, 50);
        assert_eq!(evidence.received, 50);
        assert_eq!(evidence.p50_ms, 11);
        assert_eq!(evidence.p95_ms, 21);
        assert_eq!(evidence.min_ms, 10);
        assert_eq!(evidence.max_ms, 31);
    }

    #[test]
    fn a_running_report_is_not_ok() {
        let evidence = parse("mesh-latency: state=running sent=7 received=5 count=50\n")
            .expect("parse");
        assert!(!evidence.ok);
        assert_eq!(evidence.received, 5);
    }
}
