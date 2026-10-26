# M2-ADR-0004 — Native stackless actors for the M2 runtime

**Status:** accepted for M2
**Date:** 2026-10-26
**Deciders:** M2-0, for review at the M2 gate
**Related:** [M2-ADR-0002](m2-0002-transport.md), [M2-ADR-0003](m2-0003-authority.md),
[`../spec/system.md`](../spec/system.md) §3.2 and §8,
[`../plan/milestones.md`](../plan/milestones.md) Phase 4

## Context

M2-5 must show **L0 migration at a message boundary** in both directions: an actor returns
to its host, its state is serialised and transferred, the destination verifies it and
resumes, and the source is fenced by a lease. The M2 gate also says live TLS, socket and
file-descriptor state is explicitly **not** migrated.

The current kernel has no process or thread model, no user-mode actor host and no kernel
Wasm interpreter. The existing pieces are:

- `agnostic-core` defines the architecture-independent actor record (id, home, host,
  lifecycle, placement epoch), `ActorTable` and `RunQueue`. Its module comment is
  explicit: "The native shim owns instruction contexts; this crate owns the portable
  actor identity, home, host, lifecycle and placement epoch."
- `wasm-host` is a **workstation prototype** for the wasmi host ABI (`i7host`). It is not
  the kernel host. It has fuel metering and a `HostHooks` seam, but no actor state
  serialisation and no migration.
- `modules/wasm` runs a policy probe in slot B on iris; it proves a Rust guest can run
  through the module ABI, not that the kernel can host mobile actors.
- The M1 kernel is single-threaded, n32, big-endian MIPS III. It polls the network,
  modules and console from an idle loop. There is no pre-emption or context switch.

The long-term system architecture describes a Wasm core. Phase 4 integrates wasmi with
fuel metering and starts an echo/scheduler actor, and explicitly says to **keep the native
core option**. M2 is before Phase 4. Adding wasmi to the kernel now would mean a new
runtime, a host ABI, sandbox memory validation, fuel accounting, a Wasm state
serialisation format and a migration protocol before the first actor can move. That is a
larger and riskier dependency than M2 needs.

## Decision

1. **The M2 actor runtime is native and stackless.** An actor is a Rust state machine
   with an explicit message entry point, not a thread and not a captured stack:

   ```text
   trait Actor {
       fn step(&mut self, message: &MessageFrame) -> StepResult;
   }

   enum StepResult {
       Continue,          // more work is runnable without a new message
       Suspend,           // return to the host at a message boundary
       Done,              // actor has exited
       Migrate(MigrationRequest),
   }
   ```

   The actor's state is a concrete Rust value. It is polled from the kernel's run queue
   (or a small runtime loop in a module) one message at a time. `Suspend` is the normal
   return after a message; because there is no stack to capture, "message boundary" is
   the only suspension point by construction. L2 arbitrary-safepoint migration is not
   attempted.

2. **State is serialised through a versioned codec.** M2-5 should define:

   ```text
   trait StateCodec {
       fn encode(&self, out: &mut [u8]) -> Result<usize, CodecError>;
       fn decode(bytes: &[u8]) -> Result<Self, CodecError> where Self: Sized;
       fn schema() -> u32; // bump on any layout change
   }
   ```

   The canonical state bytes are **little-endian** and field-by-field, the same
   discipline as the mesh wire. The state hash is BLAKE3 over the canonical bytes plus the
   schema and actor identity. A destination rejects a state whose schema is unknown or
   whose hash does not match. Do not send a raw `#[repr(C)]` struct: the source and
   destination are both n32 MIPS for M2, but the state format must survive a future
   cross-architecture move (M6).

3. **Migration is an explicit lease protocol, not a copy.** M2-5 implements:

   ```text
   source -> destination: PREPARE(actor, schema, state_len, hash, lease_epoch)
   destination -> source: READY(reservation)
   source -> destination: STATE(bytes)
   destination:           verify schema + hash, allocate the actor
   destination -> source: COMMIT(new_home, new_epoch)
   source:                fence the old instance, release local resources
   ```

   `ABORT` is the failure path at any point before `COMMIT`. The source must not run the
   actor after it sends `STATE`; the destination must not run it before `COMMIT`. A stale
   home answers `MOVED` with the new home/epoch and the sender retries once (M2-3). The
   lease/fencing details belong to M2-5 and M2-6; the M2-0 decision is that the protocol
   is explicit and observable, not a hidden memcpy.

