# M3-ADR-0002 — A bounded multi-member hub and a real eight-kernel convergence path

**Status:** accepted for the M3 multi-member slice, limited
**Date:** 2026-10-27
**Deciders:** M3 shard slice, for review at the M3 gate
**Related:** [M3-ADR-0001](m3-0001-shard-slice.md), [M2-ADR-0002](m2-0002-transport.md),
[M2-ADR-0005](m2-0005-join-epoch.md), [M2-ADR-0006](m2-0006-homes.md),
[`../spec/system.md`](../spec/system.md) §5 and §9,
[`../plan/m3-rig.md`](../plan/m3-rig.md)

## Context

M3-ADR-0001 recorded a real-kernel limit of two nodes.  Eight iris instances
booted, but the root's emulated TCP receive path appeared to truncate a
`MeshEnvelope` when several member streams were multiplexed onto its single
mesh slot.  The ADR left the eight-node gate as host policy only and named a
multi-peer transport as the next work item.

Re-running the three-node path with a traced hub separated that claim into
three distinct defects:

1. **The hub dropped complete frames.**  Each destination had a bounded
   `sync_channel`, but the reader used `try_send`; a full queue silently
   discarded a whole record.  Under a slow root, member join requests and
   confirmations were lost with no protocol signal.  The first revision called
   this backpressure, but it was drop-on-full.
2. **The hub hid and amplified latency.**  It wrote every record in 128-byte
   chunks with a 2 ms sleep.  That made one slow destination gate all sources on
   that link and stretched a 388-byte record over many scheduler ticks.  The
   guest's receive deadlines were then measured against the hub's pacing, not
   the guest's actual progress.
3. **The guest module checked deadlines before reading.**  `Mesh::connected`
   evaluated the receive, send and heartbeat deadlines before calling
   `tcp_recv`.  A bounded serial burst (the root writing its confirmed table)
   could keep the tick busy for longer than the 10 s receive deadline while the
   next member's bytes were already queued in the socket.  On return the module
   disconnected a healthy link without reading those bytes.  Two further
   ordering effects made it worse: the root sent a join confirmation *after*
   logging the previous epoch, and every routine refresh wrote a full snapshot
   line.  The emulated serial line writes at roughly 50 bytes/s, so at eight
   members the transcript itself starved the receive path.  Finally, the
   module's 2 KiB TCP receive buffer collapsed to a tiny window while the root
   logged; the host forward then trickled the rest of a record through its
   zero-window probe and the 10 s deadline expired on an 18-byte partial frame.

The failures were therefore not a kernel/ROM framing defect, and not a reason
to substitute the policy simulator for real kernels.  They were a harness queue
policy, a guest receive-path ordering bug, and a receive buffer sized for two
nodes.

## Decision

### 1. The hub is a bounded, lossless link switch

`tools/m2-rig/src/hub.rs` keeps one session per peer:

- one reader, one writer and one queue per guest;
- whole records only, with the existing four-byte little-endian prefix;
- a queue bounded by both frames and bytes;
- a reader that **waits** for queue space instead of dropping a complete frame,
  applying TCP backpressure to that source only;
- a writer with a socket write timeout and a total write deadline, so a dead
  guest cannot pin a thread;
- a reader with an idle deadline and a per-record receive deadline, so a
  half-written record is discarded rather than forwarded;
- `add_link` for the paced join order and `remove_link` for a leave;
- a per-session last-frame time, so the runner can reconnect a non-converged
  link that has been silent for 15 s (the iris host forward can keep a healthy
  host TCP connection while its guest-facing window stalls);
- counters for self/unknown/malformed/truncated/backpressure/queue/write
  errors, exposed in the evidence bundle.

The hub still never parses an object ID, computes a home, confirms an epoch or
rewrites a frame.  It remains test harness only, not a production router or a
directory.

### 2. The guest receive path drains before it judges silence

`modules/mcp/src/mesh.rs` now:

- reads and feeds queued socket bytes **before** evaluating the receive,
  heartbeat and send deadlines;
- sends a join reply before writing the bounded serial evidence, so a member's
  confirmation cannot wait behind the previous epoch's transcript;
