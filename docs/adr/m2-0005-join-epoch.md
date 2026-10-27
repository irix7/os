# M2-ADR-0005 — Join handshake and authority-confirmed membership epoch

**Status:** accepted for M2, limited
**Date:** 2026-10-26
**Deciders:** M2-2, for review at the M2 gate
**Related:** [M2-ADR-0001](m2-0001-two-node-rig.md), [M2-ADR-0002](m2-0002-transport.md),
[M2-ADR-0003](m2-0003-authority.md), [`../spec/system.md`](../spec/system.md) §5 and §9,
[`../plan/milestones.md`](../plan/milestones.md) §2, [`../plan/m2-rig.md`](../plan/m2-rig.md)

## Context

M2-ADR-0002 put a framed `MeshEnvelope` stream between two independent iris kernels.
M2-ADR-0003 decided that node 0 is the 1-of-1 authority root and the only writer of the
membership epoch, but it left the wire handshake, the authority proof and the restart rule
to M2-2. The M2-1 nodes still print `mesh: single node` and each keeps a one-node
`MembershipView`; no epoch has ever crossed the link.

The existing pieces are:

- `mesh-transport` owns the record framing, the 64 KiB bound, deadlines, heartbeat,
  reconnect backoff and request tags.  Nothing about it changes for the join.
- `mesh-service` owns the membership table: liveness, incarnation, join/leave epochs and a
  deterministic snapshot over `mesh-core::MembershipView`.
- `cap-engine` owns Ed25519 verification, a trust store and `SigningAuthority`.  It does
  not own a key store or a threshold root yet.
- The module ABI has `config_get` only; it cannot write NVRAM.  ABI 11 does expose
  `block_read`/`block_write`, so a module can persist a signed record on a dedicated SCSI
  scratch target without a kernel change.

## Decision

### 1. The handshake

```text
member B -> root A: JoinRequest(node=B, incarnation, last_epoch, last_digest, nonce)
root A:             if B is absent, table.join(B); the epoch advances exactly once
root A -> member B: EpochConfirm(root=A, epoch=E, members=[...], subject=B,
                                 request_nonce, incarnation, Ed25519 signature)
member B:           verify root, signature, subject, nonce, incarnation and epoch;
                    install the full member list at E
member B -> root A: JoinAck(node=B, epoch=E, digest, request_nonce)
```

The four bodies share one versioned, little-endian envelope prefix (`I7J1`, version 1,
kind, zero flags).  A body is carried in an existing `MessageFrame` with
`OpCode::Hello`; the request is `MessageKind::Request`, the confirmation/refusal is
`Response`, and the acknowledgement is `Completion`.  The body kind and the frame kind
are bound: a Response carrying a request body is rejected.  The transport header, CRC,
`frame_len` agreement and 64 KiB record bound are unchanged.

The member retries the **same** request body (same nonce and incarnation) every 5 s, up to
five sends.  After that it logs `mesh-join: timeout` and keeps its last view.  A new link
resets the budget.  There is no unbounded retry loop and no timer in the kernel.

### 2. The epoch is the root's, and it is signed

The root is the only writer.  A member never increments the epoch: it installs exactly the
epoch and full member list in the confirmation.  A local join/heartbeat path is not used
to manufacture a confirmed view.

The confirmation carries the **full member list**, not just an epoch number, so two nodes
cannot disagree about the membership at the same epoch.  `MembershipTable::confirmed` and
`MembershipTable::install` are the only install paths; `install` rejects a lower epoch and
a same-epoch different list.

The authority proof is an Ed25519 signature from the configured root over:

```text
"IRIX7-M2-EPOCH-v1" || version || root NodeId || epoch u32 ||
member count u16 || sorted member NodeIds || subject NodeId ||
request nonce u64 || incarnation u64
```

The member verifies it through `cap-engine`'s trust store.  It checks, in order:

1. `root` equals the configured `mesh.authority`;
2. the signature verifies under `mesh.pub` (a valid but wrong key fails with
   `bad-signature`);
3. the subject is this node;
4. the request nonce and incarnation match the outstanding attempt;
5. the epoch is non-zero and not lower than the local epoch;
6. the same epoch with a different list is an equivocation;
7. the root and subject are members.

