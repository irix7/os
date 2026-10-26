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
- The module ABI has `config_get` only.  A module cannot write NVRAM, so a confirmed
  epoch cannot be persisted by this slice without a kernel/ABI change.

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

### 4. Restart rules, given no persisted epoch store

The module cannot write NVRAM.  The confirmed epoch is therefore in memory for one boot.
The rules are explicit:

| Event | M2 outcome |
|---|---|
| Member restarts, root still up | The member starts at the seed epoch and sends `last_epoch=1`.  The root sees an existing member and re-confirms its current epoch and list.  The member installs the same epoch.  The rig proves this on real kernels. |
| Root restarts, member still up | The root starts at the seed.  The member presents its higher epoch; the root refuses with `epoch-regression` instead of lowering the member's view.  The member keeps its last confirmed view, does not advance the epoch and does not accept a new membership change.  Recovery needs an operator to re-seed both NVRAM images.  A host test covers the refusal. |
| Both restart | The same seed and the same join sequence reproduce the same epoch and snapshot.  Runs are deterministic: the two-node epoch is 2 and the digest is `2EDA13C5AC6F726132E0479DFE6045B7`. |

The missing persistent epoch store is a **documented ABI gap**, not a silent assumption.
A future `config_set` or a module-owned writable record can persist the confirmed epoch
without changing this handshake.

### 5. What this slice is not

- No leave/removal.  The root only adds members; M2-6 owns failure, removal and fencing.
  Until then, a replayed join request cannot re-add a removed node because removal does
  not exist.
- No epoch push to members that did not ask.  A two-node mesh converges because the member
  asks.  A third node's join would advance the root's epoch without telling node 1; that
  is M3 gossip and is not claimed here.
- No persisted epoch, no threshold root, no key rotation, no revocation, no TLS and no
  cross-restart replay window.  These are the same limitations recorded in
  M2-ADR-0002/0003, not new claims.
- `mesh-sim` is not evidence.  The gate is the two-kernel rig.

## Consequences

- Join and epoch convergence are testable on the host (codec, authority, state machine)
  and on two real kernels (the rig), with no kernel or ROM change.
- The root's membership table is the only authority for the epoch; a member cannot
  self-confirm, and a partition leaves it on its last confirmed view.
- The 1-of-1 root remains a single point of failure for **membership changes**, not for
  the normal data path.  The root-restart rule fails closed instead of inventing a new
  epoch.
- The join body is bounded at 4 KiB.  A 128-member confirmation fits (2,176 bytes), but
  the module logs at most two member IDs on a 256-byte line; the digest is always logged.
- The MCP module grew from 104,840 bytes (M2-1) to 198,264 bytes because it now links
  `mesh-join`, `mesh-service`, `mesh-core`, `cap-engine`, `ed25519-dalek` and `blake3`.
  It remains well inside the 2 MiB slot.

## Alternatives considered

| Alternative | Why it lost |
|---|---|
| Trust `mesh.authority` as a claimed NodeId without a signature | Any peer on the loopback path could install an epoch.  M2 must fail closed; a names-only proof is not a proof. |
| A shared-secret MAC keyed by an NVRAM value | Smaller, but it is not the `cap-engine` authority seam, gives no public verification and would be replaced by Ed25519 anyway.  A second crypto path is worse than one. |
| Persist the epoch in NVRAM now | The module ABI has no write service.  Adding one is a kernel/ABI change and expands the frozen bootstrap; the task is a join slice, so the rule is documented and the gap is handed off. |
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

with the member's rejoin request starting at `last_epoch=1` and the root re-confirming
`epoch=2`.  The rig recomputes the digest from the documented canonical bytes and requires
it to match both nodes.  The `wrong-authority` mode rejects with
`mesh-join: reject reason=bad-signature` and leaves the member at epoch 1.  The `no-peer`
mode has no mesh listener and no join, and keeps the M1 `mesh=single` and `mesh=0/0/0`
markers.