- writes a compact multi-member transcript: the detailed M2 lines are kept for
  two members, while above two members only compact epoch lines are written and
  the home/shard tables are read from the MCP HTTP surface;
- suppresses per-refresh request/ack/reject and route-fence lines above two
  members, for the same serial-bandwidth reason;
- uses a 120 s heartbeat and receive deadline for an M3 shard run, so a record
  trickling through the host forward's zero-window probe is not torn down by
  the M2 10 s/30 s deadlines.

The kernel's module TCP receive buffer grows from 2 KiB to 16 KiB
(`kernel/src/module.rs`, a BSS array).  This is the only kernel change in the
slice; it is a genuine guest transport fix, not a framing or PROM change.  The
ROM source is untouched, but the embedded kernel image and therefore the PROM
digest change.

### 3. The rig paces joins and scales its window

The shard runner still adds one member at a time and waits for the root's
signed epoch before the next, but the fixed 30/90 s windows become
`120 + 60 * nodes` seconds.  The window is a property of the emulated host, not
of the protocol; a contended eight-kernel run must not be failed by a host CPU
deadline.  The evidence bundle gains per-node epoch/digest/ownership counts, the
route result, the stale and absence markers, and the full hub counters.  The
eight-node port block and the DS1386 environment budget are host tests.

## Consequences

- A real cluster can converge on one signed epoch and one deterministic shard
  table through the bounded hub.  The evidence in the runbook records the exact
  node count and kernel/module digests.
- The hub applies bounded backpressure rather than pretending a full queue is a
  transport failure.  A source guest can still hit its own send deadline if the
  destination is silent for longer than its budget; the hub's counters and the
  guest's disconnect reason distinguish that from a frame drop.
- Above two members the serial transcript is deliberately less detailed.  The
  authoritative tables are the HTTP surfaces, which the rig fetches and checks
  independently.  The M2 two-node transcript is unchanged.
- The 16 KiB receive buffer costs 28 KiB of kernel BSS.  The PROM remains inside
  the fixed window; the exact size is recorded in the runbook.
- The hub is not a product.  The production invariant remains one logical link
  per peer; a real multi-peer transport (or a kernel/module receive path that
  multiplexes member sessions without the host hub) is still an M3 gate item.

## Alternatives considered

| Alternative | Why it lost |
|---|---|
| Keep drop-on-full and rely on join retries | A dropped confirmation is indistinguishable from a dead root; the late member times out.  Backpressure is the transport contract. |
| Use `mesh-sim` for the eight-node claim | The gate asks for replicas of the same core and a real epoch.  A policy simulator cannot prove the guest transport. |
| Fix the guest by logging nothing at all | The M3 evidence still needs a per-node epoch, digest and ownership.  Compact epoch lines plus HTTP keep both. |
| Change the serial baud rate in the kernel | A console change would touch the ROM/kernel path and every M1/M2 transcript; the logging policy is the narrower fix. |
| Keep the 2 KiB receive buffer and raise the deadline | It turns a 10 s failure into a longer one; the window still collapses during a burst. |
| Make the hub a directory or epoch authority | Duplicates placement and membership policy outside the tested core. |
| Claim eight kernels from a host policy run | The recorded limit was real; it is now tested with real kernels, and the exact count is stated. |

## Evidence

The runbook [`../plan/m3-rig.md`](../plan/m3-rig.md) records the commands,
kernel and module digests, the node count, the epoch/digest, the ownership
table, the remote route, the stale/absence checks and the host tests.  The
`run.json` bundle is the machine-readable record; its `hub` object carries the
per-source counters and its `nodes_detail` carries each node's epoch, digest,
owner counts and foreign/stale row counts.

The host gate is:

```sh
cargo test -p mesh-core -p mesh-join -p mesh-sim
cargo test --manifest-path tools/m2-rig/Cargo.toml
```

The hub tests cover routing, duplicate and truncated records, malformed
headers, backpressure with every frame preserved, late joiners, leave,
connect ordering, queue bounds, receive deadlines and clean shutdown.

## What this is not

No GPU placement, no two-node simultaneous failure, no real-kernel membership
churn, no CAS fetch-by-OID, no persistent directory and no full M3 gate.  The
128-node policy remains a host test.  The hub is not a production transport.
