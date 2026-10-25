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
`wasm-abi` v0.5.0 and the user SDK v0.4.0. A slot-B Wasm module now runs the
local single-node policy on iris; the kernel does not yet host a resident Wasm
core or join a mesh. The M1 software gate is iris-only, with the real-Indy
limitation recorded in Phase 2.

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
hello; the std image is built and booted in the same CI job. The kernel now
starts the local
namespace/CAS, actor and capability policy at boot; the signed 1-of-1 policy
remains the module probe. The slot-B Wasm module now runs the local policy
from a Rust guest: it stages `/etc` and `/etc/motd`, derives the actor, chooses
and checks the capability rights, and commits atomically. Its
`m1-wasm: root=... actor=... cap=ok rights=35 home=...` evidence has a distinct
guest root and actor; rights and home match the native `m1:` line, while MCP remains loaded in slot A; the released v0.1.3 MCP artifact has a known HTTP bug, while a current-tree build answers. The general module multiplexer is
still missing; the fixed A/B layout is the current coexistence mechanism. The
kernel has no mesh transport; the counters are self-tested and the M1 lines assert zero (a construction invariant, not packet-level instrumentation).

**PROM budget finding:** the first shim slice left the compressed PROM at
494,140 of 524,288 bytes. A trial that linked the namespace, CAS store and actor
directory directly into the kernel produced 522,248 bytes, leaving only 2,040
bytes of headroom, so it was not merged. The v0.1.3 console/sash plus EROFS
Range image is 520,840 bytes (3,448 free); the unreleased ABI v11 cache and
userland-install image with the kernel M1 policy, fat LTO and `opt-level=z` is 520,312 bytes
(3,976 free). Runtime policy, modules and userland software must
ship in the package index or a separate core image; they cannot grow the fixed
bootstrap.

**ROM freeze:** after M1 is re-closed, the compressed PROM/kernel image is
frozen for new features. Only correctness, security and hardware-enablement
fixes may change it. M2 and later policy, runtime, transport, server and device
work ships as modules through the package index; the kernel keeps only the
fixed service table and boot-critical path.
The optional SCSI write cache is deliberately a module, not PROM code: verified
downloads wait in a bounded RAM queue, and the module drains it by SHA-256 into
a cache-owned range during its idle tick.  ABI v11 now has `block_write`, and
`modules/cache` proves the idle write and readback on target 1 to target 3.
The automatic queue, lookup and module multiplexer are still to come.

