# M2-ADR-0008 — Failure detection, lease fencing and the partition rule

**Status:** accepted for M2, limited
**Date:** 2026-10-27
**Deciders:** M2-6, for review at the M2 gate
**Related:** [M2-ADR-0003](m2-0003-authority.md), [M2-ADR-0005](m2-0005-join-epoch.md),
[M2-ADR-0006](m2-0006-homes.md), [M2-ADR-0007](m2-0007-route.md),
[`../spec/system.md`](../spec/system.md) §5 and §8,
[`../plan/milestones.md`](../plan/milestones.md) §2, [`../plan/m2-rig.md`](../plan/m2-rig.md)

## Context

M2-2 made two kernels agree on a signed membership epoch. M2-3 derived rendezvous homes
from that snapshot, and M2-4 routed a capability to a home with a bounded `MOVED` retry
after a root-signed lab lease move. All three assume the other node is still there.

The M2 gate also requires **one-node failure and partition**. The pieces that already
exist are:

- `mesh-service` owns the one membership state machine: `Alive`/`Suspect`/`Failed`,
  incarnation heartbeats, a confirmed-view install path and a snapshot that excludes
  failed nodes. `mesh-join` already owns the confirmed table and the 1-of-1 authority.
- `mesh-core` owns rendezvous homes, the bounded `RouteCache`, `ObjectRoute` with
  `home_epoch`, and `Lease` with an epoch fence.
- `mesh-route` owns the `HomeLease`, `DRAINING`/`MOVED`, capability verification and the
  bounded `I7R1` bodies.
- The M2-1 transport already exchanges `Control/Ping` every 5 s on an idle link.
- The kernel has no process model, no pre-emption and no persisted epoch store. The module
  cannot write NVRAM.

What was missing was a bounded failure transition, a fence for the dead node's homes, a
partition rule that cannot produce two authoritative homes, and real-kernel evidence for
both.

## Decision

### 1. One failure model, driven by the membership heartbeat path

The failure detector is a **stateless policy over `MembershipTable`**
(`mesh_service::FailureDetector`). It adds no second directory, liveness store or
membership list:

```text
last accepted frame at T
  T + 6 s   (SUSPECT_AFTER_MS)  -> Suspect   (still in the routing snapshot)
  T + 12 s  (FAIL_AFTER_MS)     -> Failed    (excluded from the snapshot)
```

Boundaries are inclusive and the clock is injected, so a host test drives every boundary
without sleeping. Any well-formed frame from a confirmed peer is proof of life; the M2-1
ping, a join message and a route frame all count. A heartbeat with a **stale incarnation**
is ignored, so a replay cannot refresh a dead record. A **failed node is not revived by a
bare heartbeat**: re-admission is a membership change and needs a fresh signed join.
Duplicate failures are idempotent.

### 2. The root removes; the member does not

A confirmed failure is a membership change, and only the 1-of-1 root may write the epoch:

| Observer | Action |
|---|---|
| Root sees a member failed | Removes it with `MembershipTable::leave` (epoch + 1), installs the remaining full member list, and logs `mesh-fail: removed`. |
| Member sees the root failed | Logs `mesh-fail: authority state=lost`, keeps the last confirmed view and epoch, and does **not** remove the root or advance the epoch. |

A suspect member stays routable: the routing snapshot and the home table do not change
until failure is confirmed. A removing epoch moves only the objects that the failed node
homed; the rendezvous property is host-tested.

The removed member may rejoin later. That is a fresh signed confirmation at a new epoch
(`removed` + 1 for the re-add). The rig demonstrates the full sequence
`2 (join) -> 3 (removal) -> 4 (rejoin)`. There is no gossip and no epoch push: a member
learns the new epoch only through the join it already initiates on link-up.

### 3. A membership change fences the route cache and home leases locally

When `RouteNode::on_snapshot` installs a higher epoch it:

1. emits one `Fence` for every cached route whose home is not in the new view, naming the
   stale home and epoch and the recomputed home and epoch;
2. drops those entries and rebuilds the cache from `mesh_core::home_for`;
3. drops every home lease whose object is no longer homed here, with `lease-left-view`
   fence evidence;
