# M2-ADR-0006 — Rendezvous homes from the confirmed membership snapshot

**Status:** accepted for M2, limited
**Date:** 2026-10-27
**Deciders:** M2-3, for review at the M2 gate
**Related:** [M2-ADR-0005](m2-0005-join-epoch.md), [M2-ADR-0002](m2-0002-transport.md),
[`../spec/system.md`](../spec/system.md) §5, [`../plan/milestones.md`](../plan/milestones.md) §2,
[`../plan/m2-rig.md`](../plan/m2-rig.md)

## Context

M2-2 made both kernels converge on one signed membership epoch and one full member
snapshot.  The next gate item is **rendezvous homes**: both kernels must choose the same
home node for the same object from that confirmed snapshot, using the existing
`mesh-core` policy rather than a second hash or placement model.

`mesh-core` already defines the production formula:

```text
home = argmax over nodes n of BLAKE3-64("IRIX7-HOME-v1" || object || n || vnode)
```

with one virtual node per physical node.  The M1 single-node path exercises it, and the
128-node policy tests cover distribution.  What is missing is an M2-2/M2-3 seam: the
confirmed `MembershipSnapshot` in `crates/mesh-join` is not yet handed to `mesh-core`,
and there is no bounded evidence surface a two-kernel rig can compare.

The constraints are the same as M2-2:

- the compressed PROM is frozen; the feature belongs in the MCP module;
- the module ABI has `config_get` only, so the confirmed epoch is not persisted;
- the module has two TCP slots: HTTP on slot 0 and the mesh listener on slot 1 when
  `mesh.peer` is present;
- the DS1386 environment has 298 bytes before the MAC slot.

## Decision

### 1. Homes are computed only from a confirmed snapshot

A member presents homes only after it has installed an authority confirmation
(`JoinNode::converged`).  Before that it returns `409 pending` and no object rows.  The
root's seed view is confirmed by its own authority; it is a legitimate one-node mapping
at the seed epoch and is replaced when a join advances the epoch.

There is no local "best effort" table built from a heartbeat or from the member's seed
view: a member that has not converged is not allowed to present the seed as the mesh
mapping.  This is the same fail-closed rule as M2-ADR-0005.

### 2. The M2 test object set is deterministic and explicitly configured

NVRAM carries `mesh.obj=<count>`: the number of fixture objects, bounded to **1..32**,
default **8**.  The object IDs are derived, not stored:

```text
object[i] = BLAKE3-256("IRIX7-M2-HOME-OBJECT-v1" || i u32 LE)
```

This is a fixture set, not a production object directory or a second placement model.
The count is small enough to fit the environment and the response is bounded by the
32-object hard limit.  The rig, the module and the host tests all derive the same IDs.

### 3. `GET /mesh/homes` is the primary evidence surface

The MCP module serves a deterministic `text/plain` body:

```text
mesh-homes: state=current object_set=m2-v1 epoch=2 members=2 digest=<32 hex> count=8
mesh-home: object=<64 hex> home=<32 hex> epoch=2 digest=<32 hex>
...
```

Every row repeats the epoch and membership digest, so a row cannot be detached from the
view that produced it.  The module logs the same header and rows on convergence, so the
serial transcript carries an identical copy.

Status codes:

| Condition | Status | Body |
|---|---|---|
| Confirmed table, no `epoch=` query | `200` | `state=current` and one row per object |
| Confirmed table, `epoch=` equals the confirmed epoch | `200` | `state=current` |
| `epoch=` names another epoch | `409` | `state=stale requested_epoch=N epoch=M digest=... count=0`, no rows |
| Member has not confirmed a view | `409` | `state=pending ... count=0`, no rows |
| Mesh disabled (`mesh.peer` absent) | `404` | `state=disabled reason=peer` |
| Join disabled (missing/bad authority key) | `404` | `state=disabled reason=join` |
| Unknown or malformed query | `400` | `state=bad-request reason=query` |