**M1 software status (2026-10-26):** the iris gate boots the no-network,
no-module path and the slot-B Wasm-policy path; the native and Wasm
actor/rights/home evidence match; the guest root and actor are distinct, MCP remains loaded in slot A (the v0.1.3 release artifact HTTP bug is tracked), and
`mesh=0/0/0` is asserted both before and after the optional Wasm probe and
userland smoke. The rebuilt PROM is 520,312 bytes in CI (local 520,312) of 524,288 bytes
(3,976 free) and the Wasm module is 1,347,640 bytes (CI; local build 1,347,640), inside the
1,572,864-byte slot-B limit. The optional slot-B probe now runs after the
readiness marker. MCP-only readiness measured 7.3–7.7 s. The instrumented Wasm
path reports `autoload: wasm took 2140 ms` over local HTTP and 1600 ms from the
local SCSI cache, with `wasm: runtime=0 ms`; fetch/copy dominate. The
network-off/local-SCSI boot also exercises the Wasm policy with
`mcp=disabled`. The full-path load time is recorded as an observation under
issue #214, not a gate; the local cache is the preferred optional source. The live
Wasm-policy bridge is now adopted for the userland `/etc/motd` path: the
kernel validates the fixed bridge and reads that object through the committed
guest policy, with the native kernel policy as fallback. The actor/run-queue
and full resident authority remains Phase 5 work (#213). **Real-Indy
execution was not available in this workspace.** The exact limitation is hardware-specific: no physical Indy IP24
or self-hosted lab runner was attached, so the gate cannot validate the real
SCC, Seeq DMA, SCSI, REX3 or DS1386 timing and cache behaviour. The gate is
iris-only and must not be reported as a real-Indy pass; the follow-up is the
lab work in #16.

**Exit:** M1. Indy boots to `BOOT-READY`; network/telnet/MCP/module autoload
and POSIX image unchanged; no mesh RPCs. Boot time is **recorded, not gated**
for the 1994 hardware path; the optional module load is likewise an observation.

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
keep the native core option. Add the module manifest and runtime dependency
manager required by #217, without growing the frozen PROM.

**Exit:** core starts on iris; actor suite; deterministic corpus; native
POSIX/IRIX paths still pass; no packet or block data path inside Wasm.

### Phase 5 — Global namespace, CAS and actor lifecycle

Move namespace and capability policy into the core; implement CAS fetch,
manifests, cache/pinning, L0 migration, per-node run queues and placement. Add
the POSIX bridge and ABI 10 compatibility bridge (ABI 11 adds block_write).

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
tests; ABI 10 modules still work; ABI 11 adds block_write; M1–M4 security gates.

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
- ABI 10 compatibility modules and the user SDK;
- EROFS/HTTP Range and the recovery console (the clean-room `sash` on telnet
  and the local keyboard/display).
- a fully Rust native desktop based on a downstream COSMIC toolkit fork
  (`libcosmic`/`iced`/`cosmic-text`) on the native display server (D33, #194);
  no Wayland or `cosmic-comp` on the Indy. Full COSMIC on GPU nodes is a
  separate long-range path.

A phase that breaks these does not pass, even if its Wasm demo works.

## 5. Workstreams

- **Runtime:** wasmi, determinism, fuel, MIPS performance, L2 research.
- **Mesh:** homes, routing, membership, placement, authority, 128-node test.
- **Storage:** CAS, manifests, replication, pinning, GC.
- **Devices:** Seeq, SCSI, e1000e, AHCI/NVMe, virtio, GPU object protocol.
- **Compatibility:** POSIX/relibc, IRIX 6.5, ABI 10 (ABI 11 adds block_write).
- **Fabric:** frames, tags, completions, cancellation, trace, 9P gateway.
- **Toolchain/SDK:** `wasm-abi`, user SDK, core image, build/image tooling.

### 5.1 Sourcing and reuse

The [Rust repository sourcing study](../research/rust-repository-sourcing.md)
(2026-10-25) maps each component to verified Rust candidates, licence status,
portability blockers and the issues they unblock. It is research input, not an
ADR.

The standing rule is **reuse before reimplementation**:

1. adopt a compatible project as-is;
2. carry a minimal downstream patch or backend;
3. fork only where the design requires it (the COSMIC toolkit is the current
   example);
4. reimplement only where the licence, platform assumptions or architecture
   make reuse unsafe.

Every adoption still needs a spike and a work-item record: licence, source
provenance, build requirements, n32/32-bit/big-endian behaviour, allocation and
thread assumptions, and a replacement plan. A permissive licence is not a
provenance certificate, and “pure Rust” is not a portability proof.

**Sequenced candidates:**

| Order | Candidate | Licence | Why / gate |
|---:|---|---|---|
| 1 | Tooling gate suite: `cargo-deny`, `cargo-audit`, `cargo-cyclonedx`, `cargo-about`, `cargo-call-stack`, `cargo-bloat`, `proptest`, `cargo-fuzz`, `nextest` | MIT/Apache-2.0, Apache-2.0, NCSA | Host-only; unblocks clean-room, CI, security, release and performance evidence (#12, #13, #15, #17, #11). |
| 2 | `foca` | MPL-2.0 | `no_std` + `alloc` SWIM membership; the missing M2–M4 failure/membership layer. Requires a 32-bit/no_std spike (#3–#5). |
| 3 | `x11rb` + `yserver` | MIT OR Apache-2.0; MIT | X11 wire types and server path (#8), then legacy app compatibility (#6, #7, #11). |
| 4 | `uutils/coreutils` | MIT | Userland utility corpus after the native `std`/relibc environment exists (#6, #10, #11, #2). |
| 5 | `cosmic-text` + `harfrust` + `tiny-skia`, then `libcosmic` | MIT/Apache-2.0, MIT, BSD-3, MPL-2.0 | Native COSMIC toolkit fork and software renderer (#7, #8, #194). |
| 6 | `unwinding` + a clean-room IRIX `libc` module | MIT/Apache-2.0 | `std`, unwinding and the native cell ABI (#6, #10, #11). |
| 7 | `lamxfs` + `xfs-core` | MIT/Apache-2.0, Apache-2.0 | XFS v4/v5 read-only (#9), subject to provenance review. |
| 8 | `object` + `xmas-elf` + `gdbstub` | Apache/MIT; MIT/Apache-2.0 | Host ELF inspection and a MIPS-capable debug stub (#2, #6, #10). |
| 9 | `rmcp` | Apache-2.0 | Official MCP SDK on a host or native cell, not the kernel module (#2, #13, #15). |

**Rejected or conditional:**

- The closest IRIX `std`/libc prior art (`rust-irixlibstd`, `rust-irix`) has no
  licence file; use it as evidence, not code. Ask for relicensing if needed.
- GPL-2.0-only sources are incompatible: Linux/XFS/`libxfs`, `git2`/libgit2,
  `fish` and similar. Do not copy them.
- AGPL, SSPL, CDDL, EPL, proprietary and unlicensed code are rejected.
- MPL-2.0 (`foca`, `libcosmic`, `ostd`, `symphonia`) is compatible with
  file-level obligations; keep the files MPL and publish modifications.
- Wayland/DRM/GBM/GPU libraries remain rejected for the Indy native path;
  `cosmic-comp` is a separate GPU-node path.
- C-backed crates (`ring`, `aws-lc-rs`, `zstd`, `littlefs2`, `git2`,
  `capstone`) need a Rust alternative or an explicit build/port work item.

No candidate is adopted by this plan entry. The next action is a host-only
tooling spike, followed by `foca`; each must pass its own portability test
before it enters a product dependency.

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
