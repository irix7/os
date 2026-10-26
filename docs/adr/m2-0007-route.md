# M2-ADR-0007 — Remote capability route and the one-`MOVED` retry

**Status:** accepted for M2, limited
**Date:** 2026-10-27
**Deciders:** M2-4, for review at the M2 gate
**Related:** [M2-ADR-0002](m2-0002-transport.md), [M2-ADR-0003](m2-0003-authority.md),
[M2-ADR-0005](m2-0005-join-epoch.md), [M2-ADR-0006](m2-0006-homes.md),
[`../spec/system.md`](../spec/system.md) §5 and §8,
[`../plan/milestones.md`](../plan/milestones.md) §2, [`../plan/m2-rig.md`](../plan/m2-rig.md)

## Context

M2-3 made both kernels agree on the same rendezvous home for every fixture object,
but explicitly stopped at an evidence surface: a home in `/mesh/homes` was not a
capability route, a lease or a `MOVED` authority.  The next gate item is a capability
issued at one node, resolved through the existing rendezvous home, served by the
home on a second real kernel, and a stale route that answers `MOVED` and retries
exactly once.

The pieces already exist:

- `mesh-core` owns `home_for`, the bounded `RouteCache`, `ObjectRoute` with
  `home_epoch`/`object_epoch`/`lease_until`, and `Lease` fencing.
- `cap-engine` owns Ed25519 verification, the trust store and the root signing seam.
  `Capability` in `sys-primitives` carries the object, issuer, subject, home,
  `home_epoch`, `object_epoch`, `root_epoch`, rights and signature.
- `mesh-join` owns the authority-confirmed snapshot and the root key; it is the only
  place that can sign as the authority without exposing the seed.
- The M2-1 transport owns record framing, bounds, deadlines and request tags; the
  module owns the two TCP slots.

What does not exist is a route protocol, a home-lease state machine, a `MOVED`
answer, or a bounded way to drive a controlled stale-home change on a two-node rig.

## Decision

### 1. The route is the confirmed rendezvous home, not a directory

A sender computes the home with `mesh_core::home_for` over its confirmed
`MembershipView` and caches it in `RouteCache`.  There is no central lookup, no
per-object index, and no second placement model.  The capability token carries the
object, issuer, subject, home and epochs; a receiver checks the token and its own
lease, not a directory.

A node serves an object only while it holds a current `HomeLease`.  Leases are
installed from the confirmed snapshot for the objects whose rendezvous winner is
this node, and are bounded by `HOME_LEASE_MS`.  The route cache entry is bounded by
`ROUTE_LEASE_MS` and is refreshed from the confirmed view when it expires.

### 2. The capability is the authority proof

The authority root issues the capability with the `cap-engine` signing seam.  The
subject is the deterministic M2 route principal
`BLAKE3-256("IRIX7-M2-ROUTE-PRINCIPAL-v1" || NodeId)`.  The receiver verifies, in
order:

1. the issuer is the configured authority key;
2. the Ed25519 signature verifies through the `cap-engine` trust store;
3. the subject is the sending node's principal;
4. the capability carries `READ`;
5. the validity window contains the receiver's clock;
6. the object generation matches the lease;
7. the token's home/home-epoch pair is the active lease, or the documented
   one-epoch predecessor of a moved lease.

The signature is checked before any semantic field is trusted.  A capability issued
at the old home is accepted at the new home only for the one-epoch move recorded by
the lease; any other home/epoch pair fails closed with `stale-capability`.

### 3. Bounded `I7R1` bodies over the M2-1 transport

Each route message is one `MeshEnvelope` `MessageFrame` with `OpCode::Invoke` (data)
or `OpCode::RouteQuery` (lease move).  The body is `I7R1`, version 1, field by field
little-endian, and at most 512 bytes:

| Kind | Purpose |
|---|---|
| `INVOKE` | Capability (320 bytes), operation and reserved word. |
| `RESULT` | Object, home, home epoch, object epoch, remaining lease, 32-byte value. |
| `MOVED` | Object, old home, new home, old/new home epoch, object epoch, lease/grace, reason. |
| `HOME_MOVE` | Root-signed one-object lease move scoped to the membership epoch/digest. |
| `MOVE_ACK` | Object, new home, new home epoch, accepted/refused and reason. |
| `ERROR` | Object and terminal reason. |

The M2-1 length prefix, 64 KiB bound, header CRC, `frame_len` agreement, `src`/`dst`
checks and the 10 s receive deadline are unchanged.  The module lowers the per-body
bound to `MAX_BODY` before copying.  Request ids must increase; a response must
answer the outstanding request; a link loss aborts the outstanding operation.

### 4. `MOVED` is bounded and the sender retries exactly once

A home move uses `DRAINING`, a new `home_epoch` and a forwarding grace period:

- the old home keeps a `DRAINING` lease for `GRACE_MS` and answers a request that
  still names the old home epoch with `MOVED`;
