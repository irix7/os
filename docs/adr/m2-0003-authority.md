# M2-ADR-0003 — 1-of-1 authority root for the M2 membership epoch

**Status:** accepted for M2, limited
**Date:** 2026-10-26
**Deciders:** M2-0, for review at the M2 gate
**Related:** [M2-ADR-0001](m2-0001-two-node-rig.md), [M2-ADR-0002](m2-0002-transport.md),
[`../spec/system.md`](../spec/system.md) §5, [`../plan/milestones.md`](../plan/milestones.md)
Phase 3 and Phase 8

## Context

M2 must join a second replica to the same logical computer, agree a membership epoch,
survive a restart with the same epoch, route a capability to a remote home, migrate an L0
actor, and define the outcome of one-node failure and partition. The authority question
is: **who is allowed to say that the membership view has changed?**

The project's long-term direction is a threshold root and home certificates from
`cap-engine` (Phase 8). M2 is earlier. The roster explicitly allows **1-of-1** for M2 if
the limitation is documented, and the M2 gate is a two-node demonstration, not a
Byzantine-fault-tolerant deployment.

The relevant existing pieces are:

- `mesh-service` has a membership table with join/leave epochs, incarnation heartbeats,
  suspect/failed state and deterministic snapshots; its host tests cover the state
  machine.
- `mesh-core` implements BLAKE3 rendezvous homes, a bounded route cache, leases and
  placement over a `MembershipView` (an immutable epoch-tagged sorted list of at most 128
  node IDs).
- `cap-engine` has the capability/authority vocabulary but no threshold root and no
  home-certificate lifecycle yet.
- The kernel initialises a one-node `MembershipView` at boot. It has no join handshake and
  no authority confirmation.

## Decision

1. **Node 0 is the authority root for M2.** The authority root is a **policy role**, not
   a data-path bottleneck. It is identified by the 16-byte `NodeId` derived from its MAC
   and written into every node's NVRAM as `mesh.authority`. In the rig, both nodes carry
   `mesh.authority=08:00:69:95:f5:00` (node 0).

2. **The root is the only writer of the membership epoch.** A non-root node may propose a
   join or leave and may persist a **confirmed** epoch, but it must not increment the
   epoch itself. The M2-2 handshake is:

   ```text
   B -> A: JOIN_REQUEST(node=B, incarnation, last_epoch)
   A -> B: EPOCH_CONFIRM(epoch=E, members=[A,B], root=A, signature/MAC later)
   B:      persist E, install MembershipView(E), ack
   A:      install MembershipView(E), ack
   ```

   The confirmation must carry the full member list and the epoch, not just an epoch
   number. `mesh-service` already models the table; M2-2 adds the wire messages and the
   persistence. The initial epoch is `1` and is written as `mesh.epoch=1` by the rig.

3. **The normal data path does not wait for a quorum.** Reads, writes, capability
   resolution and actor messages use the current confirmed view and the home's lease.
   They do not consult the root per request. The root is on the membership-change path
   only. This is the same "no quorum on the normal data path" rule as the M2 gate.

4. **1-of-1 means one failure domain.** If the root is unavailable:

   - no new join, leave or epoch change is confirmed;
   - existing nodes keep serving their current homes under the last confirmed epoch;
   - a restart must reload the last confirmed epoch from persistent storage and must not
     invent a higher one;
   - the partition/failure policy (M2-6) decides when a lease expires and a home becomes
     unavailable. It must not create a second authoritative home.

   If the non-root node is unavailable, the root may remove it after the heartbeat
   timeout and move the affected homes (M2-6). The root remains authoritative for the
   epoch.

5. **A non-root must fail closed.** If `mesh.authority` names a node that is not the
   sender of an epoch confirmation, the non-root rejects the confirmation. If a node has
   no confirmed epoch, it serves only local objects and does not claim remote homes. A
   node must never self-confirm a membership change because it cannot see the root.

6. **The authority interface is a seam, not a hard-coded node.** M2-2 should expose a
   small trait or function set:

   ```text
   authority_root() -> NodeId
   confirm_epoch(request) -> EpochConfirmation
   verify_confirmation(confirmation) -> Result<MembershipView, AuthorityError>
   ```

   The 1-of-1 implementation returns the configured root's confirmation. A later
   threshold implementation can replace it without changing the join state machine.