4. re-installs `Active` leases at the new epoch for objects homed here, including an
   object that the old view had left `DRAINING` (a confirmed view supersedes a lab lease
   move).

The fence is **local**. The survivor does not send a query to the removed node, does not
consult a directory and does not wait for a quorum. A route to a node that is not in the
confirmed view is never used: it is invalidated and re-resolved from the view.

A fence row names two epoch namespaces explicitly: `stale_home_epoch` is the old home's
assignment epoch (from a rendezvous install or a lab lease move) and `home_epoch` is the
new confirmed membership epoch used to recompute the home. The two may coincide
numerically when a lab move and a membership change happen in the same run; the check is
that the home changed and the epoch is the current one, not that one number is larger than
the other.

A delayed duplicate from the old epoch is refused by the receive path before any lease
lookup: the epoch check comes before the request-id replay check, so the answer is
`stale-epoch` (a fence), not a generic replay. A node that is not the home and holds no
lease answers `not-home`; it never synthesises a redirect. `MOVED` still comes only from a
home that holds the old lease.

### 4. Two leases: read availability and write freshness

Every home holds a **read lease** (`HOME_LEASE_MS`, 120 s) and, for mutable service, an
**authority write lease** (`WRITE_LEASE_MS`, 6 s):

- The read lease follows the M2-4 model. It is installed from the confirmed view. The
  authority root renews its own read leases from its confirmed view; a member renews the
  leases it already holds when it accepts a frame from the authority. A member cannot
  create a lease or move a home. After the authority is lost no frame arrives, so its read
  availability is bounded by the lease and then the object is `not-current`.
- The write lease is renewed by an accepted frame from the authority (the same heartbeat
  path) and by a confirmation. At the root it is renewed by the root itself. A mutable
  commit needs `authority_current && now <= write_until`.

The write lease is deliberately **shorter than the failure window**, and the module
carries a compile-time assertion:

```text
heartbeat (5 s) + write lease (6 s) = 11 s < failure window (12 s)
```

After the link is cut, the old home can accept a commit for at most 6 s after the last
frame it received. The worst case gives the member a 5 s later last frame than the root;
even then its commit window closes at `root_last + 11 s`, and the root cannot re-home the
object until `root_last + 12 s`. **The old home stops committing before the new home can
exist.** This is the handover fence the M2 gate asks for. It is an asynchronous bound, not
a global transaction: see the limits below.

### 5. The availability rule

| State | Read (`OP_STAT`) | Mutable commit (`OP_COMMIT`) |
|---|---|---|
| Current view, current read lease | Available at the home. | Available if the authority write lease is current. |
| Current view, read lease expired | `not-current` (unavailable here). | `not-current`. |
| Isolated member, read lease valid, write lease expired | Available at the home from the unexpired read lease. | **`no-authority`**, immediately. |
| Isolated member, read lease expired | `not-current`. | `no-authority`. |
| Root after removal, new view and leases | Available at the survivor. | Available at the survivor, **without a quorum**. |
| Non-home, no lease | `not-home`. | `not-home`. |

Reads remain available on the isolated side only because the existing `HomeLease` model
explicitly supports serving a client's capability under an unexpired lease; nothing in
this slice adds a second read path. The member does not issue capabilities to itself, so
the two-node rig records the isolated read rule as lease state (`reads=lease`,
`write_lease_ms=0`) plus host tests; it does not drive a third-party read across the
partition. The mutable path fails closed **before** the read lease expires.

### 6. The partition rule: no second authoritative home

A partition is not a membership change on the side without the authority.

- The member keeps its last confirmed `MembershipView` and epoch. It never increments the
  epoch, never removes the root and never re-homes an object.
- The root may remove the unreachable member after the failure window and continue with
  the remaining confirmed members. In the two-node case that is the one-member view; it is
  a new epoch signed by the same authority, not a second authority.
- The old member's homes are fenced by the write-lease expiry and then the read-lease
  expiry. Its epoch is lower than the root's new epoch; any frame or capability that names
  the old epoch is refused with `stale-epoch`.
