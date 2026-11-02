# Bounded mesh streams (#246, #337)

**Status:** live stream mux, host tests and a two-kernel control/cancel gate.
**Date:** 2026-11-03
**Issues:** [#337](https://github.com/irix7/os/issues/337),
[#246](https://github.com/irix7/os/issues/246),
[#330](https://github.com/irix7/os/issues/330)
**ADR:** [M3-ADR-0003](../adr/m3-0003-multiplexed-streams.md)

## 1. Implementation

`crates/mesh-stream` provides:

- `StreamTable`: 16 streams, initiator odd / responder even IDs, fixed slots, explicit
  `open`/`insert`/`accept`/`reset`, per-stream `credit_tx`, `credit_rx`, `queued`,
  `close_local`/`close_remote`, and counters for opens, resets and credit stalls.
- `Scheduler`: control-first queue plus a round-robin data queue.
- `StreamMux`: the table, scheduler and a bounded FIFO send ledger in one policy object.
- Bounds: `MAX_STREAMS=16`, `MAX_CREDIT=64 KiB`, `MAX_QUEUED_BYTES=64 KiB`,
  `INITIAL_CREDIT=16 KiB`, `MAX_INFLIGHT_RECORDS=64`.

The live MCP module registers five fixed control channels and allocates one data channel
per bulk transfer:

| Stream | Class | Purpose |
|---|---|---|
| 1 | control | heartbeat and handshake `Ping` |
| 2 | control | join request, confirmation, ack, summary, epoch |
| 3 | control | route query and invoke |
| 4 | control | migration |
| 5, 7, 9, ... | data | one dynamically opened bulk transfer at a time |
| 6 | control | latency probe |

The lower `NodeId` is the stream initiator, so a bulk transfer opened there gets an odd
ID and the peer accepts it with the opposite parity. Fixed IDs are inserted, which moves
the dynamic allocator past them. This is still Version 1: the stream ID is the existing
`FrameHeader::stream` field, with no old-peer format or negotiation.

Every outbound frame follows one path:

1. `StreamMux::begin`/`begin_control` checks the stream state, the per-stream queue bound
   and, for data, the send credit.
2. The module stages the frame in a bounded per-tick buffer.
3. `StreamMux::schedule` places the stream in the control or data class.
4. `flush` drains the scheduler. Control records are encoded and pushed before any data
   record, even when the data record was produced first.
5. A record accepted by `SendQueue` is committed to the ledger; the ledger is released
   only when `SendQueue::flush` reports that the sink wrote it. A blocked socket holds the
   stream's queue reservation and backpressures the next bulk chunk.
6. A control record that cannot reserve or encode never takes a "sent" path: the link
   closes (`stream-control`, `stream-queue`, `stream-encode` or `stream-ledger`) and the
   join path retries on a fresh queue.

`SendQueue` keeps its 8 KiB control reserve, so data cannot take the space a control
record needs. The scheduler is a second, independent ordering guarantee: a `WindowUpdate`
or `Cancel` produced after a bulk chunk in the same tick is still drained first.

## 2. Credit and bounds

- A bulk request carries its own initial window (two chunks). The responder sets its send
  credit from that value before the first chunk; each chunk consumes credit, and the
  transfer stops at `CreditExhausted` until a `WindowUpdate` arrives.
- `WindowUpdate` is consumed with `grant_send`; receive credit is tracked separately with
  `consume_receive` and `grant_receive`. A chunk above the receive window is a protocol
  error, not silent acceptance.
- Each stream has a 64 KiB queue bound. The send ledger is bounded at 64 records and the
  staged-frame set at 8 frames. Join bodies stay in `JOIN_TX`, small control bodies share
  a 1.5 KiB stage, and one bulk body reuses `FRAME_RX` after the receive pass; the module
  does not add a second maximum-size buffer.
- Control channels never consume data credit. `begin_control` is used for the `Cancel`
  and `WindowUpdate` records that travel on a data stream, so a backpressured bulk stream
  cannot block its own cancellation.

## 3. Cancel and reset

`GET /mesh/bulk?cancel=1` queues a `Cancel` on the transfer's stream, marks the transfer
failed, and retires the stream after the sink accepts the cancel record. The peer's
`on_cancel` resets the stream, clears its reply state and logs
`mesh-stream: cancel stream=N reset=...`. A transfer that completes normally also retires
its stream; a new transfer opens a new ID. A chunk or window update that crosses the
cancel is treated as stale and does not reset the link.

## 4. Host tests

```sh
cargo test -p mesh-stream
```

Thirteen tests cover allocation parity and the fixed-ID allocator bump, reserve/commit/
settle on every live stream, control bypassing exhausted data credit, cancel on a data
stream bypassing credit, a blocked sink backpressuring only the data stream, reset
reclaiming a stream with a queued record, and send/receive credit accounting staying
separate.

## 5. Two-kernel evidence

`tools/mesh-stream-gate.sh` runs the host suites, builds the two-node artefacts and runs
`m2-rig stream`. The gate records:

- idle-link control RPC p50/p95;
- control RPC p50/p95 while a 512 KiB bulk transfer is active (the transfer must still be
  `running` when the series ends);
- the bulk transfer's chunk count and elapsed time;
- the cancel result, both nodes' reset counters and a 32 KiB reuse transfer after the
  cancel.

Measured result on two real `iris` kernels (2026-11-03, direct transport):

```text
baseline control: 4 samples, p50 14 ms, p95 15 ms
control during bulk: 4 samples, p50 57 ms, p95 126 ms, min 13 ms, max 126 ms
bulk: 524288 B in 128 x 4096 B chunks, 2332 ms, 0.214 MiB/s
cancel: state=failed error=cancel after 114 chunks
resets: origin 1->2, peer 1->2
reuse: 32768 B in 8 x 4096 B chunks, 196 ms
stream_control_stall=0 stream_credit_stall=0
```

The gate starts the bulk and control series in one module request
(`GET /mesh/bulk?start=1&bytes=N&chunk=M&latency=K`).  The module's HTTP surface can
starve while the mesh socket is continuously ready, so starting the latency probe through
a second HTTP request would measure the HTTP phase machine rather than the mesh control
stream.  One request queues both, and the control series then runs on stream 6 while the
bulk stream is credit-limited.

The earlier #247 credit run, kept for comparison, was:

```text
n0 -> n1: 65536 B in 64 x 1024 B chunks, 832 ms, 0.075 MiB/s (baseline)
n0 -> n1: 65536 B in 16 x 4096 B chunks, 191 ms, 0.327 MiB/s
n1 -> n0: 65536 B in 16 x 4096 B chunks, 591 ms, 0.106 MiB/s
```

The 4 KiB chunk size is deliberate for the measurement path: larger 8 KiB records hit an
intermittent mid-record stall on the direct/relay path on the shared host. The module's
receive deadline fails closed with a partial decoder (`buffered=… expected=…`), and the
rig records it. That is a transport/harness limit, not a credit deadlock: the credit
counters advance and the control class is not blocked.

## 6. What is not claimed

- One bulk transfer per link at a time. The stream table supports 16 streams, but the HTTP
  measurement surface still has one `bulk` slot per node.
- One TCP link per peer; TCP-level head-of-line blocking remains between peers sharing a
  hub link.
- Control frames share the transport's 8 KiB control reserve; when a sink is fully blocked
  the link fails closed rather than growing a control queue.
- Flow control is per stream, not per peer.
- The scheduler is record-level.  A bulk record that is partially written to the single
  transport byte queue can still delay a control record queued after it; byte-level
  preemption would need separate control/data transport queues.  This is the remaining
  head-of-line gap.  It showed up as control p95 126 ms against a 15 ms idle p95 under
  heavy host load, but the control series completed and no control record was dropped.
- No TLS/peer authentication.
- The gate's control series is a `Ping` on stream 6, not a route invoke. Route and
  migration frames use the same control class and the same `reserve` path, but the
  two-kernel timing series is the ping probe.
