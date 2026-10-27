# M2-ADR-0009 — L0 stackless-actor migration at a message boundary

**Status:** accepted for M2, limited
**Date:** 2026-10-27
**Deciders:** M2-5, for review at the M2 gate
**Related:** [M2-ADR-0004](m2-0004-actor-runtime.md), [M2-ADR-0005](m2-0005-join-epoch.md),
[M2-ADR-0006](m2-0006-homes.md), [M2-ADR-0007](m2-0007-route.md),
[M2-ADR-0008](m2-0008-failure-fence.md), [`../spec/system.md`](../spec/system.md) §5 and §8,
[`../plan/milestones.md`](../plan/milestones.md) §2, [`../plan/m2-rig.md`](../plan/m2-rig.md)

## Context

The M2 gate is the last routing item after join, homes, the remote capability route and
the failure/partition rule: **L0 migration at a message boundary, both directions, on two
real kernels**. M2-ADR-0004 chose native stackless actors and an explicit
`PREPARE/READY/STATE/COMMIT/ABORT` lease protocol, but no actor state codec, migration
wire body or kernel evidence existed.

The existing pieces are:

- `agnostic-core` owns the portable actor record (`id`, `home`, `host`, lifecycle,
  placement epoch), the actor table and the run queue. It has no state payload, codec or
  migration transport.
- `mesh-join` owns the authority-confirmed membership snapshot and the root signing seam.
- `mesh-core` owns the rendezvous actor home and the bounded route cache.
- `mesh-route` owns the `I7R1` capability/lease protocol, the `cap-engine` checks and the
  one-`MOVED` retry. It is about objects, not actors.
- The MCP module owns the M2-1 framing, the HTTP evidence surface and the two TCP slots.
- The kernel is single-threaded n32 MIPS III. There is no thread, process, TLS server or
  file-descriptor table to migrate.

## Decision

### 1. A separate `mesh-migrate` policy crate

`crates/mesh-migrate` owns the portable L0 actor state, its canonical codec and hash, the
root-signed `ActorMove` directive, the `I7M1` bodies and the migration state machine. It
is `no_std`, host-tested and has no socket or persistent store. The MCP module owns the
frame loop and the evidence surface; `agnostic-core` remains the portable actor directory
and placement policy.

The actor is a concrete Rust value polled one message at a time. There is no stack to
capture, so the only suspension point is a message boundary. L2 arbitrary-safepoint
migration is not attempted.

### 2. Deterministic identity, state and hash

The fixture actor is derived, not stored:

```text
actor[i]     = BLAKE3-128("IRIX7-M2-ACTOR-v1" || i u32 LE)
home         = mesh_core::actor_home(actor, confirmed view)
object       = BLAKE3-256("IRIX7-M2-ACTOR-OBJECT-v1" || actor)
principal    = BLAKE3-256("IRIX7-M2-ACTOR-PRINCIPAL-v1" || actor)
initial      = BLAKE3-256("IRIX7-M2-ACTOR-INIT-v1" || actor)
step(prev,m) = BLAKE3-256("IRIX7-M2-ACTOR-STEP-v1" || prev || id u64 LE || payload)
payload[i]   = BLAKE3-256("IRIX7-M2-ACTOR-MSG-v1" || i u32 LE)
```

The canonical state bytes are fixed-length, field-by-field little-endian, schema 1. They
carry the actor id, rendezvous home, current host, placement epoch, applied count, highest
message id, logical digest, at most one queued message, at most one unacknowledged message
and at most one serialised capability handle. A truncated, over-long, unknown-schema or
non-canonical absent field is rejected; the bound is 512 bytes (the encoded state is 248).

The state hash is BLAKE3 over schema, actor, home, applied count, highest id, logical
digest, queued message, unacknowledged message and capability handle. **Placement (`host`
and `epoch`) is deliberately excluded** because a move changes it; the logical state must
be identical at the source and destination. The authoritative full hash including
placement would therefore not be a useful source/destination equality check. Placement is
reported separately.

### 3. Root-signed move and the capability boundary

A move is authorised by a fixed `ActorMove` directive signed with the configured authority
root's Ed25519 key. It names the actor, home, old host, new host, confirmed membership
epoch and digest, old/new placement epoch, schema and lease. The signature is verified
before any field is trusted. Both ends check:

- the membership epoch and digest equal the local confirmed view;
- the actor's home, old host and new host are confirmed members;
- the old host is the source and the new host is the destination;
- the placement epoch advances by exactly one and the lease is bounded.

A delivery carries a root-issued `cap-engine` capability. The subject is the actor
principal, the object is the actor object, `INVOKE` is required, and the capability's
home/home epoch/object epoch must equal the current placement. A tampered or stale token is
refused before the actor advances.

The serialised capability handle is authority, not a pointer or a file descriptor. The
destination validates its shape, object, rights, home and epochs; it does not pretend to
migrate a kernel handle.

### 4. The lease protocol

```text
root -> source:        AUTHORISE(ActorMove)          (or the root is the source)
source:                freeze the actor; no further step
source -> destination: PREPARE(directive, state_len, hash)
destination -> source: READY(directive, hash)
source -> destination: STATE(directive, canonical bytes)
destination:           decode, verify schema/length/hash/identity/epochs
destination -> source: COMMIT(directive, hash)
source:                fence the old instance, discard the live state
source -> destination: ACK(directive, hash)
destination:           activate the actor
```

