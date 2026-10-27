// SPDX-License-Identifier: GPL-3.0-only
//! #247 bulk-transfer evidence over two real kernels.
//!
//! The module serves `GET /mesh/bulk?start=1&bytes=N&chunk=M`; the origin sends
//! one `Request/Read` record to its peer and the peer streams `M`-byte chunks
//! back.  Every chunk is a real `MeshEnvelope` on the same TCP link as join,
//! route and heartbeat traffic.  The module reports the mesh start/finish
//! timestamps, so the throughput below is not the MCP HTTP phase machine.

use std::time::{Duration, Instant};

use crate::homes::fetch;
use crate::rig::Node;

/// The logical record size used by the evidence run (64 KiB).
pub const BULK_BYTES: u32 = 64 * 1024;
/// The chunk size used by the tuned evidence runs (16 KiB).
pub const BULK_CHUNK: u32 = 8 * 1024;
/// The chunk size used for the same-host baseline.  One chunk is queued per
/// module tick, so this reproduces the M2 small-window path on the same code.
pub const BULK_BASELINE_CHUNK: u32 = 1024;
/// Tuned runs.
const RUNS: usize = 5;

/// One completed transfer.
#[derive(Clone, Debug)]
pub struct BulkRun {
    /// Origin node name.
    pub from: String,
    /// Peer node name.
    pub to: String,
    /// Logical bytes.
    pub bytes: u32,
    /// Chunk size.
    pub chunk: u32,
    /// Chunks observed by the origin.
    pub chunks: u32,
    /// Mesh start-to-finish time reported by the module.
    pub elapsed_ms: u64,
    /// Logical throughput.
    pub mb_per_s: f64,
    /// Origin stats line, verbatim.
    pub raw: String,
}

/// Bulk evidence for the bundle.
#[derive(Clone, Debug, Default)]
pub struct BulkEvidence {
    /// Whether every transfer completed with the exact byte count.
    pub ok: bool,
    /// Human-readable result.
    pub message: String,
    /// The small-chunk same-host baseline.
    pub baseline: Option<BulkRun>,
    /// Tuned runs.
    pub runs: Vec<BulkRun>,
}

#[derive(Clone, Debug, Default)]
struct Stat {
    state: String,
    bytes: u32,
    chunk: u32,
    received: u32,
    chunks: u32,
    elapsed_ms: u64,
    error: Option<String>,
}

fn parse(body: &str) -> Result<Stat, String> {
    let line = body
        .lines()
        .find(|line| line.contains("mesh-bulk:"))
        .ok_or_else(|| format!("no mesh-bulk line in {body:?}"))?;
    let mut stat = Stat::default();
    for token in line
        .split_once("mesh-bulk:")
        .map(|(_, rest)| rest)
        .unwrap_or("")
        .split_whitespace()
    {
        let (key, value) = token
            .split_once('=')
            .ok_or_else(|| format!("malformed mesh-bulk token {token:?}"))?;
        match key {
            "state" => stat.state = value.into(),
            "bytes" => stat.bytes = parse_num(value)?,
            "chunk" => stat.chunk = parse_num(value)?,
            "received" => stat.received = parse_num(value)?,
            "chunks" => stat.chunks = parse_num(value)?,
            "elapsed_ms" => stat.elapsed_ms = parse_num(value)? as u64,
            "error" => stat.error = Some(value.into()),
            _ => {}
        }
    }
    if stat.state.is_empty() {
        return Err("mesh-bulk line has no state".into());
    }
    Ok(stat)
}

fn parse_num(value: &str) -> Result<u32, String> {
    value
        .parse()
        .map_err(|_| format!("malformed number {value:?}"))
}

/// Drive one transfer and wait for the module's terminal state.
fn fetch_retry(node: &Node, path: &str) -> Result<crate::homes::HttpResponse, String> {
    let mut last = String::new();
    for attempt in 0..10 {
        match fetch(node, path, Duration::from_secs(20)) {
            Ok(response) => return Ok(response),
            Err(error) => {
                last = error;
                std::thread::sleep(Duration::from_millis(1_000 + attempt * 500));
            }
        }
    }
    Err(last)
}