- the `MOVED` names exactly `old_home_epoch + 1`, a confirmed member, the remaining
  lease and the remaining grace;
- the sender checks that the answer came from its cached old home, validates the
  object, epochs, member and bounds, replaces the route cache entry and retries
  **once**;
- a second `MOVED`, a lower epoch, an unknown epoch, a wrong home, a non-member
  target or a malformed frame fails closed with `repeated-moved`, `stale-epoch`,
  `unknown-epoch`, `wrong-home`, `not-home` or `malformed`.

A node that is not the current home and holds no draining lease rejects with
`not-home`; it never answers with a computed redirect.  This keeps a non-home node
from acting as a directory.

### 5. The controlled change is a root-signed lab lease move

M2 has no leave/removal and no membership churn, so the stale path cannot be driven
by a real epoch change.  Instead the root may sign a `HOME_MOVE` for one fixture
object.  The move:

- names the confirmed membership epoch and digest;
- requires the old home to be the current rendezvous winner and the new home to be
  the other confirmed member;
- advances `home_epoch` by exactly one;
- is bounded by `lease_ms` and `grace_ms` and is refused outside the bounds;
- is scoped to one object and one epoch; it is not a directory, not a placement
  policy and not persisted.

The root applies the move locally and sends it to the other confirmed member.  The
old home verifies the authority signature and the view before entering `DRAINING`; the
new home installs the next-epoch lease.  The move is a lab fixture for the `MOVED`
gate, not a claim that M2 has general home migration.

### 6. The rig drives the steps; the policy chooses the objects

`GET /mesh/route` is a bounded in-memory report.  `GET /mesh/route?step=N` drives
one step:

| Step | Effect |
|---|---|
| 1 | A sends a capability for the first peer-homed fixture object; B serves it. |
| 2 | A signs a move of that object from B to A; B enters `DRAINING`. |
| 3 | A repeats the stat on its stale B route; B answers `MOVED`; A retries locally and completes. |
| 4 | A signs a move of the first A-homed object to B. |
| 5 | A repeats the stat on its stale A route; the local old home answers `MOVED`; A retries at B and completes. |

The rig chooses the order and never injects a home, capability or epoch.  The route
policy selects the objects from the confirmed rendezvous view, exactly as a normal
sender would.  The rig recomputes the fixture object IDs, homes, epoch pairs and
values independently and records the raw reports.

### 7. Opt-in and marker preservation

`mesh.route=1` enables the route.  When it is absent the module logs
`mesh-route: disabled reason=config` and keeps the M2-1/M2-2/M2-3 behaviour exactly:
the listener, join and `/mesh/homes` surface are unchanged.  When `mesh.peer` is
absent the mesh listener and route do not exist, and the M1 `mesh=single` path is
preserved.  `--fault no-route` exercises the opt-in negative; `--fault no-peer`
exercises the absent-peer negative.

## Consequences

- Both kernels run one route protocol, one home calculation and one lease state
  machine.  There is no central directory and no trusted redirector other than the
  authority-confirmed home.
- The `MOVED` path is observable in both directions on two real kernels: a wire
  `MOVED` with a local retry, and a local `MOVED` with a wire retry.
- A capability is a bearer token.  The M2 transport is plain TCP on host loopback,
  so there is no confidentiality, peer authentication or replay window.  The token
  is bound to the sending principal at the application layer, but a host attacker
  can still observe or inject TCP bytes.  TLS/Noise is deferred with M2-ADR-0002.
- The capability validity window uses each kernel's monotonic boot clock because
  the module ABI has no wall-clock service.  The window is deliberately wide; the
  route lease is the real time bound.  A shared time service is a later slice.
- The controlled move is a lab fixture.  General home moves, leave/removal, failure
  detection and lease fencing across a partition remain M2-6/M3.  The `?step=N`
  control surface is a rig-only API: it exists only when `mesh.route` is set, it is
  not a production management interface, and it does not bypass the capability or
  authority checks.
- This slice does not implement L0 actor migration, live TLS/socket/fd migration,
  multi-hop forwarding or a persistent route/directory.  The MCP module stays
  inside its 512 KiB slot A; the route report is served over the existing HTTP
  slot and the mesh link stays on slot 1.

## Alternatives considered

| Alternative | Why it lost |
|---|---|
| A central route directory | Violates the no-directory invariant and duplicates `mesh-core`. |
| A second rendezvous hash | Two placements can disagree; the gate is about the existing home calculation. |
| Retry until the route stops moving | Unbounded chase and replay risk; the spec says one retry. |
| A real leave/removal for the stale path | That is M2-6; it needs epoch push, failure detection and lease fencing. |
| An unsigned test move | The move changes home authority; it must be root-signed and view-scoped. |
| Automatic scenario inside the module | Hides the rig's control and adds code to slot A; the rig-driven steps keep the policy and the evidence separate. |
