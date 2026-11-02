# M3 membership slice: per-link state, gossip and root failure

**Status:** implemented and evidenced on real kernels
**Date:** 2026-11-03
**Issues:** [irix7/os#249](https://github.com/irix7/os/issues/249),
[#264](https://github.com/irix7/os/issues/264),
[#3](https://github.com/irix7/os/issues/3),
[#330](https://github.com/irix7/os/issues/330)
**Branch:** `audit-membership`

## What changed

The live MCP module no longer keeps one anonymous peer list over one shared send path.
`modules/mcp/src/mesh.rs` now has a bounded per-source link table:

| Per link | Authority link | Gossip link |
|---|---|---|
| Source `NodeId` | yes | yes |
| `SendQueue` | 32 KiB | 4 KiB |
| `PendingTable` | 8 tags | 2 tags |
| `Heartbeat` | yes | yes |
| Last receive time | yes | yes |
| Accepted summary counter | yes | yes |
| Last aggregate epoch sent | yes | yes |

The record decoder stays single: the 32-bit length prefix belongs to the one TCP session
and has no source field.  Everything after `MeshHeader.src` is per link.  The table lives
on the module heap and is bounded to `MAX_MESH_LINKS = 8`.

`crates/mesh-join` adds a shared deterministic target selector (`gossip_targets`) and
member-side validation of a peer `MemberSummary` against the installed aggregate's grant.
The root's summary handler now:

- refuses a body whose subject is not the link source;
- refreshes liveness only for a direct subject;
- answers a stale epoch/digest with the current signed aggregate, so a missed push is
  repaired in one bounded record.

The root signs at most one aggregate per changed view.  `publish_aggregate` returns the
cached object when the epoch and digest have not changed, so the 5 s member refresh no
longer costs one Ed25519 signature per member.  Aggregates are seeded to at most
`FANOUT = 3` peers; every recipient verifies, installs and forwards once per epoch to at
most three peers, excluding the sender and the root.  A root with more than two members
re-sends the same signed object to a rotating window every 60 s; no extra signature is
produced.

## Authority

The authority is **1-of-1**: one Ed25519 key, one `mesh.authority` node, one epoch writer.
This slice does not implement threshold authority.  The key is installed on exactly one
node; a second node with the same key is a provisioning error, not a failover path.

A member that loses the root keeps its last confirmed view, logs
`mesh-fail: authority state=lost epoch=N ... mutable=0 reads=lease`, and never advances the
epoch.  The root restart path restores the signed record from SCSI target 3 before it
listens and resumes the same epoch.  A root without a valid record starts at the seed and
refuses a live member's higher epoch with `epoch-regression`; the member keeps its view.
That refusal is the recovery rule that cannot create two roots.

## Epoch store provisioning

Every node configuration written by `tools/m2-rig` now includes a dedicated 4 MiB scratch
volume:

```toml
[scsi.3]
path = "<work>/nN/scratch.raw"
scratch = true
size_mb = 4
```

The store starts at LBA 8 and holds a signed `EpochRecord` (root) or a signed
`EpochConfirmation` (member).  The write is read back and compared before success.  Target
2 is the module's EROFS fixture and target 1 is the boot disk, so neither is reused.  The
M2 and M3 runs use the same layout; `m2_and_shard_configs_provision_the_epoch_store` pins
it.

## Real-kernel evidence

The rig ran the current module and PROM.  The kernel has no change in this slice beyond the
existing 16 KiB module TCP receive buffer.

| Mode | Real kernels | Result |
|---|---|---|
| `run --fault none` | 2 | pass: join epoch 2, homes before/after the member restart, five route steps with two `MOVED` retries, eleven-step L0 migration both ways, member restart, root restart |
| `run --fault kill-root` | 2 | pass: n0 killed, n1 `authority state=lost`/`mutable=0`, n1 kept epoch 2 and two members, no fence/commit, n0 restored epoch 2 from `source=root-record`, n1 re-presented epoch 2, both `authority=current`/`mutable=1`, no `epoch-regression` |
| `shard --nodes 4 --hub` | 4 | pass: all four kernels at epoch 4, digest `EF943AF5E45ED3002E636DED56AE2F83`; full `/mesh/shard`, `/mesh/homes`, stale, absence and remote-route bundle for all four |

The four-kernel transcript shows the live gossip path:

```text
n0: mesh-link: peer=... role=member links=2
n0: mesh-link: peer=... role=member links=3
n0: mesh-gossip: aggregate epoch=4 members=4 targets=3 forward=0
n1: mesh-gossip: aggregate epoch=4 members=4 targets=2 forward=1
n2: mesh-link: peer=... role=gossip links=2
n2: mesh-gossip: aggregate epoch=4 members=4 targets=2 forward=1
n3: mesh-gossip: aggregate epoch=4 members=4 targets=2 forward=1
```

The root's window covers all three members at four nodes; the members still forward the
aggregate to each other, so the forward path is exercised.  The hub recorded one session
recovery and 49 dropped frames while a destination session was closed; no complete frame
was dropped while its destination session was open.

## Limits and remaining M3 work

- **Four real kernels, not eight.**  The eight-kernel run is still limited by the iris host
  forward's guest-facing TCP window under the boot burst.  The rig's `--nodes 8 --hub` path
  exists but is not claimed here.
- **One TCP session, one decoder.**  Per-source state begins at the envelope source.  A
  true per-link decoder needs one TCP connection per peer, which the two-slot module
  transport does not provide; the hub is still a link switch.
- **Single root, 1-of-1.**  No threshold authority, delegation or second epoch writer.
  Root failure is a safe stop plus store-backed restart, not an election.
- **Root failure on two kernels only.**  The four-kernel run does not kill and restart the
  root.
- **The hub is harness-only.**  It is not a production router, and it is still a central
  host process in the real-kernel rig, even though it holds no membership or epoch state.
- **No four-node membership churn.**  The four-kernel run joins once and does not remove or
  re-add a member.
- **No stream-scheduler work.**  This slice touches membership, epoch and root-failure code
  only; the fixed stream labels and the single bulk window are unchanged.
