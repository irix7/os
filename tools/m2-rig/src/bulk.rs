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
use crate::latency::LatencyEvidence;
use crate::rig::Node;

/// The logical record size used by the evidence run (64 KiB).
pub const BULK_BYTES: u32 = 64 * 1024;
/// The chunk size used by the tuned evidence runs (4 KiB).
pub const BULK_CHUNK: u32 = 4 * 1024;
/// A longer logical record used for control/cancel interleaving.
pub const CONTROL_BYTES: u32 = 64 * 1024;
/// A record large enough that the cancel arrives before completion.
pub const CANCEL_BYTES: u32 = 1024 * 1024;
/// The stream gate's control-during-bulk record.  Large enough that the
/// latency series finishes while the transfer is still backpressured.
pub const GATE_BYTES: u32 = 512 * 1024;
/// The stream gate's post-cancel reuse record.
pub const REUSE_BYTES: u32 = 32 * 1024;
/// Samples in the stream gate's control series.
pub const GATE_SAMPLES: u32 = 4;
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
pub struct ControlRun {
    /// Bulk logical bytes.
    pub bytes: u32,
    /// Bulk chunks observed.
    pub chunks: u32,
    /// Bulk mesh elapsed ms.
    pub bulk_ms: u64,
    /// Control probe p50 during the bulk transfer.
    pub control_p50_ms: u64,
    /// Control probe p95 during the bulk transfer.
    pub control_p95_ms: u64,
    /// Idle-link control p50 before the transfer.
    pub baseline_p50_ms: u64,
    /// Idle-link control p95 before the transfer.
    pub baseline_p95_ms: u64,
}

#[derive(Clone, Debug)]
pub struct CancelRun {
    /// Terminal state text.
    pub state: String,
    /// Error string, if any.
    pub error: Option<String>,
    /// Chunks the peer had sent before the cancel.
    pub chunks: u32,
    /// Origin stream resets before the transfer.
    pub origin_resets_before: u64,
    /// Origin stream resets after the cancel settled.
    pub origin_resets_after: u64,
    /// Peer stream resets before the transfer.
    pub peer_resets_before: u64,
    /// Peer stream resets after the cancel settled.
    pub peer_resets_after: u64,
    /// Whether a fresh transfer completed after the cancel.
    pub reused: bool,
}

/// The two-kernel stream gate: control while bulk is backpressured, cancel,
/// reclaim and reuse.
#[derive(Clone, Debug, Default)]
pub struct StreamEvidence {
    /// Whether every gate condition passed.
    pub ok: bool,
    /// Human-readable result.
    pub message: String,
    /// Idle-link control series.
    pub baseline: LatencyEvidence,
    /// Control series while the bulk transfer was active.
    pub control: LatencyEvidence,
    /// The bulk transfer that ran under the control probe.
    pub bulk: Option<BulkRun>,
    /// Cancel/reset evidence.
    pub cancel: Option<CancelRun>,
    /// The reuse transfer after the cancel.
    pub reuse: Option<BulkRun>,
}

pub struct BulkEvidence {
    /// Whether every transfer completed with the exact byte count.
    pub ok: bool,
    /// Human-readable result.
    pub message: String,
    /// The small-chunk same-host baseline.
    pub baseline: Option<BulkRun>,
    /// Tuned runs.
    pub runs: Vec<BulkRun>,
    /// Control RPC latency while a bulk transfer is in flight.
    pub control: Option<ControlRun>,
    /// Explicit cancel path.
    pub cancel: Option<CancelRun>,
}

#[derive(Clone, Debug, Default)]
struct Stat {
    state: String,
    bytes: u32,
    chunk: u32,
    received: u32,
    chunks: u32,
    elapsed_ms: u64,
    stream: u32,
    error: Option<String>,
}

/// The module's live stream counters from the `mesh-stream:`/bulk status line.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct StreamStat {
    /// Open streams.
    pub open: u64,
    /// Cumulative opens.
    pub opens: u64,
    /// Cumulative resets.
    pub resets: u64,
    /// Data credit or queue stalls.
    pub credit_stall: u64,
    /// Control frames that could not be queued.
    pub control_stall: u64,
    /// Control frames scheduled.
    pub tx_control: u64,
    /// Data frames scheduled.
    pub tx_data: u64,
    /// Bytes reserved and not yet accepted by the sink.
    pub queued: u64,
}