fn one(from: &Node, to: &Node, chunk: u32) -> Result<BulkRun, String> {
    let start = format!("/mesh/bulk?start=1&bytes={BULK_BYTES}&chunk={chunk}");
    let response = fetch_retry(from, &start)?;
    if response.status != 202 {
        return Err(format!(
            "{} refused bulk start with {}: {}",
            from.name, response.status, response.body
        ));
    }
    let deadline = Instant::now() + Duration::from_secs(40);
    loop {
        std::thread::sleep(Duration::from_millis(300));
        let response = fetch_retry(from, "/mesh/bulk?status=1")?;
        let stat = parse(&response.body)?;
        match stat.state.as_str() {
            "done" => {
                if stat.received != BULK_BYTES || stat.bytes != BULK_BYTES {
                    return Err(format!(
                        "{} reported received={} bytes={} for {BULK_BYTES}",
                        from.name, stat.received, stat.bytes
                    ));
                }
                if stat.chunks != BULK_BYTES / chunk {
                    return Err(format!(
                        "{} reported {} chunks, expected {}",
                        from.name,
                        stat.chunks,
                        BULK_BYTES / chunk
                    ));
                }
                if stat.elapsed_ms == 0 {
                    return Err(format!("{} reported zero elapsed time", from.name));
                }
                let mb_per_s =
                    stat.bytes as f64 / stat.elapsed_ms as f64 * 1000.0 / (1024.0 * 1024.0);
                return Ok(BulkRun {
                    from: from.name.clone(),
                    to: to.name.clone(),
                    bytes: stat.bytes,
                    chunk: stat.chunk,
                    chunks: stat.chunks,
                    elapsed_ms: stat.elapsed_ms,
                    mb_per_s,
                    raw: response.body,
                });
            }
            "failed" => {
                return Err(format!(
                    "{} bulk failed: {} ({})",
                    from.name,
                    stat.error.as_deref().unwrap_or("no reason"),
                    response.body
                ));
            }
            "running" | "started" | "idle" => {}
            other => return Err(format!("{} reported unknown state {other}", from.name)),
        }
        if Instant::now() >= deadline {
            return Err(format!(
                "{} bulk did not finish within 40 s: {}",
                from.name, response.body
            ));
        }
    }
}

/// Run the bulk scenario in both directions.
pub fn run(first: &Node, second: &Node) -> Result<BulkEvidence, String> {
    let baseline = one(first, second, BULK_BASELINE_CHUNK)?;
    let mut runs = Vec::with_capacity(RUNS);
    for index in 0..RUNS {
        // The peer queues one chunk per module tick.  A short settle lets the
        // previous run's tail leave the send queue; without it the next run's
        // first chunks share the tick with the previous transfer and the
        // measured throughput describes the test harness, not the path.
        std::thread::sleep(Duration::from_secs(6));
        let (from, to) = if index % 2 == 0 {
            (first, second)
        } else {
            (second, first)
        };
        runs.push(one(from, to, BULK_CHUNK)?);
    }
    let ok = baseline.received_ok()
        && baseline.elapsed_ms > 0
        && runs
            .iter()
            .all(|run| run.received_ok() && run.elapsed_ms > 0);
    let total: u64 = runs.iter().map(|run| run.bytes as u64).sum();
    let elapsed: u64 = runs.iter().map(|run| run.elapsed_ms).sum();
    let mean = total as f64 / elapsed as f64 * 1000.0 / (1024.0 * 1024.0);
    let baseline_rate = baseline.bytes as f64 / baseline.elapsed_ms as f64 * 1000.0
        / (1024.0 * 1024.0);
    Ok(BulkEvidence {
        ok,
        message: format!(
            "baseline {:.3} MiB/s at {} B chunks; tuned {:.3} MiB/s at {} B chunks ({:.2}x)",
            baseline_rate,
            BULK_BASELINE_CHUNK,
            mean,
            BULK_CHUNK,
            mean / baseline_rate.max(f64::MIN_POSITIVE)
        ),
        baseline: Some(baseline),
        runs,
    })
}

impl BulkRun {
    fn received_ok(&self) -> bool {
        self.bytes == BULK_BYTES
            && self.chunks == BULK_BYTES / self.chunk.max(1)
            && self.elapsed_ms > 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_the_module_report() {
        let stat = parse(
            "mesh-bulk: state=done bytes=65536 chunk=16384 received=65536 chunks=4 \
             elapsed_ms=52 writes=7 batched=2 peak_queue=16800 peak_decoder=16800 \
             bulk_rx=4 bulk_tx=0 bulk_abort=0\n",
        )
        .expect("parse");
        assert_eq!(stat.state, "done");
        assert_eq!(stat.bytes, 65_536);
        assert_eq!(stat.chunk, 16_384);
        assert_eq!(stat.received, 65_536);
        assert_eq!(stat.chunks, 4);
        assert_eq!(stat.elapsed_ms, 52);
        assert!(stat.error.is_none());
    }

    #[test]
    fn parses_a_failure_reason() {
        let stat = parse("mesh-bulk: state=failed bytes=65536 received=16384 error=timeout\n")
            .expect("parse");
        assert_eq!(stat.state, "failed");
        assert_eq!(stat.error.as_deref(), Some("timeout"));
    }

    #[test]
    fn rejects_a_body_without_the_marker() {
        assert!(parse("mesh-route: state=current\n").is_err());
        assert!(parse("mesh-bulk: state=done bytes=nope\n").is_err());
    }
}
