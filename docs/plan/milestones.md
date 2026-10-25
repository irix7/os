# IRIX 7 — Milestone Plan

**Status:** active plan
**Architecture:** [`../spec/system.md`](../spec/system.md)
**Strategy:** one logical computer built from 1–128 kernel replicas. Every
phase is a vertical slice with a runnable demo and a hard gate.

## 1. Current state

The native Indy bring-up is the base: the assembly ROM tests DRAM and starts
the n32 kernel, which owns Seeq networking, DHCP/DNS/NTP, HTTP/HTTPS, the REX3
boot log, telnet, HTTPS module autoload and the static Rust `std` image on
SCSI target 2. The released image is v0.1.3.

The shared architecture crates are merged and host-tested:
`sys-primitives`, `mesh-core`, `mesh-service`, `fabric`, `cas-core`,
`cap-engine`, `guest-abi`, `wasm-host` and `mesh-sim`. The SDK owns
`wasm-abi` v0.5.0 and the user SDK v0.4.0. The kernel does not yet run the Wasm
core or join a mesh; those are the first two gates.

Native POSIX/relibc, the IRIX 6.5 container and ABI 10 modules remain native
compatibility surfaces throughout.

## 2. Hard gates

| Gate | Nodes | What must pass |
|---|---|---|
| M1 | 1 | Boot with no network, quorum or seed; zero mesh RPCs; local namespace, CAS, capabilities, actors and devices; POSIX/IRIX smoke |
| M2 | 2 | Join, membership epoch, rendezvous homes, remote capability route, L0 migration both ways, one-node failure and partition |
| M3 | 8 | Sharded homes, namespace/CAS fetch-by-OID, placement, two-node failure, membership churn, GPU object path |
| M4 | 128 | 128 replicas, ≤2 normal hops, no global queue/lock/all-to-all, eight-node failure, 64/64 partition, churn, 1M OIDs, 10k actors |

M1 and M2 must run on real kernels. M4 may use a simulator plus QEMU, but the
simulator must use the same `mesh-core` and `mesh-service` policy as the
kernel.

## 3. Phases

### Phase 0 — Baseline and decisions

Record boot, DHCP/DNS/NTP, TLS, HTTP, telnet, MCP, module-load and user-image
baselines; image/RAM budgets; the compatibility contract; and ADRs for the
two-stage kernel, Wasm runtime, capability v2, migration level, global kernel,
home ownership, threshold authority and the 1/2/8/128 gates. Build a
traceability matrix. No production code change.

**Exit:** baseline recorded; ADRs accepted or explicitly deferred; every
requirement mapped to a phase and a test.

### Phase 1 — Shared vocabulary and workspace

Add `sys-primitives`, `mesh-core`, `platform-api`, `fabric`, `guest-abi`,
`cas-core`, `cap-engine`, `mesh-service`, `wasm-host` and `mesh-sim`.
`os-sdk/wasm-abi` is the single guest ABI.

**Status (2026-10-25):** merged. `sys-primitives` has wire round-trips;
`mesh-core` uses BLAKE3 rendezvous, leases and GPU placement; `mesh-service`
covers membership and failure state; `fabric` covers tags, streams, cancellation and wait dependencies; `cas-core`
covers OIDs/manifests and `cas-store` covers put/pin policy; `cap-engine`
covers Ed25519, trust, threshold roots and home certificates; `agnostic-core`
covers actor home/host/lifecycle; `wasm-host` runs an `i7host` WAT module;
`mesh-sim` passes 1/2/8/128 policy gates. `cas-core` now defines the canonical
directory object and `cas-store` resolves a local namespace tree over it. The
native kernel also initialises the one-node membership view from the DS1386 MAC
and exercises local home policy; it does not yet construct a mesh frame.

**Exit:** native image boots; new crates host-tested and wasm32-checked where
applicable; clippy/deny clean; no existing public API changes.

The native compatibility workstream now has a minimal o32 loader/syscall probe:
`user/o32-hello` is loaded with `EF_MIPS_ABI_O32`, calls classic `open(5)`,
`read(3)`, `write(4)`, `close(6)` and `exit(1)`, and is CI-booted alongside the
local logical module.  It is not an IRIX 6.5 binary; the
libc/`rld`/`sproc`/signal container is still ahead.

### Phase 2 — Indy shim and single-node kernel

Implement `PlatformShim` for the Indy behind the existing kernel: pages, MMU,
timer, IRQ, Seeq, SCSI, DMA, console, RNG and reset. Keep the old path
selectable. Add the single-node `MembershipView::single` path and exercise the
global object, namespace, capability and actor code with no mesh envelope.

**Status (2026-10-25):** the first shim slice is in the kernel: physical frames,
the kseg0 direct map, DMA cache maintenance, the polled serial/graphics console
and keyboard, the DS1386 clock, SHA-256-seeded boot entropy and reset. A boot
self-test checks page allocation, identity DMA, entropy and the capability
flags. Device submission, general IRQ delivery and one-shot timers still return
`NotSupported`; the polled Seeq/SCSI path remains authoritative until the
completion engine lands. Membership, namespace/CAS, capability and actor policy
now meet in the local host gate (`mesh-sim/tests/single_node.rs`) and in the
loadable `logical` module, which autoloads on iris, prints its root/motd/actor
and verified 1-of-1 capability, and ticks three times. The CI boot gate now
loads that module from the local SCSI target through an `mcp.local` NVRAM record
with `net=off`, so the boot uses no network, and checks the policy and ticks on
every pull request. The static Rust `std` image also boots in that no-network
local-module configuration and exits 0; the CI boot gate uses the smaller native
hello until the std image is a build artifact. The kernel itself does not start
that policy yet.