Any failure keeps the previous view.  A replayed confirmation after convergence has no
outstanding request and is rejected; a replayed confirmation during a later attempt fails
the nonce check.  A replayed **join request** is idempotent at the root: it re-confirms the
current epoch and list and does not advance the epoch.

### 3. NVRAM keys and the authority fixture

| Key | Meaning |
|---|---|
| `mesh.peer` | The peer's mesh host port through this node's gateway.  Its presence enables the M2-1 listener and the join. |
| `mesh.authority` | The root's MAC-derived 16-byte NodeId. |
| `mesh.epoch` | The bootstrap seed epoch (1 in the rig). |
| `mesh.pub` | The root's Ed25519 public key as 64 hex characters; written to the member. |
| `mesh.key` | The root's 32-byte Ed25519 seed as 64 hex characters; written **only** to the root. |

The seed is a **lab fixture**, not a production key store.  It is derived from a fixed
domain string, so a run is reproducible, and the evidence bundle redacts it.  The property
being demonstrated is that an epoch is not trusted without a proof from the configured
root, not key custody.

The DS1386 environment has 298 bytes before the MAC slot.  The two 64-hex keys do not both
fit on the root, so the root stores only `mesh.key` and derives its public key; the member
stores only `mesh.pub`.  This is why the keys have short names.

### 4. Restart rules and the signed epoch store

The module cannot write NVRAM.  A confirmed epoch is instead written to the ABI v11
`block_write` / `block_read` service on a dedicated SCSI scratch target (target 3, starting
at LBA 8).  The record is signed by the authority root and verified on the next boot:

- the root writes an `EpochRecord` signed over `IRIX7-M2-EPOCH-STORE-v1`, the epoch and the
  full member list;
- a member writes the exact Ed25519-signed `EpochConfirmation` it installed;
- a restore verifies the signature, root, subject (member only), epoch and canonical member
  list before installing anything;
- a corrupt, foreign or role-mismatched record **disables the join** rather than silently
  starting from the seed.

| Event | M2 outcome |
|---|---|
| Member restarts, root still up | The member verifies its stored confirmation, starts at the confirmed epoch and sends `last_epoch=E`.  The root re-confirms the same epoch and list.  The rig requires the restore line and `last_epoch` equal to the confirmed epoch; a seed fallback fails the gate. |
| Root restarts, member still up | The root verifies its stored `EpochRecord`, resumes epoch `E`, and the live member presents `last_epoch=E`.  The root confirms the same epoch; no `epoch-regression` is allowed.  The rig restarts the root while the member stays up and requires both views to remain at `E`. |
| Both restart | Each verifies its own signed record and resumes the same epoch.  If the scratch volume is absent both fall back to the seed, log `epoch-store: unavailable`, and the join still converges, but that run does not claim persistence.  Runs with the store are deterministic: the two-node epoch is 2 and the digest is `2EDA13C5AC6F726132E0479DFE6045B7`. |

The store is deliberately **not** NVRAM: the DS1386 environment has no write service, and
the 298-byte environment is already full at the M2 route/failure configuration.  It is a
local scratch record, not a replicated log; a node with a lost or reformatted scratch
volume logs the absence and fails back to the seed.  The store is enabled automatically
whenever the join is enabled and an ABI 11 kernel exposes target 3; the M2 rig provisions
that target on every node.  The root writes its record before sending the confirmation,
but there is no fsync or write-ahead log: a power loss can lose the newest record.  If the
member kept a newer confirmation the root refuses with `epoch-regression` rather than move
backwards; if both nodes lost it the epoch can regress.  The rig proves clean process
restarts, not power-loss durability.  A future `config_set` service or a replicated log
can replace the scratch volume without changing the handshake or the fail-closed rules.

### 5. What this slice is not

- No leave/removal.  The root only adds members; M2-6 owns failure, removal and fencing.
  Until then, a replayed join request cannot re-add a removed node because removal does
  not exist.
- No epoch push to members that did not ask.  A two-node mesh converges because the member
  asks.  A third node's join would advance the root's epoch without telling node 1; that
  is M3 gossip and is not claimed here.
