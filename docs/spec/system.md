# IRIX 7 — system architecture

**Status:** accepted architecture; implementation in progress
**Version:** 1.0
**Owner:** kernel / mesh
**Related:** [`architecture.md`](architecture.md) (native and compatibility layers),
[`native-abi.md`](native-abi.md), [`native-libc.md`](native-libc.md),
[`elf.md`](elf.md), [`boot-protocol.md`](boot-protocol.md),
[`../plan/milestones.md`](../plan/milestones.md)

## 1. Definition

IRIX 7 is **one logical computer** built from **1 to 128 kernel replicas**.
Each replica is the same architecture-independent Wasm core running on a
different native shim:

```text
one logical computer
  global object space · global namespace · global capability graph
  global actor directory · global device namespace
  global scheduling view = an aggregation, not a queue
        |
  kernel replica 0..127
    same core OID · same shim contract · different native drivers
    per-node run queue · per-node timer wheel · per-node route cache
        |
  Indy MIPS · x86_64 PC · x86_64/AArch64 KVM + VirtIO
```

The single-node path is the base case, not a degraded mode:

- one replica, `home = self`;
- no mesh envelope, quorum, seed node or network requirement;
- every object, actor and device is local;
- the global namespace and capability graph are complete on that node.

The mesh is an extension of the same kernel. There is no separate “cluster
manager”, “remote mount” or application-level RPC service. A capability names
an object; the mesh routes the operation to the object’s home.

“One logical computer” is a **naming and authority** statement, not a claim of
global serialisability.

## 2. Hard invariants

1. **One node works with no network.** Boot, namespace, capabilities, actors,
   CAS, devices and scheduling all work on one replica.
2. **128 nodes is a hard target.** 1, 2, 8 and 128 are test gates.
3. **No global lock.** Locks are per object, per home and per node.
4. **No global run queue.** Each node schedules locally; the global view is an
   aggregate of node summaries.
5. **No all-to-all event traffic per operation.** Data-path operations go to
   the object or actor home in at most two hops in the normal case.
6. **Homes are sharded.** Rendezvous hashing over `ObjectId`/`ActorId` with a
   membership view; no central object directory.
7. **Mutable authority is lease-fenced.** Stale homes, actor hosts and routes
   cannot commit after a lease expires or a higher epoch is observed.
8. **Consensus only for membership and root authority.** The normal data path
   never waits for a quorum; a partition can make an object unavailable, not
   create two authoritative homes.
9. **Capabilities route without a directory.** A token carries the object,
   issuer, home, home epoch, object epoch and root epoch; a route hint is
   cached and stale routes answer `MOVED`.
10. **The single-node path is tested first at every gate.**

## 3. Layers and contracts

### 3.1 Native shim

The shim owns the hardware and the TCB: boot, MMU/domain, physical timer,
interrupts, DMA, NIC, block, console, entropy, reset and the local device
namespace. Its safe Rust contract is `crates/platform-api`.

The current Indy shim is the existing assembly ROM plus the native n32 kernel.
The architecture extracts the hardware-facing pieces behind `PlatformShim`; the ROM
remains assembly and the native POSIX/IRIX paths remain available.

### 3.2 Wasm core

The same core image runs on every node. It owns scheduling policy, the
namespace, the capability engine, the actor directory, the global placement
view and the service graph. It never sees native pointers, Rust trait objects
or device registers; the shim exposes a small `i7host` import surface
([`../plan/milestones.md`](../plan/milestones.md)).

The v1 core runs in kernel mode on the Indy via a deterministic wasmi
interpreter. A user-mode interpreter is a later PC/cloud option; it is an open
security decision, not a silent assumption.

### 3.3 Fabric

- `MessageFrame`: 128-byte header, 64-byte payload descriptor, optional inline
  body. Tags, streams, deadlines, dependency edges, cancellation, flow control
  and W3C Trace Context are protocol properties.
- `MeshHeader`: 128-byte source/destination/home/route and epoch envelope for
  inter-node transport. The single-node path does not construct one.
- Local and remote operations are asynchronous completions. Bulk data uses
  CAS OIDs, shared-memory rings or DMA-buf-style native handoffs; Wasm actors
  receive metadata, not host pointers.

### 3.4 Native and compatibility surfaces

The following remain native and are not moved into Wasm:

- POSIX syscalls and the relibc/native-libc fast path;
- the IRIX 6.5 o32/n32 compatibility container;
- the ABI 10 loadable-module interface;
- NIC, block, MMU, timer and DMA drivers.

The Wasm core manages namespace/capability policy and actor placement. It does
not reinterpret `read(2)`, `fork(2)` or IRIX binaries. A POSIX bridge resolves a
path through the global namespace once and then uses the native data path.

## 4. Object and capability model

- **ObjectId** is a 256-bit BLAKE3 digest. Immutable objects are
  content-addressed. Mutable objects have a stable identity plus an immutable
  version state; the home owns the mutable root record.
- **Capability** is a 320-byte token carrying object, issuer, subject, rights
  (`READ`, `WRITE`, `EXECUTE`, `INVOKE`), home, home epoch, object epoch, root
  epoch, delegation, validity and an Ed25519 signature.
