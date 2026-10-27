# M3-ADR-0004 — readiness-driven module I/O with a 10 ms fallback

**Status:** implemented; limited (no hardware RX interrupt)
**Date:** 2026-10-27
**Issue:** <https://github.com/irix7/os/issues/248>
**Related:** [M2-ADR-0002](../adr/m2-0002-transport.md), ABI 14 `tcp_events`

## Context

The module tick was gated at 10 ms and the idle loop parked only on the CP0 timer.  A
mesh record that arrived just after a poll waited up to 10 ms; the wire study measured
10.005 ms p50 against 0.030 ms for a blocking exchange.  Lowering the timer alone would
not help because `module::poll` has its own 10 ms gate, and a 1 ms poll would raise idle
CPU without removing the poll.

## Decision

1. **Level-triggered readiness, not an event queue.**  `tcp_events(slot)` (ABI 14) returns
   RX/TX/CLOSED/LISTEN/ESTABLISHED bits recomputed from the live socket.  A lost
   notification cannot lose work because the next pass re-derives the mask.

2. **Readiness wakes the module immediately.**  `module::poll` calls the registered ticks
   when any module TCP slot has RX, recovered TX space or CLOSED, even if the 10 ms gate
   has not elapsed.  The 10 ms gate remains the deadline/fallback cadence for retries,
   leases, heartbeats and timers.

3. **One short observation after activity.**  The idle loop flushes module/telnet replies
   with a second `net::poll`, then parks in `wait_short` (about 0.5 ms) while a socket is
   ready or a module did work in the last 50 ms.  The timer fast path re-arms the normal
   10 ms interval after that wake, so this is one early observation, not a 2 kHz polling
   regime.  A fully idle node parks on the 10 ms timer as before.

4. **No hardware interrupt yet.**  The Seeq RX interrupt path is not enabled.  The trap
   vector currently handles the timer and exceptions; adding IP2 requires an `EXC_INT`
   branch, INT2/Seeq acknowledge ordering and a deferred-work flag.  That is a separate
   kernel slice; this ADR does not claim it.

## Evidence

Two real `iris` kernels, direct transport, 50-sample `/mesh/latency` probe
(at `4eec6f5`, on top of the current protocol stack):

| Path | p50 | p95 | min | max |
|---|---:|---:|---:|---:|
| 10 ms tick only (before) | 30 ms | 31–32 ms | 30 ms | 282 ms |
| readiness + 0.5 ms observation | 2 ms | 3 ms | 1 ms | 7 ms |

The p50 improvement is about 15x; the worst sample in this run was 7 ms.  M1/M2 markers are
unchanged (`mesh=single`, `m1-post: mesh=0/0/0`, join epoch 2, homes, route, migration).
The PROM is 480,044 / 524,288 bytes with the hash-chain payload encoder; the module is
392,248 bytes.

## Consequences

- A ready socket is serviced on the next loop pass instead of the next 10 ms tick.
- The 10 ms tick is still the only deadline clock; no work is lost when no event arrives.
- Idle CPU is unchanged after the 50 ms activity window; during active traffic the guest
  wakes about every 0.5 ms, which is the measured cost of the lower latency.
- The hardware IRQ, multi-slot fairness beyond the existing round-robin and a general
  async runtime are not part of this ADR.
