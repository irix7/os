# M3 multi-member runbook — bounded hub and real kernels

**Status:** four real kernels have converged on one epoch; the full shard/route bundle is
recorded for three kernels because the fourth member's HTTP slot stalled.  The 128-node
shard policy remains host-only and eight real kernels are not claimed.
**Scope:** the multi-member transport slice: bounded per-peer hub sessions, the guest
receive-path ordering fixes, a real multi-kernel convergence run, one remote route across
the shard boundary, stale-epoch refusal and absence evidence

The decisions are [M3-ADR-0001](../adr/m3-0001-shard-slice.md) (the first shard surface)
and [M3-ADR-0002](../adr/m3-0002-multi-node-hub.md) (this slice).  The M2 runbook remains
[`m2-rig.md`](m2-rig.md); its two-node join, homes, route, migration and failure modes are
unchanged except for the port-block move and the guest module's compact multi-member
transcript.

## What this slice proves

On a real iris cluster (the exact count is recorded in the evidence bundle and the
summary):

- every kernel confirms the same signed epoch and canonical membership digest;
- every kernel derives the same 32-object M3 fixture table with `mesh_core::home_for`;
- each node serves only its own assignments at `GET /mesh/shard`, with the epoch and
  digest on every row;
- the union of the per-node owned rows is the whole fixture set, and every node reports
  zero foreign rows;
- a stale `epoch=` is a `409` with no rows on every node;
- the M2 `GET /mesh/homes` surface remains current;
- the root completes one remote capability stat at a member that owns the object, with
  one attempt and no `MOVED`;
- the hub counted every record it read, forwarded and refused, and did not silently drop
  a complete frame (the per-source `queue_full`/`queue_closed`/`write_errors` counters are
  in `run.json`).

At the host policy layer:

- 1/2/8/128-node views are deterministic and bounded;
- every node derives its own shard from the view, with no directory lookup;
- adding or removing a node moves only the objects whose rendezvous winner changes;
- duplicate epochs, same-epoch different members and stale epochs fail closed;
- eight join state machines converge through the same signed join plus one bounded
  signed refresh.

## Prerequisites

- The pinned Rust nightly with `rust-src` (`rust-toolchain.toml`).
- An `iris` binary with instance support (`--instance`, `--print-instance`).
- The matching `iris-ci`.
- The repository's target spec and the kernel/ROM/MCP sources.
- No root, no `CAP_NET_RAW` and no pcap feature.

The harness is the standalone `tools/m2-rig` crate.  It is not a workspace member.

## Build

```sh
source /path/to/toolchain/env.sh   # nightly-2026-10-03 + rust-src
tools/m2-rig/build.sh              # defaults to target/m2-rig
```

The build writes the PROM, the MCP module and `SHA256SUMS`.  The module must stay inside
the 512 KiB slot.  The kernel's module TCP receive buffer is 16 KiB per slot; the PROM
and module sizes from the recorded run are in the evidence.

## Run

```sh
cargo run --manifest-path tools/m2-rig/Cargo.toml -- shard \
  --iris /path/to/iris --iris-ci /path/to/iris-ci \
  --nodes 8 --hub
```

`--nodes 2` uses the proven direct M2 relay.  `--nodes 3..8` requires `--hub`; the runner
adds one member at a time, waits for the root's signed epoch before the next, waits for
every node to converge, then fetches `/mesh/shard`, `/mesh/homes` and the stale-epoch
negative from every node, drives one route step, and writes:

| Artefact | Contents |
|---|---|
| `evidence/shard/run.json` | Nodes, ports, epoch, digest, per-node ownership and owner counts, route report, stale and absence markers, topology, hub counters and the real-kernel limit. |
| `evidence/shard/summary.md` | Human-readable checks, ownership table and limits. |
| `evidence/shard/serial-n0.log`, … | Full serial transcripts. |

`--keep` leaves the nodes running after the run; `--work DIR` changes the work directory.

### Convergence window

The guest writes its serial evidence from the same cooperative loop that reads the mesh
socket.  The emulated serial line writes at roughly 50 bytes/s; a fixed 30 s host deadline
is a property of the host CPU, not of the protocol.  The runner uses
`120 + 60 * nodes` seconds for each epoch/convergence wait.  Above two members the module
writes compact epoch lines only; the authoritative tables are the HTTP surfaces.

### Port block

| Node | Monitor | Telnet | MCP HTTP | Mesh |
|---|---|---|---|---|
| `n0` | `base` | `base+1000` | `base+1001` | `base+1002` |
| `n1` | `base+100` | `base+1010` | `base+1011` | `base+1012` |
| `nN` | `base+100N` | `base+1000+10N` | `base+1001+10N` | `base+1002+10N` |