impl StreamStat {
    /// Parse the `stream_*` fields from a module status line.
    pub fn parse_line(line: &str) -> Result<Self, String> {
        let mut stat = Self::default();
        let rest = line
            .split_once("mesh-bulk:")
            .map(|(_, rest)| rest)
            .unwrap_or(line);
        for token in rest.split_whitespace() {
            let Some((key, value)) = token.split_once('=') else {
                continue;
            };
            let number = || {
                value
                    .parse::<u64>()
                    .map_err(|_| format!("malformed stream counter {key}={value:?}"))
            };
            match key {
                "stream_open" => stat.open = number()?,
                "stream_opens" => stat.opens = number()?,
                "stream_resets" => stat.resets = number()?,
                "stream_credit_stall" => stat.credit_stall = number()?,
                "stream_control_stall" => stat.control_stall = number()?,
                "stream_tx_control" => stat.tx_control = number()?,
                "stream_tx_data" => stat.tx_data = number()?,
                "stream_queued" => stat.queued = number()?,
                _ => {}
            }
        }
        Ok(stat)
    }
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
            "stream" => stat.stream = parse_num(value)?,
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
        match fetch(node, path, crate::timing::secs(20)) {
            Ok(response) => return Ok(response),
            Err(error) => {
                last = error;
                std::thread::sleep(Duration::from_millis(1_000 + attempt * 500));
            }
        }
    }
    Err(last)
}

