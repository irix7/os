# M3-ADR-0001 — First sharded home slice, with a two-kernel real limit

**Status:** accepted for the first M3 slice, limited
**Date:** 2026-10-27
**Deciders:** M3 shard slice, for review at the M3 gate
**Related:** [M2-ADR-0003](m2-0003-authority.md), [M2-ADR-0005](m2-0005-join-epoch.md),
[M2-ADR-0006](m2-0006-homes.md), [M2-ADR-0007](m2-0007-route.md),
[`../spec/system.md`](../spec/system.md) §5 and §9,
[`../plan/milestones.md`](../plan/milestones.md) Phase 5,
[`../plan/m3-rig.md`](../plan/m3-rig.md)

## Context

M2 proved join, a signed membership epoch, rendezvous homes, a remote capability
route and L0 migration on two real kernels.  M3 must scale the same system to
eight nodes without adding a central directory or a second placement model.

The starting pieces already exist:

- `mesh-core::home_for` is the production rendezvous function.  It is the only
  object-to-home rule in the tree.
- `mesh-join` owns the 1-of-1 authority, the signed `EpochConfirmation` and the
  member's install path.
- `mesh-service` owns the membership table, epochs, liveness and removal.
- `mesh-sim` already exercises the 1/2/8/128-node shape at the policy layer.
- The MCP module has one mesh TCP slot.  The guest has no TCP connect
  primitive, so the M2 rig gives it one listener and a host relay.

What was missing was a bounded shard evidence surface, an eight-node membership
story that does not add a push protocol, and an honest answer to whether eight
live kernels can join over the emulated TCP path.

## Decision

### 1. One placement model, one fixture

The M3 evidence set is a **bounded fixture**, not a directory:

```text
object[i] = BLAKE3-256("IRIX7-M3-SHARD-OBJECT-v1" || i u32 LE)
```

`mesh.shard=<count>` enables the surface; the count is bounded to 1..256 and
defaults to 64 when the key is present but malformed.  The rig configures 32
objects for the two-kernel evidence so the serial transcript stays small.

`HomeTable::new` still calls `mesh_core::home_for` for every object.  The new
methods are views over that table, not assignments:

- `owned_by(node)` — the entries whose home is this node;
- `owned_count(node)` — their count;
- `owner_counts()` — per-owner counts;
- `balance()` — object and owner counts plus the smallest and largest
  non-empty shard.

A node computes its own shard by filtering the same table every node computes
from the confirmed snapshot.  It never asks the owner or a directory.

### 2. `/mesh/shard` is the shard evidence surface

The MCP module serves a bounded `text/plain` body:

```text
mesh-shard: state=current object_set=m3-v1 role=root node=<hex> epoch=8 \
  members=8 digest=<32 hex> count=64 owners=8 owned=9 min=6 max=11
mesh-shard-owner: node=<hex> count=<n>
mesh-shard-owned: object=<64 hex> home=<hex> epoch=8 digest=<32 hex>
```

Every owned row repeats the epoch and membership digest.  The header carries the
node identity, role, epoch, digest, total count, owner count, this node's owned
count and the balance bounds.  The owner rows let a reviewer see the split
without a lookup.

Status codes match `/mesh/homes`:

| Condition | Status | Body |
|---|---|---|
| Current table, no query | `200` | `state=current` plus owner and owned rows |
| Current table, `epoch=` matches | `200` | `state=current` |
| Another epoch | `409` | `state=stale requested_epoch=N epoch=M ... count=0 owned=0`, no rows |
| No confirmed view | `409` | `state=pending ... count=0 owned=0` |
| `mesh.shard` absent | `404` | `state=disabled reason=config` |
| Mesh disabled | `404` | `state=disabled reason=peer` |
| Join disabled | `404` | `state=disabled reason=join` |
| Malformed query | `400` | `state=bad-request reason=query` |

The M2 `/mesh/homes` body is unchanged.  The M3 endpoint is opt-in so the M1/M2
evidence and the 8-row M2 response budget are not silently relabelled.

### 3. Eight-node membership keeps the single epoch writer

The root remains the only writer of the membership epoch.  A second member
joining advances the epoch and carries the full sorted member list, exactly as
M2-ADR-0005 specifies.

M2 stopped after the confirming member.  An earlier member never heard that the
epoch had advanced.  M3 adds **one bounded signed refresh**:

```text
member (already converged): every REFRESH_INTERVAL_MS
  -> JoinRequest(last_epoch, last_digest, new nonce/incarnation)
root: confirm its current epoch and full list (never advance for an existing member)
member: verify, install, ack
```

There is no push, no gossip, no second authority horizon and no central
directory.  The refresh is the same request/confirmation/ack message; it does
not add a wire kind.  A member that has lost the authority does not refresh on
its own: the existing `request_rejoin` path runs when a root frame arrives.

A root that advanced the epoch before an ack arrived now treats the ack as
`ack-stale`, not `ack-mismatch`.  The ack was correct when issued; the member's
next refresh confirms the current view.  A same-epoch different digest remains
`ack-mismatch`.

The module accepts multiple source `NodeId`s on the root's one link.  A member
still accepts exactly one source (the root).  This is what lets a host link
switch multiplex the star without changing the guest's single TCP slot.  The
first source remains the route/migration peer.