A stale epoch is an explicit negative: the old table is never relabelled as current and
no `mesh-home:` row is returned.  This is the M2-3 check that a cached route cannot pass
for a newer epoch.

### 4. The rig recomputes, it does not trust

`tools/m2-rig` recomputes the fixture object IDs, the rendezvous scores and the
membership digest independently of the module.  It fetches `/mesh/homes` from both host
forwards on the clean control plane and requires:

- both nodes are on the join-confirmed epoch and digest;
- the object list and every home match the rig's independent table;
- both nodes report the same rows;
- the serial group equals the MCP body on each node;
- `?epoch=<seed>` returns `409 stale` with no rows on both nodes.

The after-rejoin check uses the module's serial group rather than a second MCP fetch.
The module logs the same header and rows from the same `HomeTable`; using the serial
transcript avoids the iris host forward retaining a half-open guest connection across
the member restart.  The rig still compares both kernels' after-rejoin groups with each
other and with the pre-restart MCP table.  This is a harness constraint, not a protocol
exception; the MCP body is checked end to end before the restart.

The before/after check is a **rejoin**, not a membership change.  Leave/removal does not
exist in this slice, so the root cannot advance the epoch by removing or adding a node;
that remains M2-6/M3.  The host tests cover membership changes at the policy layer,
including the rendezvous property that removing a node moves only its objects.

### 5. No routing, fencing or migration in this slice

The endpoint is an evidence surface.  It does not route a capability, answer `MOVED`,
lease a home, fence a stale writer or migrate an actor.  Those remain M2-4..M2-6 and M3.
In particular, a home in this table is not yet a capability route.

## Consequences

- Both kernels now produce a comparable home table from the confirmed snapshot, with the
  existing `mesh-core` formula and no second implementation.
- The table is bounded by design: at most 32 rows, each ~180 bytes, inside the 16 KiB
  MCP response buffer.
- The module grows by the `mesh-core` link it already carried transitively through
  `mesh-join`; it remains well inside the 2 MiB slot.
- The missing persisted epoch still limits restart behaviour: a member rejoin at the
  same epoch is demonstrated; a root restart fails closed as M2-ADR-0005 records.
- The root serves its one-node seed table before any member joins.  That is a confirmed
  local view, not a two-node claim; the rig only compares after convergence.
- `mesh.obj` consumes 11 bytes of the 298-byte environment at the default count.  The
  full two-node environment is regression-tested before the MAC slot.

## Alternatives considered

| Alternative | Why it lost |
|---|---|
| Compute homes from `home_for` separately in the module with a second hash | Exactly the second placement model the gate forbids.  The module calls `mesh-core::HomeTable`/`home_for`. |
| Serve the member's seed view before confirmation | It would present a one-node table as the joined mapping; stale or pending evidence must fail closed. |
| Store explicit object IDs in NVRAM | The full two-node environment is already near 298 bytes; 8 x 32-byte IDs do not fit.  A count plus a documented derivation is bounded and reproducible. |
| Serial lines only | Workable, but harder to query and to test the stale-epoch negative.  MCP is the preferred control-plane surface; the same lines are still logged. |
| A new `mesh-homes` crate | The policy and the fixture derivation belong with `mesh-core`'s rendezvous space; a new crate would invite drift. |
| Add `MOVED`/route-cache semantics now | Out of scope for M2-3 and would mix evidence with authority.  This slice must not claim a remote capability route. |

## Evidence

Host tests in `crates/mesh-core`:

- fixture object derivation is deterministic, distinct and bounded;
- empty membership is rejected and an empty object set is accepted;
- single-node homes are all self;
- two-node homes are deterministic, member-local and bounded over 256 objects;
- epoch changes are visible and `at_epoch` rejects a stale query;
- mismatch detection reports epoch, membership digest, object, home and count changes;
- removing a node moves only the objects that node owned.

Host tests in `tools/m2-rig` cover the evidence codec, independent fixture derivation,
stale/pending bodies and before/after comparison.  The two-kernel run is recorded in the
M2-3 evidence bundle (`run.json`, `summary.md`, `serial-*.log`).