The source must not run the actor after it sends `STATE`; the destination must not serve a
delivery until it activates on `ACK`. A step that exceeds the 10 s migration deadline is
aborted; a source abort restores the frozen actor at its old placement. `ABORT` is the
terminal failure path before commit. The MCP module handles one frame at a time, so no
delivery is interleaved between `STATE` and the `COMMIT` response.

The old host keeps a fence record after the move: the actor id, old/new host and epoch, and
the canonical state hash. A delivery naming the old host or an old epoch is refused with
`stale-epoch`/`stale-home`/`not-home`; it never invents a redirect. The new host owns the
only live instance.

### 5. Queued and unacknowledged messages

The actor has one queue slot and one acknowledgement slot. A message can be delivered in
`step` mode (apply now when idle) or `queue` mode (leave it at the next boundary). A
message id at or below the highest applied id is an idempotent duplicate: it returns the
current state and does not advance. This makes a retransmit after a lost `RESULT` safe and
is observable in the report as `duplicates`.

The rig's scenario stops after message 10 with message 11 queued and message 10
unacknowledged, moves the actor, resumes message 11 at the destination, queues message 12,
moves the actor back, resumes message 12 and duplicate-delivers at both hosts. The source
and destination state hashes are compared at each move boundary, and the rig recomputes
the hash, logical chain, queued/unacknowledged messages and capability handle
independently of the module.

### 6. Boundaries and non-goals

- No live TLS session, socket, file descriptor, thread, process, timer or kernel module
  state is migrated.
- No L2 arbitrary-safepoint migration, no general actor scheduler, no actor image or
  object migration and no persistent migration directory.
- The actor state is small and native; a bug can panic the module/kernel. There is no
  sandbox or fuel limit in M2.
- `mesh.route=2` is the opt-in. `mesh.route=1` keeps the M2-4 route and disables
  migration; absent keeps M2-3; `--fault no-peer`, `no-route` and `wrong-authority` keep
  their earlier markers. No new NVRAM key is needed, so the full route+failure
  environment still fits the 298-byte DS1386 area.
- The MCP module remains inside the 512 KiB slot A.

## Consequences

- The M2 gate now has a real two-kernel L0 migration in both directions with one actor
  identity and one message-boundary state hash.
- The state codec is a compatibility surface. A layout change bumps the schema and needs a
  migration test.
- The old host's fence is local and immediate at `COMMIT`; a delayed duplicate is refused
  by the epoch/home check. A lost `ACK` after `COMMIT` leaves the destination provisional
  and the source fenced; the bounded deadline reports an abort rather than silently
  running two instances.
- The authority remains 1-of-1 and the transport is plain TCP. A host attacker can still
  observe or inject bytes; migration does not add transport authentication.
- The fixture actor's placement is chosen by the rig, not by a scheduler. A general
  placement policy and live resource re-establishment remain later work.

## Alternatives considered

| Alternative | Why it lost |
|---|---|
| Extend `mesh-route` with actor state | Mixes object routing and actor lifecycle; a second policy would share the route report and leases. |
| Migrate a thread or process | The kernel has no thread/process model, and L2 stack capture is explicitly out of scope. |
| Wasm state migration now | Phase 4 runtime, host ABI and fuel metering are not available; native stackless actors are the M2 substrate. |
| Unsigned migration directive | A move changes placement authority; it must be root-signed and view-scoped. |
| A central migration directory | Contradicts the no-directory invariant and duplicates the confirmed view. |
| Reuse the object `MOVED` body | Actors have identity, queue/ack and capability-handle state; the `I7R1` object lease body does not carry it. |
| Persist actor state between boots | No writable module store in M2; the actor is in memory for one boot. |

## Evidence

Host tests in `crates/mesh-migrate`:

- canonical state round-trip, truncation, over-length, unknown schema and non-canonical
  absent fields;
- placement-independent hash equality and deterministic message chain;
- one-way and return migration with the same source/destination state hash, queued and
  unacknowledged messages and capability handle;
- stale old-home refusal after each move;
- idempotent duplicate delivery without advancing the state;
- queue/resume at the message boundary;
- malformed state and hash mismatch rejected before installation;
- tampered capability and wrong-authority signature rejected.

Real kernels (`tools/m2-rig run` with `mesh.route=2`):

```text
migration [none]: pass (actor 759FD61E30F4DFA28311542283B188BF moved
  08006995F50000000000000000000000 -> 08006969B60100000000000000000000 ->
  08006995F50000000000000000000000; boundary hash
  246A7909F572ED1185DF45ECBB0C2B56E87426BBEDA3607233F9880B4D3768A0 returned as
  C0C3BD1275EE5C78619F4F2352AF24FEAED989947F4C2F8BDD99EFD55CA967FC)
```

The report records both nodes' actor rows, the move rows, counters (`migrations=2,
incoming=1, outgoing=1` each), the stale refusals (one local at the old root, one wire at
the old member) and the capability refusal. `--fault no-peer`, `no-route`,
`wrong-authority`, `kill-peer` and `partition` still pass their earlier markers; the
migration endpoint returns `404 disabled` with `reason=peer|config`.

## Limits

- No general actor scheduler or placement policy; the rig chooses the hosts and messages.
- No live resources, no L2 safepoints, no persistent state or migration directory.
- One actor and two nodes. Multi-actor/multi-hop migration and gossip remain M3 work; the
  local signed epoch store is M2-ADR-0005 and is not a replicated epoch log.
- The lab authority and transport limitations of M2-ADR-0002/0003 still apply.
