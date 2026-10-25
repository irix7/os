# IRIX 7

IRIX 7 is a Rust-native operating system that runs existing IRIX 6.5
application binaries through a legacy ABI layer. It inherits the IRIX 6.5
compatibility mandate without forward compatibility: IRIX 6.5 applications run
on IRIX 7, but IRIX 7-native binaries are not promised on IRIX 6.5.

The first hardware target is the SGI Indy (IP22, MIPS III); x86_64 PCs and
KVM/VirtIO cloud machines follow. The design target is bigger than a single
workstation: **one logical computer built from 1 to 128 kernel replicas**.

Each replica runs the same architecture-independent Wasm core on a
target-specific native shim. Objects, actors, capabilities, namespaces,
devices and scheduling are global; the mesh has no central directory, global
lock or global run queue. A single node is the base case and works with no
network, quorum or seed.

This is the “network is the computer” architecture: installing another node
adds its CPU, memory, storage and devices to the capability graph. The Indy
can resolve a GPU object on a modern node, or an actor can migrate to that
node, without an application-level remote-mount or RPC protocol.

> **The Tree of IRIX 7** — [irix7.github.io/os/tree](https://irix7.github.io/os/tree/)
> is the living roadmap. The implementation plan is in
> [`docs/plan/milestones.md`](docs/plan/milestones.md).

## Architecture

- **Native shims** own the hardware and TCB: boot firmware, MMU, interrupts,
  timers, DMA, NIC, block, console and the local device namespace. The Indy
  keeps its assembly ROM and native n32 kernel during bring-up; the PC and
  cloud shims are Limine/UEFI + ACPI/VirtIO.
- **One Wasm core** owns scheduling policy, the global namespace, the
  capability engine, the actor directory and placement. The core image has one
  BLAKE3 OID and is loaded by every shim.
- **One asynchronous fabric** connects local and remote operations. A
  `MessageFrame` carries tags, streams, deadlines, dependency edges, flow
  control and W3C Trace Context; a `MeshHeader` adds source/destination, home,
  route hints and epochs. Local calls bypass the mesh envelope.
- **Global object-capabilities.** A 320-byte capability names an object, its
  home, rights (`READ | WRITE | EXECUTE | INVOKE`), epochs and an Ed25519
  signature. Authority is a threshold root, not a single key holder.
- **Content-addressed storage.** Immutable objects and actor state are BLAKE3
  OIDs; manifests use FastCDC chunks. Mutable roots are owned by one home under
  a lease. The namespace is a global tree of directory objects, not a set of
  remote mounts.
- **1–128 nodes.** Homes are computed by rendezvous hashing over an immutable
  membership view; failed nodes leave the routing snapshot; stale routes
  answer `MOVED` and are fenced by epochs. The 128-node target has bounded
  gossip and at most two normal forwarding hops.
- **Devices are global objects.** `GpuDevice`, `BlockDevice` and `NetDevice`
  live in the namespace. The Indy submits offscreen GPU command buffers and
  receives CAS/compressed results; no remote BAR mapping or fake local-bus
  coherency is assumed.
- **Migration is explicit.** v1 supports stackless actor migration at message
  boundaries. Arbitrary-safepoint migration is a later runtime project.
  Wire and state metadata is little-endian; MIPS/Wasm conversion is canonical,
  never a blanket byte swap of linear memory.

## Compatibility

The architecture does not throw away the current system:

- POSIX/relibc and the IRIX 6.5 compatibility container stay native. The Wasm
  core manages policy and namespace, not their instruction streams.
- The ABI 10 loadable-module interface stays native and separate from the new
  Wasm ABI.
- The Indy assembly ROM, Seeq/SCSI drivers and native n32 POSIX image remain the
  bring-up and recovery path.
- IRIX 6.5 binaries cannot run inside the Wasm interpreter.
- “Any computer” means a supported node matrix, not a universal driver claim.
  The first cloud/PC targets are VirtIO, Limine/UEFI and a conservative hardware
  set; AMD GPU support starts through a hosted/node service, not an immediate
  bare-metal GPU stack.

The normative system specification is
[`docs/spec/system.md`](docs/spec/system.md). The native architecture and
compatibility layer remain in
[`docs/spec/architecture.md`](docs/spec/architecture.md).

## Current status

**Released:** v0.1.2 is the current native Indy ROM. It boots the assembly ROM,
tests DRAM, decompresses the n32 kernel, brings up Seeq networking, DHCP/DNS/
NTP, HTTP/HTTPS, the REX3 boot log, telnet and HTTPS module autoload, then
enters the static Rust `std` image on SCSI target 2.

**Native bring-up:** the `std` image exercises args, time, sleep, a small
read/write ramfs, `stat`, `read_dir`, `chdir` and relative paths through the 42-
symbol C shim. The kernel stack top is `0x88300000`; the loader uses cacheable
user pages, real 256 KiB PageMask pairs, a 64-bit trap frame and LL-bit
clearing. There is no process table, threads/TLS, `posix_spawn`, signal
delivery, termios or socket API yet.

**Architecture foundation merged on `main`:**

- `sys-primitives` — ObjectId, NodeId, capability v2, MessageFrame,
  MeshHeader/Envelope, completions and trace vocabulary; wire tests.
- `mesh-core` — BLAKE3 rendezvous homes, bounded route cache, leases and
  GPU-aware placement; 1/128-node tests.
- `mesh-service` — membership, join/leave epochs, incarnation heartbeats,
  suspect/failed state and deterministic snapshots.
- `mesh-sim` — 1/2/8/128-node host gates for load bounds, deterministic homes
  and failed-node exclusion.
- `fabric` — tag allocation, stream flow control, cancellation and bounded
  completion/wait dependencies.
- `guest-abi` — OS re-export of the SDK `wasm-abi` v0.5.0 host-call surface.
- `cas-core` — BLAKE3 OIDs, canonical chunk manifests, directory objects and
  verification.
- `cas-store` — host-checkable put/fetch/pin policy and a single-node namespace
  tree over immutable directory objects.
- `agnostic-core` — portable actor identity, home/host placement and lifecycle.
- `cap-engine` — Ed25519 trust store, capability signing/verification, threshold
  root manifests and signed home certificates.
- `wasm-host` — deterministic wasmi lifecycle prototype with fuel metering and
  the `i7host` import surface; it runs a WAT module, not the kernel core.
- `logical` module — runs the local namespace/CAS, actor directory/run queue
  and a verified 1-of-1 capability on iris, outside the compressed PROM.

The kernel now initialises the one-node membership view from the DS1386 MAC and
resolves local object homes through the shared rendezvous policy. A first Indy
`PlatformShim` slice covers physical pages, the kseg0 direct map, DMA cache
maintenance, console/keyboard, the DS1386 clock and boot entropy; asynchronous
device submission still belongs to the polled native path. The single-node
policy has a combined host gate and now runs on iris in the loadable `logical`
module, including a verified local capability; the CI boot gate loads it from
local SCSI with `net=off`, so the single-node path has no network dependency.
The kernel itself does not start the policy yet, and it does not run the Wasm
core or a mesh; M1 is partial and M2 is still ahead.

The SDK is at [irix7/os-sdk](https://github.com/irix7/os-sdk): module ABI 10,
user SDK and `wasm-abi` v1. The production POSIX layer is the
[relibc fork](https://github.com/irix7/relibc); its n32 Linux/QEMU gate passes,
while the native PAL, error channel, TLS and `stat` ABI remain open.

## Plans and documentation

- [Milestone plan](docs/plan/milestones.md) — phases, 1/2/8/128 gates, rollback.
- [System architecture](docs/spec/system.md) — the logical computer.
- [Install guide](docs/install.md), [testing](docs/plan/testing.md),
  [interfaces](docs/plan/interfaces.md), [risks](docs/plan/risks.md).
- [Research](docs/research/) — the evidence base, including the Wasm/mesh
  system design and the native bring-up reports.

## Licence

GPL-3.0.
