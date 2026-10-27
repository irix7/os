# M3-ADR-0003 — Version 1 bounded mesh streams

**Status:** implemented for the measurement streams; limited
**Date:** 2026-10-27
**Issue:** <https://github.com/irix7/os/issues/246>
**Related:** [M2-ADR-0002](../adr/m2-0002-transport.md),
[M2-ADR-0010](../adr/m2-0010-direct-peer-transport.md),
[`../research/mesh-streams.md`](../research/mesh-streams.md)

## Context

Every M2 record carries a `FrameHeader.stream`, but the live module always used
streams 1–4 and one `SendQueue`.  A bulk transfer could therefore delay a join, lease or
route acknowledgement for the whole transfer, and the module had no per-stream credit,
cancel or reset.

## Decision

1. **Version 1 is the only target.**  The stream layer uses the existing `FrameHeader.stream`
   and `flags::MORE`/`EOS`; there is no old-peer negotiation or dual format.

2. **Bounded table.**  `crates/mesh-stream` owns a 16-entry `StreamTable`, initiator odd /
   responder even allocation, per-stream byte credit (64 KiB maximum, 16 KiB initial),
   a 64 KiB per-stream queue bound, explicit close/reset and a control/data scheduler
   that always selects control first.  It is `no_std + alloc`, has no socket dependency
   and is unit-tested on the host.

3. **Control is never credit-blocked.**  Join, route, lease, migration, latency and
   `WindowUpdate`/`Cancel` frames use the control class.  The bulk stream uses data credit;
   it can stall without closing the link.

4. **Credit loop.**  The bulk origin grants two chunks in the request; the peer sends at
   most that many, then waits.  The origin grants two more after it has consumed each
   pair with a `Control/WindowUpdate` record.  The receiver's stream table and the peer's
   reply credit are updated together, so a slow consumer applies backpressure instead of
   a TCP reset.

5. **Cancel/reset.**  `GET /mesh/bulk?cancel=1` sends `Cancel/Cancel` for stream 5,
   clears the local transfer and resets the stream slot; the peer clears its reply state.
   There is no half-cancelled stream and no tag leak.

6. **No kernel/ROM change.**  The stream layer is module-side.  The only transport
   requirements are the existing stream ID, offset/length and MORE/EOS fields.

## Evidence and limits

- `mesh-stream` host tests: allocation parity, table bound, credit exhaustion, control
  bypass, per-stream queue bound, reset/reclaim, control-first scheduling and close
  transitions.
- Real two-kernel direct run with credit: 64 KiB in 16 × 4 KiB chunks, 191 ms
  (0.327 MiB/s) in the clean runs; the 1 KiB baseline was 832 ms.  A reverse-direction
  run on the shared host stalled mid-record at 12 KiB; the module's receive deadline
  failed closed and the rig recorded it.  This is the same intermittent large-record
  stall seen in the hub/relay paths, not a credit deadlock: the credit counters advanced
  and `WindowUpdate` frames were accepted.
- Control-during-bulk and cancel paths are implemented and have a rig scenario; the
  final end-to-end bundle is not claimed because the host was not able to complete a
  clean run at the time of writing.  The host policy tests are green.

## Consequences

- A bulk stream can no longer starve the control class; a slow consumer stalls only its
  own stream.
- The module still has one TCP link and one `SendQueue`; the scheduler orders records but
  does not create TCP-level fairness between peers.
- A general multi-stream application API is not added.  The measurement streams (5 data,
  6 control) are the v1 consumers.
