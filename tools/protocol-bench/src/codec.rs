// SPDX-License-Identifier: GPL-3.0-only
//! Codec cost for the real record framing.
//!
//! Two measurements per payload size:
//!
//! * `roundtrip` — build the envelope (mesh header plus frame), `encode_record`
//!   into a buffer and `decode_record` it back: two header CRCs, field
//!   encoding, one body copy and full validation;
//! * `queue` — `SendQueue::push_record` + `flush` into a sink that accepts
//!   everything, which is the module's send path minus the socket.
//!
//! Each timed batch cycles over 64 distinct payloads and tags so the optimiser
//! cannot hoist an idempotent copy or a pure decode out of the loop, and every
//! result is folded through a sparse checksum of the whole buffer so no store
//! can be dropped.  The reported number is the median of five batch means.

use std::hint::black_box;
use std::time::Instant;

use mesh_transport::{encode_record, record_len, SendQueue, WriteOutcome};
use sys_primitives::{
    mesh_flags, MeshEnvelope, MeshHeader, MessageFrame, MessageKind, OpCode, StreamId, Tag,
};

use crate::proto::{CLIENT, SERVER};
use crate::stats::{header, row, us};

const SIZES: [usize; 6] = [0, 16, 332, 1024, 8192, 65536 - 324];

fn envelope<'a>(tag: u32, body: &'a [u8]) -> MeshEnvelope<'a> {
    let frame = MessageFrame::inline(
        MessageKind::Request,
        OpCode::Ping,
        StreamId(1),
        Tag(tag),
        body,
    )
    .expect("frame");
    let mut header = MeshHeader::new(CLIENT, SERVER, SERVER);
    header.flags = mesh_flags::PROBE;
    header.request_id = u64::from(tag);
    header.ttl = MeshHeader::MAX_HOPS;
    MeshEnvelope { header, frame }
}

/// A sparse checksum that touches every 64th byte, so every cache line of the
/// buffer must hold the value the encoder wrote.
fn sparse_checksum(bytes: &[u8]) -> u64 {
    bytes.iter().step_by(64).fold(0u64, |acc, byte| {
        acc.wrapping_mul(31).wrapping_add(u64::from(*byte))
    })
}

/// A sink that accepts everything and keeps only a byte count.
struct NullSink {
    bytes: u64,
}

impl mesh_transport::ByteSink for NullSink {
    fn write(&mut self, data: &[u8]) -> WriteOutcome {
        self.bytes += data.len() as u64;
        WriteOutcome::Wrote(data.len())
    }
}

/// Run `iterations` of `work`, returning nanoseconds per iteration.
fn batch(iterations: usize, mut work: impl FnMut(usize) -> u64) -> u64 {
    let start = Instant::now();
    let mut acc = 0u64;
    for index in 0..iterations {
        acc = acc.wrapping_add(work(index));
    }
    let elapsed = start.elapsed().as_nanos() as u64;
    black_box(acc);
    (elapsed / iterations as u64).max(1)
}

fn median_of(mut values: [u64; 5]) -> u64 {
    values.sort_unstable();
    values[2]
}

/// Keep the batch short enough for the largest payload.
fn iterations_for(size: usize) -> usize {
    (100_000_000 / (size + 360)).clamp(1_500, 100_000)
}

/// A 256-entry CRC32C table, built once, for the calibration comparison only.
fn crc_table() -> [u32; 256] {
    let mut table = [0u32; 256];
    for (index, slot) in table.iter_mut().enumerate() {
        let mut crc = index as u32;
        for _ in 0..8 {
            let mask = (crc & 1).wrapping_neg();
            crc = (crc >> 1) ^ (0x82f6_3b78 & mask);
        }
        *slot = crc;
    }
    table
}