7. **Persist the epoch and the confirmed member list.** NVRAM is a battery-backed chip,
   not a distribution channel, but it is the natural place for a two-node lab image. The
   rig already writes `mesh.epoch`; M2-2 should write the confirmed epoch and member list
   atomically (or to a small on-disk record if a writable filesystem is available) and
   verify them at boot. A corrupt record must fall back to the local-only view and fail
   closed, not to a larger epoch.

## Consequences

- M2 can demonstrate join, epoch convergence and restart with a simple, testable
  authority. The root's decision is deterministic and easy to log.
- The authority is a single point of failure for **membership changes**, not for normal
  traffic. This is an accepted M2 limitation, not a claim of high availability.
- There is no Byzantine safety: a malicious or buggy root can confirm a bad epoch. There
  is no threshold, no signature, no revocation and no home certificate in M2.
- Split-brain is possible if `mesh.authority` is misconfigured or if a node is restored
  from an old NVRAM image with a higher epoch. M2-2 must validate the configured root and
  the persisted epoch, and M2-6 must test the partition case.
- The confirmation format is a protocol decision. It should be versioned and should not be
  a raw struct on the wire; use the same little-endian field-by-field discipline as
  `MeshHeader`.

## Failure and partition outcomes for M2

| Event | M2 outcome |
|---|---|
| Root (node 0) stops | Node 1 keeps the last confirmed epoch; no new membership change; local homes continue under lease. |
| Non-root (node 1) stops | Node 0 may remove it after the heartbeat timeout; affected homes move; epoch increments only with the root's confirmation. |
| Both stop and restart | Each reloads its last confirmed epoch from NVRAM; if they disagree, the root's epoch wins and the non-root must rejoin. |
| Partition | The side without the root cannot confirm a new epoch and must not create a new authoritative home for a mutable object; the side with the root may continue with the existing members. M2-6 owns the lease fencing and the exact availability rule. |
| Stale route | The home answers `MOVED` with the new home/epoch; the sender retries once. M2-3. |

## Alternatives considered

| Alternative | Why it lost |
|---|---|
| 2-of-2 authority | One node failure blocks every membership change and every new epoch; worse availability than 1-of-1 for a two-node gate, and it still has no Byzantine safety. |
| Threshold root now | `cap-engine` has no threshold root or home certificates; it is Phase 8 work and would delay M2. |
| No authority (each node confirms itself) | Produces two authoritative membership views after a partition; violates the M2 partition criterion. |
| Root per object / per home | More complex, more messages, and it still needs a root for the membership view; not needed for M2. |
| A central directory service | Contradicts the "without a central directory" M2 criterion and adds a new service to the data path. |

## Evidence

- The M2-0 rig writes `mesh.authority` and `mesh.epoch` into each guest's NVRAM and records
  the exact environment in the evidence bundle.
- The kernel already derives a stable `NodeId` from the MAC and prints it in `mesh: single
  node`; the rig checks it against the derived identity.
- `mesh-service` host tests cover the membership state machine; `mesh-sim` covers the
  1/2-node policy. M2-2 must add the real two-kernel join and confirmation test.
- The M2-0 report records the 1-of-1 decision and its limitations for the M2 gate review.

## M2-2 implementation note

**Status:** implemented for M2-2; see [M2-ADR-0005](m2-0005-join-epoch.md).

The seam this ADR asked for now exists in `crates/mesh-join`: `AuthorityRoot` is the
root's confirmed `mesh-service` table plus a `cap-engine::SigningAuthority`;
`AuthorityVerifier` checks a confirmation against one configured `NodeKey`.  The
confirmation is a versioned little-endian body carrying the full member list, epoch,
subject, request nonce and incarnation, signed with Ed25519 over a domain-separated
canonical form.  A member rejects a wrong root, a wrong key, a bad signature, a stale
epoch and a same-epoch different list; it never increments the epoch itself.

The 1-of-1 key is a lab fixture in NVRAM (`mesh.key` on the root, `mesh.pub` on the
member), not a production key store.  The module ABI has `config_get` only, so the
confirmed epoch is not persisted; M2-ADR-0005 records the precise member-restart,
root-restart and both-restart rules.  The root-restart case fails closed with
`epoch-regression` rather than lowering a member's confirmed view.