- No *replicated* epoch store, no threshold root, no key rotation, no revocation, no TLS
  and no cross-restart replay window.  The local signed record survives a process restart
  on a provisioned scratch target; it is not a consensus log and does not recover a node
  whose scratch volume is lost.  These are the same limitations recorded in
  M2-ADR-0002/0003, not new claims.
- `mesh-sim` is not evidence.  The gate is the two-kernel rig.

## Consequences

- Join and epoch convergence are testable on the host (codec, authority, state machine)
  and on two real kernels (the rig), with no kernel or ROM change.
- The root's membership table is the only authority for the epoch; a member cannot
  self-confirm, and a partition leaves it on its last confirmed view.
- The 1-of-1 root remains a single point of failure for **membership changes**, not for
  the normal data path.  A restart with a valid signed record resumes the confirmed view;
  a corrupt or missing record fails closed instead of inventing a new epoch or silently
  lowering a member.
- The join body is bounded at 4 KiB.  A 128-member confirmation fits (2,176 bytes), but
  the module logs at most two member IDs on a 256-byte line; the digest is always logged.
- The MCP module grew from 104,840 bytes (M2-1) to 198,264 bytes because it now links
  `mesh-join`, `mesh-service`, `mesh-core`, `cap-engine`, `ed25519-dalek` and `blake3`.
  The signed epoch store adds a few hundred bytes of code and one 2.5 KiB static buffer;
  it remains well inside the 512 KiB slot A.

## Alternatives considered

| Alternative | Why it lost |
|---|---|
| Trust `mesh.authority` as a claimed NodeId without a signature | Any peer on the loopback path could install an epoch.  M2 must fail closed; a names-only proof is not a proof. |
| A shared-secret MAC keyed by an NVRAM value | Smaller, but it is not the `cap-engine` authority seam, gives no public verification and would be replaced by Ed25519 anyway.  A second crypto path is worse than one. |
| Persist the epoch in NVRAM now | The module ABI has no NVRAM write service, and the 298-byte environment is already full at the route/failure configuration.  ABI 11 `block_write` on a dedicated scratch target closes the gap without expanding the frozen bootstrap. |
| Self-confirm on the member | Produces two authoritative views after a partition, which M2-ADR-0003 forbids. |
| A central membership broker | Contradicts the "no central directory" M2 criterion; the root is already the authority. |
| Gossip the epoch now | M3 work; a two-node join does not need it. |
| Use `mesh-sim` for the join gate | Policy only; the M2 gate requires real kernels. |

## Evidence

Host tests in `crates/mesh-join`:

- two nodes converge on one epoch and one snapshot;
- wrong authority key, wrong root identity and a tampered signature fail closed;
- a stale epoch and a same-epoch different list are rejected;
- duplicate confirmation is idempotent and does not advance the epoch;
- member restart rejoins at the same epoch;
- root restart refuses to lower the member's epoch;
- partition/no response retries the same request, times out and keeps the last view;
- replayed confirmation (after convergence and during a later attempt) is rejected;
- replayed join request is idempotent;
- malformed bodies (magic, version, kind, flags, count, canonical members, truncation,
  oversize) and header/body kind mismatches are rejected.

`crates/mesh-service` covers confirmed views and install regression/equivocation.
`cap-engine` covers signing and verifying an arbitrary message.

The iris rig (`tools/m2-rig run`, default mode) produced:

```text
n0: mesh-join: converged role=root epoch=2 members=2 \
    digest=2EDA13C5AC6F726132E0479DFE6045B7 \
    snapshot=08006969B60100000000000000000000,08006995F50000000000000000000000
n1: mesh-join: converged role=member epoch=2 members=2 \
    digest=2EDA13C5AC6F726132E0479DFE6045B7 \
    snapshot=08006969B60100000000000000000000,08006995F50000000000000000000000
n0: mesh-join: ack from=08006969B60100000000000000000000 epoch=2 digest=2EDA13C5AC6F726132E0479DFE6045B7
rejoin n1: epoch 2 -> 2 digest 2EDA13C5AC6F726132E0479DFE6045B7 -> 2EDA13C5AC6F726132E0479DFE6045B7 same=true
```