### 4. Real-kernel limit: two joined kernels

The M2 relay is the proven two-kernel seam.  The M3 rig keeps it for
`--nodes 2` and produces the real-kernel slice:

- both kernels confirm epoch 2 and the same digest;
- both serve `/mesh/shard` with the same 32-object table and only their own
  rows;
- both refuse a stale `epoch=` with `409` and no rows;
- both keep the M2 `/mesh/homes` surface current;
- the root completes one remote stat at the member with one attempt, no
  `MOVED`.

Eight iris kernels were launched and booted during the slice.  Joining them
through the experimental host hub (`--hub`) exposed a guest transport limit: the
emulated smoltcp receive path delivered a partial `MeshEnvelope` at the root
when several member streams were multiplexed onto its single slot, and the 10 s
receive deadline then closed a link that had not actually failed.  The exact
limit is therefore:

- **real-kernel convergence: 2 nodes;**
- **eight-node shard policy: host tests only (`mesh-sim`, `mesh-join`);**
- **128-node policy: host tests only.**

The host hub remains in the rig behind `--hub` as an explicitly experimental
link switch.  It is not a kernel or module component, not a production router
and not an object directory.  It does not make the eight-node gate pass.

### 5. Module defect found and fixed

The slice found one defect in the MCP module, not in the kernel or ROM:
**buffered records were stranded.** `Mesh::feed` processed at most one complete
record per socket read.  A burst of join requests left the rest in the
`StreamDecoder` with no further `tcp_recv` bytes to trigger them, until the
receive deadline closed the link.  `feed` now drains every complete record
before pushing more bytes, and `connected` drains the decoder each tick.

An HTTP `send_all` retry was also trialled and reverted: the guest-to-guest HTTP
probe in this workspace fails on the unmodified M2 baseline as well, so the
slice does not claim it as a fix or a regression.  The decoder change does not
alter a wire format, a kernel entry point or the PROM.

### 6. What this slice is not

No GPU placement, no two-node simultaneous failure, no general home migration,
no membership churn on real kernels, no CAS fetch-by-OID, no persistent
directory and no full M3 gate.  Membership churn, duplicate/stale epochs and
1/2/8/128 balance are host policy tests.  They are not claimed as real-kernel
churn.

## Consequences

- The shard view reuses the existing rendezvous function and the existing
  confirmed snapshot.  A node's ownership is reproducible from the membership
  alone.
- The eight-node policy is testable without a fragile live topology, while the
  real-kernel evidence is limited to what the current guest TCP path can carry.
- The decoder drain improves every M2 and M3 run; it does not enlarge the wire
  protocol.
- The eight-node gate still needs a real transport that can carry several
  member streams, or a kernel/module receive path that does not truncate a
  `MeshEnvelope`.  That is the explicit next M3 work item.

## Alternatives considered

| Alternative | Why it lost |
|---|---|
| A second hash or an explicit per-node object list | It is the second placement model the gate forbids. |
| A central shard directory | Contradicts the no-central-directory invariant and duplicates `home_for`. |
| Push every epoch to every member | Adds an unsolicited protocol and a second way to learn membership; the signed refresh already converges and stays bounded. |
| Let a member advance its own epoch | Creates two authoritative views after a partition. |
| Claim eight live kernels from a host-hub run | The emulated receive path truncated frames; the run is not evidence of convergence. |
| Fix the guest TCP stack inside this slice | It is a kernel/transport defect and outside the no-kernel-change rule; it is reported as the next gate item. |
| Make `/mesh/shard` always on | Changes the M1/M2 response budget and evidence; the opt-in key keeps those markers intact. |

## Evidence

Host policy:

- `cargo test -p mesh-core -p mesh-join -p mesh-sim` — 19 mesh-core, 35
  mesh-join and 5 `mesh-sim/tests/shard.rs` tests.
- One owner per object for 1/2/8/128 nodes; every 8-node member owns a shard;
  the 128-node table remains bounded; adding/removing a node moves only its
  objects; stale and duplicate epochs fail closed; a node derives its own shard
  without a lookup.
- The `mesh-join` eight-member test drives seven joins and one signed refresh
  per member; the root epoch ends at 8 and every member installs the same
  snapshot.

Real kernels (`tools/m2-rig shard --nodes 2`):

```text
membership: 2 kernels converged on epoch 2 digest \
  2EDA13C5AC6F726132E0479DFE6045B7 (2 members)
n0: role=root owned=14 min=14 max=18
n1: role=member owned=18 min=14 max=18
stale: every node refused an older epoch with no rows
M2: /mesh/homes remains current on every node
route: n0 -> n1 object=BEB6D1C7... home_epoch=2 attempts=1 moved=0
```

The evidence bundle is `evidence/shard/run.json`, `summary.md` and
`serial-n0.log`/`serial-n1.log`.

M1/M2 preservation: `m2-rig run --fault no-peer` on this workspace reaches the M1
`mesh=single` marker and passes the join, homes, route and migration disabled checks.  Its
guest-to-guest HTTP probe fails on the unmodified M2 baseline as well; that is recorded as
an environment observation, not a shard regression.
