# IRIX 7 — Advanced Architecture Specification

**Layer:** spec (ADR-0015: layered spec, similarity-screen verification)
**Status:** proposed advanced architecture; decision-complete for v1, with the open questions in §17
**Licence:** GPL-3.0 (ADR-0012), see `LICENCE`
**Related:** `docs/map.md` (project map); `github.com/irix7/project` (toolchain + ADRs 0001–0020); `github.com/irix7/os` (this repo, planned)
**Research inputs:** [`../research/`](../research/) — the hardware, X11, native-window, filesystem, Plan 9 and EROFS reports

---

## Contents

1. [Identity and scope](#1-identity-and-scope)
2. [Governing decisions](#2-governing-decisions)
3. [Toolchain and build](#3-toolchain-and-build)
4. [Architecture overview](#4-architecture-overview)
5. [Native ABI and process model](#5-native-abi-and-process-model)
6. [Legacy IRIX compatibility container](#6-legacy-irix-compatibility-container)
7. [Memory management](#7-memory-management)
8. [Scheduling and concurrency](#8-scheduling-and-concurrency)
9. [Drivers and hardware](#9-drivers-and-hardware)
10. [Boot and deployment](#10-boot-and-deployment)
11. [Storage and filesystems](#11-storage-and-filesystems)
12. [Networking](#12-networking)
13. [Display and media](#13-display-and-media)
14. [Control plane (MCP)](#14-control-plane-mcp)
15. [Clean-room and verification](#15-clean-room-and-verification)
16. [Alternatives rejected](#16-alternatives-rejected)
17. [Open questions](#17-open-questions)
18. [Sources](#18-sources)

---

## 1. Identity and scope

IRIX 7 is a clean-room, GPL-3.0, Rust-native operating system for the SGI Indy. It is
not a Linux distribution with an IRIX theme, and it is not a Plan 9 clone. It is the
successor SGI might have built if the company had not abandoned IRIX for Linux: an
IRIX-compatible system with modern internals.

The design has two software worlds.

1. **The IRIX world.** Existing IRIX 6.5 o32 and n32 binaries run unchanged in a
   compatibility container: the same ELF/ECOFF images, the same syscall conventions,
   the same libc/X11/Motif/GL/dmedia library surfaces, the same `fork`/`exec`/`sproc`
   semantics. This is the backward-compatibility mandate (ADR-0008).
2. **The native world.** New software uses a new native ABI: capability-based cells,
   synchronous IPC with shared memory, a Unix-shaped libc over capabilities, and a
   new IRIX-inspired desktop. The native ABI is deliberately *not* the IRIX ABI; it is
   the part of the system that is allowed to be futuristic.

The compatibility container is the IRIX identity. The native world is the future. They
share one kernel, one scheduler, one object model, and one hardware substrate.

### 1.1 Hardware target

The first target is the **iris emulator default**, because the emulator is the primary
development and test machine:

| Component | Target |
|---|---|
| Machine | SGI Indy (iris profile `IndyIp24`) |
| CPU | MIPS R4400, MIPS III, big-endian; 16 KiB L1 I/D caches, 1 MiB L2 |
| RAM | 256 MiB (`banks = [128, 128, 0, 0]`) |
| Graphics | Newport XL (REX3 + RB2 + VC2 + XMAP9), guest-programmed mode |
| Audio | HAL2 |
| Ethernet | Seeq 8003, 10 Mbit |
| SCSI | WD33C93, disk at ID 1, CD-ROM at ID 4 |
| Video | Vino capture present; source off by default |
| Video output | Not modelled (the separate Indy Video GIO board is absent) |
| Firmware | SGI PROM / ARCS |

The two real stock Indys have different graphics boards and at least one board that may
not be supported under Linux; the OS therefore detects the board at boot and enables only
the features the hardware provides. The emulator default is the first reference, not the
only configuration. ISDN is explicitly out of scope.

> **Naming note.** The hardware report uses "IP22 / Guinness" for the Indy, while the
> iris emulator calls the Indy `IP24` and the Indigo2 `IP22`. The project should settle
> on one convention before the hardware tables are frozen; see §17.

### 1.2 Goals

- **Run IRIX 6.5 binaries unchanged**, including o32 and n32 applications, the IRIX
  desktop stack under X11, X11 applications, GL, dmedia and audio.
- **Be modern in concept**: capabilities, cells, safe Rust, userspace drivers,
  asynchronous I/O, network-first deployment, remote control, and an IRIX-inspired
  desktop that is not a museum piece.
- **Use the Indy's hardware**, not a generic PC abstraction: HPC3 DMA, the R4400
  caches and TLB, Newport's 2D blitter, HAL2, Vino, the DS1386 watchdog.
- **Boot from the network**, with no dependence on a working SCSI disk.
- **Be remotely controllable** through an in-OS MCP server, so the OS can be developed
  and inspected by an agent.
- **Keep the trusted computing base small**: a privileged framework that uses
  `unsafe`; everything else in safe Rust wherever the hardware permits.

### 1.3 Non-goals

- Forward compatibility: native IRIX 7 binaries running on IRIX 6.5 or 5.3.
- ISDN.
- The original Indigo Magic desktop as the *native* desktop. Legacy IRIX desktops may
  run under X11, but the native desktop is new code.
- A literal Plan 9 reimplementation. Plan 9 supplies selected interface ideas, not the
  identity.
- Boot-time security ceremony. This is a hobby OS; digests are checked, but a full
  measured-boot/signature chain is future work.
- 3D hardware. Newport has no Z-buffer or geometry engine; 3D is software
  rasterisation, and not a v1 goal.
- Hard real-time guarantees. Media scheduling is soft real-time.

### 1.4 Design principles

1. **IRIX is the identity; modernity is the method.** The compatibility container and
   the IRIX user experience are the point. The internals are allowed to be radical.
2. **Hardware wins where it must.** The framekernel keeps the core safe, but drivers
   may use `unsafe` when an abstraction would cost a feature or unacceptable
   performance. Every such use is documented and contained.
3. **Do not depend on dead hardware.** The R2 boot path works without a SCSI disk. A
   local cache is opportunistic.
4. **Authority is a capability, not a name.** A path, a namespace or a mount point is
   not a security boundary in a single-address-space kernel.
5. **Measure for information, not for gates.** Benchmarks are tracked; they do not
   fail a build.
6. **The development harness is part of the product.** The MCP server, the serial
   console, the emulator rig and the smoke tests are first-class from the beginning.
7. **Clean-room, always.** Public sources and published specifications are references;
   SGI source and decompiled expression stay in the private `irix7/irix6` repo.

---

## 2. Governing decisions

### 2.1 Existing ADRs (0001–0021)

All governing decisions apply. The numbered, dated ADRs live in
`github.com/irix7/project/docs/adr/`.

| ADR | Decision | Relevance here |
|---|---|---|
| 0001 | Publish toolchain work only | Keeps SGI-derived material private |
| 0002 | GCC 16.2 on pdaxrom base | Toolchain lineage |
| 0003 | All ABIs from day one | o32/n32/n64 support; target name |
| 0004 | Own rig with a fresh 6.5.7m guest | Test environment |
| 0005 | Reference-first rebuild | Compatibility method |
| 0006 | Dynamic-first smoke harness | Testing method |
| 0007 | Static-first rebuilt runtime | Runtime bring-up |
| 0008 | Backward-only compatibility | IRIX 6.5 binaries on IRIX 7; not the reverse |
| 0009 | Clean-room-only publication | No SGI expression in `irix7/os` |
| 0010 | Rust monolith, n32/MIPS III | Kernel shape and ABI |
| 0011 | Native capability substrate + compat container | Two-world architecture |
| 0012 | GPL-3.0 | Licence gate for every dependency |
| 0013 | Full reimplementation, factory PROM | Clean-room reimplementation |
| 0014 | Harness-enforced clean-room | Hybrid transcription |
| 0015 | Layered spec, similarity screen | This document's verification |
| 0016 | Rust ROM replaces the factory PROM; n32/MIPS III from the PROM window into the n32 kernel | Boot path |
| 0017 | Modern equivalents as references | Research method |
| 0018 | GCC backend primary, LLVM parallel | Codegen |
| 0019 | IRIX fork on `rust-lang/gcc` | GCC 17 trunk + libgccjit |
| 0020 | Rebuild ladder | Recompile vs reconstruct |
| 0021 | Native Rust std is load-bearing; its libc baseline is POSIX.1-2024 | Target/product split and std plan |

### 2.2 New decisions (advanced architecture)

These are the decisions reached in the architecture grilling of 2026-10-20. They
should be recorded as ADR-0022 onward (ADR-0021 records the native Rust std decision),
or folded into a single "advanced architecture" ADR set. The decision register is
normative for this spec.

#### Identity and product

| ID | Decision | Rationale |
|---|---|---|
| D1 | **IRIX 7 is the successor SGI might have built**, not a Plan 9 clone | The compatibility goal and the IRIX user experience are the identity |
| D2 | **Native ABI is new; IRIX lives only in the compatibility container** | Keeps the native world free to be modern |
| D3 | **Native process = one cell with a Unix personality** | `fork` copies the cell, `exec` replaces it, signals are events, file descriptors are capabilities |
| D4 | **Capability ABI underneath, Unix-shaped libc on top** | `open`/`read`/`write`/`fork`/`exec` for apps; capabilities for authority |
| D5 | **New native desktop, IRIX-inspired** | Indigo Magic workflow and aesthetics, new code; not the original binaries |
| D6 | **All Indy hardware except ISDN** | Graphics, audio, Ethernet, SCSI, Vino capture, serial, RTC, CPU; video output deferred |
| D7 | **First target is the iris default** | Indy `IndyIp24`, R4400, Newport XL, 256 MiB, HAL2, Seeq, WD33C93; other boards detected later |

#### Kernel and isolation

| ID | Decision | Rationale |
|---|---|---|
| D8 | **Framekernel** (Asterinas/Ostd model) | Small privileged `unsafe` framework; de-privileged safe Rust cells |
| D9 | **Hybrid isolation** | SAS/SPL for native cells; MMU address spaces for untrusted IRIX binaries |
| D10 | **Core safety** | Scheduler, VM and IPC stay safe Rust; drivers may use `unsafe` where the hardware demands it |
| D11 | **Capabilities: object + rights + revocation + delegation** | seL4-style authority; recoverable, attenuable |
| D12 | **Synchronous IPC + shared memory** | Simple to reason about; bulk data is shared, not copied |
| D13 | **Cells load from Rust object files via a custom loader** | Theseus-style; one crate per cell |
| D14 | **EDF for media, fair scheduling for the rest** | Soft real-time without penalising interactive work; no admission control in v1 |
| D15 | **Aggressive huge pages + cache-coloured allocation** | The R4400 TLB has 48 entries; large pages and cache-aware layout are the MIPS performance lever |
| D16 | **DMA-first, cached buffers + explicit sync; no full IOMMU (MC VDMA DMA TLB used where possible); raw DMA grants accepted** | Hardware wins; the risk is documented and contained |
| D17 | **All drivers are de-privileged cells**, not ring-3 processes | The framekernel trust boundary is `unsafe`, not a privilege ring |
| D18 | **Userspace async; kernel event-driven** | Cells use async/await; the kernel exposes event queues and completions |
| D19 | **Thin HAL**: CPU, MMU/TLB, cache, interrupts, timers abstracted; devices direct | Indy-first with a cheap portability seam |

#### Boot, storage and data

| ID | Decision | Rationale |
|---|---|---|
| D20 | **Minimal Rust ROM + boot bundle from R2** | The ROM has the Seeq driver; the bundle carries network, block, FS and init cells |
| D21 | **Root image: standard little-endian EROFS + byte-order codec** | No fork; keeps Linux/`fsck`/`dump` as an independent oracle |
| D22 | **R2-first boot; optional local cache** | A working SCSI disk may cache blocks; nothing depends on it |
| D23 | **XFS compatibility server read-only first; native FS (I7FS preferred, RedoxFS fallback) decided later** | XFS read-only is tractable; write is the long pole; the native FS choice can wait |
| D24 | **User data: periodic batched Git commits (small) + R2 (large)** | Git's strengths for versioned text; object storage for media |
| D25 | **Networking: Seeq 8003 + smoltcp; `/net` tree; optional 9P** | One owner, dual-stack, scriptable |
| D26 | **Boot trust: SHA-256 digests; signatures are future work** | Hobby OS; integrity is cheap, ceremony is not |

#### Display, media and control

| ID | Decision | Rationale |
|---|---|---|
| D27 | **Native display server: Plan 9 `8½`/BeOS `app_server` lineage** | Server-side drawing, retained images, damage repair; Wayland's ARGB recomposition exceeds the memory bandwidth |
| D28 | **Clean-room Rust X11 server; lease → nested → rootless** | No Xorg/kdrive port; fastest path to running real IRIX apps |
| D29 | **Userspace graphics server owns the blitter; userspace audio server owns HAL2** | One owner per device; capability-multiplexed |
| D30 | **In-OS MCP server: full root, bearer token, no audit, hardware watchdog** | The development harness; the user accepted the risk |
| D31 | **Plan 9 ideas only where they serve IRIX**: `ctl`/`data`/`status`, `/net`, optional 9P; no per-process namespaces by default | Selective, not a clone |
| D32 | **Native libc API baseline is POSIX.1-2024 (SUSv5), not the IRIX 6.5 ABI** | Rust std and new applications need a modern, documented Unix surface; the compatibility container keeps IRIX 6.5 exactly |

### 2.3 Decision principles

- **Compatibility is a contract.** If an IRIX 6.5 binary runs on IRIX 6.5, it should
  run on IRIX 7. Regressions are bugs.
- **The native ABI is allowed to be unfamiliar.** It is for new software only.
- **The compatibility container may be ugly.** It has to match a 1990s ABI; the native
  world is where the clean design lives.
- **No hidden dependencies on hardware that may be dead.** In particular, no boot path
  that requires a working SCSI disk.
- **No hidden dependencies on a cloud service at runtime if a local path exists.** R2 is
  the distribution channel; the local cache is opportunistic.
- **Every `unsafe` block has a written invariant.** The framework's invariants are in
  §4.4; driver `unsafe` is documented at the module level.

---

## 3. Toolchain and build

### 3.1 Build host and cross toolchain

This is grounded in `irix7/project`, not speculation.

| Component | Choice | Source |
|---|---|---|
| Environment | GNU Nix (`flake.nix`), `#default` / `#rig` devshells | `irix7/project/flake.nix` |
| Host compiler | GCC 13 (pdaxrom-proven) | `stack.md` §1 |
| binutils | 2.47 + IRIX series | `patches/binutils-2.47/` |
| GCC | 17.0.0 trunk, `rust-lang/gcc` fork; IRIX series (8 patches) applies cleanly to 17 trunk | `irix7/gcc`, ADR-0019 |
| libgccjit | built into the fork (`--enable-host-shared --enable-languages=c,jit`) | the Rust codegen backend |
| Cross target | `mips-sgi-irix6.5`: big-endian MIPS, ELF32, o32/n32/n64, MIPS III/IV | ADR-0003 |
| sysroot | captured IRIX 6.5.7m headers and libraries | `oracle/`, `scripts/irix-media/` |

### 3.2 Rust toolchain

| Component | Choice | Status |
|---|---|---|
| rustc | nightly, `-Z build-std` for `core`/`alloc` | codegen proven |
| Codegen backend | `rustc_codegen_gcc` (libgccjit from §3.1) — **primary** | ADR-0018 |
| Parallel path | LLVM custom target `mips3-sgi-irix7-n32` (n32/MIPS III) | codegen proven |
| ABI | ROM and kernel are both n32/MIPS III; o32 belongs to the compatibility container | — |

The local kernel/module target spec is `targets/mips3-sgi-irix7-n32.json` (the LLVM
path).  The hosted native target is `targets/mips3-sgi-irix7-native-n32.json`
(`target_os = "irix7"`, Unix family); it is for the POSIX-2024 libc and Rust `std`, and
is not used by the kernel.

Current status (`stack.md` §5): codegen and `-Z build-std` for n32 are **proven**;
the ROM boots from the PROM image and runs DHCP/NTP/HTTP under iris; the IRIX patch
series applies cleanly to `rust-lang/gcc` — **proven**; GCC-backend build — **blocked**
on disk space (the host is 99% full).

Prior art on Rust IRIX targets (issue #41 research): `danifunker/rust-irixlibstd`
(o32/MIPS II + n32/MIPS III) and `atomchild411/rust-irix` (rustc fork, n32/MIPS IV).
Conclusion: no rustc/LLVM fork is required; a custom n32/MIPS III target spec +
`-Z build-std` is the plan.

### 3.3 Kernel build profile

- `#![no_std]`, `#![no_main]`, `#[panic_handler]`, `panic = "abort"` in both profiles.
- No unwinding on bare metal.
- The kernel is n32/MIPS III.
- The boot ROM is a separate `no_std` artefact, built for n32/MIPS III and executed directly
  from the PROM socket at `0xBFC00000`; it does not rely on the factory ARCS ECOFF loader.
- `-Z build-std` builds `core` and `alloc` for the kernel target today.  A hosted
  native target is planned for the POSIX.1-2024 libc and Rust `std`; the kernel and
  loadable modules remain `no_std` and must not link it.
- Host-side tooling (image builder, MCP bridge if needed, tests) may use `std`.

### 3.4 Testing and the rig

The emulated Indy runs under the `irix7/iris` emulator fork (a fork of
`techomancer/iris`), managed by `scripts/rig/`. Each agent gets a private rig instance
(`scripts/rig/task-rig.sh start|stop <item>`). The smoke harness (`scripts/smoke.sh`)
compiles a test program against the captured sysroot, runs it on the guest, and diffs
stdout against expected output — the primary testing seam, dynamic-first per ADR-0006.

The MCP control plane (§14) extends this into an agent-driven loop: edit, build,
deploy, boot, run, observe, reset. The loop is the development method; the smoke
harness is the regression guard.

### 3.5 Build risks

| Risk | Effect | Mitigation |
|---|---|---|
| Host-toolchain glue | Clean builds need `cc` and `pkg-config` | CI installs GCC; the Nix devshell provides both |
| GCC-backend disk space | Cannot build `rustc_codegen_gcc` | Free space; build on a larger volume; use the LLVM path in the meantime |
| MIPS III + o32 | Legacy o32 binaries need a MIPS II decoder | ROM and kernel are n32/MIPS III; the compatibility container owns o32 |
| Big-endian Rust ecosystem | Some crates assume little-endian | Use explicit codecs; test on the Indy; prefer endian-neutral crates |

---

## 4. Architecture overview

### 4.1 System shape

```text
+------------------------------------------------------------------+
| Native cells (SAS/SPL, kernel mode, shared address space)         |
|   init · display server · audio server · graphics server          |
|   network cell · block cell · FS cell · MCP cell                  |
|   native applications (a process = one cell with a Unix face)     |
+------------------------------------------------------------------+
| Privileged framework (the unsafe TCB)                             |
|   traps · TLB/cache · frames · IRQ/timers · MMIO/DMA broker       |
+------------------------------------------------------------------+
| Kernel compatibility layer                                        |
|   IRIX syscall decode · ELF/ECOFF loader · fork/exec/sproc        |
+-------------------------------+----------------------------------+
| Legacy IRIX processes          | Hardware                          |
| (MMU spaces, user mode, kuseg) | R4400 · HPC3 · WD33C93 · Seeq     |
|                                 | HAL2 · Newport · Vino · DS1386    |
+-------------------------------+----------------------------------+
```

The kernel and all native cells occupy the MIPS **kseg0** unmapped segment (with kseg1
used for early boot and uncached MMIO) and execute in kernel mode: one address space, one
privilege level (Theseus SAS/SPL). Legacy IRIX processes occupy **kuseg** in their own MMU
address spaces and execute in user mode. On a legacy syscall trap the CPU enters the
kernel in kseg0; the compatibility layer reads and writes the legacy process's memory
through the user's TLB entries (`copyin`/`copyout`). The 8-bit `EntryHi.ASID` separates
legacy address spaces; native cells do not need an ASID because they share the kernel's
mapping.

kseg0 is unmapped, so the kernel direct map needs no TLB entries. The R4400's 48-entry
JTLB is used for kuseg (legacy processes) and the optional kseg2 window; 16 MiB pages are
used there and for large legacy mappings. I/O is mapped with 4 KiB or 16 MiB pages
depending on the device granularity.

### 4.2 Privilege tiers

| Tier | Hardware mode | Address space | `unsafe` | Responsibility |
|---|---|---|---|---|
| Privileged framework | Kernel | kseg0/kseg1 | Yes, minimally | Traps, TLB, cache maintenance, frames, IRQ/timers, MMIO/DMA brokering, sensitive memory |
| Core services | Kernel | kseg0 (SAS) | No | Scheduler, VM, IPC, capability table, VFS, namespaces, process table |
| Drivers | Kernel | kseg0 (SAS) | Allowed, documented | Device cells (SCSI, Seeq, HAL2, Newport, Vino, serial, RTC) |
| Native applications | Kernel | kseg0 (SAS) | Safe Rust | New software on the native ABI |
| MCP server | Kernel | kseg0 (SAS) | Safe Rust | Remote control plane |
| Legacy IRIX processes | User | kuseg (MMU) | N/A (foreign binary) | IRIX 6.5 o32/n32 applications and their libraries |

The trust boundary is the framework's `unsafe`, not a privilege ring. A native cell is
"de-privileged" in the Asterinas sense: it is safe Rust outside the framework, and it
reaches hardware only through framework capabilities. A bug in safe Rust cannot violate
memory safety, but a bug in a native cell **can** corrupt another cell's data, because
they share one address space. The capability model limits *authority*, not *memory
corruption*. This is the deliberate SAS trade-off.

### 4.3 Cells and processes

A **cell** is one Rust crate compiled to a relocatable ELF object (`.o`). At runtime a
cell is a loaded structure containing its sections (`.text`, `.rodata`, `.data`, `.bss`)
plus metadata derived from ELF relocation entries. Cells are loaded and linked into a
`CellNamespace`, which is the set of publicly visible symbols. This gives deep crates
(ADR-0010) with fine-grained, per-section dependency tracking, and leaves the door open
to live update and fault recovery.

A **native process** is one cell with a Unix personality:

- `fork` clones the cell's memory and file-descriptor table (copy-on-write where the
  memory manager can support it; a plain copy is acceptable for v1).
- `exec` replaces the cell's image with a newly loaded cell or ELF image.
- Signals are events delivered to the cell's signal handlers.
- File descriptors are capability handles; `open` returns one, `read`/`write` invoke it.
- Threads are tasks within the cell. `sproc` is a legacy-only ABI; native threads use
  the native task API.

The kernel starts the boot-bundle cells (network, block, FS, MCP). The **`init` cell**
then mounts the root, starts the display, audio and user-session cells, and hands control
to the user session.

### 4.4 Framework invariants

The framework enforces the Asterinas/Ostd invariants, adapted to the Indy:

- **Inv. 1** — Any newly allocated frame or segment originates from currently unused
  memory; frame metadata carries reference counts.
- **Inv. 2** — Native cells are safe Rust and cannot execute privileged instructions;
  the framework exposes only safe APIs for CPU state. A cell that needs privileged access
  is a driver and is documented.
- **Inv. 3** — *Adapted.* Kernel-mode CPU state cannot be tampered with by peripheral
  devices. The Indy has **no full IOMMU**; the MC's VDMA provides a 4-entry DMA TLB and
  MASK/SUBST address protection for GIO/PBUS masters. The framework uses that TLB to
  constrain DMA where the device path allows, but it is not per-process and cannot cover
  every master. The residual risk is accepted (D16).
- **Inv. 4** — Sensitive memory (kernel code, stacks, page tables) remains under
  framework control.
- **Inv. 5** — Sensitive memory cannot be exposed to user space; legacy user mappings may
  only use untyped (insensitive) memory.
- **Inv. 6** — *Adapted.* DMA mappings are made only over untyped memory, and the
  framework programs the VDMA DMA TLB where possible. Without a full IOMMU, this is a
  policy guarantee, not a hardware guarantee.
- **Inv. 7** — Sensitive MMIO/PIO cannot be instantiated by clients; I/O ranges are
  labelled sensitive or insensitive from the firmware and core-driver information.
- **Inv. 8** — A task runs on at most one CPU at any time (`is_running` checked before a
  context switch). The Indy is uniprocessor; the invariant is kept for portability.
- **Inv. 9** — A heap slot or object must not outlive its parent slab.
- **Inv. 10** — An object is created from a heap slot only if the slot meets the object's
  size and alignment requirements.

Safe policy injection (Asterinas §4.4): the scheduler, frame allocator and slab allocator
live outside the core framework and are injected through traits (`Scheduler`, `RunQueue`,
`FrameAlloc`, `Slab`/`HeapSlot`). This keeps the TCB small while allowing the EDF and
fair schedulers of §8 to be safe Rust.

### 4.5 Thin HAL

The HAL seam is deliberately narrow (D19):

| Abstracted | Why |
|---|---|
| CPU exception vectors, CP0, `eret` | Differs between MIPS III/IV parts |
| MMU/TLB refill, page sizes, ASIDs | R4000/R4400/R4600/R5000 differ |
| Cache maintenance (`Hit_*`, `sync`) | Line size and associativity differ |
| Interrupt controller (HPC3/IOC2) | Board-specific but shared by all Indy variants |
| Timers (CP0 Count/Compare, RPSS, DS1386) | Calibration and tick source |

Everything else talks directly to the device registers: Seeq 8003, WD33C93, HAL2, Newport
REX3/RB2/VC2/XMAP9, Vino, DS1386. There is no generic device model, no bus abstraction,
and no Linux-style driver framework. A driver is a cell with a `ctl`/`data`/`status`
interface (§9.2).

### 4.6 Safety policy

- The framework is the only place where `unsafe` is unavoidable. Every `unsafe` block has
  a written invariant and a safety comment.
- Core services are safe Rust. If a core service needs `unsafe`, it belongs in the
  framework or in a documented driver cell.
- Drivers may use `unsafe` where the hardware demands it (D10, D16). The module
  documents the invariant, the volatile accesses, the DMA/cache protocol, and the
  failure modes. The alternative — a safe abstraction that costs a feature or
  unacceptable performance — is rejected ("hardware wins").
- Native applications are safe Rust, but they share the address space. The capability
  model is the authority boundary; the Rust type system is the memory-safety boundary
  within a cell, not between cells.
- Legacy IRIX processes are hardware-isolated by the MMU. A bug in a legacy binary can
  crash that process, not the kernel — unless it has been granted raw DMA (D16), in which
  case the DMA TLB is the only constraint.
- The MCP server is a native cell with full root. A bug there is a kernel-level bug. The
  watchdog (§10.6) is the recovery path.

---

## 5. Native ABI and process model

### 5.1 Capability model

A capability is an unforgeable reference to an object plus a rights bitmap (D11). The
kernel's capability table is per cell; a handle is an index plus a generation counter, so
a stale handle cannot be reused after revocation.

- **Rights**: `read`, `write`, `execute`, `map`, `dma`, `invoke`, `transfer`, `dup`,
  `revoke`, `manage`.
- **Derivation** is rights-attenuating only: a child capability can have fewer rights,
  never more.
- **Delegation** is explicit: a cell may transfer or duplicate a capability if it holds
  `transfer`/`dup`.
- **Revocation** is recursive and generation-checked: revoking a capability invalidates
  all derived capabilities and wakes waiters.
- **No ambient authority**: a cell starts with exactly the capabilities its parent
  granted. There is no global "root" handle, no implicit filesystem access, and no
  implicit device access.

### 5.2 Objects

| Object | Purpose | Key rights |
|---|---|---|
| `Job` | A process/cell group; resource accounting and revocation root | `manage`, `invoke` |
| `Channel` | Synchronous IPC endpoint pair | `read`, `write`, `invoke` |
| `Port` | Event/notification queue | `read`, `write`, `invoke` |
| `VMAR` | Virtual memory region | `map`, `read`, `write`, `execute` |
| `VMO` | Physical/virtual memory object backing a VMAR | `read`, `write`, `map`, `dma` |
| `Device` | A device instance: MMIO range + IRQ + DMA window | `invoke`, `dma`, `manage` |
| `Irq` | An interrupt line delivered as an event | `read`, `invoke` |
| `DmaWindow` | A physically contiguous, cache-managed DMA buffer | `read`, `write`, `dma` |
| `File` | A VFS object (regular file, directory, device file, synthetic file) | `read`, `write`, `invoke` |
| `Socket` | A network endpoint | `read`, `write`, `invoke` |
| `Namespace` | Optional mount/bind table; a projection of capabilities, not an authority | `manage`, `invoke` |

`Namespace` exists for composition (the selective Plan 9 idea of `bind`/`mount`), not as a
security boundary. The default native process has a global IRIX-like namespace; a cell
may receive a private namespace if its parent chooses to construct one.

### 5.3 Invocation and IPC

Native code does not use the IRIX syscall instruction. It performs **capability
invocations**: typed, layout-stable calls into kernel objects. The call surface is
defined in Rust and crosses the FFI seam with `abi_stable` (candidate, cleared MIT OR
Apache-2.0) or `repr(C)` where a C ABI is needed.

IPC is **synchronous with shared memory** (D12): a `Channel` call blocks until the server
replies, and large buffers travel as `VMAR`/`VMO` mappings rather than copies. A `Port`
carries asynchronous events (interrupts, completion notifications, signals). The
combination gives simple request/response semantics with zero-copy bulk transfer.

- Every invocation has a timeout and a cancellation path.
- A server may donate its remaining time slice to a client; priority inheritance is
  available for EDF servers to avoid priority inversion.
- The kernel never copies a large buffer on the IPC path; it maps it.

### 5.4 Process = one cell with a Unix personality

The native process model is Unix-shaped on the surface and cell-shaped underneath (D3,
D4):

| Unix concept | Native implementation |
|---|---|
| Process | One cell + a `Job` capability + a capability table |
| Thread | A task within the cell; scheduled by the EDF/fair scheduler |
| `fork` | Clone the cell's memory (COW where possible) and capability table |
| `exec` | Replace the cell's image with a newly loaded cell/ELF; flush caches |
| Signal | An event on the process's `Port`; handlers run on a dedicated task |
| File descriptor | A capability handle in the process's table |
| `open` | Path resolution in the process's namespace → `File` capability |
| `read`/`write` | `File` invocation with a `VMAR`/`VMO` buffer |
| `mmap` | `VMAR` mapping of a `VMO` or file |
| `wait` | Read the child's exit event from the process's `Port` |
| `sproc` | Legacy-only; native threads use the native task API |

`fork` in a single address space is the expensive operation: a plain copy is O(memory),
and COW requires page-table manipulation that the SAS kernel does not otherwise need.
The native libc should prefer `posix_spawn`/`exec` for new processes; `fork` remains for
compatibility with Unix-shaped code and for the IRIX container.

### 5.5 Unix-shaped libc

The native libc is a separate library from the IRIX libc. Its normative API baseline is
**POSIX.1-2024** (the core of Single UNIX Specification, Version 5 (2024)), plus a
documented XSI subset; the profile and ABI obligations are in [`posix.md`](posix.md).
The IRIX 6.5 compatibility libc is a different source contract and keeps the IRIX ABI
unchanged.  POSIX defines the source API, not the n32 binary layout, symbol versions,
TLS model or syscall numbers; IRIX 7 owns and versions those.

The native libc presents the familiar surface — `openat`, `close`, `read`, `write`,
`lseek`, `mmap`, `fork`, `posix_spawn`, `execve`, `wait`, `signal`, `socket`, `ioctl` —
and implements it with capability invocations.

- `open` resolves a path in the process's namespace and returns a `File` capability
  wrapped as an fd.
- `read`/`write` invoke the `File` with a buffer; the kernel maps the buffer, not copies
  it, where possible.
- `ioctl` is deliberately narrow: it exists for legacy devices and for IRIX-compatible
  libraries; new devices use `ctl`/`data`/`status` files instead.
- `errno` is mapped from the native error type at the libc boundary.
- The libc is a cell; it can be updated independently of the kernel.

### 5.6 Asynchronous I/O

Async lives in userspace (D18). Each cell may run an executor (`core::future`); the
kernel exposes **event queues** (`Port`) and completion notifications. The libc's
blocking calls are implemented as `async` operations plus a `block_on` at the API
boundary. The kernel's trap handler completes the operation and posts an event; it does
not run an async executor.

This keeps the kernel simple and the scheduler predictable: EDF and fair scheduling see
tasks, not futures. A future that is waiting is a task blocked on a `Port`.

### 5.7 ABI stability

- The native ABI is versioned. A cell declares the ABI version it was built against; the
  loader refuses an incompatible cell.
- `abi_stable` is the candidate for Rust-to-Rust stable layout across compiler versions;
  `repr(C)` is used for anything that crosses to C or to the legacy container.
- Capability invocation argument layouts are fixed and documented; adding a field is a
  new ABI version, not a silent change.
- The loader links cells by symbol name; a missing symbol is a load error, not a runtime
  crash.

### 5.8 Security model

- **Authority is a capability, not a name.** A path, a mount point or a namespace does
  not grant authority; the capability does.
- **Least authority by default**: `init` starts with the root and device capabilities it
  needs; every child gets a subset.
- **Revocation is real**: a parent can revoke a child's device capability, and the
  device cell must stop using it. The framework generation-checks every invocation.
- **Legacy exceptions**: a legacy process with a raw DMA capability can write physical
  memory outside its address space (D16). The DMA TLB limits this where the device path
  allows; the residual risk is accepted.
- **MCP is full root** (D30). The bearer token is the only gate; there is no audit. The
  watchdog is the recovery path.

---

## 6. Legacy IRIX compatibility container

### 6.1 Goals

The container runs IRIX 6.5 o32 and n32 binaries **unchanged**: the same ELF images, the
same syscall numbers and conventions, the same libc/X11/Motif/GL/dmedia surfaces, the
same `fork`/`exec`/`sproc` semantics. It is the backward-compatibility contract
(ADR-0008), and it is the reason the project exists.

It is **not** a userspace server and **not** an emulator. The CPU executes the IRIX
instructions natively (the R4400 is the same architecture); the container translates
syscalls and provides the libraries.

### 6.2 Architecture

A legacy process is:

- a **kuseg MMU address space** with its own page table and ASID;
- a user-mode MIPS III context (o32 or n32);
- an ELF (or ECOFF, for the PROM bootstrap) image plus its shared libraries;
- a file-descriptor table whose entries are kernel `File`/`Socket`/`Device` capabilities;
- a set of signal handlers and a `sproc` thread group.

On a syscall trap, the CPU enters the kernel in kseg0. The compatibility layer:

1. decodes the ABI (o32 or n32) from the process's personality;
2. reads the syscall number and arguments from the saved register frame;
3. validates user pointers with `copyin`/`copyout` against the process's address space;
4. translates the operation into a native capability invocation or a VFS operation;
5. writes the result (or `errno`) back to the saved frame;
6. returns to user mode.

The compatibility layer is kernel code; there is no context switch to a userspace server
and no IPC hop. This is the performance argument for the in-kernel container (and the
reason the original spec rejected a microkernel).

The first layer of the container is [`crates/irix-elf`](../../crates/irix-elf), a `no_std`
parser and load-plan view for ELF32 big-endian MIPS images. It validates the file header,
the program-header table and every file and memory range, then exposes the `PT_LOAD`
segments, `PT_INTERP` path and `PT_DYNAMIC` header without mapping anything. The loader
contract, including the o32/n32 `e_flags` classification and the deliberate omission of
dynamic linking, is fixed in [`elf.md`](elf.md).

### 6.3 ABI details

| ABI | Data model | Syscall number | Arguments | Result |
|---|---|---|---|---|
| o32 | ILP32, MIPS II | `$v0` | `$a0`–`$a3`; 5th–8th on the user stack | `$v0` |
| n32 | ILP32, MIPS III/IV, 64-bit registers | `$v0` | `$a0`–`$a5`; further args on the stack | `$v0` |

Both are big-endian. IRIX 6.5 applications are ELF; the PROM bootstrap is o32 ECOFF
(ADR-0016). The kernel itself is n32/MIPS III. "MIPS III + o32" is not buildable, so the
o32 bootstrap is MIPS II (issue #41).

The compatibility layer must also implement the IRIX-specific surfaces that are not
POSIX: `sproc` (SGI threads/processes), `arenal`/`arenas`, `procntl`, `syssgi`, the
`/hw` tree, `sysmp`, and the IRIX `ioctl` set. Each is mapped onto a native object or
emulated in the container. STREAMS is legacy-compatibility only; it is not a first-class
native interface.

### 6.4 Process model

- **`fork`** clones the legacy address space and fd table. The child gets a new ASID and
  a copy of the parent's memory (COW where the MMU and page-table code support it).
- **`exec`** loads a new ELF/ECOFF image into the existing address space, resets the
  signal dispositions, and starts at the new entry point.
- **`sproc`** creates a new thread in the same address space, with the IRIX `sproc`
  flags and sharing semantics. The native scheduler sees it as a task.
- **Signals** are delivered through the process's `Port`; the compat layer builds the
  IRIX signal frame on the user stack.
- **Job control** (`wait`, process groups, sessions, `SIGTSTP`/`SIGCONT`) is implemented
  in the container, using native `Job` capabilities.

### 6.5 Library-level compatibility

The IRIX libraries are clean-room reimplementations that run in the legacy process's
user address space:

- `libc` (o32 and n32 C ABIs), `libm`, `libdl`;
- `libX11`, `libXext`, `libXt`, `libXm`/Motif, `libICE`, `libSM`;
- `libGL` (the IRIX GL API), `libdmedia`, `libaudio`, `libC` (C++ runtime);
- the IRIX desktop libraries (4Dwm, Toolchest, Icon Catalog) if the original desktop is
  run under X11.

Some of these libraries need device access: GL and dmedia want the Newport blitter and
Vino; audio wants HAL2. The compat layer grants them **device capabilities** (MMIO, IRQ,
DMA windows) consistent with D16, or routes them through the native graphics/audio
servers. The choice is per library: a legacy GL library that drives REX3 directly is
faster but shares the raw-DMA risk; a library that talks to the native graphics server is
safer but adds a translation layer. v1 should prefer the native servers where the
protocol is understood, and fall back to direct device capabilities for libraries that
cannot be mediated without breaking compatibility.

### 6.6 X11 server

X11 is the largest single compatibility workstream. The decision is a **clean-room Rust
X11 server**, not a port of Xorg/kdrive (D28).

- **Phase 1 — display lease.** The X server owns the screen: it takes a lease from the
  native display server, sets the mode, and runs its own window manager. This is the
  fastest path to running real IRIX applications and the 4Dwm desktop.
- **Phase 2 — nested.** X runs in a native window, with a shadow framebuffer; damage is
  uploaded to the native compositor. This is the best cost/benefit point for coexistence.
- **Phase 3 — rootless.** Individual X windows are embedded as native surfaces. This is
  the most work and the best integration.

The server starts with **MSBFirst only** (every IRIX client is big-endian), 8-bit
PseudoColor and 24-bit TrueColor, and a backend seam (`memory`, `host-window`,
`Newport`, `Express`). The IRIX `libX11`/`libXm` client ABI is a separate, larger
workstream: roughly 20–30k lines for Xlib, plus Xt and Motif.

Research estimates: 11–17 person-months for a core server that runs `xterm`/`twm`;
16–25 for 4Dwm/`dtterm`; 19–30 for broad IRIX parity; GLX is a separate 30–60
person-month bridge.

### 6.7 Testing and clean-room

- The captured IRIX 6.5.7m sysroot is the header and library oracle.
- The smoke harness (ADR-0006) compiles a test program against the sysroot, runs it on
  the guest, and diffs stdout.
- The compatibility corpus includes o32 and n32 binaries, the IRIX desktop, X11 clients,
  GL/dmedia samples and audio tools.
- The container is clean-room Rust. The private `irix7/irix6` source and decompiled
  binaries inform the behavioural specification; only the clean-room Rust reaches
  `irix7/os` (ADR-0009/0014/0015).

### 6.8 Effort

| Workstream | Estimate | Source |
|---|---|---|
| Syscall ABI + ELF/ECOFF loader | TBD | project |
| IRIX libc (o32 + n32) | TBD | project |
| Xlib/Xt/Motif client ABI | 20–30k lines | X11 report |
| X11 server (core) | 11–17 PM | X11 report |
| X11 server (4Dwm/dtterm) | 16–25 PM | X11 report |
| X11 server (broad parity) | 19–30 PM | X11 report |
| GLX bridge | 30–60 PM | X11 report |
| XFS read-only server | 4–6 PM | FS report |
| XFS log replay | 3–5 PM | FS report |
| XFS write | 18–30 PM | FS report |
| I7FS | 4–6 PM | FS report |

These are research estimates, not commitments. The compatibility container and libc are
the critical path; the X11 server and XFS server can proceed in parallel once the native
object model and the VFS are stable.

---

## 7. Memory management

### 7.1 Core abstractions

The kernel uses virtual memory translation for convenience and memory management, not
primarily for hardware protection (Theseus). The core abstraction is an **owned memory
region** (`MappedPages`): exclusive ownership of a virtually contiguous region backed by
exclusively owned physical frames. This preserves the safety of Rust references (`&T`)
borrowed from the range.

For memory that external agents can modify — MMIO, DMA buffers, memory shared with a
legacy process — Rust reference guarantees cannot hold. The framework provides **untyped
memory** (`UFrame`/`USegment`, Asterinas): a read-write, plain-old-data (POD) only
interface that permits copying POD values (e.g. `u32`) without invalidating Rust
invariants. This is the safe driver-development abstraction that Tock, RedLeaf and
Theseus lack, and it is why their drivers are full of `unsafe`.

### 7.2 Address space layout

| Region | MIPS segment | Use |
|---|---|---|
| `0x0000_0000`–`0x7FFF_FFFF` | kuseg | Legacy IRIX process address spaces (MMU, ASIDs) |
| `0x8000_0000`–`0x9FFF_FFFF` | kseg0 | Kernel and native cells (unmapped, cached) |
| `0xA000_0000`–`0xBFFF_FFFF` | kseg1 | Early boot, uncached MMIO windows |
| `0xC000_0000`–`0xFFFF_FFFF` | kseg2 | Mapped kernel windows (optional) |

The kernel's kseg0 direct map is unmapped and needs no TLB entries. The R4400 JTLB has 48
entries, each mapping an even/odd page pair, and `PageMask` selects 4 KiB–16 MiB in powers
of four; it is used for kuseg (legacy processes) and the optional kseg2 window. A handful
of wired entries maps kseg2; the rest are available to legacy processes and large
mappings. `EntryHi.ASID` is 8 bits on R4000/R4400; legacy processes get distinct ASIDs,
native cells share the kernel's.

I/O is mapped through kseg1 (uncached) for registers that must not be cached, and through
4 KiB or 16 MiB pages for larger apertures. The framebuffer is not a CPU write target in
the normal path: the graphics server drives REX3 and lets the blitter write VRAM.

### 7.3 Huge pages and cache colouring

The R4400 TLB is small, so the memory manager is deliberately aggressive (D15):

- **kseg2 window**: 16 MiB pages, wired; kseg0 is unmapped and needs no entries.
- **Large legacy user mappings**: promote contiguous regions (heaps, framebuffers, DMA
  pools) to 64 KiB/256 KiB/1 MiB/4 MiB/16 MiB pages when alignment and contiguity allow.
- **Cache colouring**: the R4400 D-cache is direct-mapped, 16 KiB, 16-byte lines; the
  allocator colours large mappings so that hot regions do not collide in the cache.
- **TLB shootdown**: native cells share one address space, so a native unmap is local
  (no cross-CPU shootdown on a uniprocessor); legacy address spaces need ASID-scoped
  invalidation.
- **Page-fault path**: the kernel handles TLB misses and page faults; the fault handler
  is part of the framework and must not allocate on the critical path (pre-allocate a
  small pool).

### 7.4 Cache management

The R4400 data cache is **write-back**; the instruction cache is not coherent with the
data cache. Software must:

- flush/invalidate the D-cache (`Hit_Writeback_Invalidate_D`) over a DMA buffer before
  handing it to a device, and again after the device writes it;
- issue `sync` after the cache operations and before the device doorbell;
- after writing code, flush D over the range, then `cache 0x10` (I-cache invalidate) and
  `sync`.

The framework owns these operations and exposes a `DmaWindow` API that pairs a buffer
with its sync operations. Drivers must not call raw cache instructions outside the
framework. The R4400 line is 16 bytes; the R4600/R5000 line is 32 bytes and 2-way, so the
HAL's cache operations take the CPU model into account (D19).

### 7.5 DMA

DMA is the default for SCSI, Ethernet and audio (D16):

- A `DmaWindow` is physically contiguous, cache-managed, and owned by one cell.
- The HPC3 PBUS descriptor count is **14 bits** (16,383 bytes per descriptor); larger
  transfers are split into a descriptor chain. Descriptors are 8-byte aligned and each
  points at a physically contiguous buffer.
- Descriptor endianness is selected globally by `gio_misc` bit 1; the kernel selects
  big-endian and never changes it.
- The MC's VDMA provides a **4-entry DMA TLB** plus MASK/SUBST address protection for
  GIO/PBUS masters. The framework programs it per transfer where the device path allows,
  which gives limited address confinement. It is **not a full IOMMU**: it is small, not
  per-process, and not every master goes through it.
- Devices that generate their own addresses (GIO masters) either use PIO or must have
  the DMA TLB programmed. A driver may not program the DMA TLB directly; the framework
  does it as part of the `DmaWindow` grant.
- **Raw DMA grants** (D16) let a legacy library or a privileged native cell drive a
  device with a DMA window. The framework still confines the window where the DMA TLB
  allows. The residual risk — a buggy cell scribbling over physical memory — is accepted.

### 7.6 Allocation

- **Frame allocator**: a buddy allocator over the physical memory described by the ARCS
  memory descriptor list. No probing of nonexistent banks (the hardware does not
  bus-error cleanly).
- **Slab allocator**: per-size caches for kernel objects, with the Ostd `Slab`/`HeapSlot`
  invariants.
- **Huge-page pool**: a reserve of contiguous 16 MiB regions for the kseg2 window and
  large legacy user mappings; falls back to smaller pages under fragmentation.
- **DMA pool**: physically contiguous regions below the 32-bit limit (trivially true on
  the Indy) with cache-coloured alignment.
- **No swap in v1**: the Indy has 32–256 MiB and no reliable local disk. Native cells must
  be lean; an OOM killer terminates the largest cell. A swap file on a working local disk
  is a future option.

### 7.7 Legacy address spaces

A legacy process gets a kuseg page table and an ASID. The compatibility layer uses
`copyin`/`copyout` to access user memory safely; it never dereferences a user pointer
directly. `fork` copies the page table (COW where supported); `exec` replaces the
mapping. Page faults in a legacy process are handled by the kernel and, for a stack
growth or a demand-zero page, resolved transparently; a protection fault becomes
`SIGSEGV`.

### 7.8 Memory-mapped I/O

- Device registers are mapped through kseg1 (uncached) or through a cached mapping with
  explicit `sync` where the device allows it. Most Indy registers are uncached.
- The framebuffer and VRAM are not mapped writable by arbitrary cells; the graphics
  server owns them and exposes surfaces.
- Vino's descriptor tables and capture buffers are 4 KiB-page aligned; the Vino cell
  allocates them from the DMA pool.

---

## 8. Scheduling and concurrency

### 8.1 Scheduling policy

The scheduler has two classes (D14):

- **EDF (earliest deadline first)** for media threads: audio, video capture, display
  composition and any cell that declares a deadline. EDF is optimal on one CPU when the
  task set is schedulable; the Indy is uniprocessor, so EDF is a natural fit.
- **Fair scheduling** for everything else: shells, editors, compilers, file servers and
  background work. A weighted fair queue (CFS-like) provides proportional share and
  interactive latency.

There is **no admission control** in v1. A media cell that declares an impossible
deadline will miss it; the scheduler does not refuse the task. Overload is best-effort,
and the display server's damage accounting degrades gracefully. Admission control is a
future option if media quality becomes a problem.

Benchmarks are tracked for information, never as a build gate.

### 8.2 Scheduler structure

- One run queue per CPU. The Indy is uniprocessor; the structure is per-CPU so a future
  SMP port does not require a rewrite.
- The EDF queue is a red-black tree keyed by absolute deadline; the fair queue is a
  weighted red-black tree keyed by virtual runtime.
- A task belongs to exactly one class. A media cell can move a thread between classes
  with a capability invocation, subject to its job's rights.
- The scheduler is injected into the framework through the `Scheduler`/`RunQueue` traits
  (safe policy injection, §4.4). The framework owns the context switch; the policy is
  safe Rust.
- `is_running` prevents a task from running on two CPUs at once (Inv. 8); on a
  uniprocessor it is a debug assertion.

### 8.3 Tasks and threads

- A **task** is the schedulable unit: a saved register context, a kernel stack, a
  priority/deadline, and a pointer to its cell.
- A native cell may have many tasks (threads); the cell's `Job` capability owns them.
- A legacy process's `sproc` threads are tasks with the legacy address space and the
  IRIX scheduling parameters mapped onto the native classes.
- Kernel tasks (the idle loop, the MCP cell, the display server) are cells too; there is
  no separate kernel-thread model.

### 8.4 Timebase and timers

The hardware report is unambiguous: **CP0 Count/Compare is the only usable periodic
timer**. The 8254's interrupt output is broken on the Indy; both Linux and NetBSD use it
only as a counter.

- **Calibration**: program 8254 counter 2 in mode 0, read CP0 `Count`, poll the 8254 until
  its top byte reaches zero, read `Count` again. The delta gives the CP0 ticks per known
  1 MHz interval; round to the nearest MHz. This avoids trusting the PROM's clock report
  (Indy shipped at 50, 75, 100, 150 and 180 MHz).
- **Tick**: `Compare = Count + cycles_per_tick`; writing `Compare` clears `Cause.IP7`.
  Extend the 32-bit `Count` to a 64-bit software timebase in the tick handler.
- **Monotonic clock**: CP0 `Count` extended to 64 bits; the MC's RPSS 100 ns counter is a
  second monotonic source for profiling.
- **Wall clock**: DS1386 at 10 ms resolution; NTP over the network when available. A dead
  RTC battery means the clock starts at the compile-time epoch or at the last persisted
  value; the OS tolerates a wrong wall clock (it does not gate boot on TLS validity).

### 8.5 Interrupts

The interrupt tree is three levels deep:

```text
CPU IP2 ── HPC3 (istat0/istat1, 10 sources)
CPU IP3 ── local1 (panel, HPC DMA done, AC fail, Vino, GIO2)
CPU IP4/IP5 ── 8254 (broken; not used)
CPU IP6 ── bus error (MC parity)
CPU IP7 ── CP0 Count/Compare
INT3 (IOC2) ── graphics vblank, EISA, keyboard, serial, GIO EXP0/1
```

The HPC3 interrupt status is split across two non-adjacent registers: `istat0` reports
bits 4:0, `istat1` reports bits 9:5. The framework reads both and synthesises the 10-bit
status; a driver that reads only one will miss interrupts.

Interrupt handling is split:

- **Top half** (framework): acknowledge the source, mask it if necessary, and post an
  event to the owning cell's `Port`. It does not allocate, block or take a sleeping lock.
- **Bottom half** (driver cell): the driver's event loop reads the `Port`, services the
  device, and re-arms the interrupt through the `Irq` capability.
- **IRQ forwarding**: a cell with an `Irq` capability receives events; it cannot install a
  raw handler or touch the CPU's exception vector. The framework owns the vectors.

### 8.6 Synchronisation

- **Spinlocks** for short, IRQ-safe critical sections in the framework; interrupts are
  disabled only for the few instructions needed to update a queue.
- **Mutexes** for sleeping locks in cells; the wait queue is a `Port`.
- **Futex-like wait/wake** on user addresses for the native libc; the kernel validates the
  address against the cell's `VMAR`.
- **Priority inheritance** for EDF servers: a client donating its deadline to a server
  prevents unbounded priority inversion. The MCP, display and audio servers use it.
- **Revocation wakeups**: revoking a capability wakes any task blocked on it with a
  distinct error, not a generic timeout.

### 8.7 Real-time behaviour

Media is **soft real-time**. The following can add latency and are acknowledged:

- page faults and TLB misses (mitigated by huge pages and pre-faulting);
- DMA completion and cache maintenance;
- a driver's bottom-half scheduling;
- the display server's damage repair.

The display server's **display lease** gives a full-screen media cell the screen for the
duration of playback, so composition is not in the frame path. The audio server runs at a
higher EDF priority than the display server; audio glitches are less acceptable than a
dropped frame.

---

## 9. Drivers and hardware

### 9.1 Driver model

All drivers are **de-privileged cells** (D17). A driver:

- is safe Rust outside the privileged framework, except where a device register access or
  a DMA/cache sequence requires `unsafe` (documented, D10);
- receives **MMIO**, **IRQ** and **DMA window** capabilities from the framework;
- exposes a `ctl`/`data`/`status` file interface (the selective Plan 9 idea, D31);
- never programs the DMA TLB, the interrupt controller or the CPU's exception vectors
  directly;
- is restartable: if a driver cell faults, the supervisor can revoke its capabilities and
  restart it without rebooting the kernel (a future goal; v1 may panic on a driver fault).

The ROM's network driver and the framework's early serial/framebuffer console are the
exceptions to "all drivers are cells": they must exist before any cell can be loaded. The
ROM driver lives in the boot ROM so the ROM can fetch the kernel and boot bundle (§10);
the early console is replaced by the console cell once the boot bundle starts.

### 9.2 The `ctl`/`data`/`status` interface

Every driver exposes three files in its directory:

| File | Semantics |
|---|---|
| `ctl` | ASCII verbs, one per write: `start`, `stop`, `reset`, `mode 1280x1024`, `gain 12` |
| `data` | The byte stream: PCM samples, captured frames, SCSI blocks, network packets |
| `status` | Read-only state: counters, errors, current mode, DMA position |

This is the single most valuable Plan 9 export: it makes a driver scriptable, testable
and MCP-wrappable without a new ABI per device. A native client opens the files with
capabilities; a legacy client uses the IRIX `ioctl` surface, which the compat layer maps
onto the same files.

### 9.3 Hardware inventory

| Subsystem | Part | Key facts | Driver |
|---|---|---|---|
| CPU | R4400 (MIPS III) | 16 KiB L1 I/D, 1 MiB L2, 48-entry JTLB, 4 KiB–16 MiB pages | framework |
| Memory controller | SGI MC | 256 MiB max, 266 MB/s peak, parity, RPSS counter, VDMA DMA TLB | framework |
| I/O hub | HPC3 | PBUS DMA, SCSI, Ethernet, audio, serial, parallel; 30 MB/s aggregate | framework + cells |
| SCSI | WD33C93A/B | Fast SCSI-2, 10 MB/s, HPC3 DMA | SCSI cell |
| Ethernet | Seeq 8003 | 10 Mbit, 8 registers, PBUS DMA | network cell |
| Audio | HAL2 | One playback + one capture, no hardware mixing | audio cell |
| Graphics | Newport XL (REX3/RB2/VC2/XMAP9) | 8-bit or 24-bit planar, 2D blitter, no 3D, 140 MHz pixel clock | graphics cell + display server |
| Video in | Vino | Input-only, composite/S-Video, 4 KiB descriptors, two channels | Vino cell |
| RTC/NVRAM | Dallas DS1386 | Wall clock, NVRAM, watchdog | framework + RTC cell |
| Expansion | GIO32/GIO64 | Product ID word, no hotplug, 32-bit addressing, DMA TLB | per-card |
| Firmware | SGI PROM/ARCS (factory) | o32/ECOFF, memory descriptor list, environment variables; replaced by the n32 Rust ROM | boot |

### 9.4 Subsystem notes

**CPU, caches and TLB.** Caches power up disabled and undefined. The kernel enables them
only after the memory controller is known-good. The D-cache is write-back; DMA requires
explicit flush/invalidate and `sync`. The R4000 exception vector with `Status.BEV` set
lands inside the PROM; early trap handlers must account for that. The R4400 has no
`wsbh`, so byte swaps are shift/mask sequences (9 instructions for a 32-bit swap); this
matters for EROFS and network headers.

**HPC3.** The interrupt-status split is a documented bug; drivers must read `istat0` and
`istat1` and synthesise the status. PBUS DMA descriptors are 8 bytes, 8-byte aligned,
with a 14-bit byte count; each descriptor points at a physically contiguous buffer. The
HPC3 aggregate bandwidth is about 30 MB/s, so SCSI, Ethernet and audio can run
concurrently without contention; a "fast" Indy disk tops out around 10 MB/s.

**WD33C93 SCSI.** Fast SCSI-2, narrow, single-ended, 10 MB/s. The HPC3 SCSI DMA engine
handles the bus; the driver programs the WD33C93 registers and the HPC3 channel. The
direction bit has a documented discrepancy between the two OS drivers; the hardware
report records it. The driver must handle disconnect/reselect and parity errors.

**Seeq 8003 Ethernet.** Eight registers, PBUS DMA, 10 Mbit. The frame format has a quirk
(the hardware report records it); the driver must handle the Seeq's external byte count
and the HPC3 descriptor flags. The network cell runs smoltcp over this driver.

**HAL2 audio.** One playback stream and one capture stream; **no hardware mixing**. The
audio server mixes in software (D29), resamples if necessary, and feeds the codec through
DMA. The codec's register access protocol is indirect (address/data); the hardware report
records it.

**Newport graphics.** REX3 is the drawing engine; RB2 formats VRAM; VC2 generates video
timing; XMAP9 is the RAMDAC/pixel formatter. 8-bit mode is indexed with a 256×24 CLUT;
24-bit mode is three 8-bit planes, not packed RGB. There is no Z-buffer and no geometry
engine; 3D is software rasterisation. The pixel clock ceiling is 140 MHz; 1280×1024 is
the practical maximum for 8-bit, while 24-bit may be VRAM-limited. The graphics server
owns the blitter; apps receive surfaces, not registers.

**Vino video capture.** Input-only. It captures composite/S-Video through the SAA7191B
decoder and DMAs YUV 4:2:2 into memory. Descriptors cover one 4 KiB page each; the table
is quadword-aligned and the last descriptor must have STOP or JUMP set. Two channels can
capture concurrently. There is no video output without the separate Indy Video GIO board;
that board is not modelled by iris, so video output is deferred (D6, §17).

**DS1386 RTC/NVRAM/watchdog.** The RTC provides wall-clock time at 10 ms resolution; the
NVRAM holds the MAC address and environment; the watchdog can reset a hung machine. A
dead battery means the clock is wrong at boot; the OS tolerates it. The MCP recovery
strategy uses the watchdog (§10.6).

**GIO expansion.** GIO32 and GIO64 slots with a product ID word; no configuration space,
no hotplug. The MC's VDMA DMA TLB is the only remapping layer for GIO masters. Real
Indy graphics boards and the Indy Video board are identified by their product ID; the
full SGI product-ID table is incomplete in the available sources, so the OS matches on
known IDs and on the ARCS device tree.

**PROM/ARCS.** The *factory* PROM is o32/ECOFF-only. It provides the memory descriptor list,
environment variables, console and disk/network loaders. The Rust ROM replaces the factory
firmware entirely: it is a raw n32/MIPS III image executed from the PROM socket at
`0xBFC00000`, not an ECOFF image loaded by ARCS. The factory PROM remains a fallback and the
source of the reset-time hardware state.

### 9.5 Interrupts and timers

See §8.4–§8.5. The driver-facing rule is: a driver never touches the CPU's exception
vectors, the HPC3 interrupt mask, or the CP0 timer. It receives `Irq` events and posts
completions. The framework owns the top half.

### 9.6 Out of scope and deferred

- **ISDN**: out of scope (D6).
- **Video output**: requires the Indy Video GIO board; deferred until the real Indys are
  probed (D6, §17).
- **XZ/Extreme and IMPACT graphics**: not the iris default; detected and supported later.
- **3D**: no hardware; software rasterisation is not a v1 goal.
- **IndyCam**: Vino capture is supported; the camera itself is a video source, not a
  separate driver.

### 9.7 Hardware detection

At boot the kernel reads the ARCS memory descriptor list for RAM, probes the GIO
graphics slot product ID, probes the expansion slots, and reads the DS1386 NVRAM for the
MAC address and environment. It selects the graphics driver (Newport 8-bit, Newport
24-bit, XZ/Extreme, IMPACT), the video path (Vino present/absent, Indy Video
present/absent), and the RAM size. It does not probe nonexistent memory banks. The
resulting hardware description is a capability tree handed to `init`.

### 9.8 Bring-up order

1. Serial console (Z85C30 in IOC2) and early printk.
2. DS1386 RTC/NVRAM and the CP0 timer calibration.
3. HPC3 interrupt controller and the 8254 calibration reference.
4. SCSI (WD33C93) and the block cell.
5. Seeq 8003 and the network cell.
6. Newport and the display server.
7. HAL2 and the audio server.
8. Vino and the capture cell.
9. The MCP cell and the agent loop.

The **iris emulator work starts with graphics (Newport) and Ethernet (Seeq)** — the two
devices on the critical path for the display and boot loops. SCSI, HAL2 and Vino follow.

---

## 10. Boot and deployment

### 10.1 Boot paths

There are three boot paths, in order of preference:

1. **Production: Rust ROM → R2 → kernel → EROFS root.** The ROM has the Seeq driver and a
   small network stack; it fetches the kernel and boot bundle from R2, verifies their
   SHA-256 digests, jumps to the kernel, and the kernel mounts the root image from R2 over
   HTTP Range.
2. **Fallback: SGI PROM/ARCS → local disk or TFTP → kernel.** The factory PROM loads an
   o32 ECOFF image. This is the tracer-bullet path (ADR-0016) and the recovery path when
   the network or R2 is unavailable.
3. **Emulator: iris direct load.** The emulator can load an ELF/ECOFF image directly,
   bypassing the ROM; this is the fastest development loop.

### 10.2 Phase 1: the Rust ROM

The ROM is a pure MIPS III assembly stub; the kernel owns everything after the jump:

| Property | Current |
|---|---|
| Size | about 476 KiB PROM image (inside the 512 KiB window) |
| ISA | n32/MIPS III (raw PROM image; the factory ARCS loader is not used) |
| Network | kernel: Seeq 8003, HPC3 PDMA |
| Stack | kernel: smoltcp, IPv4 + DHCPv4 (with renewals), DNS, NTP, ICMP |
| Application | kernel: HTTP/1.1 and HTTPS/TLS 1.3 with verified chains |
| Integrity | SHA-256 of fetched bodies; modules are gated on their digest |
| Console | serial (Z85C30) and a plain-text telnet console on guest port 23 |
| Payload | LZ4-compressed kernel, decompressed to `0x8804_0000` (physical `0x0804_0000`) |
| Handoff | `$a0` = 0 (no BootInfo), `$a1` = RAM bytes, `$a2` = RAM base, `$sp` = kernel stack; the kernel initialises the network, console and clock once |
| Modules | flat images linked at `0x8840_0000`, fetched over HTTPS into RAM, digest-checked and called through a C service table |

The kernel's network driver is an exception to "all drivers are cells" (D17): it must exist
before any cell can be loaded. It is a minimal polled/DMA driver in the kernel, not the full
network cell.

The ROM and kernel are one program at two link addresses, not a firmware/kernel pair. The
ROM is a serial-only assembly stub: it sizes and tests DRAM, decompresses the kernel and
jumps. The kernel initialises the Seeq device, smoltcp `Interface`, `SocketSet`, clock and
telnet console once, in place, and keeps them live. There is no second network stack and no
ROM-owned network state to inherit.

TLS is deliberately in the kernel: it is the transport that makes module downloads
trustworthy. The core verifies chains against an embedded SSL.com TLS ECC Root CA 2022,
fetches modules over HTTPS, and gates execution on their SHA-256. The ROM itself has no
network stack.

### 10.3 The boot bundle

A future **boot bundle** (fetched by the kernel over HTTPS, not by the ROM) is an
initramfs-like container (a small EROFS image or a simple archive) containing:

- the **network cell** (Seeq driver + smoltcp + `/net`);
- the **block cell** (SCSI + block device + RAM/disk cache);
- the **FS cell** (EROFS reader + overlay + optional 9P client);
- the **init cell** (mounts the root, starts the servers);
- the **MCP cell** (the development harness; see §14);
- the **console/serial cell** and the **RTC cell**.

The bundle is kept small (target ≤ 16 MiB) because it travels over 10 Mbit Ethernet. The
kernel and the bundle together should fit in a few seconds of transfer on a LAN and under
a minute over the internet. The root image is **not** in the bundle; it is mounted on
demand.

### 10.4 Root image

The root filesystem is a **standard little-endian EROFS** image (D21):

- The image is monolithic, read-only and content-addressed: its R2 object name is its
  SHA-256, so range GETs are safe to cache at the edge.
- The reader is a new `no_std` crate; it decodes fields through a `ByteOrder` codec
  (`LE` default, `BE` available). It does **not** port `erofs-rs` (13k lines, `std` and
  `rustix` coupled).
- The reader supports the core format first (superblock, compact inodes, flat and inline
  layouts, dirents), then LZ4 pclusters, then fragments/ztailpacking.
- The image is fetched over HTTP Range, in 64–256 KiB chunks, with a read-ahead window and
  an LRU block cache. Metadata is decoded on demand; a full 1.5 MiB metadata pass costs
  about 45 ms at 75 MHz, which is negligible against the network.
- The image is verified by SHA-256 before mount; per-block Merkle verification is a future
  option, not v1.

### 10.5 R2, GitHub and the free tiers

| Service | Free tier | Role |
|---|---|---|
| Cloudflare R2 | 10 GB-month storage, 1M Class A + 10M Class B ops/month, free egress | OS image, boot bundle, large user media |
| GitHub Free | 10 GB per repo, 2 GB push, 1 MB recommended object (100 MB hard), 6 pushes/min, 15 reads/s | Source, small user files, Git history |
| GitHub LFS | 10 GiB storage + 10 GiB/month bandwidth | Small binaries only; not the OS image |
| jsDelivr | Free CDN for public GitHub/npm content, with file-size limits | Optional mirror, not the primary |

The OS image lives on R2 because egress is free and the object is immutable. GitHub is the
source of truth for code and user data; it is not a block device. The "free unlimited"
claim is false for GitHub LFS; the architecture does not depend on it.

### 10.6 Local cache and watchdog

- A working SCSI disk may hold a **block cache** of the root image and a writable overlay.
  The cache is optional: the boot path works with no disk.
- The cache is keyed by the image SHA-256, so an update invalidates it naturally.
- A/B update slots are a future option; v1 fetches the current image and caches it.
- The **DS1386 watchdog** resets a hung machine. The MCP client reconnects after the reset
  and resumes the development loop. The watchdog is armed only after the kernel has
  started the periodic service routine (the MC watchdog is separate and must not be armed
  during long interrupt-disabled sections).

### 10.7 Boot trust

This is a hobby OS (D26):

- SHA-256 digests of the kernel, boot bundle and root image are checked.
- The digests are baked into the ROM or fetched from a signed manifest; v1 bakes them in.
- There is no measured boot, no TPM, no secure boot and no signature ceremony.
- A dead RTC does not block boot; the wall clock is set from the DS1386 if valid, from the
  compile-time epoch otherwise, and from NTP when the network is up.
- The public image is fetched over HTTP; integrity is the digest, not TLS. User data
  (GitHub) uses TLS where the stack supports it.

### 10.8 Endianness

The Indy is big-endian; the cloud is little-endian. The boundaries are explicit:

- **EROFS**: standard little-endian fields; the reader byte-swaps through the codec.
- **Network headers**: big-endian on the wire; smoltcp handles them.
- **Git objects**: byte streams; the Git cell parses them without assuming host order.
- **R2/S3 XML**: UTF-8 text; no endianness.
- **Hashes and lengths**: byte arrays and explicit `u64` conversions.
- **No `transmute` across an endianness boundary.** Use `from_be_bytes`/`to_le_bytes`.

The R4400 has no `wsbh`; a 32-bit swap is 9 ALU instructions. This is cheap relative to
decompression and network I/O, which is why standard EROFS is the right call.

### 10.9 Boot sequence

1. ROM initialises the CPU, caches, memory controller (read-only), serial and Seeq.
2. ROM brings up smoltcp, DHCP (or static), DNS and the R2 endpoint.
3. ROM fetches `kernel` and `boot.bundle`; verifies SHA-256; loads them at the agreed
   addresses; flushes caches.
4. ROM jumps to the kernel entry with a boot-info structure (memory map, console, MAC,
   R2 URL, digests).
5. Kernel initialises traps, TLB, frame allocator, scheduler and capability table.
6. Kernel starts the boot-bundle cells: network, block, FS, init, MCP.
7. The network cell reinitialises the Seeq and takes over `/net`.
8. The FS cell mounts the EROFS root from R2 over HTTP Range; the block cell caches blocks
   in RAM and, if present, on the local disk.
9. The MCP cell starts the control plane; the agent connects.
10. `init` starts the display server, audio server, graphics server and user session.
11. The watchdog is armed; the idle loop runs.

### 10.10 Failure modes

| Failure | Behaviour |
|---|---|
| R2 unreachable | ROM retries; if it fails, fall back to the PROM/local image; the OS can boot a minimal rescue bundle |
| Hash mismatch | Refuse to boot the image; log the expected/actual digest; fall back |
| Root image 404 | Boot the boot bundle's rescue shell; network cell reports the error |
| Kernel hang | DS1386 watchdog resets; MCP reconnects |
| Disk cache corrupt | Ignore the cache; re-fetch from R2; verify the digest |
| RTC dead | Boot proceeds with the compile-time epoch; NTP corrects later |

---

## 11. Storage and filesystems

### 11.1 Overview

| Layer | Choice | Status |
|---|---|---|
| Root image | Standard little-endian EROFS, read-only, HTTP Range from R2 | v1 |
| Writable overlay | RAM overlay; optional local-disk overlay | v1 |
| Native writable FS | I7FS (big-endian, 4 KiB, CRC32C, extents, B+tree, journal) | later |
| IRIX compatibility | Clean-room XFS v4 server, read-only first, log replay next, write later | v1 read-only |
| User data | Periodic batched Git commits (small) + R2 (large) | v1 |
| Remote mounts | Optional 9P2000; not the root transport | later |

### 11.2 Root image: EROFS

EROFS is the right root format because it is read-only, compressed, random-access and
well specified. The project **does not fork it to big-endian** (D21). The numbers:

| Consideration | Standard EROFS + byte swap | EROFS-BE fork |
|---|---|---|
| Metadata to re-encode | 1–4% of the image | same |
| Added CPU per cold boot | ~45 ms (full pass) or ~0 (lazy) | 0 |
| Added CPU per path walk | 9.6 µs (warm) | 0 |
| Versus LZ4 decode | ~1/250 | — |
| Versus a 10 Mbit R2 fetch | ~1/2400–1/6000 | — |
| Independent readers | Linux, `fsck.erofs`, `dump.erofs`, `erofsfuse`, `erofs-rs` | none |
| Host verification | mount on the CI host | impossible without the fork |
| Writer work | none (`mkfs.erofs`) | 3–6 weeks |

The standard format keeps the only independent oracle: mount the image on the CI host,
run `fsck.erofs`, list it with `dump.erofs`, read it with `erofs-rs`, and compare the
tree. A BE fork saves 45 ms on a boot dominated by a 10 Mbit fetch. Do not fork.

Implementation notes:

- New `no_std` reader; core format first, then LZ4, then fragments/ztailpacking.
- `mkfs.erofs` in CI, pinned and hermetic, with `-T` and `-U clear` for reproducibility.
- Golden images (plain, LZ4, fragments, xattrs) in the test corpus.
- Decode on demand into native structs with a small LRU; do not swap a whole metadata
  region in place (EROFS interleaves inline data and names with numeric fields).
- The R2 object name is the SHA-256; verify the digest before mount.

### 11.3 Overlay

The root is read-only. Writes go to an overlay:

- `/tmp`, `/var/run` and other volatile paths: RAM overlay.
- `/etc` and system configuration: RAM overlay, optionally persisted to a local disk or a
  Git-backed configuration repo.
- `/home`: Git-backed (small files) + R2 (large media); the overlay caches writes locally
  and commits periodically.
- If a working SCSI disk is present, the overlay can live on it; if not, it is RAM-only
  and lost on reboot. The OS must be usable without persistence.

### 11.4 XFS compatibility server

IRIX 6.5 disks are XFS v4. The compatibility server is a clean-room, userspace cell:

- **Read-only first**: superblock, allocation-group headers, inode core, extents and bmap
  B+tree, the four directory forms, symlinks, and a clean-log check. 4–6 person-months.
- **Dirty-log replay**: IRIX writes `h_fmt = XLOG_FMT_UNKNOWN` and a big-endian log; Linux
  refuses to recover it. IRIX 7, being big-endian, can accept the IRIX format and replay
  it. 3–5 person-months.
- **Write**: 18–30 person-months; not on the critical path.

XFS v4 metadata is big-endian, which is a gift on MIPS: no byte swapping on the hot path.
The v4 format has no CRCs, no rmap, no reflink, no sparse inodes and no big timestamps,
which makes a read-only implementation tractable. The CRC field in v5 is little-endian
even in big-endian metadata — a trap the implementation must handle if v5 is ever added.

The XFS server runs as a userspace cell over a block-device capability. The kernel brokers
the SCSI DMA through the block cell; the XFS cell does not touch WD33C93 registers.

### 11.5 I7FS (native filesystem)

I7FS is the native writable filesystem for new disks. Its design:

| Property | Choice |
|---|---|
| Block size | 4 KiB |
| Byte order | Big-endian, explicit `byte_order` field |
| Checksums | CRC32C on superblock, AG headers, inodes, directory blocks, journal records, B+tree blocks |
| Allocation | Extents; allocation groups |
| Directories | B+tree |
| Journal | Internal, metadata-only |
| Features | No reflink, no rmap, no quotas, no compression in v1 |
| Tools | `mkfs`, `fsck`, image builder |

Estimated 4–6 person-months for a solid v1 including tools and tests. The native FS
decision is deferred until the compatibility layer and the root image are working (D23,
§17); RedoxFS is the fallback if I7FS slips, but its little-endian format and Redox
coupling erode the schedule advantage.

### 11.6 User data and Git

- **Small files** (documents, code, configuration) live in a Git repository owned by the
  user. The OS batches writes and commits periodically (every few minutes or on logout),
  staying under GitHub's 6 pushes/min limit.
- **Large media** (photos, video, audio) live in R2; Git holds pointers/metadata. GitHub
  LFS is an option for small binaries but its 10 GiB/month bandwidth cap makes it
  unsuitable for the OS image or large libraries.
- **Implementation**: a userspace `home` cell uses `gix` to read/write Git objects and
  push/pull over HTTPS. It keeps a local cache, queues commits while offline, and resolves
  conflicts with a 3-way merge or per-device branches.
- **Limits**: keep repositories under 1 GB; keep individual objects under 1 MB where
  possible; never commit build artefacts.

### 11.7 9P remote mounts

9P is **optional and secondary** (D31):

- Use **9P2000** semantics, not 9P2000.L (which is Linux-VFS-shaped and encodes Linux
  errno/stat semantics).
- Build `irix7-9p` (client + server); the existing Rust ecosystem is server-heavy and has
  no mature `no_std` pipelining client.
- Use 9P for **control** and for mounting remote synthetic filesystems, not for the root
  image and not for bulk data. Bulk transfer uses HTTP Range or shared memory.
- A 9P connection is a capability: the client's namespace is a projection of its
  capability set, not a security boundary.
- Plan 9's authentication protocols (`p9sk1` etc.) are obsolete; use TLS 1.3 and Ed25519
  where authentication is needed.

### 11.8 Block cache

- The block cell exposes a block-device capability; the FS cell reads blocks through it.
- The cache is an LRU keyed by image digest + block index, with a read-ahead window of
  64–256 KiB. Immutable content-addressed images make cache invalidation trivial.
- The cache lives in RAM and, if a working disk exists, in a local cache partition.
- There is no writeback to R2; the root is read-only. User data writes go to the overlay
  and then to Git/R2.

### 11.9 Crash consistency

- EROFS is read-only; a crash cannot corrupt the root image.
- The overlay is either RAM (lost on crash) or a journalled local filesystem (I7FS) once
  available.
- XFS log replay handles a crashed legacy volume; the XFS server refuses to mount a dirty
  log until replay is implemented.
- Git commits are atomic; a crash during a push leaves the local repository consistent and
  the remote either updated or not.

---

## 12. Networking

### 12.1 Hardware

The Seeq 8003 is a 10 Mbit Ethernet controller with eight registers and HPC3 PBUS DMA.
The practical throughput is about 1.25 MB/s; the HPC3 aggregate is ~30 MB/s, so the
network is the bottleneck for boot. The driver is a de-privileged cell; the ROM has its
own minimal copy.

### 12.2 Stack

The network cell runs **smoltcp** with IPv4 and IPv6, TCP and UDP, DHCP and DNS. It
exposes:

- a POSIX-like socket API to legacy processes (through the compat layer);
- a `/net` synthetic tree (`clone`, `ctl`, `data`, `local`, `remote`, `status`) for
  native clients and scripts;
- an HTTP client for R2 Range requests;
- a Git smart-HTTP client for GitHub (via the `home` cell);
- optional 9P over TCP/TLS for remote mounts.

TLS is provided by `rustls` where needed (GitHub, authenticated R2). The public OS image
is fetched over HTTP and verified by digest, so a dead RTC does not block boot.

### 12.3 The `/net` tree

Following the selective Plan 9 idea (D31):

```text
/net/
  tcp/
    clone        write an address to allocate a connection
    <n>/
      ctl        connect, hangup, keepalive
      data       the byte stream
      local      local address
      remote     remote address
      status     state and counters
  udp/
  ip/
  dns/
  arp/
```

The tree is the API. A native client opens `clone`, writes the remote address, reads and
writes `data`, and reads `status`. A legacy client uses `socket`/`connect`/`read`/`write`,
which the compat layer maps onto the same tree. This is the Plan 9 idea that survives
because it makes the network scriptable and MCP-wrappable.

### 12.4 Boot networking

The ROM's network stack is a reduced smoltcp:

1. Reset and initialise the Seeq; read the MAC from the DS1386 NVRAM.
2. DHCP (or a static address from NVRAM) and DNS.
3. Resolve the R2 endpoint; open a TCP connection.
4. HTTP GET the kernel and boot bundle; verify SHA-256; load.
5. Hand the network state (MAC, IP, gateway, DNS) to the kernel in the boot-info
   structure; the kernel's network cell reinitialises the device.

The ROM does not run TLS in v1. The public image is not secret; the digest is the
integrity boundary. If the R2 bucket is private, the ROM can use a pre-signed URL baked
into the boot configuration; TLS is future work.

### 12.5 Performance

| Payload | At 10 Mbit (theoretical) | Research model |
|---|---|---|
| 64 MiB root image | ~51 s | 109–273 s (full image, research range) |
| 256 MiB root image | ~205 s | 109–273 s (full image, research range) |
| 16 MiB boot bundle | ~13 s | — |
| One 64 KiB block | ~0.5 ms | — |

The research range is for a full 64–256 MiB image and includes protocol overhead; the
on-demand boot fetches far less. The kernel boots from the boot bundle, mounts EROFS, and
faults in only the blocks it needs. The read-ahead window and the block cache turn a cold
boot into a few megabytes of traffic, not the whole image. The first boot is slower;
subsequent boots hit the RAM/disk cache.

### 12.6 Endianness and protocols

- Network headers are big-endian; smoltcp handles them.
- HTTP and Git are byte streams; no host-order assumptions.
- EROFS metadata is little-endian; the codec swaps.
- The Seeq's frame format has a hardware quirk; the driver follows the hardware report.

### 12.7 IPv6

IPv6 is supported (dual-stack) because smoltcp provides it and because a modern LAN may be
IPv6-only. The Indy's 10 Mbit link and the R2 endpoint are the practical constraints, not
the protocol. SLAAC/DHCPv6 are supported; NAT64 is not a v1 goal.

### 12.8 Rejected

- NFS and SMB: a second security model and a large stack for coherence guarantees the OS
  does not need.
- 9P as the root transport: metadata latency is fatal over a WAN.
- 9P as bulk transport: per-message copies; use HTTP Range or shared memory.
- Plaintext 9P across a trust boundary: use TLS or a local shared-memory transport.

---

## 13. Display and media

### 13.1 Hardware constraints

The Indy's display pipeline is a 2D framebuffer with a blitter, not a GPU:

- Newport XL: REX3 drawing engine, RB2 VRAM formatter, VC2 timing generator, XMAP9
  RAMDAC. 8-bit indexed or 24-bit direct colour (three 8-bit planes, not packed RGB).
- No alpha blending, no scaling, no transforms, no Z-buffer, no geometry engine.
- 1280×1024 is the practical maximum for 8-bit; 24-bit may be limited by VRAM (the exact
  size is undocumented); the pixel clock ceiling is 140 MHz.
- The framebuffer is 1.25 MiB at 8 bpp and 3.75 MiB at 24 bpp for 1280×1024.

The bandwidth arithmetic decides the architecture. A full-screen recomposition at 60 Hz
is 79 MB/s at 8 bpp and 236 MB/s at 24 bpp; a 32-bit ARGB pipeline is 315 MB/s. The
memory controller peaks at about 266 MB/s, and GIO PIO writes measured about 66 MB/s.
An always-composite ARGB pipeline is therefore outside the hardware's budget before the
CPU does any work. The native path is **server-side drawing into retained images**, with
damage-driven repair and an 8-bit indexed default (D27).

### 13.2 Native display server

One userspace **display cell** owns the Newport pipeline (VC2 timing, XMAP9 palette,
REX3 command FIFO, hardware cursor) and multiplexes drawing for all native clients:

- **Server-side drawing** into server-owned images, with a compact binary opcode protocol
  (blit, fill, span, glyph run, colour expand, clip), not client-rendered full surfaces.
- **Retained window images** in main memory, with direct in-place drawing into the scanout
  image for the topmost visible window.
- **Damage-driven repair**, not per-frame recomposition: the common path is a clipped
  REX3 blit straight into the scanout image; the server composes only for overlap, move,
  raise and expose.
- **No alpha, no transforms, no scaling** in the native path; an opaque 8-bit indexed
  visual is the default, with 24-bit direct colour as an option at reduced resolution.
- **One hardware cursor** in the XMAP9 auxiliary plane, never a software cursor.
- **Display leases** for full-screen media cells, so an EDF video thread never pays for
  composition.
- **Capability-addressed surfaces** (rights: draw, present, read, map) that fit the
  framekernel object model, with a zero-copy mapped fast path for trusted native cells in
  the shared address space.

Estimated 8,000–12,000 lines of Rust, 10–12 person-months for a production v1. The
lineage is Plan 9 `8½`/`rio` and BeOS `app_server`, not Wayland. In v1 the display server
and the graphics server are the same cell; §13.4 describes the split if the blitter
becomes a bottleneck.

### 13.3 Native desktop

The native desktop is **new code, IRIX-inspired** (D5). It should feel like Indigo Magic —
the Toolchest, the Icon Catalog, the desktop menu, the file manager — without being the
original binaries and without the 1993 limitations.

The desktop is a cell (or a small set of cells) built on the display server's surface API.
It provides:

- a launcher/Toolchest-equivalent;
- a file manager with IRIX-style icons and drag-and-drop;
- a window manager with IRIX-like decorations and keyboard navigation;
- a settings panel (display, audio, network, users);
- a terminal and a text editor;
- integration with the MCP server for remote inspection (optional).

The UI toolkit is an open question (§17). The constraint is that it must emit the display
server's drawing protocol and must not require client-side full-surface recomposition,
alpha blending or GPU acceleration. A small immediate-mode toolkit or a retained-mode
toolkit over the surface API are both viable; the choice should follow a prototype, not a
fashion.

### 13.4 Graphics server

The **graphics cell** owns the REX3 blitter and the VRAM. Native applications do not touch
REX3 registers; they receive **surfaces** (capability-addressed framebuffer regions) and
submit drawing operations through the display server or the graphics server. The kernel
console is the only kernel-side drawing path (text output before the display server
starts).

- The display server and the graphics server may be the same cell in v1; splitting them
  is a performance optimisation if the blitter becomes a bottleneck.
- A full-screen media cell can take a **display lease** and drive the scanout directly
  through the graphics server, bypassing composition.
- The X11 server (§6.6) uses the same surface API in its nested and rootless phases.

### 13.5 X11

X11 is the compatibility path, not the native path. The clean-room Rust server is
described in §6.6. Its phases are:

1. **Display lease**: X owns the screen and runs its own window manager (4Dwm, `twm`).
2. **Nested**: X runs in a native window with a shadow framebuffer.
3. **Rootless**: individual X windows are native surfaces.

The native desktop and the X11 server share the display server's surfaces; the X11
server is a native cell, and legacy X clients are MMU processes.

### 13.6 Audio

The **audio cell** owns the HAL2 codec and mixes in software (D29):

- HAL2 has one playback stream and one capture stream and **no hardware mixing**.
- The audio server accepts PCM streams from native and legacy clients, mixes them, applies
  gain and resampling, and feeds the codec through DMA.
- It runs at a higher EDF priority than the display server; audio glitches are less
  acceptable than a dropped frame.
- The legacy `libaudio` is a client of the audio server; it does not own the codec. This
  reconciles the "libraries get device caps" decision with the single-owner rule: the
  audio server holds the device capability, and the library holds a channel capability.
- Sample-rate conversion is software; the HAL2 clock tree is programmed by the audio cell.

### 13.7 Video

- **Vino** is capture-only: composite/S-Video through the SAA7191B, YUV 4:2:2, two
  channels, 4 KiB descriptors. The Vino cell exposes `ctl`/`data`/`status` and a capture
  API.
- **Video output** requires the separate Indy Video GIO board, which iris does not model.
  It is deferred until the real Indys are probed (D6, §17).
- **3D** is software rasterisation only; not a v1 goal.

### 13.8 Media scheduling

- Audio and video capture threads declare EDF deadlines.
- The display server runs in the fair class but gets a bounded latency budget; a
  full-screen media cell uses a display lease to remove composition from the frame path.
- The audio server has priority inheritance for its clients.
- Soft real-time: occasional glitches are acceptable; hard guarantees are not claimed.

### 13.9 Why not Wayland

Wayland's `wl_shm` path makes client-side ARGB buffers the default: 5 MiB per 1280×1024
frame, 315 MB/s at 60 Hz, with per-pixel alpha blending that Newport does not implement.
The memory controller peaks at 266 MB/s. A Wayland compositor is also a Linux-shaped port
(libudev, libinput, libgbm, libEGL, libxkbcommon, DRM/KMS, Unix fd passing) onto a kernel
that has none of those interfaces. The right ideas — damage, buffer handles, frame
callbacks — are adopted; the protocol and the rendering model are not.

### 13.10 Effort

| Workstream | Estimate | Source |
|---|---|---|
| Native display server | 10–12 PM, 8–12k lines | window report |
| Native desktop | TBD (prototype first) | — |
| Graphics server | TBD (may share the display server) | — |
| Audio server | TBD | — |
| Vino capture | TBD | — |
| X11 server | 11–30 PM depending on parity | X11 report |
| GLX bridge | 30–60 PM | X11 report |

---

## 14. Control plane (MCP)

### 14.1 Purpose

The MCP server is the OS's **remote control plane** and the project's development
harness (D30). An agent connects to the running Indy, inspects and drives it, and closes
the edit–build–deploy–observe loop. It is not a late feature; it ships in the boot bundle
and is one of the first cells brought up.

### 14.2 Architecture

- A **de-privileged native cell** on the Indy, not a host-side proxy. It has full root
  authority over the OS, like a local root shell.
- Transport: **Streamable HTTP + JSON-RPC** (the MCP standard).
- Authentication: a **bearer token**. There is no audit log in v1.
- The cell is a native cell in the shared address space; a bug in it is a kernel-level
  bug. The watchdog is the recovery path.
- The server is backed by the OS's own interfaces: `/proc`, `/net`, `/dev`,
  `ctl`/`data`/`status` files, capability invocations. It is not a separate ABI.

### 14.3 Tools

| Tool group | Operations |
|---|---|
| Shell / exec | Run a command, capture stdout/stderr/exit status, set environment and cwd |
| Files | Read, write, list, stat, remove, rename, create directories; handle-based where possible |
| Processes | List cells/tasks, start, stop, kill, inspect state, read `/proc` |
| Devices / MMIO | Map a device capability, read/write registers, allocate DMA windows (full root) |
| Screen capture | Capture the framebuffer or a surface as PNG/raw |
| Serial console | Read/write the Z85C30 console, including the PROM/boot log |
| Reset / power | Reboot, power off, warm reset, watchdog arm/disarm |
| GDB / debug | Attach to a cell, set breakpoints, read/write memory and registers, single-step |
| Keyboard / mouse | Inject key and pointer events for the computer-use loop |

The tool surface is deliberately broad because the user chose "complete native control".
The risk is prompt injection: a file or a tool result can instruct the model to reset the
machine or overwrite the kernel. With full root and no audit, the only mitigations are the
bearer token, network isolation, the watchdog and the user's supervision. This is
accepted for a hobby OS; the future hardening path is capability-scoped tools, an audit
log and human approval for destructive operations.

### 14.4 Transport and authentication

- Streamable HTTP on a configurable port, JSON-RPC 2.0.
- Bearer token in the `Authorization` header; the token is provisioned at image build
  time or in the boot configuration.
- No TLS in v1 (the user chose bearer token over TLS + token). Run it on a trusted LAN or
  through an SSH tunnel if the network is untrusted.
- The server binds to the Indy's network interface; it does not listen on the public
  internet by default.

### 14.5 The development loop

1. The agent edits the source on the host.
2. The host cross-compiles the kernel/cells.
3. The agent uploads the artefacts to the Indy (or to R2 for a full image update) through
   the MCP file tools.
4. The agent reboots through MCP (or the watchdog recovers a hang).
5. The agent reads the serial log and the screen capture to see the result.
6. The agent attaches GDB for a crash, or runs a smoke test.
7. Repeat.

The loop is the reason the MCP cell is in the boot bundle. A kernel that cannot be
inspected remotely is a kernel that takes minutes per iteration.

### 14.6 Early bring-up

The first MCP milestone is deliberately small:

- serial console read/write;
- shell/exec;
- file read/write;
- reset/power;
- screen capture.

GDB, device/MMIO and keyboard/mouse follow once the display and the debug stubs exist.
The token and the HTTP transport are in the first milestone.

### 14.7 Watchdog

The DS1386 watchdog resets a hung machine (§10.6). The MCP client detects the disconnect,
waits for the reboot, reconnects with the token, and resumes. The watchdog is armed only
after the periodic service routine is running; a long interrupt-disabled section must not
trip it.

### 14.8 Open questions

- The exact MCP tool schemas and error model.
- GDB stub protocol and multi-cell debugging.
- Whether to add an audit log and approval flow later.
- The exact granularity of the device/MMIO tools (typed operations vs raw register access); v1 exposes both under full root.

---

## 15. Clean-room and verification

### 15.1 Hybrid transcription

All kernel Rust is produced by the **hybrid transcription** pipeline (SOURCE → AST →
RUST) orchestrated by `irix7/harness` (ADR-0014). The harness enforces the structural
split and runs the **similarity screen** (ADR-0015) so that no SGI-derived expression
reaches the public `irix7/os` repo.

- Dirty readers see the private `irix7/irix6` source and emit the behavioural
  specification (stays private).
- Clean writers see only the specification and emit the Rust published to `irix7/os`.
- Only clean-room Rust reaches `irix7/os`; the leaked IRIX sources and decompiled binaries
  remain in the private repo.

### 15.2 Research inputs

The advanced architecture uses public research reports, not SGI source:

- `../research/indy-hardware.md` — the Indy hardware reference (SGI specifications,
  Linux/NetBSD drivers, iris documentation).
- `../research/rust-x11-research.md` — the X11 ecosystem and the clean-room server plan.
- `../research/native-window-system-research.md` — the display-server comparison.
- `../research/rust-fs-research.md` — the Rust filesystem landscape, XFS and I7FS.
- `../research/plan9-rust-research.md` — the Plan 9 ideas and the 9P/MCP survey.
- `../research/erofs-big-endian-research.md` — the EROFS fork analysis.
- `../research/rust-os-design-research.md` — the broader Rust OS design survey.

These are reference material. They are not copied into the kernel; the clean-room
pipeline applies to every line that ships.

### 15.3 Licence gate

Every dependency must pass the ADR-0012 GPL-3.0 gate and the ADR-0009/0014 provenance
gate. Known positions:

| Component | Licence | Status |
|---|---|---|
| `abi_stable` | MIT OR Apache-2.0 | cleared (candidate) |
| EROFS on-disk header (`erofs_fs.h`) | MIT | usable; the rest of `fs/erofs` is GPL-2.0-only and is reference only |
| `smoltcp` | 0BSD | compatible |
| `rustls` | Apache-2.0 OR MIT OR ISC | compatible |
| `gix` | MIT OR Apache-2.0 | compatible |
| `rmcp` (if used) | Apache-2.0 | compatible |
| XFS compatibility readers (`xfs-core`, `lamxfs`) | Apache-2.0 | compatible, but not IRIX-ready; reference/port candidates |
| `backhand` (SquashFS) | MIT OR Apache-2.0 | compatible; fallback root image |
| `erofs-rs` | MIT OR Apache-2.0 | compatible; reference, not a port |

The exact licence of every crate is checked at incorporation time; this table is a
planning aid, not a legal opinion.

### 15.4 Verification

- **Smoke harness** (ADR-0006): compile against the IRIX 6.5.7m sysroot, run on the
  guest, diff stdout.
- **Emulator rig**: per-agent `scripts/rig/task-rig.sh`, isolated instances.
- **MCP loop**: agent-driven boot, serial log, screen capture, GDB.
- **XFS oracle**: mount the image on the CI host, `fsck.erofs`/`dump.erofs` for EROFS,
  tree diffs for XFS.
- **X11 conformance**: the X Test Suite and a corpus of IRIX X clients.
- **Rust safety**: Miri for safe-code UB, Kani for selected invariants, `cargo-call-stack`
  for stack usage, fuzzing for parsers (ELF, EROFS, XFS, 9P, HTTP, Git).
- **Similarity screen**: every contribution, not just the kernel.

### 15.5 Documentation

- ADR-0001–0020 live in `irix7/project/docs/adr/`.
- The new decisions D1–D31 should become ADR-0022 onward (or a single "advanced
  architecture" ADR set); ADR-0021 is the native Rust std decision.
- This spec is the normative architecture for the advanced phase; `docs/map.md` is the
  project map.
- The research reports are preserved in `docs/research/` for traceability.

---

## 16. Alternatives rejected

| Alternative | Why rejected |
|---|---|
| **Pure SAS/SPL for everything** | Legacy IRIX binaries cannot be trusted; without MMU isolation a buggy 1990s binary can corrupt the kernel. The hybrid model (D9) keeps SAS for native cells and MMU spaces for legacy processes. |
| **Microkernel with userspace servers in separate address spaces** | IPC and mode-switch overhead the Indy can ill afford; contradicts ADR-0010 (monolithic) and ADR-0011 (compat container over the kernel object model). The container is in-kernel; drivers are de-privileged cells, not separate processes. |
| **Traditional C kernel + Rust modules (Rust-for-Linux style)** | Safe Rust abstractions over a legacy C kernel inflate the TCB and require pervasive `unsafe`; incompatible with the clean-room Rust monolith and ADR-0012/0014. |
| **seL4 verified microkernel** | C, microkernel, and verified to a different model; the capability/object *concept* is adopted, not the implementation. |
| **Wayland protocol** | Client-side ARGB recomposition needs 315 MB/s against a 266 MB/s memory-controller peak, and Newport has no alpha blending; the compositor is also a Linux-shaped port (udev, DRM/KMS, EGL, fd passing). The right ideas (damage, buffer handles, frame callbacks) are adopted without the protocol. |
| **Port Xorg/kdrive** | ~100k lines of C plus pixman and fonts; conflicts with ADR-0010's Rust-native monolith and imports a POSIX/C stack into a 32 MB Indy. A clean-room Rust server is the plan. |
| **Fork EROFS to big-endian** | 3–6 weeks of writer work, no independent Linux/`fsck`/`dump` oracle, to save ~45 ms per cold boot on a network-bound boot. Standard LE EROFS + a byte-order codec is the decision. |
| **Literal Plan 9** | A namespace is not a security boundary in a single-address-space kernel; 9P as a root transport is fatal over a WAN; Plan 9 authentication is obsolete. Only the naming/control ideas that serve IRIX survive (D31). |
| **GitHub LFS as "free unlimited"** | GitHub Free gives 10 GiB LFS storage and 10 GiB/month bandwidth; over quota, LFS is blocked. R2 (free egress) carries the OS image. |
| **Local-disk-dependent boot** | The real Indys may have a dead SCSI disk; the boot path must work from R2. Local cache is opportunistic. |
| **Host-side-only MCP** | The user chose in-OS control; a host supervisor would be a second control plane and would not exercise the OS's own interfaces. |
| **Original Indigo Magic as the native desktop** | The user chose a new IRIX-inspired desktop. The original desktop can still run under X11 if the compatibility libraries permit it. |
| **ISDN** | Explicitly out of scope. |
| **3D hardware** | Newport has no Z-buffer or geometry engine; software rasterisation is not a v1 goal. |
| **Separate MMU processes for native drivers** | IPC and address-space overhead; the framekernel's trust boundary is `unsafe`, not a privilege ring. De-privileged cells are the model. |
| **Bounce buffers for DMA** | The user chose raw DMA; the MC's 4-entry DMA TLB provides limited confinement where the device path allows, and the residual risk is accepted. |
| **NFS/SMB** | A second security model and a large stack for coherence guarantees the OS does not need. |
| **ext4/FAT32 as the root image** | Little-endian, feature-heavy, not designed for read-only random-access images. EROFS is the right tool. |
| **RedoxFS as the primary native FS** | Little-endian, Redox-coupled, no fsck. It is the fallback if I7FS slips, not the first choice. |
| **9P2000.L as the native dialect** | Linux-VFS-shaped; encodes Linux errno/stat semantics. 9P2000 is native; 9P2000.L is a compatibility front end only. |
| **CRAM-style compressed RAM, zram/zswap, tiered memory** | The Indy has no cache-coherent memory-side compression, CXL or NUMA. Software compression costs ~0.2–0.45 ms per 4 KiB page at 75 MHz (LZ4 decode, estimated), thrashes a 16 KiB direct-mapped D-cache and adds TLB/PTE churn for a ~1.3–2× capacity gain. Compression stays in the read-only EROFS root (§11.2); see [`../research/compressed-ram.md`](../research/compressed-ram.md). |

---

## 17. Open questions

These are deliberately unresolved; they should become ADRs or be settled during the
corresponding implementation phase.

### 17.1 Filesystems

- **I7FS vs RedoxFS** (D23): I7FS is the design fit; RedoxFS is the fallback. Decide when
  writable native disks become a priority.
- The I7FS on-disk layout details (allocation-group count, inode size, journal format,
  directory hash).
- The XFS compatibility server's v1 target: read-only only, or read-only + clean-log
  replay? (Research estimate: 4–6 PM vs 7–11 PM.)
- EROFS reader crate name and licence; whether to reuse `erofs-rs` code or write fresh.
- Overlay persistence: RAM-only, local disk, or Git-backed configuration.

### 17.2 Graphics and media

- The native UI toolkit (immediate-mode vs retained-mode; custom vs existing Rust
  toolkit). The constraint is no GPU, no client-side full-surface recomposition.
- The native graphics API: a simple 2D API (rects, blits, text), or a subset of OpenGL.
  Deferred (§13.3).
- Video output: the Indy Video GIO board is not modelled by iris; add it only if the real
  Indys have it.
- The real Indys' graphics boards: one is reportedly not usable under Linux; identify it
  and decide whether to support XZ/Extreme or 24-bit XL.
- Audio latency and sample-rate targets; HAL2 clock-tree programming.
- Whether the native desktop runs the original IRIX desktop under X11 as an option.

### 17.3 Boot and deployment

- A/B update slots, a Merkle tree over EROFS blocks, and signed manifests (future
  hardening; v1 uses SHA-256 digests).
- The exact boot-bundle format (small EROFS vs a custom archive) and size budget.
- R2 bucket layout, CDN caching headers, and the pre-signed URL strategy for a private
  bucket.
- The local cache format and eviction policy.
- Whether the ROM should speak TLS at all; v1 uses HTTP + digest.

### 17.4 Kernel and ABI

- Native libc ABI surface and versioning; which POSIX calls are in v1.
- The capability invocation encoding and the `abi_stable` vs `repr(C)` decision at each
  seam.
- `fork` implementation: plain copy vs COW in a SAS kernel; the performance target.
- Stack guard pages and stack-usage analysis for cells.
- The SAS memory-corruption risk: a bug in one native cell can corrupt another. The
  capability model limits authority, not memory. Accept, or add a debug-mode guard?
- The scheduler's behaviour under overload and whether admission control is needed later.
- Whether to expose raw MMIO/DMA to native cells or only typed device capabilities.

### 17.5 Compatibility

- The compat container's v1 scope: which IRIX subsystems (`sproc`, arenas, `procntl`,
  `syssgi`, STREAMS, `/hw`) are required for the first binaries.
- The IRIX libc's ABI surfaces (o32 + n32) and their layout.
- The X11 parity target: `xterm`/`twm`, 4Dwm/`dtterm`, or broad parity.
- GLX: bridge to the native GL service, or a separate project?
- The original Indigo Magic desktop under X11: supported, best-effort, or out of scope?
- The compatibility corpus and the regression gate.

### 17.6 MCP and tooling

- The MCP tool schemas, error model and streaming behaviour.
- GDB stub protocol and multi-cell debugging.
- Audit log and approval flow (future; v1 has none).
- The exact granularity of the MCP device/MMIO tools (typed operations vs raw register access); v1 exposes both under full root.
- The `-Z build-std` host-toolchain glue and the GCC-backend disk-space blocker.
- Benchmark suite: what to track (boot time, block throughput, X11 latency, audio
  glitches) even though it does not gate.

### 17.7 Naming and legal

- **IP24 vs IP22**: iris calls the Indy `IP24` and the Indigo2 `IP22`; the hardware report
  and common usage call the Indy IP22. Settle one convention.
- The "IRIX 7" name and any SGI trademark implications; the project is clean-room and
  unaffiliated, but the name should be reviewed before distribution.

---

## 18. Sources

### 18.1 Project sources

- `github.com/irix7/project` — README, `docs/stack.md`, `CONTEXT.md`, `AGENTS.md`,
  `docs/adr/` (ADR-0001–0020), `patches/`, `scripts/`, `oracle/`.
- `github.com/irix7/project/issues` #41–#44 — Rust target, kernel architecture, tracer
  bullet, SOURCE→AST→RUST pipeline (all resolved/completed, 7 October 2026).
- Related repos: `irix7/gcc` (GCC 17 trunk + libgccjit), `irix7/binutils-gdb`,
  `irix7/ghidra`, `irix7/irix6` (private IRIX 6.5.7m source), `irix7/iris` (Indy
  emulator fork), `irix7/harness` (planned), `irix7/os` (this repo, planned).

### 18.2 Research reports

- `../research/indy-hardware.md` — Indy hardware reference.
- `../research/rust-x11-research.md` — Rust X11 survey and clean-room server plan.
- `../research/native-window-system-research.md` — native window-system comparison.
- `../research/rust-fs-research.md` — Rust filesystems, XFS and I7FS.
- `../research/plan9-rust-research.md` — Plan 9 ideas, 9P, MCP and content-addressed
  images.
- `../research/erofs-big-endian-research.md` — EROFS byte-order analysis.
- `../research/rust-os-design-research.md` — broader Rust OS design survey.

### 18.3 External primary sources

- Asterinas / Ostd, *Framekernel* (arXiv:2506.03876v1) — framekernel architecture,
  privilege separation, Ostd APIs, safe policy injection, TCB and performance results.
- Theseus OS Book (theseus-os.com) — SAS/SPL, cells ≈ crates, `MappedPages`,
  P.I.E./PHIS, `nano_core`, `captain`, `tlibc`.
- seL4 Reference Manual (sel4.systems) — capability = object reference + access rights.
- Plan 9 papers: *The Use of Name Spaces in Plan 9*; *Plan 9 from Bell Labs*;
  *Security in Plan 9*; *8½, the Plan 9 Window System*; `intro(5)`, `version(5)`,
  `attach(5)`, `bind(2)`, `rio(4)`, `factotum(4)`.
- EROFS documentation (`Documentation/filesystems/erofs.rst`), `mkfs.erofs`,
  `fsck.erofs`, `dump.erofs`, `erofs-rs`.
- X11 protocol specification and extensions (x.org); Xlib, Xt, Motif, X Test Suite.
- Wayland architecture and protocol (wayland.freedesktop.org) — rejected, but the
  damage/frame-callback ideas inform the native display server.
- XFS on-disk format (SGI XFS papers, `xfsprogs` documentation); IRIX 6.5 XFS v4 log
  format.
- MIPS R4000/R4400/R4600/R5000 user manuals — caches, TLB, exceptions, `Count`/`Compare`.
- SGI Indy hardware specifications: REX3, RB2, VC2, XMAP9, Vino, HAL2, WD33C93, Seeq
  8003, DS1386, HPC3, MC, GIO, ARCS.
- Linux `arch/mips/sgi-ip22` and NetBSD/OpenBSD `sgimips` drivers — executable evidence
  for the hardware behaviour.
- `irix7/iris` emulator documentation — `docs/Indy_rex3.md`, `docs_Indy_hal2.md`,
  `docs_Indy_wd33c93a.md`, `docs_indy-xz-elan.md`, `docs_interrupt_map.md`.
- GitHub Docs — Git LFS billing, repository limits; Cloudflare R2 pricing.
- Rust reference — type layout, `repr(C)`, `abi_stable`; `rustc` custom targets;
  `-Z build-std`.
- `smoltcp`, `rustls`, `gix`, `rmcp` documentation.

---

*Revision: 2026-10-20. Rewritten after the advanced-architecture grilling; supersedes the
earlier framekernel/SAS-only draft. The research reports in `docs/research/` are the
evidence base; the decision register in §2.2 is the normative summary.*
