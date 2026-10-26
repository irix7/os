# M3 first shard slice — runbook

**Status:** working for two real kernels; the 1/2/8/128-node shard policy is host-only
**Scope:** the first M3 slice: bounded shard evidence from the confirmed rendezvous table,
the signed refresh that converges an earlier member after a later join, one remote route
across the shard boundary, and an explicit statement of the eight-node real-kernel limit

The decisions are [M3-ADR-0001](../adr/m3-0001-shard-slice.md).  The M2 runbook remains
[`m2-rig.md`](m2-rig.md); its two-node join, homes, route, migration and failure modes are
unchanged except for the port-block move described below.

## What the slice proves

On two real iris kernels:

- both confirm epoch 2 and the same canonical membership digest;
- both derive the same 32-object M3 fixture table with `mesh_core::home_for`;
- each node serves only its own assignments at `GET /mesh/shard`, with the epoch and
  digest on every row;
- a stale `epoch=` is a `409` with no rows;
- the M2 `GET /mesh/homes` surface remains current;
- the root completes one remote capability stat at the member with one attempt and no
  `MOVED`.

At the host policy layer:

- 1/2/8/128-node views are deterministic and bounded;
- every node derives its own shard from the view, with no directory lookup;
- adding or removing a node moves only the objects whose rendezvous winner changes;
- duplicate epochs, same-epoch different members and stale epochs fail closed;
- eight join state machines converge through the same signed join plus one bounded
  signed refresh.

It does **not** prove eight live kernels converging.  Eight iris instances were launched
and booted, but the experimental host hub exposed a guest smoltcp receive truncation when
several member streams were multiplexed onto the root's single mesh slot.  The real
limit and the next work item are recorded in the ADR.

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
the 512 KiB slot; the slice's module is about 357 KiB.

## Run

```sh
cargo run --manifest-path tools/m2-rig/Cargo.toml -- shard \
  --iris /path/to/iris --iris-ci /path/to/iris-ci \
  --nodes 2
```

The shard command starts the nodes one at a time, waits for `BOOT-READY`, joins them
through the existing M2 mesh relay, waits for the root's epoch-2 confirmation and both
members' convergence, fetches `/mesh/shard`, `/mesh/homes` and the stale-epoch negative
from both nodes, drives route step 1, and writes:

| Artefact | Contents |
|---|---|
| `evidence/shard/run.json` | Nodes, ports, epoch, digest, shard headers, route reports, checks, topology and the real-kernel limit. |
| `evidence/shard/summary.md` | Human-readable checks and limits. |
| `evidence/shard/serial-n0.log`, `serial-n1.log` | Full serial transcripts. |

`--keep` leaves the nodes running after the run; `--work DIR` changes the work directory.

### Port block

The M2 two-node layout used to put the first host forward at `base + 600`.  Eight nodes
would then collide with the monitor block (`base + 600` is node 6).  The forward block now
starts at `base + 1000`:

| Node | Monitor | Telnet | MCP HTTP | Mesh |
|---|---|---|---|---|
| `n0` | `base` | `base+1000` | `base+1001` | `base+1002` |
| `n1` | `base+100` | `base+1010` | `base+1011` | `base+1012` |
| `nN` | `base+100N` | `base+1000+10N` | `base+1001+10N` | `base+1002+10N` |

The default `--base-port 9100` therefore puts n0's MCP HTTP forward at 10101.

### Experimental hub

`--nodes 3..8` requires `--hub`.  It is a host link switch: it opens one connection to
each guest's existing mesh forward and forwards whole `MeshEnvelope` records by
`MeshHeader.dst`.  It never parses an object ID, computes a home, confirms an epoch or
rewrites a frame.  The hub is test harness only.  It is **not** part of the eight-node
gate in this slice; see the ADR for the transport limit that blocks it.

## NVRAM keys

| Key | Meaning |
|---|---|
| `mesh.peer` | Presence enables the mesh listener.  Under the hub the guests do not dial it; the rig connects to each forward. |
| `mesh.authority` | The root's MAC-derived `NodeId`. |
| `mesh.epoch` | The seed epoch (1). |
| `mesh.pub` | The root's Ed25519 public key (members). |
| `mesh.key` | The root's 32-byte Ed25519 seed (root only). |
| `mesh.shard` | M3 shard object count; absent keeps the M2 surface unchanged. |
| `mesh.route` | Enables the M2 route policy on the root and the chosen member. |

The full eight-node member environment is still below the DS1386 298-byte limit before
the MAC slot; the route key is written only to the two nodes that use it, and the shard
key is 12–15 bytes.

## Host policy gate

```sh
cargo test -p mesh-core -p mesh-join -p mesh-sim
```

The M3 integration test is `crates/mesh-sim/tests/shard.rs`; the eight-member join and
refresh test is in `crates/mesh-join`.

## Exact limits

- Real-kernel convergence: **2 nodes**.
- Eight-node and 128-node shard policy: **host tests only**.
- No GPU path, two-node simultaneous failure, general home migration, real-kernel churn,
  CAS fetch-by-OID, persistent directory or full M3 gate.
- The module decoder-drain fix is module-local; no kernel or ROM change is included.
- The guest-to-guest HTTP probe fails on the unmodified M2 baseline in this workspace.
  It is recorded as an environment observation, not a shard or M2 regression; the
  shard's host-forward HTTP fetches and the M2 join/homes/route/migration markers pass.