- Recovery is a normal signed join on a new epoch. The member installs the new view, gets
  fresh leases and commits again. The rig requires the rejoin at epoch 4 with the same
  two-member table on both kernels.

A two-node partition cannot distinguish "the peer is dead" from "the peer is alive but
unreachable". The root's removal is therefore a decision under uncertainty. The design
handles it with the lease margin, not with an assumption.

### 7. Evidence surface and rig control

The module adds bounded evidence to the existing control plane:

```text
mesh-fail: suspect node=<id> epoch=E
mesh-fail: failed node=<id> epoch=E
mesh-fail: recovered node=<id> epoch=E
mesh-fail: removed node=<id> epoch=E members=N digest=<32 hex> snapshot=<ids>
mesh-fail: authority state=lost|restored epoch=E [root=<id>] mutable=0|1 [reads=lease]
```

`GET /mesh/route` gains:

```text
mesh-route: authority state=current|lost epoch=E mutable=0|1 write_lease_ms=N
mesh-route: fence object=<64 hex> stale_home=<16 byte> stale_home_epoch=N home=<...> home_epoch=N reason=left-view|lease-left-view
mesh-route: result ... op=stat|commit ...
mesh-route: counters ... fences=N commits=N replays=N last_replay=stale-epoch
```

Steps 6..9 are the failure control surface, chosen and ordered by the rig:

| Step | Effect |
|---|---|
| 6 | Re-route the object that was homed at the removed node; on the isolated member, report a remote home as `no-authority` instead of re-homing it. |
| 7 | Replay the last stale wire frame through the receive path; require `stale-epoch`. |
| 8 | Commit the first object homed here; require a completion when the authority is current and `no-authority` when it is lost. |
| 9 | Read the first object homed here; require a completion while the read lease is valid. |

`--fault kill-peer` stops n1 with `SIGKILL` (no clean close) after the M2-4 sequence.
`--fault partition` pauses only the mesh relay (frames are dropped, the guest TCP sessions
stay open, because the iris port forward cannot reliably reopen a half-open guest
session): both kernels stay alive. With failure detection enabled the transport
heartbeat timeout is a 300 s backstop; the 12 s membership detector is the failure path.
Both modes assert
the `suspect -> failed -> removed` transition, the home movement, the fence rows, the
stale-frame refusal and the mutable-object rule on real `iris` kernels. `no-peer`,
`wrong-authority` and `no-route` preserve the earlier markers.

### 8. Opt-in and marker preservation

Failure detection is enabled by `mesh.fail=1` in NVRAM. When it is absent the module logs
`mesh-fail: disabled reason=config` and the M2-2 restart/rejoin check keeps its original
same-epoch behaviour: a member restart shorter than the failure window is not a membership
change. The failure fault modes set the key. When `mesh.peer` is absent there is no
listener, no join and no failure detector, and the M1 path is unchanged. The key reuses
the module's default seed epoch and the rig omits `mesh.epoch` when it is 1, so the full
route+failure environment still fits the 298-byte DS1386 area.

### 9. No kernel, ROM or ABI change

The detector and fences live in `mesh-service`, `mesh-join` and `mesh-route`; the module
carries the evidence. The kernel keeps its M1 `mesh=single` bootstrap line and its two
module TCP slots. The MCP module stays inside its 512 KiB slot A.

## Consequences

- A joined node now leaves the routing snapshot on a bounded, observable transition, and
  the survivor's data path continues locally without contacting it.
- A failed or partitioned home is fenced by its lease epoch. The write lease closes the
  handover window before the root can re-home the object.
- The authority root remains a single point of failure for membership changes. In a
  two-node partition the root side continues with one member; the non-root side cannot
  confirm anything and becomes read-only, then unavailable.
- A write lease renewal is driven by any accepted peer frame, so a link that carries only
  the 5 s heartbeat still satisfies the 11 s bound. The bound assumes the two kernels'
  monotonic clocks advance at the same rate (true for two `iris` instances on one host;
  a shared time service is later work).
