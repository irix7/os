# M3 multi-member runbook — per-link membership and bounded gossip

**Status:** four real kernels converged on one signed epoch and the full shard/route bundle
is recorded for all four.  The live module keeps per-source membership links and bounded
aggregate gossip; the root remains the single 1-of-1 epoch writer.  Eight real kernels are
not claimed.
**Scope:** the multi-member membership slice: per-source link state in the live module,
signed member summaries, bounded gossip fan-out, the host hub, epoch-store provisioning,
root-failure recovery and a real multi-kernel convergence run

The decisions are [M3-ADR-0001](../adr/m3-0001-shard-slice.md) (the first shard surface)
and [M3-ADR-0002](../adr/m3-0002-multi-node-hub.md) (the host hub).  The M2 runbook remains
[`m2-rig.md`](m2-rig.md); its two-node join, homes, route, migration, member failure and
root failure modes are unchanged except for the port-block move and the guest module's
compact multi-member transcript.

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
- the hub counted every record it read, forwarded and refused, and dropped no complete
  frame while its destination session was open (the per-source
  `queue_full`/`queue_closed`/`write_errors` counters are in `run.json`).
- every member's transcript shows the per-link state and at least one forwarded
  `mesh-gossip: aggregate ... forward=1` record; the root's transcript shows one link per
  member.

At the host policy layer:

- 1/2/8/128-node views are deterministic and bounded;
- every node derives its own shard from the view, with no directory lookup;
- adding or removing a node moves only the objects whose rendezvous winner changes;
- duplicate epochs, same-epoch different members and stale epochs fail closed;
- eight join state machines converge through signed summaries, one signed aggregate per
  change set and bounded forwarding.

## Live membership state

The module has one TCP session and one record decoder, because the length prefix is a
property of the session and not of a source.  Everything from the envelope source onwards
is per link:

- a bounded link table (`MAX_MESH_LINKS = 8`, allocated on the module heap) keyed by the
  source `NodeId`;
- one `SendQueue`, `StreamMux`, `PendingTable`, `Heartbeat`, last-seen time, accepted
  summary counter and last-sent aggregate epoch per link;
- one stream send ledger per link, settled from that link's FIFO queue.  A control frame
  that cannot be staged closes the session on the authority link and is dropped on a
  gossip link, where the next summary or resync repairs it;
- a 32 KiB queue on the authority link for bulk and latency, and a 4 KiB queue on each
  gossip link.

A member always knows its root and binds the authority link at connect; a root binds its
first link to the first accepted source.  The route and migration policy is bound to the
authority peer; a member accepts membership gossip from another confirmed member but refuses
route, migration or bulk frames from it.

### Signed summaries and bounded gossip

Every converged member signs one `MemberSummary` per 5 s interval and sends it to the root
plus a rotating window of at most `FANOUT - 1` other members.  The root validates a summary
only when its subject is the link source; a member validates one against the aggregate it
installed, including the granted incarnation.  A summary is never authority and never
advances an epoch.

The root signs at most one `EpochSummary` per changed view and seeds it to a deterministic
window of at most `FANOUT = 3` peers.  Each member verifies the signature, installs the
snapshot and forwards the same signed object once per epoch to at most `FANOUT` peers,
excluding the sender and the root.  A node that has already forwarded an epoch does not
forward it again.  The root re-sends the current signed object to a rotating window every
60 s only when the view has more than two members; no extra signature is produced.  A stale
member's next summary gets the current aggregate as a reply, so a lost push is repaired in
one bounded record rather than by a root broadcast.

At four members the root's window covers every member directly; the members still forward
the aggregate to each other, and the transcripts show `mesh-gossip ... forward=1` on n1, n2
and n3.  At five or more members the root's seed window is smaller than the member count and
the forward path carries the rest.

### Authority and root failure

The authority remains **1-of-1**: one Ed25519 key, one node whose MAC-derived `NodeId` is
`mesh.authority`, and one epoch writer.  This slice does not implement threshold authority.
The key must be provisioned on exactly one node; a second node with the same key is a
provisioning error, not a supported second root.

A member that loses the root keeps its last confirmed view and reports
`mesh-fail: authority state=lost epoch=N ... mutable=0 reads=lease`.  It never advances the
epoch and never creates a second home.  A root restart restores the signed record from the
epoch store before it listens, so it resumes the same epoch.  A root without a valid record
starts at the seed and refuses a live member's higher epoch with `epoch-regression`; the
member keeps its view.  That refusal is the recovery rule that cannot create two roots.

### Epoch store

Every node the rig starts has a dedicated 4 MiB scratch volume at SCSI target 3.  The store
starts at LBA 8 and holds a signed root record or a signed member confirmation; the write is
read back and compared before success.  Target 2 is the module's EROFS fixture and target 1
is the boot disk, so neither is reused.  The M2 and M3 configurations use the same layout;
the host test `m2_and_shard_configs_provision_the_epoch_store` pins it.

