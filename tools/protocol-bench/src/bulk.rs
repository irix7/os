// SPDX-License-Identifier: GPL-3.0-only
//! Bulk throughput and batching for the #247 buffer/window change.
//!
//! Two kinds of number are printed:
//!
//! * a **host measurement** of the real `SendQueue`: how many records one
//!   `flush` coalesces into one sink write, and how long it takes to encode a
//!   16 KiB chunk;
//! * a **derived model** of the kernel's ticked TCP path.  The model is the
//!   same one used in `docs/research/wire-efficiency.md`: the receiver
//!   advertises `min(socket RX, max_burst_size * MSS)` and the sender can put
//!   at most that many bytes in flight per tick.  The two configurations are
//!   the shipped M2 path and the #247 path; the real-kernel rig is the
//!   authority for the model.
//!
//! The model is deliberately pessimistic: it assumes one tick per window and
//! does not credit pipelining across ticks.  That is the same assumption the
//! M2 evidence used.

use std::hint::black_box;
use std::time::Instant;

use mesh_transport::{SendQueue, WriteOutcome};
use sys_primitives::{
    mesh_flags, MeshEnvelope, MeshHeader, MessageFrame, MessageKind, NodeId, OpCode, StreamId, Tag,
};

use crate::stats::{header, row, us};

const TICK_MS: f64 = 10.0;
const TOTAL_BYTES: usize = 64 * 1024;
const MSS: usize = 1460;

/// One kernel/module configuration.
struct Config {
    name: &'static str,
    socket_rx: usize,
    burst: usize,
    read_per_tick: usize,
}

impl Config {
    fn window(&self) -> usize {
        (self.burst * MSS).min(self.socket_rx)
    }

    fn bytes_per_tick(&self) -> usize {
        self.window().min(self.read_per_tick)
    }

    fn ticks_for(&self, total: usize) -> usize {
        total.div_ceil(self.bytes_per_tick()).max(1)
    }
}

fn node(byte: u8) -> NodeId {
    NodeId([byte; 16])
}

struct NullSink {
    bytes: usize,
    writes: usize,
}

impl mesh_transport::ByteSink for NullSink {
    fn write(&mut self, data: &[u8]) -> WriteOutcome {
        self.bytes += data.len();
        self.writes += 1;
        WriteOutcome::Wrote(data.len())
    }
}

/// Encode `chunks` records of `body` bytes into one queue and flush once.
fn encode_chunks(body_len: usize, chunks: usize) -> (u64, usize, usize, usize) {
    let body = vec![0u8; body_len];
    let frame = MessageFrame::inline(
        MessageKind::Response,
        OpCode::Read,
        StreamId(5),
        Tag(0),
        &body,
    )
    .expect("frame");
    let mut mesh = MeshHeader::new(node(1), node(2), node(2));
    mesh.flags = mesh_flags::PROBE;
    let envelope = MeshEnvelope {
        header: mesh,
        frame,
    };
    let per_record = mesh_transport::record_len(&envelope).expect("record");
    // Two chunks plus the framing fit in 32 KiB; measure one chunk per tick
    // as the module does, so a 64 KiB logical record is four writes.
    let mut queue = SendQueue::with_capacity(32 * 1024);
    let start = Instant::now();
    let mut sink = NullSink {
        bytes: 0,
        writes: 0,
    };
    for _ in 0..chunks {
        queue.push_data_record(&envelope, 0).expect("queue");
        queue.flush(&mut sink);
    }
    let elapsed = start.elapsed().as_nanos() as u64;
    (
        elapsed / chunks as u64,
        sink.writes,
        per_record,
        queue.peak_pending(),
    )
}

pub fn run() {
    println!("## Bulk throughput and batching");
    println!();
    println!("Host model: one 10 ms tick per window, no pipelining credit.");
    println!();

    let baseline = Config {
        name: "M2 shipped (2 KiB RX, burst 1, 8 KiB/tick)",
        socket_rx: 2048,
        burst: 1,
        read_per_tick: 8 * 1024,
    };
    let tuned = Config {
        name: "#247 (16 KiB RX, burst 15, 16 KiB/tick)",
        socket_rx: 16 * 1024,
        burst: 15,
        read_per_tick: 16 * 1024,
    };

    header(&[
        "configuration",
        "window B",
        "bytes/tick",
        "64 KiB ticks",
        "64 KiB p50 ms",
        "MB/s",
    ]);
    let mut baseline_rate = 0.0;
    let mut tuned_rate = 0.0;
    for config in [&baseline, &tuned] {
        let ticks = config.ticks_for(TOTAL_BYTES);
        let latency = ticks as f64 * TICK_MS;
        let rate = TOTAL_BYTES as f64 / (latency / 1000.0) / 1_000_000.0;
        if config.name.starts_with("M2") {
            baseline_rate = rate;
        } else {
            tuned_rate = rate;
        }
        row(&[
            config.name.into(),
            config.window().to_string(),
            config.bytes_per_tick().to_string(),
            ticks.to_string(),
            format!("{latency:.1}"),
            format!("{rate:.3}"),
        ]);
    }
    row(&[
        "ratio (tuned / baseline)".into(),
        format!("{:.2}x", tuned.window() as f64 / baseline.window() as f64),
        format!(
            "{:.2}x",
            tuned.bytes_per_tick() as f64 / baseline.bytes_per_tick() as f64
        ),
        format!("{:.2}x", baseline.ticks_for(TOTAL_BYTES) as f64 / tuned.ticks_for(TOTAL_BYTES) as f64),
        String::new(),
        format!("{:.2}x", tuned_rate / baseline_rate),
    ]);
    assert!(
        tuned_rate >= baseline_rate * 4.0,
        "the #247 configuration must model at least 4x the M2 path"
    );

    println!();
    println!("Host measurement: one `SendQueue` flush per 16 KiB chunk, 64 KiB logical record.");
    header(&[
        "chunk B",
        "record B",
        "chunks",
        "encode us/chunk",
        "sink writes",
        "peak queued B",
        "logical MB/s",
    ]);
    for (body, chunks) in [(16 * 1024 - 20, 4usize), (4 * 1024 - 20, 16)] {
        let (ns, writes, record, peak) = encode_chunks(body, chunks);
        let logical = body * chunks;
        let rate = logical as f64 / (ns as f64 * chunks as f64 / 1_000_000_000.0) / 1_000_000.0;
        row(&[
            format!("{body}"),
            record.to_string(),
            chunks.to_string(),
            format!("{:.2}", us(ns)),
            writes.to_string(),
            peak.to_string(),
            format!("{rate:.1}"),
        ]);
    }

    println!();
    println!(
        "Bulk model floor: {} B / {:.1} ms = {:.3} MB/s (M2); {:.3} MB/s (#247).",
        TOTAL_BYTES,
        baseline.ticks_for(TOTAL_BYTES) as f64 * TICK_MS,
        baseline_rate,
        tuned_rate
    );
    black_box(baseline_rate);
}
