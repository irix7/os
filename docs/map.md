## Destination

IRIX 7, a GPL-3.0 Rust-native operating system for the SGI Indy (IP22; `IP24` in iris naming): a monolithic Rust kernel (n32/MIPS III) reimplementing everything on the IRIX discs clean-room, running existing IRIX 6.5 binaries unchanged through a legacy ABI layer, and offering new Rust programs a native capability/object interface. It carries IRIX's identity — `sproc`, XFS, real-time scheduling, graphics and digital media, hardware inventory, topology awareness — as first-class services. Backward-only, no-regressions; forward compatibility is out.

The advanced architecture (2026-10-20) frames IRIX 7 as **the successor SGI might have built if it had not gone boring**: a framekernel with hybrid SAS/MMU isolation, de-privileged cells, userspace drivers, an R2-first boot, an in-OS MCP control plane, and a new IRIX-inspired desktop. The native ABI is new; IRIX lives in the compatibility container. The normative architecture is `docs/spec/architecture.md`; the evidence base is `docs/research/`.

## Notes

- This is the final phase of the project. Its work starts only after the earlier phases clear, in order:
  1. Collate a complete IRIX source repo: the leaked sources plus decompilation of the 6.5.7m binaries to fill gaps (Ghidra; map #1's recovery work).
  2. Stock rebuild (map #1): 6.5.7m rebuilt with the modernised GCC (rust-lang/gcc 17 trunk + IRIX series, ADR-0019) and binutils 2.47, cross-compiled to `mips-sgi-irix6.5` and booted/run on the guest.
  3. Upgrade the source repo to 6.5.22: decompile the 6.5.22 overlay contents and apply the findings to the 6.5.7m base source manually (optionally port back 6.5.30 updates).
  4. SOURCE to AST to RUST: this map.
- Governing decisions: ADR-0001 (publish toolchain work only), ADR-0008 (compatibility mandate, backward-only), ADR-0009 (clean-room publication), ADR-0010 (Rust monolith, n32/MIPS III), ADR-0011 (native capability substrate + compat container), ADR-0012 (GPL-3.0), ADR-0013 (full reimplementation, factory PROM), ADR-0014 (harness-enforced clean-room, hybrid transcription), ADR-0015 (layered spec, similarity-screen verification), ADR-0016 (o32 ECOFF bootstrap into the n32 kernel), ADR-0017 (modern equivalents as references), ADR-0018 (GCC codegen backend primary, LLVM parallel), ADR-0019 (IRIX fork on rust-lang/gcc), ADR-0020 (rebuild ladder). ADRs live at `github.com/irix7/project/docs/adr/`.
- The legacy layer order is syscall ABI -> loader -> libc; the tracer bullet is a freestanding Rust kernel reached through an o32 ECOFF bootstrap.
- Transcription is orchestrated by `irix7/harness`; only clean-room Rust reaches the public `irix7/os` repo.
- Skills: grilling + domain-modelling for decision tickets; research for fact-finding tickets.

## Decisions so far

- [IRIX 7: Rust target for MIPS III big-endian ELF32](https://github.com/irix7/project/issues/41) — **resolved/completed**. Stock rustc + custom target + `-Z build-std` is feasible, no fork required; n32/MIPS III and o32/MIPS II build, "MIPS III + o32" does not (o32 implies MIPS II). Custom n32/MIPS III target spec + build-std is the plan (ADR-0010).
- [IRIX 7: kernel architecture](https://github.com/irix7/project/issues/42) — **resolved/completed**. Monolithic Rust kernel, n32/MIPS III, deep crates with language-level isolation, decoding both o32 and n32 syscall conventions (ADR-0010 / ADR-0011).
- [IRIX 7: tracer bullet](https://github.com/irix7/project/issues/43) — **resolved/completed**. A freestanding `no_std` Rust kernel that boots via the PROM and prints a marker, proven on the emulated Indy (ADR-0016).
- [IRIX 7: SOURCE to AST to RUST pipeline and the clean-room spec](https://github.com/irix7/project/issues/44) — **resolved/completed**. Hybrid transcription, clean-room enforced by the harness (structural split + verification) (ADR-0014).

## Advanced architecture decisions (2026-10-20)

The grilling of 2026-10-20 settled the advanced architecture. The normative register is
`docs/spec/architecture.md` §2.2 (D1–D31); this is the project-map summary.

| Area | Decision |
|---|---|
| Identity | IRIX 7 is the successor SGI might have built; the native ABI is new, IRIX lives in the compatibility container. |
| Isolation | Framekernel: privileged `unsafe` framework + de-privileged safe-Rust cells; SAS/SPL for native, MMU address spaces for legacy binaries. |
| Native ABI | Capability/object model with rights, revocation and delegation; Unix-shaped libc; process = one cell. |
| Compatibility | In-kernel IRIX o32/n32 container; full fork/exec/sproc; library-level libc/X11/Motif/GL/dmedia; clean-room Rust X11 server phased lease → nested → rootless. |
| Scheduling | EDF for media, fair for the rest; soft real-time; benchmarks tracked, never gated. |
| Memory | Aggressive huge pages + cache colouring; DMA-first with cached buffers + explicit sync; raw DMA accepted; MC VDMA DMA TLB used where possible; no full IOMMU. |
| Drivers | All de-privileged cells; the kernel brokers MMIO/DMA/IRQ; the ROM carries the network driver. |
| Boot | Minimal Rust ROM with Seeq + smoltcp → R2 kernel + boot bundle → EROFS root over HTTP Range; optional local disk cache; R2-first. |
| Root image | Standard little-endian EROFS + byte-order codec; no big-endian fork. |
| Storage | XFS compatibility server read-only first; native FS (I7FS preferred) later; user data via periodic Git commits + R2. |
| Display | Plan 9 `8½`/BeOS `app_server`-lineage native display server; new IRIX-inspired desktop; userspace graphics/audio servers. |
| Networking | Seeq 8003 + smoltcp; `/net`; optional 9P2000 remote mounts; HTTP Range for bulk. |
| Control plane | In-OS MCP server, full root, bearer token, no audit, DS1386 watchdog; built early as the development harness. |
| Hardware | Everything except ISDN; first target the iris default (R4400, Newport XL, 256 MiB, HAL2, Seeq, WD33C93); video output deferred. |
| Plan 9 | Selective only: `ctl`/`data`/`status`, `/net`, optional 9P; no per-process namespaces by default. |

Research reports are preserved in `docs/research/`:

- `indy-hardware.md` — Indy hardware reference (CPU, HPC3, SCSI, Seeq, HAL2, Newport, Vino, DS1386, GIO, PROM).
- `rust-x11-research.md` — Rust X11 survey and the clean-room server plan.
- `native-window-system-research.md` — native window-system comparison.
- `rust-fs-research.md` — Rust filesystems, XFS and I7FS.
- `plan9-rust-research.md` — Plan 9 ideas, 9P, MCP and content-addressed images.
- `erofs-big-endian-research.md` — EROFS byte-order analysis.
- `rust-os-design-research.md` — broader Rust OS design survey.

The new decisions D1–D31 should be recorded as ADR-0021 onward (or as a single "advanced
architecture" ADR set) in `irix7/project/docs/adr/`.

## Crate dependencies

External Rust crates the kernel and compat container may depend on are recorded here, subject to the licence gate in ADR-0012 (GPL-3.0). A crate may be incorporated only when its licence is compatible with GPL-3.0 and its provenance is clean-room (ADR-0009 / ADR-0014).

### abi_stable

Source: https://crates.io/crates/abi_stable

The abi_stable crate provides a stable ABI surface for Rust types across crate and compiler-version boundaries, which is what the native ABI / legacy ABI layer boundary wants: Rust code exporting types and functions in a layout that does not depend on the particular rustc version that built them. It is a candidate dependency for the FFI seam between the kernel's native capability/object model and the compat container's translation layer, and for any Rust ABI exported to legacy IRIX 6.5 binaries.

Licence (as published on crates.io): dual-licensed under the MIT Licence or the Apache Licence, Version 2.0, at the licensee's option (MIT OR Apache-2.0).

Compatibility check (ADR-0012 — GPL-3.0): compatible.

Both MIT and Apache-2.0 are permissive licences and are compatible with GPL-3.0. Incorporating abi_stable into this GPL-3.0 work is permitted: the combined program is conveyed under GPL-3.0 (as a whole, per GPLv3 section 5), while the abi_stable source retains its original copyright and licence notices. No further restriction is imposed on downstream recipients beyond those of GPL-3.0 itself.

Status: candidate — cleared for incorporation subject to this licence check.

### Candidate crates (advanced architecture)

| Crate | Purpose | Licence | Status |
|---|---|---|---|
| `abi_stable` | Stable Rust FFI across compiler versions | MIT OR Apache-2.0 | cleared (candidate) |
| `smoltcp` | TCP/IP stack for the network cell and ROM | 0BSD | candidate |
| `rustls` | TLS 1.3 | Apache-2.0 OR MIT OR ISC | candidate |
| `gix` | Git object store and smart HTTP | MIT OR Apache-2.0 | candidate |
| `rmcp` | MCP server (if used) | Apache-2.0 | candidate |
| `erofs-rs` | EROFS reference reader/builder | MIT OR Apache-2.0 | reference, not a port |
| `xfs-core` | XFS read-only reference | Apache-2.0 | reference |
| `lamxfs` | XFS read-only reference (v4/v5) | Apache-2.0 | reference |
| `backhand` | SquashFS fallback | MIT OR Apache-2.0 | fallback |
| `crc32c` | CRC32C for I7FS | Apache-2.0 OR MIT | candidate |

The exact licence of every crate is checked at incorporation time (ADR-0012); this table
is a planning aid, not a legal opinion.

## Not yet specified

- I7FS vs RedoxFS for writable native disks (D23).
- Native graphics API and UI toolkit; the native desktop prototype.
- Video output and Indy Video emulation (deferred until the real Indys are probed).
- The real Indys' graphics boards and the board that Linux may not drive.
- A/B update slots, a Merkle tree over EROFS blocks, and signed manifests.
- Native libc ABI surface and versioning; capability invocation encoding.
- Compat container v1 scope (`sproc`, arenas, `procntl`, `syssgi`, STREAMS, `/hw`).
- IRIX libc ABI surfaces (o32 + n32) and their layout.
- X11 parity target and the GLX bridge.
- Original Indigo Magic desktop under X11: supported, best-effort, or out of scope.
- MCP tool schemas, GDB stub, audit/approval flow.
- `-Z build-std` host-toolchain glue and the GCC-backend disk-space blocker.
- Overlay persistence and local cache format.
- `irix7-9p` scope and the optional 9P remote mounts.
- Benchmark suite contents (tracked, never gated).
- IP24 vs IP22 naming, and the "IRIX 7" name/trademark review.

## Out of scope

- Forward compatibility: native IRIX 7 binaries running on IRIX 6.5 or 5.3 (ADR-0008).
- Publishing any SGI-derived expression to `irix7/os` (ADR-0009, ADR-0014).
- Reproducing the stock IRIX 6.5.7m build — that is map #1.
- The 6.5.22 decompilation and upgrade — a precursor effort, not this map.
- STREAMS as a first-class native interface (legacy-compat only).