## Prerequisites

- The pinned Rust nightly with `rust-src` (`rust-toolchain.toml`).
- An `iris` binary with instance support (`--instance`, `--print-instance`).
- The matching `iris-ci`.
- The repository's target spec and the kernel/ROM/MCP sources.
- No root, no `CAP_NET_RAW` and no pcap feature.

The harness is the standalone `tools/m2-rig` crate.  It is not a workspace member.

## Build

```sh
nix develop                        # pinned nightly-2026-10-03 + rust-src
tools/m2-rig/build.sh              # defaults to target/m2-rig
```

The build writes the PROM, the MCP module and `SHA256SUMS`.  The module must stay inside
the 512 KiB slot.  The kernel's module TCP receive buffer is 16 KiB per slot; the PROM
and module sizes from the recorded run are in the evidence.

## Run

```sh
cargo run --manifest-path tools/m2-rig/Cargo.toml -- shard \
  --iris /path/to/iris --iris-ci /path/to/iris-ci \
  --nodes 4 --hub
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

### Membership markers

The module's per-link and gossip state is visible in the serial transcript:

```text
mesh-link: peer=<NodeId> role=authority|member|gossip links=N
mesh-gossip: aggregate epoch=N members=N targets=N forward=0|1
mesh-join: converged role=member epoch=N members=N digest=<32 hex> compact=1
mesh-join: refreshed epoch=N members=N digest=<32 hex>
```

The root logs one `mesh-link` line for each member after the authority link; a member logs
a `gossip` link when it first addresses another member.  A member that receives an aggregate
logs `mesh-gossip ... forward=1` once for that epoch.  The rig accepts `converged`,
`duplicate`, `refreshed` or `local` as the confirmed line for a member, because a member that
learns a new epoch from a forwarded aggregate does not receive a new full confirmation.

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
- `run --fault none` was re-run on two real kernels for this slice: join, homes before
  and after the member restart, the five route steps with two `MOVED` retries, the
  eleven-step L0 migration both ways, the member restart and the root restart all passed.
- `run --fault kill-root` is the new root-failure gate.  It kills n0 after the route
  steps, requires n1's `authority state=lost` and `mutable=0`, restarts n0 from the
  epoch store and requires the same epoch and digest with no fence and no epoch
  regression.

## Host policy gate

```sh
cargo test -p mesh-core -p mesh-join -p mesh-sim
cargo test --manifest-path tools/m2-rig/Cargo.toml
```

The M3 integration test is `crates/mesh-sim/tests/shard.rs`; the eight-member join and
refresh test is in `crates/mesh-join`.  The hub tests in `tools/m2-rig/src/hub.rs` cover
routing, duplicate and truncated records, malformed headers, backpressure with every
frame preserved, late joiners, leave, connect ordering, queue bounds, receive deadlines
and clean shutdown.  The gossip selector tests in `crates/mesh-join/src/gossip.rs` prove
that a bounded root window plus one forward per recipient covers eight members within
three hops.  The rig tests cover the eight-node port block, the NVRAM budget, the
epoch-store provisioning and the `kill-root` mode table.

## Exact limits

- **Real-kernel convergence: four iris kernels.**  All four reported epoch 4 with digest
  `EF943AF5E45ED3002E636DED56AE2F83`; the full `/mesh/shard`, `/mesh/homes`, stale-epoch,
  absence and remote-route bundle is recorded for all four.  The run had one host-hub
  session recovery and 49 frames dropped while a destination session was closed (the hub
  counts every drop in `run.json`); no complete frame was dropped while its destination
  session was open.
- **Eight real kernels are not achieved.**  The iris host forward's guest-facing TCP
  window stalls under an eight-member burst even though the host TCP connection is
  healthy; the rig reconnects a silent session and the module uses compact M3 transcripts.
  That is a harness workaround, not a production multi-peer transport.
- **Root failure is evidenced on two kernels, not four.**  `run --fault kill-root` kills
  n0, proves n1's safe stop and restarts n0 from SCSI target 3; the four-kernel run has no
  root-failure scenario.
- **The authority is 1-of-1.**  No threshold authority, delegated home certificate or
  second epoch writer is implemented; the live link table is bounded to eight sources
  even though the wire membership allows 128.
- The 128-node shard policy is host-only.
- No GPU path, two-node simultaneous failure, general home migration, real-kernel
  membership churn, CAS fetch-by-OID, persistent directory or full M3 gate.
- The only kernel change is the 16 KiB module TCP receive buffer; the ROM source is
  unchanged, but the embedded kernel image and PROM digest move.
- The hub is harness-only and is not a production multi-peer transport or a directory.