/// Table-driven CRC32C over `data`.
fn table_crc32c(table: &[u32; 256], data: &[u8]) -> u32 {
    let mut crc = 0xffff_ffffu32;
    for &byte in data {
        crc = (crc >> 8) ^ table[((crc ^ u32::from(byte)) & 0xff) as usize];
    }
    !crc
}

/// Run the codec section.
pub fn run() {
    println!("\n## Codec cost (measured on this host)\n");
    header(&[
        "payload B",
        "record B",
        "roundtrip us",
        "roundtrip ns/B",
        "queue us",
        "roundtrip records/s",
    ]);

    for size in SIZES {
        let bodies: Vec<Vec<u8>> = (0..64u8).map(|fill| vec![fill; size]).collect();
        let iterations = iterations_for(size);
        let mut out = vec![0u8; 4 + 128 + 128 + 64 + size];
        let roundtrip_ns = median_of([0; 5].map(|_| {
            batch(iterations, |index| {
                let slot = index & 63;
                let envelope = envelope(slot as u32 + 1, &bodies[slot]);
                let written = encode_record(&envelope, &mut out).expect("encode");
                let decoded = mesh_transport::decode_record(&out[..written]).expect("decode");
                sparse_checksum(decoded.frame.body)
            })
        }));
        let queue_ns = median_of([0; 5].map(|_| {
            let mut queue = SendQueue::new();
            let mut sink = NullSink { bytes: 0 };
            batch(iterations.min(20_000), |index| {
                let slot = index & 63;
                let envelope = envelope(slot as u32 + 1, &bodies[slot]);
                queue.push_record(&envelope, 0).expect("push");
                queue.flush(&mut sink);
                sink.bytes
            })
        }));
        let record_len = record_len(&envelope(1, &bodies[0])).expect("record length");
        let per_byte = roundtrip_ns as f64 / record_len as f64;
        row(&[
            size.to_string(),
            record_len.to_string(),
            format!("{:.2}", us(roundtrip_ns)),
            format!("{per_byte:.2}"),
            format!("{:.2}", us(queue_ns)),
            (1_000_000_000 / roundtrip_ns).to_string(),
        ]);
    }

    println!("\nCRC32C over one 128-byte header (shipped table-free loop versus a 1 KiB table):\n");
    header(&["variant", "128 B us", "1024 B us"]);
    let mut data = [0x5au8; 1024];
    let table = crc_table();
    let ns_one = batch(50_000, |index| {
        data[0] = index as u8;
        u64::from(sys_primitives::crc32c(&data[..128]))
    });
    let ns_four = batch(20_000, |index| {
        data[0] = index as u8;
        u64::from(sys_primitives::crc32c(&data))
    });
    row(&[
        "shipped bitwise".to_string(),
        format!("{:.3}", us(ns_one)),
        format!("{:.3}", us(ns_four)),
    ]);
    let ns_one = batch(50_000, |index| {
        data[0] = index as u8;
        u64::from(table_crc32c(&table, &data[..128]))
    });
    let ns_four = batch(20_000, |index| {
        data[0] = index as u8;
        u64::from(table_crc32c(&table, &data))
    });
    row(&[
        "table-driven".to_string(),
        format!("{:.3}", us(ns_one)),
        format!("{:.3}", us(ns_four)),
    ]);

    println!();
    println!("Notes:");
    println!("- `roundtrip` builds the `MeshHeader` and `MessageFrame` (as");
    println!("  `modules/mcp/src/mesh.rs::queue_ping` does), encodes and decodes one record.");
    println!("- `roundtrip records/s` is a codec ceiling, not a socket throughput.");
    println!(
        "- The guest is an emulated 50 MHz-class R4000; these host microseconds are a lower bound."
    );
    println!("- `queue` adds one body copy into the `SendQueue` buffer and the framing write.");
    println!("- CRC32C is table-free (8 shift rounds per byte) in `sys-primitives::crc32c`; the");
    println!("  encoder and the decoder each compute two of them (mesh header, frame header).");
}
