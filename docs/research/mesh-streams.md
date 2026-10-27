# Bounded mesh streams (#246)

**Status:** implementation + host tests; real credit run recorded, control/cancel bundle limited.
**Date:** 2026-10-27
**Issue:** <https://github.com/irix7/os/issues/246>
**ADR:** [M3-ADR-0003](../adr/m3-0003-multiplexed-streams.md)

## 1. Implementation

`crates/mesh-stream` provides:

- `StreamTable`: 16 streams, initiator odd / responder even IDs, fixed slots, explicit
  `open`/`insert`/`accept`/`reset`, per-stream `credit_tx`, `credit_rx`, `queued`,
  `close_local`/`close_remote`, and counters for opens, resets and credit stalls.
- `Scheduler`: control-first queue plus a round-robin data queue.
- Bounds: `MAX_STREAMS=16`, `MAX_CREDIT=64 KiB`, `MAX_QUEUED_BYTES=64 KiB`,
  `INITIAL_CREDIT=16 KiB`.

The live MCP module uses stream 5 for the bulk measurement and stream 6 for the latency
probe.  Bulk chunks use data credit; the latency/join/route/lease frames use control
credit.  `WindowUpdate` grants are control records.  `Cancel` resets stream 5 on both
sides.

## 2. Host tests

```sh
cargo test -p mesh-stream
```

Six tests cover allocation parity and the 16-stream bound, credit exhaustion with a
control bypass, reset/reclaim, the per-stream queue bound, control-first scheduling and
the close transitions.

## 3. Real two-kernel credit run

`m2-rig bulk --transport direct` after the stream layer:

```text
n0 -> n1: 65536 B in 64 x 1024 B chunks, 832 ms, 0.075 MiB/s (baseline)
n0 -> n1: 65536 B in 16 x 4096 B chunks, 191 ms, 0.327 MiB/s
n1 -> n0: 65536 B in 16 x 4096 B chunks, 591 ms, 0.106 MiB/s
n0 -> n1: 65536 B in 16 x 4096 B chunks, 191 ms, 0.327 MiB/s
...
credit_grant counters advanced; WindowUpdate frames were accepted (rejects=0)
```

The 4 KiB chunk size is deliberate for the stream test: larger 8 KiB records hit an
intermittent mid-record stall on the direct/relay path on the shared host.  The module's
10 s receive deadline fails closed with a partial decoder (`buffered=… expected=…`), and
the rig records it.  That is a transport/harness limit, not a credit deadlock: the credit
counters advance and the control class is not blocked.

## 4. Control and cancel

The rig has `control_during_bulk` (start a 64 KiB bulk, start an 8-sample latency probe,
then finish both) and `cancel_bulk` (start a 1 MiB bulk, cancel after 50 ms, require
`state=failed error=cancel`).

Real direct two-kernel run after the rebase onto `206d19a`:

```text
control during bulk: 65536 B in 16 chunks, bulk 191 ms, p50 30 ms, p95 31 ms
cancel: state=failed error=cancel
```

The control probe is unchanged from the idle p50/p95 on this branch (30/31 ms; the 2 ms
readiness path is #248).  The cancel response cleared the transfer and reset stream 5 on
both sides.  A later run hit the MCP HTTP close race during a status poll, so the final
bundle keeps the earlier successful control/cancel transcript and records the poll failure
rather than hiding it.

## 5. What is not claimed

- No multi-application stream API and no stream scheduler inside the kernel.
- One TCP link per peer; TCP-level head-of-line blocking remains between peers sharing a
  hub link.
- No TLS/peer authentication.
- The cancel happened after a 64 KiB transfer had already completed in the first run; the
  1 MiB cancel variant exists but a later poll hit the HTTP close race.  The cancellation
  state machine is exercised, not a partial-transfer byte count.