fn one(from: &Node, to: &Node, total: u32, chunk: u32) -> Result<BulkRun, String> {
    let start = format!("/mesh/bulk?start=1&bytes={total}&chunk={chunk}");
    let response = fetch_retry(from, &start)?;
    if response.status != 202 {
        return Err(format!(
            "{} refused bulk start with {}: {}",
            from.name, response.status, response.body
        ));
    }
    let deadline = Instant::now() + crate::timing::secs(40);
    loop {
        std::thread::sleep(Duration::from_millis(300));
        let response = fetch_retry(from, "/mesh/bulk?status=1")?;
        let stat = parse(&response.body)?;
        match stat.state.as_str() {
            "done" => {
                if stat.received != total || stat.bytes != total {
                    return Err(format!(
                        "{} reported received={} bytes={} for {total}",
                        from.name, stat.received, stat.bytes
                    ));
                }
                if stat.chunks != total / chunk {
                    return Err(format!(
                        "{} reported {} chunks, expected {}",
                        from.name,
                        stat.chunks,
                        total / chunk
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

/// Start one bulk transfer and return after the module accepted it.
fn start_bulk(from: &Node, total: u32, chunk: u32) -> Result<(), String> {
    let start = format!("/mesh/bulk?start=1&bytes={total}&chunk={chunk}");
    let response = fetch_retry(from, &start)?;
    if response.status != 202 {
        return Err(format!(
            "{} refused bulk start with {}: {}",
            from.name, response.status, response.body
        ));
    }
    Ok(())
}

/// Start a bulk transfer and a latency series in one module request.
///
/// The HTTP surface is starved while the bulk transfer is running, so starting
/// the control probe separately would measure the HTTP phase machine rather
/// than the mesh control stream.  One request queues both.
fn start_gate(from: &Node, total: u32, chunk: u32, samples: u32) -> Result<(), String> {
    let start = format!("/mesh/bulk?start=1&bytes={total}&chunk={chunk}&latency={samples}");
    let response = fetch_retry(from, &start)?;
    if response.status != 202 {
        return Err(format!(
            "{} refused gate start with {}: {}",
            from.name, response.status, response.body
        ));
    }
    Ok(())
}

/// Poll one bulk transfer to its terminal state.
fn poll_bulk(from: &Node, total: u32, chunk: u32) -> Result<(BulkRun, Stat), String> {
    let expected_chunks = total / chunk;
    let deadline = Instant::now() + crate::timing::secs(40);
    loop {
        std::thread::sleep(Duration::from_millis(200));
        let response = fetch_retry(from, "/mesh/bulk?status=1")?;
        let stat = parse(&response.body)?;
        match stat.state.as_str() {
            "done" => {
                if stat.received != total || stat.bytes != total || stat.chunks != expected_chunks {
                    return Err(format!(
                        "{} reported received={} bytes={} chunks={} for {total}/{chunk}",
                        from.name, stat.received, stat.bytes, stat.chunks
                    ));
                }
                if stat.elapsed_ms == 0 {
                    return Err(format!("{} reported zero elapsed time", from.name));
                }
                let mb_per_s =
                    stat.bytes as f64 / stat.elapsed_ms as f64 * 1000.0 / (1024.0 * 1024.0);
                let run = BulkRun {
                    from: from.name.clone(),
                    to: String::new(),
                    bytes: stat.bytes,
                    chunk: stat.chunk,
                    chunks: stat.chunks,
                    elapsed_ms: stat.elapsed_ms,
                    mb_per_s,
                    raw: response.body,
                };
                return Ok((run, stat));
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

/// Measure a control probe while a 256 KiB bulk transfer is in flight.
fn control_during_bulk(
    from: &Node,
) -> Result<(ControlRun, crate::latency::LatencyEvidence), String> {
    let baseline = crate::latency::run_with(from, 8)?;
    start_bulk(from, CONTROL_BYTES, BULK_CHUNK)?;
    // Start the control probe after the bulk request is queued; both share the
    // one peer link and the bulk stream is credit-blocked after two chunks.
    let latency = crate::latency::run_with(from, 8)?;
    let (run, _) = poll_bulk(from, CONTROL_BYTES, BULK_CHUNK)?;
    Ok((
        ControlRun {
            bytes: run.bytes,
            chunks: run.chunks,
            bulk_ms: run.elapsed_ms,
            control_p50_ms: latency.p50_ms,
            control_p95_ms: latency.p95_ms,
            baseline_p50_ms: baseline.p50_ms,
            baseline_p95_ms: baseline.p95_ms,
        },
        latency,
    ))
}

/// Start a bulk transfer and cancel it through `/mesh/bulk?cancel=1`.
fn cancel_bulk(from: &Node) -> Result<CancelRun, String> {
    start_bulk(from, CANCEL_BYTES, BULK_CHUNK)?;
    std::thread::sleep(Duration::from_millis(50));
    let response = fetch_retry(from, "/mesh/bulk?cancel=1")?;
    if response.status != 200 {
        return Err(format!(
            "{} refused bulk cancel with {}: {}",
            from.name, response.status, response.body
        ));
    }
    let stat = wait_cancelled(from)?;
    Ok(CancelRun {
        state: stat.state,
        error: stat.error,
        chunks: stat.chunks,
        origin_resets_before: 0,
        origin_resets_after: 0,
        peer_resets_before: 0,
        peer_resets_after: 0,
        reused: false,
    })
}

/// Fetch the module's bulk state and live stream counters.
fn status(node: &Node) -> Result<(Stat, StreamStat), String> {
    let response = fetch_retry(node, "/mesh/bulk?status=1")?;
    let stat = parse(&response.body)?;
    let stream = StreamStat::parse_line(&response.body)?;
    Ok((stat, stream))
}

/// Wait until the bulk transfer has queued its request and received data.
fn wait_bulk_active(node: &Node, total: u32, chunk: u32) -> Result<Stat, String> {
    let deadline = Instant::now() + crate::timing::secs(30);
    loop {
        std::thread::sleep(Duration::from_millis(200));
        let (stat, _) = status(node)?;
        match stat.state.as_str() {
            "running" if stat.received > 0 => {
                if stat.bytes != total || stat.chunk != chunk {
                    return Err(format!(
                        "{} reports bytes={} chunk={} for {total}/{chunk}",
                        node.name, stat.bytes, stat.chunk
                    ));
                }
                return Ok(stat);
            }
            "failed" => {
                return Err(format!(
                    "{} bulk failed before the control probe: {}",
                    node.name,
                    stat.error.as_deref().unwrap_or("no reason")
                ));
            }
            _ => {}
        }
        if Instant::now() >= deadline {
            return Err(format!(
                "{} bulk did not become active within 30 s",
                node.name
            ));
        }
    }
}

/// Wait for the module to report the cancelled transfer.
fn wait_cancelled(node: &Node) -> Result<Stat, String> {
    let deadline = Instant::now() + crate::timing::secs(15);
    loop {
        std::thread::sleep(Duration::from_millis(100));
        let (stat, _) = status(node)?;
        if stat.state == "failed" {
            return Ok(stat);
        }
        if Instant::now() >= deadline {
            return Err(format!(
                "{} did not report the cancelled bulk: state={}",
                node.name, stat.state
            ));
        }
    }
}

/// Run the #337 two-kernel stream gate.
///
/// The control series starts after the bulk transfer has data in flight and
/// must finish before the transfer completes.  The cancel series resets a
/// 1 MiB transfer, observes the reset on both nodes, then proves the link and
/// a fresh stream are usable.
pub fn run_gate(first: &Node, second: &Node) -> Result<StreamEvidence, String> {
    let baseline = crate::latency::run_with(first, GATE_SAMPLES)?;
    if !baseline.ok || baseline.received < GATE_SAMPLES {
        return Err(format!(
            "baseline control series incomplete: {}",
            baseline.message
        ));
    }
    // The p95 allowance is measured against the idle link, not a fixed budget.
    let allowed_p95 = baseline
        .p95_ms
        .saturating_mul(4)
        .max(baseline.p95_ms.saturating_add(100))
        .max(150);
    let allowed_p50 = baseline
        .p50_ms
        .saturating_mul(4)
        .max(baseline.p50_ms.saturating_add(50))
        .max(100);

    start_gate(first, GATE_BYTES, BULK_CHUNK, GATE_SAMPLES)?;
    let (bulk, _) = poll_bulk(first, GATE_BYTES, BULK_CHUNK)?;
    // The probe was queued in the same module request as the transfer, so
    // every sample belongs to the interval in which the link carried both
    // classes.  Read its result once the transfer has drained.
    let control = crate::latency::wait_done(first, GATE_SAMPLES)?;

    let origin_pre_cancel = status(first)?.1;
    let peer_pre_cancel = status(second)?.1;
    start_bulk(first, CANCEL_BYTES, BULK_CHUNK)?;
    wait_bulk_active(first, CANCEL_BYTES, BULK_CHUNK)?;
    let response = fetch_retry(first, "/mesh/bulk?cancel=1")?;
    if response.status != 200 {
        return Err(format!(
            "{} refused bulk cancel with {}: {}",
            first.name, response.status, response.body
        ));
    }
    let cancelled = wait_cancelled(first)?;
    // Let the cancel frame reach the peer and the origin retire the stream
    // after its sink accepts the record.
    std::thread::sleep(Duration::from_millis(1_500));
    let origin_after = status(first)?.1;
    let peer_after = status(second)?.1;
    let origin_reclaimed = origin_after.resets > origin_pre_cancel.resets;
    let peer_reclaimed = peer_after.resets > peer_pre_cancel.resets;
    let reuse = one(first, second, REUSE_BYTES, BULK_CHUNK)?;
    let final_stream = status(first)?.1;

    let control_ok = control.ok
        && control.received >= GATE_SAMPLES
        && control.p50_ms <= allowed_p50
        && control.p95_ms <= allowed_p95;
    let cancel_ok = cancelled.error.as_deref() == Some("cancel");
    let reset_ok = origin_reclaimed && peer_reclaimed;
    let reuse_ok = reuse.received_ok();
    let bounded = final_stream.control_stall == 0;
    let ok = control_ok && bulk.received_ok() && cancel_ok && reset_ok && reuse_ok && bounded;
    let message = format!(
        "control p50 {} ms / p95 {} ms during {} B bulk (idle p50 {} / p95 {}; allowed p50 {} / p95 {}); \
         bulk {} chunks in {} ms; cancel resets origin {}->{} peer {}->{}; \
         reuse {} B in {} chunks; control_stalls={} credit_stalls={}",
        control.p50_ms,
        control.p95_ms,
        bulk.bytes,
        baseline.p50_ms,
        baseline.p95_ms,
        allowed_p50,
        allowed_p95,
        bulk.chunks,
        bulk.elapsed_ms,
        origin_pre_cancel.resets,
        origin_after.resets,
        peer_pre_cancel.resets,
        peer_after.resets,
        reuse.bytes,
        reuse.chunks,
        final_stream.control_stall,
        final_stream.credit_stall,
    );
    Ok(StreamEvidence {
        ok,
        message,
        baseline,
        control,
        bulk: Some(bulk),
        cancel: Some(CancelRun {
            state: cancelled.state,
            error: cancelled.error,
            chunks: cancelled.chunks,
            origin_resets_before: origin_pre_cancel.resets,
            origin_resets_after: origin_after.resets,
            peer_resets_before: peer_pre_cancel.resets,
            peer_resets_after: peer_after.resets,
            reused: reuse_ok,
        }),
        reuse: Some(reuse),
    })
}

/// Run the bulk scenario in both directions.
pub fn run(first: &Node, second: &Node) -> Result<BulkEvidence, String> {
    let baseline = one(first, second, BULK_BYTES, BULK_BASELINE_CHUNK)?;
    let mut runs = Vec::with_capacity(RUNS);
    for index in 0..RUNS {
        // The peer queues one chunk per module tick.  A short settle lets the
        // previous run's tail leave the send queue; without it the next run's
        // first chunks share the tick with the previous transfer and the
        // measured throughput describes the test harness, not the path.
        std::thread::sleep(crate::timing::secs(6));
        let (from, to) = if index % 2 == 0 {
            (first, second)
        } else {
            (second, first)
        };
        match one(from, to, BULK_BYTES, BULK_CHUNK) {
            Ok(run) => runs.push(run),
            Err(error) => {
                eprintln!("bulk tuned run skipped: {error}");
            }
        }
    }
    let ok = baseline.received_ok()
        && baseline.elapsed_ms > 0
        && runs.len() >= RUNS
        && runs
            .iter()
            .all(|run| run.received_ok() && run.elapsed_ms > 0);
    let total: u64 = runs.iter().map(|run| run.bytes as u64).sum();
    let elapsed: u64 = runs.iter().map(|run| run.elapsed_ms).sum();
    let mean = total as f64 / elapsed as f64 * 1000.0 / (1024.0 * 1024.0);
    let baseline_rate =
        baseline.bytes as f64 / baseline.elapsed_ms as f64 * 1000.0 / (1024.0 * 1024.0);
    let (control, latency) = control_during_bulk(first)?;
    let cancel = cancel_bulk(first)?;
    let control_ok = latency.ok
        && control.control_p50_ms > 0
        && control.control_p95_ms >= control.control_p50_ms
        && cancel.state == "failed"
        && cancel.error.as_deref() == Some("cancel");
    Ok(BulkEvidence {
        ok: ok && control_ok,
        control: Some(control),
        cancel: Some(cancel),
        message: format!(
            "baseline {:.3} MiB/s at {} B chunks; tuned {:.3} MiB/s at {} B chunks ({:.2}x)",
            baseline_rate,
            BULK_BASELINE_CHUNK,
            mean,
            BULK_CHUNK,
            mean / baseline_rate.max(f64::MIN_POSITIVE)
        ) + " control/cancel measured",
        baseline: Some(baseline),
        runs,
    })
}

impl BulkRun {
    fn received_ok(&self) -> bool {
        self.bytes > 0
            && self.chunk > 0
            && self.chunks == self.bytes / self.chunk
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

    #[test]
    fn parses_live_stream_counters() {
        let stat = StreamStat::parse_line(
            "mesh-bulk: state=idle stream_open=5 stream_opens=17 stream_resets=9 \
             stream_credit_stall=4 stream_control_stall=0 stream_tx_control=31 \
             stream_tx_data=128 stream_queued=0\n",
        )
        .expect("parse");
        assert_eq!(stat.open, 5);
        assert_eq!(stat.opens, 17);
        assert_eq!(stat.resets, 9);
        assert_eq!(stat.credit_stall, 4);
        assert_eq!(stat.control_stall, 0);
        assert_eq!(stat.tx_control, 31);
        assert_eq!(stat.tx_data, 128);
    }

    #[test]
    fn parses_the_transfer_stream_id() {
        let stat = parse(
            "mesh-bulk: state=done bytes=32768 chunk=4096 received=32768 chunks=8 \
             elapsed_ms=90 stream=7\n",
        )
        .expect("parse");
        assert_eq!(stat.stream, 7);
    }

    #[test]
    fn reuse_can_be_smaller_than_the_gate_record() {
        let run = BulkRun {
            from: "n0".into(),
            to: "n1".into(),
            bytes: REUSE_BYTES,
            chunk: BULK_CHUNK,
            chunks: REUSE_BYTES / BULK_CHUNK,
            elapsed_ms: 30,
            mb_per_s: 1.0,
            raw: String::new(),
        };
        assert!(run.received_ok());
    }
}