**PROM budget finding:** the first shim slice left the compressed PROM at
494,140 of 524,288 bytes. A trial that linked the namespace, CAS store and actor
directory directly into the kernel produced 522,248 bytes, leaving only 2,040
bytes of headroom, so it was not merged. The current console/sash plus EROFS
Range image is 520,840 bytes (3,448 free). Runtime policy, modules and userland
software must ship in the package index or a separate core image; they cannot
grow the fixed bootstrap.

The optional SCSI write cache is deliberately a module, not PROM code: verified
downloads wait in a bounded RAM queue, and the module drains it by SHA-256 into
a cache-owned range during its idle tick.  It lands after the network/RAM path
is stable and needs an appended `block_write` ABI service plus a module
multiplexer while the module area has one slot.

**Exit:** M1. Indy boots to `BOOT-READY`; network/telnet/MCP/module autoload
and POSIX image unchanged; no mesh RPCs; no regression beyond the 5%/1 ms
budgets.

### Phase 3 — Asynchronous fabric and two-node routing

Implement tag/stream flow control, completions, cancellation, W3C Trace
Context, shared-memory local transport and TCP/TLS transport. Add the
`MeshHeader`, route cache, `MOVED` handling, membership v0 and remote
capability routing. Keep 9P2000.L as an edge gateway.

**Exit:** M2 routing; cancellation races, 10k tags, flow-control exhaustion,
trace propagation, v9fs/Plan 9 gateway; MCP works through both paths.

### Phase 4 — Wasm host and minimal core on the Indy

Integrate wasmi deterministically with fuel metering, the guest ABI, host
memory validation and the completion ring. Start an echo/scheduler actor. Add
the MIPS build gate and a kill criterion for runtime or performance failure;
keep the native core option.

**Exit:** core starts on iris; actor suite; deterministic corpus; native
POSIX/IRIX paths still pass; no packet or block data path inside Wasm.

### Phase 5 — Global namespace, CAS and actor lifecycle

Move namespace and capability policy into the core; implement CAS fetch,
manifests, cache/pinning, L0 migration, per-node run queues and placement. Add
the POSIX bridge and ABI 10 bridge.

**Exit:** M2 full; M3 sharded homes, CAS fetch, placement, two-node failure;
L0 migration both ways; compatibility suite.

### Phase 6 — x86_64 PC shim

Limine/UEFI boot, ACPI/APIC, e1000e, AHCI/NVMe, framebuffer and serial. Reuse
the same core OID and host. Add a PC smoke image and Indy↔PC migration.

**Exit:** PC boots the core; DHCP/HTTP/block; cross-endian state hash; e1000e
throughput; Indy unchanged.

### Phase 7 — Cloud VirtIO and 128-node gate

virtio-net/blk/vsock, ACPI/DT, GIC/PSCI; 128-node simulator with fault and
churn injection; placement and authority scale tests; M4 instrumentation for
hops, mesh RPCs, gossip bytes and lease rejects.

**Exit:** cloud x86_64/AArch64 boots; cross-architecture migration; M3; M4
policy and real-mesh limitations recorded.

### Phase 8 — CAS, threshold authority and revocation

Make CAS primary for actor images/state and boot manifests; implement
threshold root manifests, home certificates, trust store, revocation epochs,
bounded revocation sets, key rotation and partition windows. Keep EROFS as a
read-only oracle during migration.

**Exit:** forge/replay/expiry/revocation fail closed; crash-consistency and GC
tests; ABI 10 still works; M1–M4 security gates.

### Phase 9 — Remove duplicate paths

Remove the polled monolith, old scheduler stub, duplicate driver paths and old
HTTP/MCP server where the fabric gateway replaces them. Keep POSIX/IRIX and
the recovery console.

**Exit:** no feature flags; all targets boot from the same core OID;
compatibility suites green; rollback drill.

## 4. Native compatibility workstream

This work continues in parallel and is a gate on every phase:

- native Indy ROM, Seeq, SCSI, REX3 and DS1386;
- POSIX/relibc, the 42-symbol bring-up shim and the IRIX 6.5 container;
- ABI 10 modules and the user SDK;
- EROFS/HTTP Range and the recovery console (the clean-room `sash` on telnet
  and the local keyboard/display).

A phase that breaks these does not pass, even if its Wasm demo works.

## 5. Workstreams

- **Runtime:** wasmi, determinism, fuel, MIPS performance, L2 research.
- **Mesh:** homes, routing, membership, placement, authority, 128-node test.
- **Storage:** CAS, manifests, replication, pinning, GC.
- **Devices:** Seeq, SCSI, e1000e, AHCI/NVMe, virtio, GPU object protocol.
- **Compatibility:** POSIX/relibc, IRIX 6.5, ABI 10.
- **Fabric:** frames, tags, completions, cancellation, trace, 9P gateway.
- **Toolchain/SDK:** `wasm-abi`, user SDK, core image, build/image tooling.

## 6. Budget and release

- **New effort:** 60–95 person-weeks on top of the native compatibility work.
- The Wasm runtime/migration and hardware shims dominate, not the frame format.
- v1 is L0 migration, per-object linearisability, sharded homes and threshold
  authority. L2 migration, home groups and multi-object transactions are v2.
- The native v0.1.3 image remains the last shipped release until M1/M2 pass.
- A release manifest must state native-only, single-node or mesh-capable; it
  must name the supported hardware matrix.

## 7. Definition of done

A phase is done when its demo and gate run on the target hardware or documented
simulator, the compatibility suites pass, the relevant spec/ADR is updated,
no new global lock/queue/central directory exists, and the previous recovery
image still boots.