- The removed member is re-admitted by a join at a new epoch. There is no automatic
  epoch push and no persisted epoch; a root restart still fails closed as M2-ADR-0005
  records.
- `mesh-sim` is not evidence. The gate is two real kernels.

## Limits

- **No linearisable write handover.** The write lease gives a bounded, one-sided margin;
  it is not a consensus round. A commit that the old home accepted before its lease
  expired is not visible at the new home. M2 does not claim multi-node transactions.
- **1-of-1 authority.** A malicious or buggy root can remove a healthy member. There is no
  threshold, no Byzantine safety and no key rotation.
- **No gossip.** With more than two nodes, a removal reaches a member only when that
  member rejoins; a third node that stays connected to the root would need the M3 epoch
  push. This slice is the two-node gate.
- **No persisted epoch and no home renewal service.** Read leases at a member are not
  renewed by the root; a partition longer than `HOME_LEASE_MS` makes its objects
  unavailable even if the member is still alive.
- **No central directory.** A fence is computed from the confirmed view. There is no
  lookup service to ask and no trusted redirector other than a home holding the old lease.
- **No L0 migration.** Actor state transfer is M2-5 and is not touched here.

## Alternatives considered

| Alternative | Why it lost |
|---|---|
| A second timeout/heartbeat implementation in the route crate | Violates "one membership model"; the route would disagree with the join about who is a member. |
| Remove a member immediately on TCP close | A clean close is not a failure, and a half-open forward is not a dead node; the gate asks for a bounded heartbeat transition. |
| Member self-removes from the view on partition | Produces two authoritative membership views; M2-ADR-0003 forbids it. |
| No write lease, rely on the 120 s read lease | The old home could commit for up to 120 s after the root re-homed the object. A 6 s write lease closes the window before the 12 s failure window. |
| Shorten the read lease to 6 s | Reads would become unavailable during a normal heartbeat hiccup; read availability and write freshness have different safety needs. |
| A quorum on the data path | The M2 gate requires the normal data path never to wait for a quorum; the survivor continues alone. |
| A central failure service or directory | Contradicts the no-directory invariant and duplicates `mesh-service`. |
| Persist the epoch in NVRAM now | The module ABI has `config_get` only; adding a write service is a kernel/ABI change and is out of this slice. |

## Evidence

Host tests:

- `mesh-service`: exact suspect/fail boundaries, duplicate failure, stale-incarnation
  replay, suspect recovery, failed node requires a join, snapshot exclusion.
- `mesh-join`: root removal advances the epoch once, duplicate ticks are no-ops, the
  removed node's objects move and the survivor's do not churn, a member loses authority
  without removing the root or advancing the epoch, a rejoin is a new epoch and restores
  mutable service, and a suspect recovery does not advance the epoch.
- `mesh-route`: a membership change fences the stale route and completes locally with zero
  sends and zero timeouts, duplicate snapshots do not repeat a fence, a replayed stale
  frame is refused with `stale-epoch`, a commit requires a current authority and a fresh
  write lease, an isolated home refuses a remote commit without moving the object, the
  root renews from its view while a member renews from authority frames, expiry yields
  `not-current`, the removed node is never used as a router, and the write lease is
  shorter than the failure window.
- The MCP module carries compile-time assertions that the write lease plus one heartbeat
  fits inside the failure window.

Real kernels (`tools/m2-rig run --fault kill-peer` and `--fault partition`):

- `suspect`, `failed` and `removed` at the documented boundaries;
- the survivor's `/mesh/homes` at the new one-member epoch with every row recomputed;
- a fence row naming the member as the stale home and the survivor as the new home;
- step 6 completing at the survivor with one attempt, no `MOVED`, no send to the removed
  node and no timeout;
- step 7 refused with `stale-epoch`;
- step 8 committing at the survivor at the new epoch;
- the isolated member keeping epoch 2, refusing the commit with `no-authority`, reporting
  `reads=lease` and `write_lease_ms=0`, and reporting a remote home as `no-authority`;
- recovery at epoch 4 with the two-member table on both kernels, a fresh authority line
  and a remote commit served by the recovered member (step 10, driven by the root).