- **Trust** is a threshold root (`1-of-1` single node, `2-of-2` pair,
  `3-of-5` or `5-of-7` larger meshes) with home certificates delegating
  issuance. The root threshold is a trust anchor, not a single key holder; root
  collusion and partition windows are documented security limitations.
- **Revocation** uses object epochs, bounded token revocation sets and short
  validity windows. Global revocation is not instantaneous on a partitioned
  node.

## 5. Mesh and homes

A membership view is an immutable, epoch-tagged sorted list of at most 128
node IDs. A single node uses `home = self`. For a mesh:

```text
home = argmax over nodes n of BLAKE3-64("IRIX7-HOME-v1" || object || n)
```

One virtual node per physical node keeps a cold lookup to one BLAKE3 hash per
node; route caches absorb small-set tails. Membership changes move only the
objects whose winner changes.

- A mutable object has exactly one home at a time.
- A home move uses `DRAINING`, a lease, a new `home_epoch` and a forwarding
  grace period; stale routes return `MOVED` and the sender retries once.
- Actor homes are computed in the same rendezvous space and choose an actor
  host from aggregated `NodeSummary` placement data.
- Membership uses SWIM-style failure detection and a bounded gossip fan-out;
  a join/leave epoch is confirmed by the authority root. Failed nodes are
  removed from the routing snapshot.

## 6. Storage

- OID = `BLAKE3-256("IRIX7-OBJ-v1" || kind || length_le64 || payload)`.
- Chunks use FastCDC; manifests are canonical little-endian records and are
  themselves content-addressed.
- Immutable objects are fetched by OID from any replica, with verified
  streaming and pinning. Mutable roots live at their home and refer to
  immutable state OIDs.
- The global namespace is a tree of directory objects. Every directory has a
  home; path lookup follows directory entries and is not a global snapshot.
  Cross-home rename is `EXDEV`/`NotSupported` in v1 rather than a hidden global
  transaction. A directory object is canonical little-endian data: the magic
  `I7DR`, version 1, the `Tree` kind, a reserved zero halfword, a 32-bit entry
  count, then sorted entries of `u16` name length, a reserved zero halfword,
  the target OID and a UTF-8 name of 1–255 bytes. Names are non-empty, cannot
  be `.`/`..` or contain `/` or NUL, and the decoder rejects unsorted or
  non-canonical records. The target's kind comes from the CAS record, not the
  directory entry.
- Replication defaults are policy: boot-critical images two copies, actor
  snapshots three during migration, ordinary blobs one. GC is home-owned and
  pin-aware.

## 7. Scheduling and devices

- Per-node run queues, timer wheel and completion engine; no central scheduler.
- EDF with Constant Bandwidth Server admission for deadline actors, a weighted
  fair queue for the rest. A small reserve prevents deadline starvation.
- Placement is actor-home policy over aggregated node summaries. A remote
  placement decision is asynchronous; the local node never waits for a remote
  scheduler.
- Devices are global objects with homes. `GpuDevice` is a normal example: the
  Indy resolves the object, submits offscreen command buffers/fences over the
  fabric, and receives CAS/compressed results. It never maps remote base-address registers.
  An actor may instead migrate to the GPU node.

## 8. Migration and endianness

v1 supports **L0 migration at message boundaries**: an actor returns to the
host, its linear memory, globals, tables, metadata and capability handles are
serialised, the state is verified and transferred, and a `PREPARE/STATE/
COMMIT/ABORT` lease protocol fences the old instance. TLS sessions, open
sockets and file descriptors are not transparently migrated; they are
node-local resources.

L2 arbitrary-safepoint migration is **not solved by wasmi** and is a separate
runtime project. That limitation is explicit.

All wire and state metadata is little-endian. Wasm linear-memory bytes are
never blanket-swapped when moving between big-endian MIPS and little-endian
x86_64/AArch64; the module sees the same bytes. Pointers are rebased; globals
and tables are decoded by type; canonical state hashes and section lengths
catch corruption. See the endian matrix in `docs/plan/milestones.md`.

## 9. Gate summary

| Gate | Nodes | Required |
|---|---|---|
| M1 | 1 | Boot without network/quorum/seed; local namespace, CAS, caps, actors, devices, POSIX/IRIX smoke |
| M2 | 2 | Join, epoch convergence, remote capability route, L0 migration both ways, one-node failure/partition |
| M3 | 8 | Sharded homes, CAS fetch, placement, two-node failure, membership churn, GPU object path |
| M4 | 128 | 128 replicas, ≤2 hops normal, no global queue/lock/all-to-all, eight-node failure, churn, 1M OIDs, 10k actors |

The 128-node gate may use a simulator plus QEMU, but 1/2-node gates must run on
real kernels.

## 10. Non-goals and open decisions

Non-goals for v1:

- global serialisability or a single shared address space;
- a central scheduler, central object directory or global lock;
- transparent migration of live TLS/socket/fd state;
- running IRIX binaries inside Wasm;
- a universal GPU driver on arbitrary hardware;
- RDMA/DMA-buf zero-copy into Wasm actor memory.

Open decisions:

- L2 migration as a v1 requirement (currently no).
- Wasm core in kernel mode versus a user-mode sandbox on PC/cloud.
- Root-key custody on machines without a TPM.
- Whether the POSIX fd table is a permanent compatibility alias or transitional.
- Exact hardware support matrix for “any computer”; see the release plan.