The default `--base-port 9100` puts n0's MCP HTTP forward at 10101.  The monitor block for
eight nodes ends at `base+700`; the forward block starts at `base+1000`, so the ranges
cannot overlap.  Host tests assert uniqueness for all eight nodes.

### Bounded hub

`--hub` is a host link switch.  It opens one connection to each guest's existing mesh
forward and forwards whole `MeshEnvelope` records by `MeshHeader.dst`.  It never parses an
object ID, computes a home, confirms an epoch or rewrites a frame.  `dst == 0` means the
authority root for the M3 join and probe traffic.

Each peer has one reader, one writer and one queue bounded by frames and bytes.  A reader
that finds a full destination queue **waits**; it does not drop the record.  The wait
closes the source guest's TCP send window, which is the transport's backpressure.  A
writer has a socket write timeout and a total write deadline.  A reader has an idle
deadline and a per-record receive deadline, so a half-written record is discarded rather
than forwarded.  `add_link` is the paced join path; `remove_link` closes a session for a
leave test.

Each session also records the host time of its last complete frame.  While a node has not
reached the expected epoch, the runner reconnects a session that has been silent for 15 s,
and reconnects immediately after the guest logs `mesh-transport: disconnected`.  This is
host-side link recovery, not a membership or failure policy: the guest keeps its confirmed
join view, the signed epoch is untouched, and the other sessions are not disturbed.  The
evidence records the `link_recoveries` count.  The M3 shard run uses a 120 s heartbeat and
receive deadline because the iris host forward can hold a healthy host TCP connection
while its guest-facing window stalls; the M2 two-node deadlines are unchanged.

The hub is test harness only.  It is not linked into a kernel or module, is not a
production router and is not an object directory.

## NVRAM keys

| Key | Meaning |
|---|---|
| `mesh.peer` | Presence enables the mesh listener.  Under the hub the guests do not dial it; the rig connects to each forward. |
| `mesh.authority` | The root's MAC-derived `NodeId`. |
| `mesh.epoch` | The seed epoch (1). |
| `mesh.pub` | The root's Ed25519 public key (members). |
| `mesh.key` | The root's Ed25519 seed (root only). |
| `mesh.shard` | M3 shard object count; absent keeps the M2 surface unchanged. |
| `mesh.route` | Enables the M2 route policy on the root and the chosen member. |

The full eight-node member environment fits below the DS1386 298-byte limit before the
MAC slot; the host test `eight_node_shard_environment_fits_the_ds1386_image` asserts it.
The route key is written only to the two nodes that use it.

## Preserved surfaces

- `shard --nodes 2` uses the proven direct relay (no hub) and passes the first
  M3 shard surface: epoch 2, digest `2EDA13C5AC6F726132E0479DFE6045B7`, owners
  14 / 18, one remote stat at `n1`, stale `409` with no rows.
- `run --fault no-peer` passes the M1 markers (`BOOT-READY ... mesh=single`,
  `/mesh/homes` disabled, route/migration disabled with `reason=peer`).
- The full M2 default run was not re-run while four other iris instances shared
  the host; its code path is unchanged except the join reply ordering, and the
  two-node shard exercises the same join/route surfaces.

## Host policy gate

```sh
cargo test -p mesh-core -p mesh-join -p mesh-sim
cargo test --manifest-path tools/m2-rig/Cargo.toml
```

The M3 integration test is `crates/mesh-sim/tests/shard.rs`; the eight-member join and
refresh test is in `crates/mesh-join`.  The hub tests in `tools/m2-rig/src/hub.rs` cover
routing, duplicate and truncated records, malformed headers, backpressure with every
frame preserved, late joiners, leave, connect ordering, queue bounds, receive deadlines
and clean shutdown.  The rig tests cover the eight-node port block and NVRAM budget.

## Exact limits

- **Real-kernel convergence: four kernels on one epoch** (serial transcripts); the
  **full ownership/route bundle is three kernels** because the iris host forward stalled a
  member's inbound HTTP slot after convergence.  The rig now falls back to the member's
  serial epoch/digest and the root's verified owner row (`source=root-shard+serial`) for
  that case.
- **Eight real kernels are not achieved.**  The iris host forward's guest-facing TCP
  window stalls under the eight-member burst even though the host TCP connection is
  healthy; the rig reconnects a silent session and the module uses compact M3 transcripts,
  but this is a harness workaround, not a production multi-peer transport.
- The 128-node shard policy is host-only.
- No GPU path, two-node simultaneous failure, general home migration, real-kernel
  membership churn, CAS fetch-by-OID, persistent directory or full M3 gate.
- The only kernel change is the 16 KiB module TCP receive buffer; the ROM source is
  unchanged, but the embedded kernel image and PROM digest move.
- The hub is harness-only and is not a production multi-peer transport.