The persisted-epoch run additionally records:

```text
n0: epoch-store: stored epoch=2 members=2 digest=2EDA13C5AC6F726132E0479DFE6045B7 source=root-record
n1: epoch-store: stored epoch=2 members=2 digest=2EDA13C5AC6F726132E0479DFE6045B7 source=confirmation
n1 (restart): epoch-store: restored epoch=2 members=2 digest=2EDA13C5AC6F726132E0479DFE6045B7 source=confirmation
n1 (restart): mesh-join: request from=... last_epoch=2 last_digest=2EDA13C5...
n0 (restart): epoch-store: restored epoch=2 members=2 digest=2EDA13C5... source=root-record
n0 (restart): mesh-join: local role=root epoch=2 members=2 ...
```

The rig recomputes the digest from the documented canonical bytes, requires it to match
both nodes, requires the member's rejoin `last_epoch` to equal the confirmed epoch, and
requires the root restart to recover the same epoch with no `epoch-regression`.  The
`wrong-authority` mode rejects with `mesh-join: reject reason=bad-signature` and leaves the
member at epoch 1.  The `no-peer` mode has no mesh listener and no join, and keeps the M1
`mesh=single` and `mesh=0/0/0` markers.

## M3 aggregated refresh (post-M2 gate)

M3 replaces the per-member signed refresh with an aggregated root-signed summary so a
converged member stops sending a full signed join request every interval.  The root signs
**one** `EpochSummary` per change set and pushes it to every known peer; a member reports
liveness with an unsigned `MemberSummary` instead.

**Decision (M3):**

1. **Aggregate replaces refresh.** After the first confirmation a converged member sends a
   `MemberSummary` every `SUMMARY_INTERVAL_MS` (5 s) rather than a `JoinRequest`.  The
   root validates the summary against the aggregate it last signed (epoch, digest,
   incarnation and monotonic counter) and refreshes the member's failure-clock record; it
   does not sign per summary.  On any change the root signs one aggregate per change set.
   The summary itself is small (60 bytes) and unsigned; its authority comes from the
   signed aggregate that binds `node` and `incarnation` to `epoch` and `digest`.

2. **One signed aggregate per change set.** The root builds grants from its confirmed
   table (`node` + confirmed `incarnation`), signs `EpochSummary` once, and pushes it to
   every known peer.  A member installs a verified aggregate (signature first, then the
   snapshot, never lower than the local epoch, and it must name this member).  The cost of
   a change set is linear in the member count, not quadratic.

3. **Epoch floor / persistence.** The confirmed epoch is persisted via ABI v11
   `block_write`/`block_read` on a dedicated scratch target (target 3, LBA 8), exactly as
   the M2 signed epoch record.  A root restores its signed record and resumes the
   confirmed epoch; a member restores its signed confirmation.  A corrupt or role-mismatch
   record disables the join rather than inventing a higher epoch.  Root restart refuses to
   lower a member's confirmed view (`epoch-regression`).

4. **Batch removal.** The root removes every member the failure detector has confirmed
   failed in a single change set and one epoch advance (not one epoch per failed member),
   then signs one aggregate for the post-removal view.

5. **Single-root limit.** Only the root signs.  A partition leaves a member on its last
   confirmed aggregate; it cannot self-confirm or create a second authoritative home.  An
   equivocated aggregate (re-signed different list at the same epoch) yields
   `Equivocation` and is fail-closed.  A threshold root is Phase 8 work.

6. **Per-link module state is deferred to #264.** The module owns one mesh TCP slot for
   the join handshake; aggregated refresh runs over the existing multi-source link
   (multiplexed by the host hub).  Direct per-member TCP slots and sparse fan-out over
   them are #264 work.

**Status:** proposed for M3; not yet reviewed at a gate.  The library policy is
implemented and host-tested in `crates/mesh-join` (gossip, authority, fail-closed rules,
deterministic 2/8/128-node model).  The module wiring is in `modules/mcp/src/mesh.rs`.
Real 4-node convergence over the host hub is recorded in the #249 evidence bundle.