4. **Live resources are excluded.** TLS sessions, open sockets, file descriptors, the
   network stack, timers owned by the host, and kernel module state are **not** part of
   the actor state. They are node-local resources. A migrated actor starts with its
   capability handles re-issued or revalidated at the destination; any resource that
   cannot be revalidated fails closed. This is a deliberate M2 limitation, not a bug.

5. **Capability handles are serialised as authority, not as pointers.** The state codec
   carries the object identity, rights and home/epoch, not a raw kernel pointer or a
   generation-less slot. M2-4's remote capability route and M2-5's migration must agree on
   the handle representation. A handle that is valid on the source but not confirmable at
   the destination is rejected, not silently translated.

6. **The runtime is substrate-neutral at the boundary.** `agnostic-core`'s actor table and
   run queue stay the policy layer; the native `Actor` implementation is the execution
   substrate. A future Wasm actor implements the same `step`/`StateCodec`/migration
   contract, so the join, home, route and lease logic does not need a second copy. The
   M2-5 code should not expose "native" in the wire format or in the policy API.

7. **Wasm is deferred to Phase 4.** The Wasm core remains the long-term direction. The
   Phase 4 exit criteria (wasmi determinism, fuel metering, the guest ABI, host-memory
   validation, an echo/scheduler actor, the MIPS build gate and a kill criterion) are the
   right place to pay that cost. M2 does not block on them, and it does not pretend that
   `modules/wasm` is an actor runtime.

## Consequences

- M2-5 can implement a small, testable runtime and a real L0 migration without a Wasm
  interpreter in the kernel. The actor state is ordinary Rust and can be unit-tested on
  the host.
- Message-boundary suspension is enforced by the ABI: there is no stack to capture, so
  there is no "migrate at an arbitrary safepoint" claim.
- The runtime is native kernel code. A bug can panic the kernel; there is no sandbox and
  no fuel limit. M2 actors must be small, reviewed and tested; the panic screen and
  serial dump are the recovery path. This is a real security and robustness limitation.
- The state codec is a compatibility surface. Every actor state layout change needs a
  schema bump and a migration test. Canonical little-endian encoding is mandatory even
  though both M2 nodes are big-endian, so M6 cross-architecture migration is not a
  rewrite.
- The Wasm core and native core may diverge in behaviour. The shared policy crates
  (`agnostic-core`, `mesh-core`, `mesh-service`, `cap-engine`) are the contract; the
  runtime is an implementation. M2's tests should compare the native actor's state hash
  and observable effects, not its instruction stream.
- Live TLS/socket/fd state is not migrated. A connection owned by an actor is broken by
  migration; the actor must re-establish it or fail closed. This is documented in the M2
  gate and must be visible in the test.

## Alternatives considered

| Alternative | Why it lost |
|---|---|
| Wasm actors now (wasmi in the kernel) | Large runtime, host ABI, sandbox memory, fuel metering and Wasm state serialisation before the first actor moves; Phase 4 work. |
| Wasm actors in `modules/wasm` only | The module is a policy probe, not an actor host; it has no state migration and would put the runtime outside the kernel's scheduling and capability path. |
| Native threads with stack copying | The kernel has no thread/process model, and stack copying at an arbitrary point is L2, which the spec explicitly does not solve. |
| User-mode native actors | No process model, no `posix_spawn`, no thread/TLS; building one is M5 work. |
| No migration in M2 | Violates the M2 gate; M2-5 must demonstrate L0 both ways. |

## Evidence

- `crates/agnostic-core` defines the actor record, `ActorTable` and `RunQueue` with host
  tests; its module comment assigns instruction contexts to the native shim.
- `crates/wasm-host` is explicitly a workstation prototype for the `i7host` surface, not
  the kernel host.
- `modules/wasm` runs on iris in slot B and prints `wasm: host=1` / `wasm: run=42`; it is
  evidence for the module ABI and Wasm guest, not for actor migration.
- The M2-0 rig and ADR-0001 provide the two-kernel path M2-5 will use; the M2-0 report
  records that no actor runtime exists yet.
