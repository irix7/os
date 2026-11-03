# M3-ADR-0004 — readiness-driven module I/O with a 10 ms fallback

**Status:** superseded in part by [rx-interrupt.md](../research/rx-interrupt.md):
the Seeq receive interrupt is now enabled and `Status.IM2` is unmasked after a
boot self-test.  The readiness mask and the 10 ms deadline fallback remain.
**Date:** 2026-10-27, amended 2026-11-03
**Issue:** <https://github.com/irix7/os/issues/248>
**Related:** [M2-ADR-0002](../adr/m2-0002-transport.md), ABI 14 `tcp_events`,
[#336](https://github.com/irix7/os/issues/336)

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

4. **Hardware receive interrupt.**  The Seeq RX command enables `INTEOF`, every
   receive descriptor carries `XIE`, and the IOC's local-0 Ethernet bit plus
   `Status.IM2` are unmasked only after the handler has passed a boot
   self-test.  The handler reads the split HPC3 status, acknowledges the Seeq
   and HPC3 in order, sets one deferred flag and returns; the idle loop drains
   at most one ring and then runs the module regions whose slots are ready.
   With the interrupt armed the idle loop parks on the 10 ms `wait`, so the
   0.5 ms observation is a poll-only path.  See
   [rx-interrupt.md](../research/rx-interrupt.md) for the ordering rules and
   the counter evidence.

## Evidence

Two real `iris` kernels, direct transport, 50-sample `/mesh/latency` probe
(at `4eec6f5`, on top of the current protocol stack):

| Path | p50 | p95 | min | max |
|---|---:|---:|---:|---:|
| 10 ms tick only (before) | 30 ms | 31–32 ms | 30 ms | 282 ms |
| readiness + 0.5 ms observation | 2 ms | 3 ms | 1 ms | 7 ms |
| receive interrupt, hot 20 ms gap | 2 ms | 2–11 ms | 1 ms | 12 ms |
| receive interrupt, cold 250 ms gap | 2 ms | 2 ms | 1 ms | 2 ms |
| polled control, cold 250 ms gap | 10 ms | 20 ms | 2 ms | 20 ms |

The interrupt rows are the median of three 64-sample hot / 48-sample cold
series per direction over the M2 host relay, on a load-average-2.6–3.3 host;
see [rx-interrupt.md](../research/rx-interrupt.md) §3 for the full table, the
p99/p95 detail, the idle-load numbers and the run-to-run spread.  The cold p50
is 10x inside the 10 ms tick.  M1/M2 markers are unchanged (`mesh=single`,
`m1-post: mesh=0/0/0`, join epoch 2, homes, route, migration).  The PROM is
513,296 / 524,288 bytes with the hash-chain payload encoder; the module is
431,016 bytes.

## Consequences

- A ready socket is serviced on the next loop pass instead of the next 10 ms tick.
- A frame wakes a parked node through IP2; the first request after idle no
  longer waits for the tick.
- The 10 ms tick is still the only deadline clock; no work is lost when no
  event arrives, and `net.rx=off` restores the polled path exactly.
- Idle load is 1 % of counted time and one loop pass per tick; during active
  traffic the guest wakes per frame.
- The hardware IRQ, the split HPC3 status merge and the acknowledge ordering
  are in `kernel/src/net_irq.rs` and `kernel/src/net.rs`; multi-slot fairness
  beyond per-region wake and a general async runtime are not part of this ADR.
