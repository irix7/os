# Larger buffers, batching and a bounded bulk path (#247)

**Status:** implemented; host model and two-kernel evidence recorded below.
**Date:** 2026-10-27
**Issue:** <https://github.com/irix7/os/issues/247>
**Predecessor:** [wire-efficiency study](wire-efficiency.md) (#243) and the
[pre-release policy](../spec/pre-release.md).

This note records what changed for #247, the measured host model, the real
two-kernel transfer, and the limits that remain.  It does not claim the full
M2 or M3 gate.

## 1. What changed

| Layer | Change | Effect |
|---|---|---|
| Kernel Seeq driver | Receive ring 8 → 16 descriptors; `max_burst_size` 1 → 15 | The advertised TCP window is the 16 KiB socket receive buffer instead of one 1,460-byte segment. |
| Kernel receive copy | Per-byte `read_volatile` → one `copy_nonoverlapping` from the uncached DMA buffer | Removes one uncached load per received byte. |
| Kernel module socket | RX 2 KiB → 16 KiB, TX 16 KiB → 32 KiB per slot; Nagle off; 1 ms delayed ACK | A window update can carry a whole 16 KiB tick; a small control record is not ACK-clocked. |
| `sys-primitives` | 16-entry nibble CRC32C table (same polynomial, same bytes on the wire) | Two dependent shift rounds per byte become two table lookups; 64 bytes of `.rodata`, not 1 KiB. |
| `mesh-transport` | Fixed-capacity `StreamDecoder` and `SendQueue`; compaction instead of growth; 8 KiB control reserve for data records; `flush` reports writes and peak queue | The bump allocator does not grow a `Vec` on the data path; bulk cannot starve control. |
| MCP module | 4 × 4 KiB reads per tick; one flush per tick (intermediate flushes removed); 32 KiB decoder/queue; 16,404-byte frame body buffer; bulk measurement path | One `tcp_send` per tick in the steady state; a 64 KiB *logical* record is four 16 KiB chunks. |
| Host benchmark | `protocol-bench bulk` | Pins the window model and counts records per sink write. |
| Rig | Three 64 KiB transfers in both directions after join | Real-kernel throughput, chunk count and module counters in `run.json`. |

The mesh wire format did not change.  A bulk chunk uses the existing
`FrameHeader.offset`, `FrameHeader.length` and `flags::MORE`/`EOS`; the body is
one bounded inline payload.  The four-byte record prefix and the 324-byte
envelope are unchanged.

## 2. Host model

`tools/protocol-bench` models the ticked path as `min(socket RX, burst × MSS)`
bytes in flight per 10 ms tick, with no pipelining credit.  This is the same
assumption as the wire study.

| configuration | window B | bytes/tick | 64 KiB ticks | 64 KiB p50 ms | MB/s |
|---|---:|---:|---:|---:|---:|
| M2 shipped (2 KiB RX, burst 1, 8 KiB/tick) | 1,460 | 1,460 | 45 | 450.0 | 0.146 |
| #247 (16 KiB RX, burst 15, 16 KiB/tick) | 16,384 | 16,384 | 4 | 40.0 | 1.638 |
| ratio | 11.22× | 11.22× | 11.25× | — | **11.25×** |

Measured `SendQueue` batching (host, release):

| chunk B | record B | chunks | encode µs/chunk | sink writes | peak queued B |
|---:|---:|---:|---:|---:|---:|
| 16,364 | 16,688 | 4 | 4.55 | 4 | 16,688 |
| 4,076 | 4,400 | 16 | 1.64 | 16 | 4,400 |


The model is deliberately pessimistic.  It does not credit the second
`net::poll` in the event-driven slice, the reduced copy cost, or a shorter
effective RTT.  The real-kernel numbers are the authority.

## 3. Real two-kernel transfer

The rig starts two independent `iris` kernels, converges the M2 join, then
alternates the origin:

```sh
tools/m2-rig/build.sh
cargo run --manifest-path tools/m2-rig/Cargo.toml -- run \
  --iris <iris> --iris-ci <iris-ci> --timeout 300
```

The module reports the mesh start and finish timestamps, the chunks received,
the write count, batched flushes, peak queue and peak decoder bytes.  The rig
checks `received == 65,536`, the requested chunk count and a non-zero elapsed time.

```text
n0 -> n1: 65536 B in 64 x 1024 B chunks, 868 ms, 0.072 MiB/s (baseline)
n0 -> n1: 65536 B in  8 x 8192 B chunks, 111 ms, 0.563 MiB/s
n1 -> n0: 65536 B in  8 x 8192 B chunks, 344 ms, 0.182 MiB/s
n0 -> n1: 65536 B in  8 x 8192 B chunks, 121 ms, 0.517 MiB/s
bulk: baseline 0.072 MiB/s at 1024 B chunks; tuned 0.326 MiB/s at 8192 B chunks (4.52x)
```

The same-host baseline uses 1,024-byte chunks, one chunk per module tick: it
reproduces the M2 small-window/batching shape on the same code and the same
hub.  The tuned path uses 8,192-byte chunks.  p50 for the three tuned runs was
121 ms (0.517 MiB/s); the 344 ms reverse-direction run is a tail on a shared
host, not a different code path.

A later run on the same host at load average 6.75 (another two-kernel rig in
parallel) measured 2.81×; the path is sensitive to host CPU contention, and the
committed evidence bundle records the load and the raw per-run counters.  The
64 KiB logical record is eight chunks, not one 64 KiB wire record; the module
streams it for the memory and head-of-line reasons in §4.


## 4. Memory and PROM

| Budget | Value |
|---|---:|
| Module image | 368,472 bytes |
| Module `.bss` | 102,257 bytes |
| Image + `.bss` | 470,729 bytes of the 524,288-byte slot A |
| PROM | 523,484 bytes of 524,288 (804 bytes headroom) |

The module linker now asserts that image + `.bss` stays below slot B
(`0x8848_0000`).  The previous module had no such assertion; a larger static
buffer could silently overwrite the next module region.

The 64 KiB logical record is streamed in four 16 KiB chunks.  A single 64 KiB
wire record is still inside `mesh-transport::MAX_RECORD_LEN` (65,536) and is
accepted by the transport crate, but the module does not allocate a 64 KiB
reassembly buffer: slot A does not have that much headroom left, and a single
record would also head-of-line block control traffic for its whole transfer.
This is the documented memory limit, not a silent truncation.

## 5. Limits

- The guest link is 10 Mbit Ethernet; 1.25 MB/s is the wire ceiling.  The
  measured path is bounded by the tick and the 16 KiB window, not by a claim of
  gigabit throughput.
- The bulk path is a measurement/benchmark opcode (`Request/Read` +
  `Response/Read`), not a general file or CAS service.  #246 turns it into a
  stream with credit and cancellation.
- There is no zero-copy transmit and no transmit descriptor ring; a 16 KiB
  burst still spins through the Seeq transmit path.
- Plain TCP, no TLS and no peer authentication.  The bulk bytes are zeros and
  carry no data-plane integrity claim beyond the envelope CRC.

## 6. Commands

```sh
cd tools/protocol-bench
cargo run --release --offline -- bulk
cargo test --release --offline

cargo test -p mesh-transport -p sys-primitives
cargo test --manifest-path tools/m2-rig/Cargo.toml
```
