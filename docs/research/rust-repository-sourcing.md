# Rust repository and crate sourcing for IRIX 7

**Status:** research report; not an ADR
**Date:** 2026-10-25
**Scope:** which existing Rust repositories and crates IRIX 7 can reuse, fork or borrow design from; licence compatibility with GPL-3.0-only; target portability to n32 MIPS III, 32-bit, big-endian, 32–256 MiB, `no_std` kernel and a thread-less bring-up userland.
**Related:** [`../plan/milestones.md`](../plan/milestones.md), [`../spec/system.md`](../spec/system.md), [`../spec/architecture.md`](../spec/architecture.md), [`../spec/native-libc.md`](../spec/native-libc.md), [`../spec/erofs.md`](../spec/erofs.md), [`rust-fs-research.md`](rust-fs-research.md), [`rust-x11-research.md`](rust-x11-research.md), [`native-window-system-research.md`](native-window-system-research.md), [`rust-std-irix7.md`](rust-std-irix7.md).

---

## 1. Method and evidence

### 1.1 What was read

The plan set (`docs/plan/*.md`), the architecture and interface specifications (`docs/spec/system.md`, `architecture.md`, `boot-protocol.md`, `elf.md`, `modules.md`, `packages.md`, `mcp.md`, `posix.md`, `native-abi.md`, `native-libc.md`, `driver-interface.md`, `erofs.md`, and the supporting `userland-services.md`, `console.md`, `overlay.md` and `mcp-apps.md`), the existing research reports, `README.md`, `Cargo.toml`, and `docs/map.md` were read before the search. The component map in §3 is derived from those documents and from the actual workspace members, not from the marketing pages of the candidate projects.

### 1.2 How candidates were checked

1. **Crates.io API** (`https://crates.io/api/v1/crates/<name>`) for the published version, declared licence, repository URL, MSRV, publication date and download count.
2. **GitHub API** (`https://api.github.com/repos/<owner>/<repo>`) for the repository licence, last push, stars, archive state and default branch.
3. **Repository `Cargo.toml` and README** where the licence metadata was ambiguous or where the decisive question was a feature (`std`/`no_std`, `alloc`, SIMD, target support, MIPS architecture support).
4. **Upstream source layout** for specific claims: `gdbstub` has a MIPS architecture module, `wasmi` has a `std` feature and a `libm` feature, `cosmic-text` has a `no_std` feature, `x11rb-protocol` builds without `std`, and so on.

All versions, stars, dates and licence fields in this report were read on **2026-10-25** and will drift. A “verified” licence means the crates.io metadata and the repository licence agree, or the discrepancy is stated. It does **not** mean the crate builds for the Indy, and it does **not** mean the crate is legally clean; provenance is a separate question (§2.4).

### 1.3 What “verified” and “needs a spike” mean here

| Marker | Meaning |
|---|---|
| **Verified** | Licence, repository identity, maintenance state and any `no_std`/architecture claim were checked against the published metadata or source manifest on 2026-10-25. |
| **Needs a spike** | The licence is clear, but the crate has not been built or run for n32/MIPS III big-endian; the target-specific claim (no_std, 32-bit, endianness, MIPS support, memory footprint) is unproven. |
| **Reject** | A licence is incompatible, the project is unlicensed, the code is C/GPL-2.0-only, or the platform assumptions are so far from the Indy that a port is a different project. |

A permissive licence is not a provenance certificate. The project’s clean-room policy (ADR-0009/0014/0015) applies to third-party code too: a crate that declares Apache-2.0 but contains copied GPL-2.0-only Linux code is still a licence problem. The XFS and EROFS candidates are the highest-risk cases because their subject matter has a large GPL-2.0-only reference implementation.

### 1.4 Issue mapping used in this report

At retrieval time the public repository has epics, not granular open tasks, for most of this work. The mapping below is used throughout; phase numbers are from `docs/plan/milestones.md`.

| Issue | Meaning | Phase / gate |
|---|---|---|
| [#2](https://github.com/irix7/os/issues/2) | M1 — single-node logical computer | Phase 2, M1 |
| [#3](https://github.com/irix7/os/issues/3) | M2 — two-node join and migration | Phase 3, M2 |
| [#4](https://github.com/irix7/os/issues/4) | M3 — eight-node sharded mesh | Phase 5, M3 |
| [#5](https://github.com/irix7/os/issues/5) | M4 — 128-node cloud and mesh | Phase 7, M4 |
| [#6](https://github.com/irix7/os/issues/6) | Native track — IRIX 6.5 compatibility | compatibility workstream |
| [#7](https://github.com/irix7/os/issues/7) | Native track — display and desktop | Phase 2+ |
| [#8](https://github.com/irix7/os/issues/8) | Native track — X11 | compatibility workstream |
| [#9](https://github.com/irix7/os/issues/9) | Native track — XFS read-only | compatibility workstream |
| [#10](https://github.com/irix7/os/issues/10) | Native track — native cells and capability ABI | M9/M9.5 |
| [#11](https://github.com/irix7/os/issues/11) | Native track — real-Indy release | release gate |
| [#12](https://github.com/irix7/os/issues/12) | Clean-room and provenance | every phase |
| [#13](https://github.com/irix7/os/issues/13) | CI and automation | every phase |
| [#15](https://github.com/irix7/os/issues/15) | Security | every phase |
| [#17](https://github.com/irix7/os/issues/17) | Performance and benchmarks | tracked, not gated |
| [#194](https://github.com/irix7/os/issues/194) | Long-range fully Rust COSMIC desktop | post-v1 |

---

## 2. Licence policy

### 2.1 The project licence

IRIX 7 is **GPL-3.0-only**. A dependency is acceptable only if the combined work can be conveyed under GPL-3.0-only without adding a conflicting obligation. The project keeps third-party notices and does not relicense third-party files.

### 2.2 Compatible licences

| Licence | Status | Notes |
|---|---|---|
| MIT, BSD-2-Clause, BSD-3-Clause, ISC, 0BSD, Zlib | Compatible | Keep copyright and licence text. |
| Apache-2.0 | Compatible with GPL-3.0 | Keep `NOTICE`; patent grant terminates on patent litigation. |
| Apache-2.0 WITH LLVM-exception | Compatible | Common in `wasm-tools`, `rustix`, `linux-raw-sys`; the exception is GPLv2-friendly and does not hurt GPLv3. |
| MPL-2.0 | Compatible, with file-level copyleft | MPL files stay MPL; source for those files must remain available. A GPL-3.0 application may link MPL-2.0 code. Do not relicense MPL files. |
| CC0-1.0, Unlicense | Compatible | Permissive/public-domain dedication; keep provenance. |
| NCSA | Compatible | Permissive; used by `libfuzzer-sys`. |
| CDLA-Permissive-2.0 | Data licence; acceptable with review | Used by `webpki-roots`; it is a data licence, not a code licence. Keep the notice; do not treat it as a patent grant. |
| GPL-2.0-or-later | Compatible if GPLv3 is chosen | The “or later” gives a GPLv3 path. Avoid where a permissive alternative exists. |
| GPL-3.0-or-later | Compatible | The combined work can be GPL-3.0-only; the dependency remains “or later”. |
| LGPL-2.1/3.0 | Compatible in principle, awkward in practice | Static kernel linking creates relink obligations. Treat as a last resort and record the relink strategy. |

### 2.3 Rejected and conditional licences

| Licence | Status and reason |
|---|---|
| GPL-2.0-only | Rejected: incompatible with GPL-3.0-only. This is the single most common hazard in storage and graphics code. |
| AGPL-3.0 | Rejected: adds a network-source obligation to the combined work; not acceptable for a general-purpose OS. |
| SSPL, proprietary, “all rights reserved”, no licence file | Rejected: not reusable. |
| CDDL-1.0 | Rejected: not GPL-compatible (OpenZFS is the canonical example). |
| EPL-2.0 | Rejected: not GPL-compatible. |
| EUPL-1.2 | Conditional: on the EUPL’s compatibility list for GPLv3, but the obligations are unfamiliar and the project has MIT alternatives. Treat as “legal review”, not “cleared”. |

### 2.4 MPL-2.0 and Apache-2.0 in practice

- **MPL-2.0** (`libcosmic`, `foca`, `ostd`, `symphonia`) is compatible, but the MPL files must remain MPL. A downstream `libcosmic` fork must keep the MPL headers and publish the source of modified MPL files. Application code and IRIX 7 glue can be GPL-3.0-only.
- **Apache-2.0** (`rustls`, `embedded-tls`, `object`, `cargo-cyclonedx`, many others) requires the `NOTICE` file to be preserved and includes a patent-termination clause. This is compatible with GPL-3.0 but must appear in `THIRD-PARTY-NOTICES.md`.
- **Dual MIT/Apache-2.0** is the easiest case; choose MIT for the combined work and keep both notices.

### 2.5 Clean-room

No SGI source, no decompiled expression and no copied GPL-2.0-only Linux code. For filesystem and ABI work the clean-room inputs are the public on-disk specifications, POSIX.1-2024, the IRIX ABI descriptions already captured in the specs, and independent implementations. A permissive crate that was itself derived from Linux must be audited before it is used as a source; the audit result belongs in the provenance record.

### 2.6 Rust-first and C dependencies

The project is Rust-first (AGENTS.md). A C or mixed-language dependency is not automatically disqualified, but it is a work item: the record must state its licence, target/platform assumptions, build requirements (C compiler, CMake, Perl, NASM, pkg-config), and the port or replacement strategy. This report therefore treats C-backed crates such as `ring`, `aws-lc-rs`, `zstd`, `lz4`, `littlefs2`, `capstone`, `git2` and `mbedtls` as **host-only or fallback** unless a pure-Rust alternative does not exist. The kernel should have no C dependency at all.

Every adopted dependency should get a short record with: licence and version/tag; target assumptions (`no_std`, `alloc`, 32-bit, big-endian, MIPS); build requirements; use of threads/TLS/processes/Linux APIs; current build/run status on n32; port or replacement plan; owner; and the issue it serves. The tables below are the screening record, not a substitute for that per-dependency record.

---

## 3. Component map

“Exists” means code is in this repository today, not that the milestone is complete. The gap column is the work external Rust code could shorten.

| Layer | Exists in IRIX 7 today | Gap that blocks the next gate | External Rust that could shorten it | Issues |
|---|---|---|---|---|
| ROM / boot | MIPS III assembly ROM; DRAM sizing/test; serial; LZ4 decompression; jump to n32 kernel; `BootInfo` specification | ROM does not emit `BootInfo`; no CRC32C conformance; no boot bundle | `lz4_flex` (host tests), `crc`/`crc32c`, `sha2`, `object` for image inspection | #2 |
| Kernel core | Trap/TLB path, frame allocator, heap, scheduler scaffolding, `platform-api`, capability table, `irix-elf`, `hpc3` | No process table, threads/TLS, general IRQ delivery, DMA engine, cell loader, EDF/fair scheduler | Asterinas/Ostd, Theseus, Redox, rCore, Hubris design; `linked_list_allocator`, `buddy_system_allocator`, `rlsf`, `talc`, `spin`, `tock-registers` | #2, #10 |
| Modules | ABI 11 flat modules, SDK `module-abi` v0.6.0, HTTPS/local load, `hello`/`ticker`/`block`/`mcp`/`erofs`/`logical`/`cache` | One module slot; no multiplexer; no unload | None; this is project-specific | #2 |
| Wasm | Host-side `wasmi` lifecycle and `i7host` imports in `wasm-host`; `guest-abi` | Kernel does not run the core; determinism, fuel, memory validation on MIPS unproven | `wasmi`, `wasmparser`, `wasm-encoder`, `wasmprinter` | #2, #3, #4, #5 |
| Mesh | `mesh-core` rendezvous/route/lease/placement; `mesh-service` membership state; `mesh-sim` 1/2/8/128 policy gates; `agnostic-core` | No gossip, transport, authority signatures, real membership epochs | `foca` (SWIM), `openraft`/`chitchat` design, `quinn`/`iroh` design | #3, #4, #5 |
| CAS / namespace | `cas-core` BLAKE3 OIDs/manifests/directory objects; `cas-store` put/fetch/pin; local namespace in `logical` module | Block-backed store, replication, GC, fetch-by-OID over the mesh | `iroh-blobs` design; no direct dependency | #2, #4, #5 |
| Storage | `erofs` reader (core, LZ4, fragments), `block-cache`, `overlay`, HTTP Range and SCSI module paths | XFS v4, I7FS, package cache, dirty-log handling | `lamxfs`, `xfs-core`, `rust-fs-erofs`/`lamfold-erofs` (oracles), `fatfs`, `tar`/`zip`/`cpio`/`ar` | #9, #2, #11 |
| Network | `smoltcp` 0.13, DHCPv4, DNS, NTP, HTTP/1.1, `embedded-tls` 0.19 with P-384/RSA, telnet, MCP prototype | TLS session reuse, streaming digest, HTTP/2/3, QUIC, mesh transport | `rustls` (userland), `quinn`/`iroh` design, `dhcproto`/`simple-dns`/`sntpc` test oracles | #2, #3, #5 |
| Drivers | Polled Seeq, WD33C93 SCSI, REX3 console, Z85C30 serial, DS1386 RTC/NVRAM, PS/2 keyboard | Descriptor DMA, IRQ delivery, HAL2 audio, Vino capture, Newport graphics server | `tock-registers`, `volatile`, `embedded-hal`/`embedded-dma` design, `pcics` | #10, #16 |
| POSIX / libc | `irix7/relibc` fork (n32 QEMU gate passes), `native-abi` layout crate, 42-symbol `irix7-libc` shim, `user/libc`, static `std` image | Process table, threads/TLS, signals, termios, sockets, `posix_spawn`, full `struct stat` | `libc` base (clean-room IRIX module), `unwinding` + MIPS support, `rustix`/`nix` design, Redox relibc tests | #6, #10, #11 |
| Display / X11 | Kernel graphics console and REX3 boot log | Native display server, toolkit, X11 server, Xlib ABI | `x11rb`, `yserver` architecture, `cosmic-text`, `harfrust`, `tiny-skia`, `iced`, `libcosmic` | #7, #8, #194, #6 |
| Userland | `sash` recovery shell, `hello`, `std-hello`, o32 probe | Coreutils, shell, editor, package tooling, terminal | `uutils/coreutils`, `brush-shell`, `alacritty_terminal`, `ripgrep`/`fd`/`bat`, `tar`/`zip` | #6, #10, #11, #2 |
| Tooling | Cargo workspace, CI, host tests, `cargo fmt`/`clippy` | Licence gate, SBOM, fuzz, coverage, stack/size budgets | `cargo-deny`, `cargo-audit`, `cargo-cyclonedx`, `cargo-about`, `cargo-call-stack`, `cargo-bloat`, `proptest`, `cargo-fuzz`, `nextest` | #12, #13, #15, #17, #11 |
| PC / cloud | None | Boot, ACPI, VirtIO, block/network shims | `bootloader`/`limine`/`uefi`, `acpi`, `x86_64`, `virtio-drivers` | #5 |

---

## 4. Summary matrix

The matrix is deliberately about **fit**, not popularity. “Adopt” means add as a dependency; “fork” means vendor and modify; “borrow” means reimplement the design without copying code; “reject” means do not use.

| Candidate | Licence (verified) | Verdict | Unblocks | One-line reason |
|---|---|---|---|---|
| **Tooling gate suite** (`cargo-deny`, `cargo-audit`, `cargo-cyclonedx`, `cargo-about`, `cargo-call-stack`, `cargo-bloat`, `proptest`, `cargo-fuzz`, `nextest`) | MIT/Apache-2.0, Apache-2.0 | Adopt | #12, #13, #15, #17, #11 | Lowest-effort, highest-breadth unblock; all host-side. |
| **`foca`** | MPL-2.0 | Adopt | #3, #4, #5 | `no_std` + `alloc` SWIM; exactly the missing membership/failure layer. |
| **`x11rb`** | MIT OR Apache-2.0 | Adopt/fork | #8, #7, #6, #11 | Complete X11 wire types; server dispatch and Xlib ABI still new. |
| **`yserver`** | MIT | Borrow design/tests | #8, #7, #11 | Only serious Rust X11 server; DRM/KMS/Vulkan and little-endian assumptions block a port. |
| **`uutils/coreutils`** | MIT | Fork subset | #6, #10, #11, #2 | Broad POSIX userland and tests; port to relibc and 32-bit. |
| **`cosmic-text`** | MIT OR Apache-2.0 | Adopt/fork | #7, #8, #194 | `no_std` text layout/shaping with `harfrust`; COSMIC-compatible. |
| **`harfrust`** | MIT | Adopt | #7, #8, #194 | Maintained successor to archived `rustybuzz`; `no_std` + `libm`. |
| **`tiny-skia`** | BSD-3-Clause | Adopt | #7, #8, #194 | CPU rasteriser with `no_std`; no GPU requirement. |
| **`iced`** | MIT | Fork | #7, #194 | Software `tiny-skia` renderer exists; winit/wgpu backend must be replaced. |
| **`libcosmic`** | MPL-2.0 | Fork (long-range) | #7, #194 | D33 toolkit; default winit/tokio/dbus/X11/Wayland, but its vendored iced uses tiny-skia, so the renderer path is plausible. |
| **`lamxfs`** | MIT OR Apache-2.0 | Fork/port | #9, #6, #11 | `no_std` read-only XFS v4/v5; no IRIX-specific dirs/ACLs; refuses dirty v4 log. |
| **`xfs-core`** | Apache-2.0 | Borrow/reference | #9 | Read-only v4/v5 parser; std, no log replay; useful oracle. |
| **`rust-fs-erofs`** | MIT | Test oracle only | #9 | Claims full reader/writer; one star and a short history; do not trust yet. |
| **`lamfold-erofs`** | MIT OR Apache-2.0 | Borrow design | #9 | `no_std` uncompressed EROFS + fs-verity; own reader already goes further. |
| **`unwinding`** | MIT OR Apache-2.0 | Adopt + port | #10, #11, #6 | `std` needs `_Unwind_*`; MIPS support is missing from the pinned version. |
| **`libc`** | MIT OR Apache-2.0 | Adopt base | #6, #10 | No IRIX module; add a clean-room one; do not copy the unlicensed IRIX ports. |
| **`object`** | Apache-2.0 OR MIT | Adopt host tools | #6, #10 | ELF/object reader/writer with big-endian support; keep `irix-elf` for the loader. |
| **`xmas-elf`** | Apache-2.0 OR MIT | Borrow/test | #6, #10 | Small `no_std` ELF parser; useful differential oracle. |
| **`gdbstub`** | MIT OR Apache-2.0 | Adopt/fork | #2, #15 | Has MIPS32/64 architecture support and `no_std`; needs trap-frame glue. |
| **`rabbitizer`** | MIT | Adopt host tool | #6, #12 | Pure-Rust MIPS decoder; ideal for disassembly tests and clean-room analysis. |
| **`rmcp`** | Apache-2.0 | Adopt host/native cell | #2, #15, #13 | Official Rust MCP SDK; std/async, so not a kernel module. |
| **`virtio-drivers`** | MIT | Adopt later | #5 | `no_std` VirtIO guest drivers; PC/cloud only. |
| **Asterinas / Ostd** | MPL-2.0 | Borrow design | #2, #10 | Best framekernel/Tcb prior art; 64-bit x86-64/AArch64/RISC-V only. |
| **Theseus** | MIT | Borrow design | #2, #10 | `MappedPages` ownership and cell model; x86-64/aarch64, stale. |
| **Redox kernel / RedoxFS** | MIT | Borrow design / fork FS | #2, #9, #10 | Unix personality, CoW FS; x86-64 and little-endian. |
| **rCore / zCore** | MIT | Borrow design | #2, #10 | Small readable kernel and Zircon-style object model; teaching scale. |
| **Hubris** | MPL-2.0 | Borrow design | #10, #16 | Message-passing kernel for deeply embedded; ARM. |
| **Tock / `tock-registers`** | Apache-2.0 OR MIT | Adopt `tock-registers` | #10, #16 | Register abstraction is directly reusable; kernel is ARM/RISC-V. |
| **`smoltcp`** | 0BSD | Already adopted | #2 | In tree at 0.13; 0.14 is an upgrade candidate. |
| **`embedded-tls`** | Apache-2.0 | Already adopted | #2 | In tree at 0.19 with P-384/RSA; no C crypto. |
| **`wasmi`** | MIT OR Apache-2.0 | Already adopted | #2, #4 | In tree at 2.0; kernel `no_std` integration unproven. |
| **`blake3` / `ed25519-dalek`** | CC0/Apache; BSD-3 | Already adopted | #2, #4, #15 | Used by `cas-core`, `mesh-core`, `cap-engine`. |
| **`quinn` / `iroh`** | MIT OR Apache-2.0 | Borrow design / host | #3, #5 | QUIC and dial-by-key; std/tokio, too heavy for the Indy kernel. |
| **`openraft` / `chitchat`** | MIT OR Apache-2.0 | Borrow design | #5 | Consensus/gossip prior art; std/async. |
| **`postcard`** | MIT OR Apache-2.0 | Adopt | #3, #4 | `no_std` canonical wire; better cross-endian choice than `rkyv`. |
| **`serde`** | MIT OR Apache-2.0 | Adopt | #3, #4 | `no_std` + `alloc`; ubiquitous. |
| **`tokio` / `hyper` / `axum`** | MIT | Host only | #13, #15 | Threads/epoll; not kernel or single-threaded bring-up. |
| **`gix`** | MIT OR Apache-2.0 | Fork/host | #2, #9, #11 | Pure-Rust Git; large and std; use a subset or a host service. |
| **`git2`** | MIT OR Apache-2.0 wrapper; libgit2 GPL-2.0-only | Reject | — | The wrapper is permissive, the C library is not. |
| **`smithay` / `wayland-rs` / `winit`** | MIT / Apache-2.0 | Reject for Indy | — | Wayland/DRM/GBM/libinput/X11 assumptions; no GPU. |
| **`pixels` / `vello` / `wgpu`** | MIT / Apache-2.0 | Reject for Indy | — | GPU compute/render; Newport has no GPU. |
| **`wasmtime`** | Apache-2.0 WITH LLVM-exception | Reject for Indy | — | JIT, 64-bit, threads; host-only alternative. |
| **`libp2p` / `nushell`** | MIT | Reject for target | — | Licence fine, size/stack wrong for 32–256 MiB. |
| **`fish-shell`** | GPL-2.0-only | Reject | — | Licence incompatible. |
| **`rust-irixlibstd` / `rust-irix`** | No licence file | Reject code | — | Best IRIX `std` prior art, but all rights reserved. Use as evidence only. |

---

## 5. Domain findings

### 5.1 Kernel, framekernel and `no_std`

Unless a row says otherwise, this section serves **#2 (M1)** and **#10 (native cells/capability ABI)**; the allocators and locks are also prerequisites for **#17** (size/stack/latency budgets).

The kernel is already `no_std` + `alloc`, n32/MIPS III, with a small `platform-api` trait and a monolithic deployment shape. The architecture is Asterinas/Ostd-style: a privileged `unsafe` framework, safe policy crates outside it, and drivers that may use documented `unsafe` (D8–D10, D17). External kernels therefore contribute **design and small portable crates**, not a replacement kernel.

#### Asterinas and Ostd

- Repository: <https://github.com/asterinas/asterinas>; crates `ostd` 0.19.0, `osdk-frame-allocator`, `osdk-heap-allocator`; **MPL-2.0**.
- Verified: active (last push 2026-10-23), 5,022 stars; 0.19.0 released 2026-10-13. It supports x86-64, AArch64 and RISC-V64; the README says “modern, 64-bit platforms only”.
- Reuse: the framework/TCB split, the `ostd-pod` untyped-memory idea, frame/heap allocator policy injection, page-table and MMIO safety invariants, and the published Inv. 1–10. The `osdk-*` allocators are worth reading before writing the Indy frame allocator.
- Blockers: no MIPS, no 32-bit, x86-centric APIC/ACPI assumptions in places; MPL-2.0 file obligations if code is ported.
- Recommendation: **borrow design**. Do not fork Ostd for MIPS; the port would be larger than the IRIX 7 framework it is meant to save. `ostd-pod` and `zerocopy` are the only small pieces worth evaluating directly.

#### Theseus

- Repository: <https://github.com/theseus-os/Theseus>; **MIT**; last push 2024-09-22; 3,222 stars.
- Reuse: `MappedPages` exclusive-owned virtual regions, cell model, `fs_node`/`memfs`/`block_cache` structure, and the “intralingual” safety argument. The current tree has no persistent on-disk FS; `redox-os/tfs` is a different abandoned project.
- Blockers: x86-64/aarch64; stale; heavily tied to its own `memory` and `task` crates.
- Recommendation: **borrow design** for memory ownership and cell loading; do not port.

#### Redox

- Repository: <https://github.com/redox-os/kernel> (mirror of GitLab); **MIT**; active 2026-09-27; 906 stars. `redoxfs` is **MIT**, active 2026-09-21; the on-disk format is little-endian and the crate depends on Redox syscall error types.
- Reuse: Unix personality over a message-passing kernel; `RedoxFS` CoW/checksum design; `relibc` tests; scheduler/IPC ideas.
- Blockers: x86-64; Redox-specific `redox_syscall`; little-endian FS.
- Recommendation: **borrow design**; consider `RedoxFS` only as the native-FS fallback (I7FS is preferred), with a little-endian port and a Redox-shim removal.

#### rCore, zCore and ArceOS

- `rcore-os/rCore` — **MIT**, last push 2023-08-24; teaching kernel, RISC-V/x86.
- `rcore-os/zCore` — **MIT**, active 2026-09-06; Zircon microkernel reimplementation in Rust; useful object/handle and VFS design.
- `rcore-os/arceos` — **Apache-2.0**, last push 2026-01-29; modular OS components (`axalloc`, `axfs`, `axtask`) but 64-bit.
- `buddy_system_allocator` — **MIT**, 0.13.0, active 2026-03-30; the cleanest small frame-allocator prior art.
- Recommendation: **borrow design** for object/handle and VFS; **adopt** `buddy_system_allocator` only if its API matches the frame allocator; otherwise keep the existing frame allocator and use its tests.

#### Hubris and Tock

- Hubris: <https://github.com/oxidecomputer/hubris>; **MPL-2.0**; active 2026-10-24; 3,639 stars. Message-passing, memory-protected kernel for deeply embedded ARM. Reuse the task/IPC and driver-server structure; no MIPS.
- Tock: <https://github.com/tock/tock>; **Apache-2.0 OR MIT**; active 2026-10-23; 6,476 stars. The kernel is ARM/RISC-V, but `tock-registers` 0.10.1 is a clean, no_std MMIO register abstraction and is directly usable.
- Recommendation: **borrow design** from Hubris; **adopt `tock-registers`** and consider `volatile` 0.6 (MIT/Apache) for raw MMIO.

#### Small `no_std` building blocks

| Crate | Licence | Use | Notes |
|---|---|---|---|
| `linked_list_allocator` 0.10.6 | MIT/Apache-2.0 | Simple kernel heap | No fragmentation guarantees; fine for bring-up. |
| `buddy_system_allocator` 0.13.0 | MIT | Frame allocator | Power-of-two; good fit for 4 KiB/256 KiB pages. |
| `rlsf` 0.2.3 | MIT/Apache-2.0 | Bounded-latency heap | TLSF; useful for audio/display cells; 32-bit supported. |
| `talc` 5.1.1 | MIT | Fast `no_std` allocator | Good alternative to `linked_list_allocator`. |
| `spin` 0.12.3 | MIT | Spinlocks | Used by many `no_std` crates; MIPS LL/SC via atomics. |
| `lock_api` 0.5.0 | MIT/Apache-2.0 | Lock traits | `parking_lot` itself needs threads/std. |
| `critical-section` 1.2.0 | MIT/Apache-2.0 | Interrupt-safe critical sections | Implement the platform hook once. |
| `portable-atomic` 1.15.0 | Apache-2.0/MIT | Atomics on MIPS III | Verify LL/SC and 64-bit atomics on R4400. |
| `heapless` 0.9.3 | MIT/Apache-2.0 | Fixed-capacity queues/maps | Good for drivers and `no_std` services. |
| `hashbrown` 0.17.1 | MIT/Apache-2.0 | `no_std` + alloc hash map | Used by `cosmic-text` optionally. |
| `intrusive-collections` 0.10.3 | MIT/Apache-2.0 | Intrusive lists/trees | Good for scheduler run queues. |
| `embassy-executor`/`embassy-time`/`embassy-sync` | MIT/Apache-2.0 | `no_std` async | Strong fit for driver cells and a future async libc; needs a MIPS timer/executor hook. |
| `futures` 0.3.34 / `async-task` 4.7.1 | MIT/Apache-2.0 | Async traits/tasks | Use only after the cell runtime exists. |

**Verdict:** no external kernel is a base. The portable allocator/lock/async crates are real dependencies; the kernels are design references.

---

### 5.2 MIPS, IRIX, ELF and debug tooling

This section serves **#6 (IRIX 6.5 compatibility)** and **#10 (native cells/loader)**; `gdbstub` also serves **#2 (MCP/debug)** and **#15 (security)**, and `rabbitizer`/`capstone` serve **#12 (clean-room analysis)**.

IRIX 7 already has `crates/irix-elf`: a `no_std`, allocation-free, big-endian ELF32 parser with o32/n32 classification and a load plan. That is the loader contract; generic ELF crates should not replace it.

| Candidate | Licence | Reuse | Blockers | Verdict |
|---|---|---|---|---|
| `object` 0.40.0 | Apache-2.0 OR MIT | Host-side ELF/object inspection, big-endian ELF32, symbol/section parsing, test fixtures | `std` for the high-level API; not a loader | **Adopt host tools**; keep `irix-elf` for the kernel. |
| `xmas-elf` 0.10.0 | Apache-2.0 OR MIT | Small `no_std` ELF parser as a differential oracle | Last push 2025-03-26; simple API; no MIPS ABI classification | **Borrow/test**, not the loader. |
| `elf` 0.8.0 | MIT/Apache-2.0 | `no_std` ELF parsing with `std`/`alloc` off | Generic; no MIPS ABI policy | Optional oracle. |
| `goblin` 0.10.7 | MIT | Host ELF/Mach-O/PE inspection | `std`; large | Host tool only. |
| `gimli` 0.34.0 / `addr2line` 0.27.1 | Apache-2.0 OR MIT | DWARF, symbolisation, backtraces | Host; `gimli` has `no_std` read support | **Adopt host tools**; useful for panic decoding. |
| `gdbstub` 0.7.10 | MIT OR Apache-2.0 | GDB Remote Serial Protocol, `no_std`, **MIPS32/64 architecture modules** | Needs trap-frame, memory and breakpoint glue; no built-in target XML for 64-bit MIPS | **Adopt/fork** for MCP GDB. Verified MIPS support in `gdbstub_arch/src/mips`. |
| `rabbitizer` 2.0.0-alpha.9 | MIT | Pure-Rust MIPS instruction decoder; N64/MIPS III heritage | Alpha version; host tool | **Adopt host tool** for disassembly and clean-room analysis. |
| `capstone` 0.14.0 | MIT wrapper; Capstone C library BSD-3 | Host disassembly, broad ISA coverage | C dependency, build system | Host only; prefer `rabbitizer` for MIPS. |
| `yaxpeax-mips` 0.1.0 | 0BSD | MIPS decoder | Last release 2021; limited | Not recommended; `rabbitizer` is better maintained. |
| `MIPS_disassembly` 0.1.8 | Not verified | Simple decoder | Tiny, unclear maintenance | Do not depend. |
| `mips` 0.2.1 / `mips-rt` / `mips-mcu` | Permissive (verify each) | Low-level MIPS microcontroller access | PIC32/MIPS32 microcontrollers, not R4400/Indy | Not relevant. |
| `probe-rs` 0.32.0 | MIT OR Apache-2.0 | Host debug probe framework | ARM/RISC-V focus; no MIPS | Reject for MIPS. |
| `danifunker/rust-irixlibstd` | **No licence file at the repository root on the branch inspected** | Evidence that o32/n32 std, threads, sockets and unwinding work on IRIX 6.5.22 | All rights reserved; cannot copy | **Reject code**; prior art only. |
| `atomchild411/rust-irix` | **No licence file at the repository root on the branch inspected** | Evidence of a `mips64-sgi-irix` rustc fork, libc module and std port; `libc`/`rust`/`pkgsrc` branches | All rights reserved; the modifications are not MIT/Apache just because upstream is | **Reject code**; request relicensing if the author is willing. |

**Endian and POD codecs.** The safest default is `u32::from_be_bytes`/`to_le_bytes` and explicit field codecs; do not `transmute` across an endian boundary. For structured on-disk or MMIO data, the useful crates are `zerocopy` 0.8/0.9 (BSD-2-Clause OR Apache-2.0 OR MIT; `FromBytes`/`IntoBytes`, no_std, big-endian-aware), `bytemuck` 1.25 (Zlib OR Apache-2.0 OR MIT; POD casts), `endian-num` 0.2 (MIT OR Apache-2.0; explicit big/little-endian numeric wrappers), `plain` 0.2 (MIT/Apache-2.0; POD derive) and `byteorder` 1.5 (Unlicense OR MIT; older but stable). `zerocopy` and `bytemuck` still require a target test: a type that is “POD” on x86 is not automatically safe to cast from a big-endian byte stream. The kernel should prefer explicit codecs; these crates are most useful in host tools and in parsers with a written layout assertion.

**MIPS emulation.** QEMU and the project’s `iris` fork are the practical system emulators; they are external test tools, not reusable Rust code. `rsemu` 0.0.6 (MIT) is a pure-Rust, multi-machine emulator framework, but it is new (0 stars) and no MIPS core is verified; do not build the test strategy on it. `unicorn` is GPL-2.0 and rejected. Instruction-level tests should use `rabbitizer` for decode and QEMU/iris for execution.

**Important correction to the prior research:** `lamxfs` does support clean XFS v4 as well as v5, but it refuses a dirty v4 log and has no IRIX-specific directory/ACL support (see §5.5). The two Rust-on-IRIX ports are technically the closest prior art for `std`, `libc` and unwinding, and they are **unlicensed**; this is a first-order risk, not a footnote.

The safest path is the one already chosen in `rust-std-irix7.md`: a clean-room `libc` IRIX module generated from POSIX.1-2024 and the project’s own ABI, plus a MIPS `unwinding` port written from the DWARF/MIPS ABI. The unlicensed ports can be read for **behavioural facts** but not copied; a clean-room reviewer should record that decision.

---

### 5.3 Boot and ROM

This section serves **#2 (M1)**; the CRC/`BootInfo` work completes the conformance that the closed boot-protocol task #33 started, and the size tools serve **#17**.

The ROM is MIPS III assembly, not Rust. It sizes and tests DRAM, brings up the serial console, LZ4-decompresses the kernel and jumps. It does not emit `BootInfo` yet; #2 covers the next boot step.

| Candidate | Licence | Reuse | Blockers | Verdict |
|---|---|---|---|---|
| `lz4_flex` 0.14.0 | MIT | Pure-Rust LZ4 block/frame codec; host tests and a reference decoder for the ROM payload | The ROM decompressor stays assembly; no `no_std` concern for host | **Adopt host/test**; do not replace the ROM. |
| `lz4` 1.28.1 | MIT wrapper; LZ4 C library BSD-2 | Host compatibility with `lz4` CLI | C dependency | Host only. |
| `crc` 3.4.0 | MIT OR Apache-2.0 | CRC32C/CRC32 for `BootInfo` and image records | Verify `no_std` and const API | **Adopt** for `BootInfo` conformance. |
| `crc32c` 0.6.8 | Apache-2.0/MIT | Host CRC32C with hardware acceleration | No `no_std` feature; `std`/build script | Host only. |
| `crc32fast` 1.5.3 | MIT OR Apache-2.0 | Fast CRC32 (IEEE), not CRC32C | Wrong polynomial for `BootInfo` | Only if a CRC32 field appears. |
| `sha2` 0.11.1 | MIT OR Apache-2.0 | SHA-256 for image/module digests; host cross-check of the in-kernel SHA-256 | Pure Rust; `no_std` | **Adopt host/test**; kernel already has a SHA-256 implementation. |
| `blake3` 1.8.7 | CC0-1.0 OR Apache-2.0 | Already used for OIDs and CAS | `no_std` + `pure`; MIPS performance | Already adopted; keep the `pure` feature. |
| `ed25519-dalek` 2.1 / `x25519-dalek` 3.0 | BSD-3-Clause | Capability signatures; key exchange | Dalek 3.0 may need API work; 32-bit/no_std | Already adopted at 2.1; evaluate 3.0 separately. |

**Verdict:** the boot path needs very little external code. `crc` (CRC32C) and `lz4_flex` (host verification) are the useful additions; the ROM and kernel payload format stay project-owned.

---

### 5.4 Networking

This section serves **#2 (M1 network/MCP)**, **#3 (M2 transport)** and **#5 (M4/cloud)**; the userland TLS/HTTP crates also serve **#6/#10**, and the mesh transport work is **#3/#4/#5**.

The kernel already has `smoltcp` 0.13 (0BSD) with Ethernet, IPv4/IPv6, TCP, UDP, DHCPv4, DNS and ICMP, plus `embedded-tls` 0.19 (Apache-2.0) with `rustpki`, P-384 and RSA. This is a deliberately C-free TLS stack; `ring` and `aws-lc-rs` are **not** used and should not be introduced into the kernel.

| Candidate | Licence | Reuse | Blockers | Verdict |
|---|---|---|---|---|
| `smoltcp` 0.14.0 | 0BSD | Upgrade from 0.13; new features and fixes | API drift; MSRV 1.91; retest DHCP/DNS/TCP on MIPS | **Adopt/upgrade candidate**, not urgent. |
| `embedded-tls` 0.19.0 | Apache-2.0 | Existing TLS 1.3 client | Session reuse, streaming digest, certificate-chain flexibility | **Already adopted**; extend, do not replace. |
| `rustls` 0.24-dev (0.23 stable line) | Apache-2.0 OR ISC OR MIT | Userland TLS, HTTP/2/3, mesh TLS | Default `ring`/`aws-lc-rs` providers are C/asm and lack MIPS; pure providers are alpha | **Needs a spike**; use only with `rustls-rustcrypto`/`rustls-pure` or a proven pure provider. |
| `rustls-webpki` 0.104-alpha / 0.103 stable | ISC | X.509 path validation | Follow `rustls` version; alpha API | Adopt with `rustls` when the provider is settled. |
| `webpki-roots` 1.0.11 | CDLA-Permissive-2.0 | Mozilla CA bundle | Data licence; the kernel already embeds a single SSL.com root | Optional; keep the existing root to save PROM. |
| `rcgen` 0.14.10 / `x509-parser` 0.18.1 / `der` / `pem` | MIT/Apache-2.0, Apache/MIT | Host test certificates and certificate inspection | `std` for the high-level APIs | **Adopt host tests**. |
| `dhcproto` 0.15.0 | MIT | DHCPv4/v6 packet parser; test oracle for the custom DHCP | `std`/`alloc`; the kernel has its own DHCP | Oracle only. |
| `simple-dns` 0.12.0 | MIT | DNS packet parser/writer; test oracle | No `no_std` claim verified | Oracle only. |
| `domain` 0.12.3 | BSD-3-Clause | Full DNS library; host resolver/tests | `std`; large | Host/test only. |
| `hickory-proto`/`hickory-resolver` 0.26.3 | MIT OR Apache-2.0 | DNS client/server reference | `std`/async; large | Host/test only. |
| `sntpc` 0.11.0 | MIT OR Apache-2.0 | `no_std` SNTP client; test oracle for the custom NTP | Small; verify `no_std` features | Oracle/test. |
| `ntp` 0.5.0 | MIT/Apache-2.0 | Old NTP parser | Last release 2018 | Reject; use `sntpc` or the in-tree NTP. |
| `quinn` 0.11.12 | MIT OR Apache-2.0 | QUIC transport; mesh/HTTP3 | `std`/tokio, rustls/ring; no `no_std` | **Borrow design/host**, not the Indy kernel. |
| `quinn-proto` 0.11.19 | MIT OR Apache-2.0 | QUIC state machine | `std` (tracing/thiserror/rand), ring/rustls | Design reference; a port is a project. |
| `iroh` 1.4.1 / `iroh-blobs` 0.103 | MIT OR Apache-2.0 | Dial-by-public-key, verified blob streaming | `std`/tokio; heavy | **Borrow design** for mesh/ CAS; host service only. |
| `libp2p` 0.57.1 | MIT | Peer-to-peer stack | Huge, `std`/tokio, many protocols | Reject for the Indy; separate PC/cloud option. |
| `s2n-quic` 1.91.0 | Apache-2.0 | QUIC | AWS-LC C crypto; no MIPS | Reject for Indy. |
| `etherparse` 0.21.0 | MIT OR Apache-2.0 | Packet parsing tests | `no_std`? verify | Test/tool. |
| `pnet` 0.35.0 | MIT OR Apache-2.0 | Host packet tools | `std`; old but maintained | Host only. |
| `httparse` 1.10.1 / `http` 1.5.0 | MIT OR Apache-2.0 | Userland HTTP parsing/types | `std`/alloc; no kernel need | Userland. |
| `ureq` 3.5.0 | MIT OR Apache-2.0 | Blocking HTTP client; host tools | `std` | Host only. |
| `reqwest` 0.13.5 | MIT OR Apache-2.0 | Host HTTP client | `std`/tokio | Host only. |

**Optional 9P.** The architecture keeps 9P2000 as a secondary control/remote-mount protocol, not the root transport. Rust options are server-heavy: `ninep` (MIT, 9P2000, sans-io server), `p9` (BSD-3-Clause, 9P2000.L server used by crosvm), `rs9p` (BSD-3-Clause, 9P2000.L server), and `styx9p` (MIT/Apache, client+server, very new). None is `no_std`; all assume `std` and most assume Tokio. **Borrow the protocol/state-machine design** if 9P lands; do not make it a v1 dependency. This also matches the Plan 9 research report’s conclusion that 9P is optional and not the bulk transport.

**Verdict:** the kernel network stack is not the gap; the gaps are mesh transport and userland sockets. Keep `smoltcp` + `embedded-tls`; use `rustls` only after a pure-provider spike; borrow `quinn`/`iroh` design for the mesh transport rather than porting tokio.

---

### 5.5 Storage

This section serves **#9 (XFS read-only)**, **#6 (IRIX data compatibility)**, **#2 (EROFS/root path)** and **#11 (release/user data)**; the archive/package crates also serve **#10**.

The in-tree storage stack is further along than the rest of the system: `crates/erofs` reads standard little-endian EROFS (core, LZ4, fragments), `block-cache` provides an LRU, `overlay` provides a RAM upper tree, and `modules/erofs` mounts EROFS from SCSI. The root format decision (D21) is “standard EROFS + byte-order codec, no BE fork”; that remains correct.

| Candidate | Licence | Reuse | Blockers | Verdict |
|---|---|---|---|---|
| `erofs-rs` 0.3.0 | MIT OR Apache-2.0 | Independent EROFS reader; `no_std` + alloc now claimed; differential oracle | `std`/`rustix` coupling in parts; the project already has its own reader | **Reference/oracle**, not a port. |
| `rust-fs-erofs` 0.5.0 | MIT | Claims full EROFS reader + writer (`mkfs.erofs`, `fsck.erofs`), LZ4/LZMA/DEFLATE/ZSTD, xattrs/ACLs | One star, very short history, no independent users; trust only after differential tests | **Test oracle only**; do not make it a dependency. |
| `lamfold-erofs` 0.1.3 | MIT OR Apache-2.0 | `no_std` uncompressed EROFS reader with fs-verity; clean small design | Uncompressed only; one star; new | **Borrow design**; own reader already covers compressed. |
| `backhand` 0.25.5 | MIT OR Apache-2.0 | SquashFS reader/writer; fallback root image | `std`; little-endian; not the chosen root | Fallback only. |
| `lamxfs` 0.1.3 | MIT OR Apache-2.0 | `no_std` read-only XFS **v4 and v5**, shortform/block/leaf/node dirs, CRC32C v5, v5 log replay; refuses a dirty v4 log | No IRIX v1-directory/ACL support found; v4 log replay absent; 1 star, new; needs big-endian host test | **Fork/port** for #9; best current XFS reader. |
| `xfs-core` 0.1.6 | Apache-2.0 | Read-only XFS v4/v5 parser, inode/extents/bmap/dirs, no log replay | `std`; low maturity (2 stars, 6k downloads) | **Borrow/reference**; useful oracle and structure guide. |
| `rust-fs-xfs` 0.14.2 | MIT | Claims v4/v5 read, v4/v5 dirty-log replay, v5 write, `mkfs`/`fsck` | Created 2026-08, ~891 downloads, no independent validation | **Needs a spike**; not trusted for IRIX disks. |
| `xfuse`/`xfs-fuse` | BSD-2-Clause | FUSE XFS reader; host test harness | v5/dir3-centric; v4/IRIX unproven; `std`/FUSE | Test harness only. |
| `redoxfs` 0.9.1 | MIT | CoW, checksums, compression, 4 KiB blocks, `no_std` core; native FS fallback | Little-endian; `redox_syscall` coupling; no fsck; small transaction engine with `unsafe` blocks | **Fork only if I7FS slips** (D23). |
| `fatfs` 0.3.6 | MIT | FAT12/16/32 read/write; exchange media (floppy/ZIP/CF) | No permissions/journal; 4 GiB file limit | **Adopt** for exchange media, not root. |
| `littlefs2` 0.8.1 | Apache-2.0 OR MIT wrapper; littlefs C BSD-3 | Power-loss-resilient flash FS | C dependency; NOR-flash design | Reject Rust-first; no Indy need. |
| `ext4-view` 1.0.0 | MIT OR Apache-2.0 | `no_std` read-only ext2/3/4 | Little-endian; not IRIX | Low priority. |
| `tar` 0.4.46 / `zip` 9.1.0 | MIT OR Apache-2.0 / MIT | Package and archive handling | `std`; host/package tool | **Adopt host/package tool**. |
| `cpio` 0.4.1 / `ar` 0.9.0 | MIT | initramfs and `.a` handling | `std`; simple | **Adopt host tool**. |
| `fuser` 0.18.0 | MIT | Linux FUSE server; host test harness for the FS cell | Linux `/dev/fuse`; `std` | **Adopt host harness**; never on Indy. |
| `gix` 0.90.0 | MIT OR Apache-2.0 | Pure-Rust Git object store/smart HTTP; home cell | `std`, large, threads/mmap; 32-bit memory | **Fork/host**; use a subset or a PC-side service. |
| `git2` 0.21.0 | MIT/Apache wrapper; **libgit2 GPL-2.0-only** with linking exception | Git bindings | Licence combination risk; C dependency | **Reject**; use `gix`. |
| `ruzstd` 0.9.1 | MIT | Pure-Rust zstd decoder | Decoder only; `no_std`? verify | **Adopt** for package decode. |
| `zstd` 0.14.3 | BSD-3-Clause | zstd bindings | C zstd; cross-compile | Avoid in kernel; host tool. |
| `miniz_oxide` 0.9.1 | MIT OR Zlib OR Apache-2.0 | Pure-Rust DEFLATE | No zstd; `no_std` | **Adopt** for DEFLATE. |
| `flate2` 1.1.10 | MIT OR Apache-2.0 | DEFLATE with Rust backend | Use `rust_backend` to avoid zlib C | **Adopt** with the pure backend. |
| `lz4_flex` 0.14.0 | MIT | Pure-Rust LZ4 | Check the block vs frame API against the package format | **Adopt** for package/archive decode. |
| `crc32fast` 1.5.3 | MIT OR Apache-2.0 | CRC32 (not CRC32C) | Wrong polynomial for XFS/BootInfo | Use `crc` for CRC32C. |

**XFS reality check.** IRIX 6.5 uses XFS v4 with a big-endian log. `lamxfs` reads clean v4 and v5 but refuses a dirty v4 log; `xfs-core` has no log replay; `rust-fs-xfs` claims v4 log replay but is unproven. The read-only server should therefore start with **clean** IRIX v4 images, and dirty-log replay remains a separate 3–5 person-month project (as the FS report says). No candidate implements IRIX v1 directories, IRIX ACLs or the IRIX `XLOG_FMT_UNKNOWN` log; those are clean-room work. The 2026-10-20 FS report listed `lamxfs` as Apache-2.0; at retrieval it is **MIT OR Apache-2.0**, and the EROFS field has new entrants (`rust-fs-erofs`, `lamfold-erofs`). Treat the older report’s version table as a starting point, not as current.

**EROFS reality check.** `rust-fs-erofs` is interesting as a **second implementation** for differential tests and as a possible host-side image writer, but it is too new to trust as the production reader. The project’s own reader is the production path; use `mkfs.erofs`/`fsck.erofs` as the primary oracle, and `rust-fs-erofs`/`lamfold-erofs` as extra checks.

---

### 5.6 Drivers and hardware

This section serves **#10 (native cells/drivers)**, **#16 (hardware lab)** and **#2 (M1 devices)**; the audio/media crates also serve **#7** and the PC PCI crates **#5**.

The Indy’s devices are unusual enough that no existing Rust crate is a driver. The reusable material is **register and DMA abstraction**, not device code.

| Candidate | Licence | Reuse | Blockers | Verdict |
|---|---|---|---|---|
| `tock-registers` 0.10.1 | MIT/Apache-2.0 | `Readable`/`Writable`/`ReadWrite` register fields, `RegisterValue` | None significant; `no_std` | **Adopt**. |
| `volatile` 0.6.1 | MIT OR Apache-2.0 | Volatile MMIO reads/writes | Last push 2024-06; simple and stable | **Adopt**. |
| `bitfield` 0.20.0 | MIT OR Apache-2.0 | Register bitfields | Macro-heavy; compile-time cost | Optional. |
| `embedded-hal` 1.0.0 / `embedded-hal-async` | MIT OR Apache-2.0 | Trait vocabulary for blockers, delays, SPI/I2C | No PCI/SCSI/DMA; traits are embedded-oriented | **Borrow design**, not a dependency. |
| `embedded-dma` 0.2.0 | MIT OR Apache-2.0 | DMA buffer ownership traits | Small, old, but sound | **Borrow design** for `DmaWindow`. |
| `pcics` 0.3.2 | MIT | PCI configuration-space access | Low activity (2024); PC shim only | Evaluate for Phase 6; not Indy. |
| `pci-ids` 0.2.6 | MIT | PCI ID database | Archived 2025-10 | Do not depend; snapshot the public `pci.ids` if needed. |
| `scsi` 0.2.1 | Apache-2.0 | SCSI command encoding, `no_std` | Last release 2019; 10 stars | Borrow command definitions; do not depend. |
| `cotton-scsi` 0.2.0 | CC0-1.0 | SCSI commands/abstractions | New (2026), 33 stars, low downloads | Evaluate; CC0 is compatible. |
| `scsir` 0.3.0 | MIT OR Apache-2.0 | SCSI command issuing | One star, new | Low priority. |
| `virtio-drivers` 0.13.0 | MIT | `no_std` VirtIO net/block/console/gpu drivers | 64-bit/PCI assumptions; PC/cloud only | **Adopt later** for #5. |
| `symphonia` 0.6.1 | MPL-2.0 | Audio demux/decode (MP3/AAC/FLAC/Vorbis/WAV) | `std`/alloc; CPU cost on R4400; MPL file obligations | **Borrow/port** for the audio cell, not the kernel. |
| `rubato` 5.0.1 | MIT OR Apache-2.0 | Sample-rate conversion | `std`/alloc; floating point | **Adopt** in the audio cell after FP policy is settled. |
| `dasp` 0.11.0 | MIT OR Apache-2.0 | Sample conversion/mixing traits | `no_std`-friendly | **Adopt** for the mixer. |
| `hound` 3.5.1 | Apache-2.0 | WAV read/write | `std`; simple | Host/test. |
| `lewton` 0.10.2 | MIT OR Apache-2.0 | Vorbis decode | `std`; pure Rust | Audio cell option. |
| `yuv` 0.8.19 | BSD-3-Clause OR Apache-2.0 | YUV conversion for Vino capture | `std`/SIMD; verify 32-bit | Evaluate for the Vino cell. |
| `cpal` 0.18.2 | Apache-2.0 | Cross-platform audio I/O | Linux ALSA/ASIO/CoreAudio; not HAL2 | Reject for Indy. |

**Verdict:** adopt `tock-registers` + `volatile`; borrow the DMA/embedded-hal vocabulary; write the Seeq, WD33C93, HAL2, Newport and Vino drivers in-house with documented `unsafe`. There is no Rust Indy driver to reuse.

---

### 5.7 POSIX, libc, processes, threads, TLS and signals

This section serves **#6 (IRIX compatibility)**, **#10 (native cells/libc ABI)** and **#11 (real-Indy release)**; `std`/`unwinding` are gated by M9/M9.5 under #10.

This is the longest pole, and the project already owns the right base: the [`irix7/relibc`](https://github.com/irix7/relibc) fork (MIT) passes an n32 big-endian Linux/QEMU gate for TLS, `setjmp` and pthreads; the native PAL is linked but not booted. The kernel has a 42-symbol shim and a static `std` image with args, time, sleep and a small ramfs.

| Candidate | Licence | Reuse | Blockers | Verdict |
|---|---|---|---|---|
| `relibc` (own fork) | MIT | POSIX C ABI, TLS, pthreads, `setjmp`, syscall seams | Native PAL, error channel, `stat` ABI, signals, termios, sockets | **Own fork**; keep as production libc. |
| `libc` 0.2 stable | MIT OR Apache-2.0 | Upstream type/constant definitions; a clean-room IRIX module | No IRIX module today; `libc` 1.0 is alpha | **Adopt base**, add `irix7` module from POSIX/IRIX facts, not from the unlicensed port. |
| `rustix` 1.1.5 | Apache-2.0 WITH LLVM-exception OR Apache-2.0 | POSIX-shaped API design, `io_uring`/Linux paths as anti-pattern examples | Linux syscall numbers and `linux-raw-sys`; not portable | **Borrow design**, reject as dependency. |
| `nix` 0.31.3 | MIT | Unix API design and tests | Linux/BSD assumptions; no IRIX | Borrow design. |
| `linux-raw-sys` 0.12.1 | Apache-2.0 WITH LLVM-exception | None | Linux ABI only | Reject. |
| `signal-hook` 0.4.5 | MIT OR Apache-2.0 | Signal-handling patterns | Linux `signalfd`/self-pipe assumptions; no IRIX | Borrow design only. |
| `parking_lot` 0.13.0 | MIT OR Apache-2.0 | Faster mutex/rwlock for userland | Threads/std; not kernel | Use after native threads land. |
| `memmap2` 0.9.12 | MIT OR Apache-2.0 | `mmap` for `std` and userland | `std`/Unix; uses `MAP_ANON` | **Adopt userland**. |
| `getrandom` 0.4.3 | MIT OR Apache-2.0 | Userland `getrandom` backend over `getentropy` | OS hooks; kernel has its own entropy | **Adopt userland** once `getentropy` exists. |
| `unwinding` 0.2.10 | MIT OR Apache-2.0 | Pure-Rust `_Unwind_*`; `std` already uses it for Xous | **No MIPS support in the pinned version**; DWARF/MIPS ABI work needed | **Adopt + port**; highest-value userland runtime task. |
| `rustc-demangle` 0.1.28 | MIT/Apache-2.0 | Symbol demangling for panics | Host/symboliser | **Adopt host/userland**. |
| `libc` IRIX ports (`danifunker`, `atomchild411`) | **No licence** | Evidence of working std/libc/unwinding | Cannot copy | **Reject code**; clean-room reimplement. |
| Redox `relibc` | MIT | POSIX-over-capability design; test ideas | Redox syscall ABI; Linux-shaped parts | Borrow design; the project fork is already based on upstream relibc. |

**Threads/TLS reality.** Native `std` currently uses pthread keys, not ELF TLS; the target sets `has-thread-local: false`. `pthread_create`/`join`, mutexes/condvars, `errno`, and `fork`/`posix_spawn` are still ahead. Any crate that needs `std::thread`, `parking_lot`, `tokio`, `rayon` or ELF TLS is **userland-later**, not kernel-ready. The `unwinding` MIPS port is a prerequisite for `panic = "unwind"` and `catch_unwind`, and even abort builds link the unwinder for backtraces (as `rust-std-irix7.md` §2.4 records).

`musl`, `newlib` and `picolibc` are C libc implementations; they are useful as POSIX behaviour references and test oracles, but they are not Rust-first candidates and none targets IRIX 7. The production C ABI layer remains the Rust `relibc` fork.

**Signals.** There is no portable Rust signal crate for IRIX. `signal-hook` and `nix` are Linux/BSD-shaped; the native signal frame and `sigreturn` must be written from the IRIX/POSIX ABI. Use them as API design references only.

---

### 5.8 Mesh, actors, CAS and Wasm

This section serves **#2 (M1 local core)**, **#3 (M2 two-node)**, **#4 (M3 eight-node)** and **#5 (M4 128-node)**; CAS/capability crates also serve **#15**.

The architecture crates already exist and are host-tested; the missing piece is the **distributed runtime**: gossip, transport, authority, and the kernel-side Wasm host.

| Candidate | Licence | Reuse | Blockers | Verdict |
|---|---|---|---|---|
| `foca` 2.0.0 | MPL-2.0 | `no_std` + `alloc` SWIM (SWIM+Inf.+Susp.), transport/identity agnostic, custom codec | Needs a UDP/timer adapter over smoltcp; 237 stars, active; MPL file obligations | **Adopt** for #3/#4/#5. This is the single best fit in the mesh space. |
| `chitchat` 0.13.0 | MIT | Scuttlebutt-style cluster membership/gossip; delta compression | `std`/tokio; design only | Borrow design. |
| `openraft` 0.10.0-alpha | MIT OR Apache-2.0 | Raft consensus for membership/root authority | `std`/async; alpha; heavy | Borrow design; the mesh only needs consensus for membership/authority, not the data path. |
| `phi-detector` 0.4.0 | MIT | Phi-accrual failure detection | Small; std; design | Borrow/test. |
| `wasmi` 2.0.0 | MIT/Apache-2.0 | Already used in `wasm-host`; interpreter, fuel, deterministic mode | Kernel `no_std` path unproven; `wat` feature needs std; MIPS performance | **Already adopted**; spike the kernel build. |
| `wasmparser` 0.262.0 | Apache-2.0 WITH LLVM-exception OR Apache-2.0 | Validation, section parsing, feature detection | `std` feature optional; no_std path unverified; `wasmparser-nostd` is a stale 0.100 fork | **Adopt host/tools**; spike no_std if used in-kernel. |
| `wasm-encoder` / `wasmprinter` 0.262.0 | Apache-2.0 WITH LLVM-exception OR Apache-2.0 | Test fixtures, WAT/WasM round-trips, core image tooling | Host tools | **Adopt host/tools**. |
| `wasmtime` 50.0.0 | Apache-2.0 WITH LLVM-exception | Host reference runtime, differential semantics | JIT, 64-bit, threads, huge | **Reject for Indy**; host-only oracle. |
| `postcard` 1.1.3 | MIT OR Apache-2.0 | `no_std` canonical wire for mesh frames/state | No schema evolution; use explicit versioning | **Adopt** for new wire records. |
| `serde` 1.0.229 | MIT OR Apache-2.0 | Derive/serialisation framework | `no_std` + alloc | **Adopt**. |
| `rkyv` 0.8.18 | MIT | Zero-copy archived state | Native-endian by default; cross-endian needs `rend`/explicit config | **Caution**; `postcard` is safer for cross-architecture state. |
| `bincode` 3.0.0 | MIT | Binary serialisation | Repository archived 2025-08; no maintainer | **Reject**; use `postcard`/`ciborium`. |
| `ciborium` 0.2.2 | Apache-2.0 | CBOR, endian-neutral | `std`/alloc; slower | Optional canonical format. |
| `iroh-blobs` 0.103 | MIT OR Apache-2.0 | Verified blob streaming (Bao), content-addressed transfer | `std`/tokio; design only | **Borrow design** for CAS fetch. |
| `blake3` / `ed25519-dalek` / `x25519-dalek` | CC0/Apache; BSD-3 | Hashing, signatures, key exchange | Already in `cas-core`/`cap-engine`; 32-bit/no_std | Already adopted; extend carefully. |
| `aes-gcm` / `chacha20poly1305` / `hkdf` | MIT OR Apache-2.0 | TLS/mesh record crypto | Pure Rust; no_std | **Adopt** as needed; `embedded-tls` already supplies the kernel path. |

**Verdict:** adopt `foca` for SWIM; keep `wasmi` but prove the `no_std`/MIPS build; use `postcard` for new cross-endian wire records; borrow `iroh-blobs`/`openraft` design rather than porting their runtimes. The 128-node gate is a policy/simulator problem first (already covered by `mesh-sim`), then a transport problem.

---

### 5.9 Desktop, display and X11

This section serves **#7 (display/desktop)**, **#8 (X11)** and **#194 (COSMIC desktop)**; the X11 client/server work also serves **#6/#11**.

The native display direction is fixed: a server-side-drawing display cell, retained window images, damage repair, no alpha/transforms/scaling, 8-bit indexed default, REX3 blitter. The toolkit direction is D33: a downstream COSMIC (`libcosmic`/`iced`/`cosmic-text`) fork on the native display server; no Wayland, `cosmic-comp`, DRM/GBM/EGL or GPU on the Indy. X11 is the compatibility surface, not the native desktop.

#### X11

| Candidate | Licence | Reuse | Blockers | Verdict |
|---|---|---|---|---|
| `x11rb` 0.14.0 | MIT OR Apache-2.0 | Complete X11 wire types, request/reply encoders, connection setup, MSBFirst client support; `x11rb-protocol` builds `no_std` | Client-side; no server dispatch/window tree; server-side request parsing needs work | **Adopt/fork** for the X11 server and Xlib shim. |
| `yserver` | MIT | Architecture, protocol coverage, X Test Suite methodology, byte-swapped-client work | ~215k Rust LOC; DRM/KMS/Vulkan; little-endian renderer; nested X11 backend abandoned; not portable as-is | **Borrow design/tests**, do not port. |
| `x12` | MIT | Hard fork of yserver aimed at cross-platform | 0 stars, one-line README, no releases | Watch, do not depend. |
| `rxserver` | Apache-2.0 | Small X11 server learning exercise | TCP-only, many core requests missing, `softbuffer` | Learning only. |
| `x11rb-protocol` | MIT OR Apache-2.0 | Wire structs and serialisers; `no_std` | No server-side dispatch | **Adopt** as the wire layer. |

The X11 server remains a **20–30k-line** clean-room job, and the client-side `libX11`/`libXt`/`libXm` ABI is a second **20–30k-line** job; neither is solved by a server library. `x11rb` saves the wire layer (perhaps 5–10k lines); it does not save the window tree, resource IDs, event delivery, ICCCM, SHAPE, XTEST or the Xlib ABI. IRIX clients are big-endian, so an MSBFirst-only first phase is legitimate; the server must still speak the client’s byte order. `yserver` is the best evidence that a Rust X11 server is tractable and the best source of XTS test ideas, but its renderer is the wrong shape for Newport.

#### Native display and toolkit

| Candidate | Licence | Reuse | Blockers | Verdict |
|---|---|---|---|---|
| `cosmic-text` 0.19.0 | MIT OR Apache-2.0 | Multi-line text layout, shaping, font fallback; explicit `no_std` feature; depends on `harfrust` 0.5 | Big-endian/32-bit unproven; `fontdb`/`sys-locale` std features; memory | **Adopt/fork** for text in the display server and X11. |
| `harfrust` 0.15.0 | MIT | HarfBuzz shaping port; `no_std` + `libm`; maintained successor to `rustybuzz` | Newer than cosmic-text’s pinned 0.5; API drift | **Adopt**; pin consistently. |
| `tiny-skia` 0.12.0 | BSD-3-Clause | CPU rasteriser (paths, fills, blends, PNG); `no_std` + `no-std-float` | SIMD disabled on MIPS; pixel byte order/32-bit needs a spike; `std` default | **Adopt** for software rasterisation in the display/X11 cells. |
| `iced` 0.14.0 (0.15-dev) | MIT | Widget/runtime architecture, `tiny-skia` software renderer, `cosmic-text` integration | Default `wgpu`/`winit`/x11/wayland/thread-pool; needs a custom native backend and executor; std | **Fork**. |
| `libcosmic` 1.0.0 | MPL-2.0 | COSMIC application API, widgets, settings/config, COSMIC look | Default winit/tokio/dbus/X11/Wayland; uses a vendored iced with `tiny-skia`; MSRV 1.93; large; needs a native window/backend fork | **Fork (long-range)**; D33/#194, not a v1 dependency. |
| `swash` 0.2.10 | Apache-2.0 OR MIT | Font introspection, shaping, glyph rendering; `no_std`/`libm` | Option in cosmic-text; API drift | **Adopt** as needed. |
| `fontdb` 0.24.0 | MIT | Font database/discovery | `std`/mmap/fontconfig; Indy has no system font service | Userland only; use an in-memory font set. |
| `ab_glyph` 0.2.32 / `fontdue` 0.9.5 | Apache-2.0 / MIT OR Apache-2.0 OR Zlib | Simple font parsing/rasterising | CPU only; less complete than swash | Optional fallback. |
| `resvg` 0.49.0 | Apache-2.0 OR MIT | SVG icon rendering over `tiny-skia` | `std`; CPU | Optional for icons. |
| `kurbo` 0.13.1 / `lyon` 1.0.19 | Apache-2.0 OR MIT | 2D curves/tessellation | CPU; `lyon` is GPU-tessellation-oriented | Optional geometry helpers. |
| `raqote` 0.8.5 | BSD-3-Clause | CPU 2D renderer | Last push 2025-02; less maintained than `tiny-skia` | Reject in favour of `tiny-skia`. |
| `smithay` 0.7.0 | MIT | Wayland compositor architecture | Wayland/DRM/GBM/libinput; no Indy | **Reject for Indy**; borrow damage/buffer ideas only. |
| `wayland-client`/`wayland-server` 0.31 | MIT | Wayland protocol | Wayland is not the native protocol; no DRM/GBM | Reject for Indy; separate GPU path. |
| `calloop` 0.14.5 | MIT | Callback event loop | Uses `polling` (epoll on Linux); no IRIX backend | Borrow design; port if needed. |
| `winit` 0.31-beta | Apache-2.0 | Window creation | X11/Wayland/Windows/macOS; threads; no Indy backend | Reject for Indy. |
| `softbuffer` 0.4.8 | MIT OR Apache-2.0 | CPU buffer presentation | Platform backends; `raw-window-handle` | Borrow design for the display server. |
| `pixels` 0.17.2 | MIT | GPU pixel buffer | `wgpu`; GPU | Reject for Indy. |
| `vello` 0.11.0 | Apache-2.0 OR MIT | GPU compute 2D | GPU | Reject for Indy. |
| `wgpu` | MIT/Apache-2.0 | GPU abstraction | No GPU on Indy | Reject for Indy. |
| `cosmic-comp` | GPL-3.0 | Full COSMIC compositor | Wayland/DRM/GPU; enormous | Reject for Indy; separate GPU-node path. |

**Verdict:** the native display stack should be built from `tiny-skia` (raster), `cosmic-text` + `harfrust` + `swash` (text), and a custom REX3 backend. `iced` is the right application/runtime model but needs a fork; `libcosmic` is a long-range fork after the backend exists. X11 should use `x11rb` for wire and a clean-room server; `yserver` is a reference, not a base.

**Big-endian warning.** Text shaping and rasterisation are mostly integer/float code and should be endian-neutral, but the pixel formats, PNG byte order, `bytemuck` casts and `tiny-skia`’s `u32` pixels need an explicit byte-order test. Do not assume “pure Rust” means “big-endian correct”.

---

### 5.10 Userland utilities, shell and package store

This section serves **#6 (compatibility userland)**, **#10 (native userland)** and **#11 (daily-driver release)**, with the `sash`/package path also touching **#2**.

The native userland is nearly empty: `sash`, `hello`, `std-hello`, the o32 probe, and no coreutils or shell. The package index is a plain-text git catalogue with SHA-256 records; `sash` fetches it, verifies modules and installs userland images to SCSI target 2.

| Candidate | Licence | Reuse | Blockers | Verdict |
|---|---|---|---|---|
| `uutils/coreutils` | MIT | Broad POSIX utility implementations and tests; `uucore` shared library | `std`, `libc`/`nix`/`rustix`, Linux `statx`/`copy_file_range`/`splice` in places; no IRIX support | **Fork a subset**; highest-value userland source. |
| `brush-shell` 0.4.0 | MIT | POSIX/bash-compatible shell in Rust | `std`; `rustix`/`nix`; process/signal/terminal assumptions | **Fork/port** after process/termios. |
| `nushell` | MIT | Structured shell/data pipelines | Huge, async, std; memory | Reject for Indy; host tool. |
| `fish-shell` | GPL-2.0-only | Interactive shell | Licence incompatible | **Reject**. |
| `ripgrep` 15.2.0 | Unlicense OR MIT | Fast search; userland tool | `std`; threads optional; regex crate | **Adopt/fork** userland. |
| `fd` 10.5.0 / `bat` 0.26.1 | MIT OR Apache-2.0 | `find`/`cat` replacements | `std`; Linux-specific bits | **Adopt/fork** userland. |
| `eza` 0.23.5 | EUPL-1.2 | `ls` replacement | Licence caution; 32-bit; std | Caution; use `lsd` (Apache-2.0) or uutils `ls`. |
| `zoxide` / `starship` | MIT / ISC | Optional shell conveniences | `std`; low priority | Optional. |
| `alacritty_terminal` 0.26.0 | Apache-2.0 | Terminal emulation core (VTE parser, grid, selection) | `std`; threads/parking_lot; no PTY yet | **Adopt/fork** for the native terminal. |
| `vte` 0.15.0 | Apache-2.0 OR MIT | VTE parser | `no_std`? verify; small | **Adopt** as a parser. |
| `vt100` 0.18.1 | MIT | Simple terminal parser | Less complete than `alacritty_terminal` | Optional. |
| `portable-pty` 0.9.0 | MIT | PTY abstraction | Linux/Unix `openpty`; no kernel PTY | Userland later. |
| `tar` / `zip` / `cpio` / `ar` | MIT/Apache, MIT, MIT, MIT | Package/archive tooling | `std`; host or userland | **Adopt host/package tool**. |

**Port strategy for uutils.** Do not port all 100+ utilities. Start with a read-only core: `ls`, `cat`, `cp`, `mv`, `rm`, `mkdir`, `rmdir`, `ln`, `readlink`, `chmod`, `stat`, `touch`, `head`, `tail`, `wc`, `sort`, `uniq`, `tee`, `env`, `printf`, `test`, `date`, `sleep`, `du`, `df`, `find`. These exercise the POSIX file, process and terminal surfaces the native libc needs, and their tests are a useful conformance corpus. `cp`/`mv`/`rm` need `renameat`, `unlinkat`, `fstatat` and directory iteration; `find` needs `readdir`/`stat`; `date` needs `clock_gettime`; `df` needs `statvfs`.

**Shell strategy.** `brush-shell` is the most promising Rust shell, but it assumes a Unix process model (fork/exec, signals, job control, termios) that the native libc does not have yet. Keep `sash` as the recovery shell; port `brush` after `posix_spawn`, signals and termios. Do not port `fish` (GPL-2.0-only) or `nushell` (too large).

---

### 5.11 Testing, tooling and security

This section serves **#12 (clean-room/provenance)**, **#13 (CI)**, **#15 (security)**, **#17 (performance)** and **#11 (release)**. It is the highest-leverage category because it affects every gate and is host-side, so target portability is not a blocker.

| Candidate | Licence | Reuse | Verdict |
|---|---|---|---|
| `cargo-deny` 0.20.2 | MIT OR Apache-2.0 | Licence allowlist, advisory and source checks | **Adopt**; configure GPL-3.0-only allowlist. |
| `cargo-audit` 0.22.2 | Apache-2.0 OR MIT | RustSec advisories | **Adopt** in CI. |
| `cargo-cyclonedx` 0.5.9 | Apache-2.0 | CycloneDX SBOM | **Adopt** for release artefacts. |
| `cargo-about` 0.9.2 | MIT OR Apache-2.0 | Third-party notices/licence page | **Adopt**. |
| `cargo-nextest` 0.9.149 | Apache-2.0 OR MIT | Fast host test runner | **Adopt** host. |
| `cargo-llvm-cov` 0.9.1 | Apache-2.0 OR MIT | Host coverage | **Adopt** host; cross-MIPS coverage needs a spike. |
| `cargo-call-stack` 0.1.16 | MIT OR Apache-2.0 | Static stack-usage analysis | **Adopt**; verify MIPS target support; important for 64 KiB kernel stacks and deep driver chains. |
| `cargo-bloat` 0.12.1 | MIT | Binary size breakdown | **Adopt**; PROM budget is 24 KiB free. |
| `cargo-mutants` 27.1.0 | MIT | Mutation testing | Optional; good for parsers. |
| `proptest` 1.12.0 | MIT OR Apache-2.0 | Property tests for ELF/EROFS/XFS/9P/HTTP/MCP | **Adopt**. |
| `arbitrary` 1.5.0 | MIT OR Apache-2.0 | Fuzz input derivation | **Adopt** with `cargo-fuzz`. |
| `cargo-fuzz` 0.13.2 + `libfuzzer-sys` 0.4.13 | MIT OR Apache-2.0; NCSA | Fuzz targets | **Adopt** host. |
| `criterion` 0.8.2 / `divan` 0.1.21 | Apache-2.0 OR MIT / MIT OR Apache-2.0 | Host microbenchmarks | **Adopt**; Indy benchmarks remain manual. |
| `insta` 1.49.0 | Apache-2.0 | Snapshot tests for parsers/golden frames | **Adopt**. |
| `typos-cli` 1.51.1 | MIT OR Apache-2.0 | Spelling, including Australian English | **Adopt**. |
| `lychee` | Apache-2.0 | Link checking | Adopt if the docs build uses it. |
| `mdbook` | MPL-2.0 | Documentation build | Adopt if docs use it; MPL file obligations. |
| `kani-verifier` 0.68.0 | MIT OR Apache-2.0 | Bounded model checking of safe Rust | Optional; host x86-64 only. |
| Miri (rustc component) | MIT OR Apache-2.0 | Undefined-behaviour detection for host tests | **Adopt host**; cannot execute the MIPS kernel. |
| `rmcp` 3.7.0 | Apache-2.0 | Official MCP Rust SDK; conformance/client | **Adopt host/native cell**; not a kernel module. |
| `jsonrpsee` 0.26.1 | MIT | JSON-RPC server/client | Optional host tooling. |
| `axum` 0.8.9 / `hyper` 1.12 / `tower` 0.5 | MIT | HTTP stack for host MCP tooling | Host only. |
| `webpki-roots` | CDLA-Permissive-2.0 | CA bundle for host tests | Optional; data licence. |

**Security CI additions:** `cargo-deny` (licence + advisories), `cargo-audit`, `cargo-cyclonedx`, `cargo-about`, `proptest`/`cargo-fuzz` for parsers, and `cargo-call-stack`/`cargo-bloat` for the PROM/stack budgets. The project already has `CodeQL`/Dependabot plans; these tools fill the licence/SBOM/fuzz gaps. `cargo-cyclonedx` covers the CycloneDX artefact; SPDX still needs `cargo-about`/REUSE or a small generator, so do not treat SBOM as solved by one tool. The fuzz targets should mirror the testing plan: ELF/o32/n32, EROFS, XFS, 9P, HTTP, Git and MCP JSON-RPC. `proptest` is the better tool for round-trip properties (encode/decode, manifest canonicalisation); `cargo-fuzz` is the better tool for memory-safety and panic hunting on untrusted bytes.

**MCP:** `rmcp` is the official Rust SDK and is useful for a host-side client and conformance suite. The in-kernel MCP prototype is a custom JSON-RPC server with two TCP slots; `rmcp` is std/async and cannot replace it in the kernel. Use `rmcp` to test the prototype’s wire behaviour, and later in a native MCP cell (not a kernel module).

---

### 5.12 PC and cloud shims

This section serves **#5 (M4 cloud)** and Phase 6/7; these do not help M1/M2 but are part of the same architecture and can be sourced now.

| Candidate | Licence | Use | Blockers | Verdict |
|---|---|---|---|---|
| `bootloader` 0.11.17 | MIT OR Apache-2.0 | x86_64 BIOS/UEFI bootloader | x86_64 only; not Limine | Alternative. |
| `limine` 0.6.5 | MIT OR Apache-2.0 | Limine boot protocol bindings | Matches the plan’s Limine/UEFI path | **Adopt later**. |
| `uefi` 0.41.0 | MIT OR Apache-2.0 | UEFI services | Firmware assumptions | **Adopt later**. |
| `acpi` 7.0.0-pre1 | MIT/Apache-2.0 | ACPI tables/AML | Pre-release; x86_64 | Evaluate. |
| `x86_64` 0.16.0-rc | MIT/Apache-2.0 | x86_64 paging/CPU | x86_64 only | **Adopt later**. |
| `virtio-drivers` 0.13.0 | MIT | VirtIO net/block/console | 64-bit/PCI assumptions; no_std | **Adopt later** for #5. |
| `virtio`/`vm-memory`/`vhost` | Apache-2.0/MIT (verify) | VirtIO queue/memory | Host-side VMM, not guest | Only if building a host VMM. |

**Verdict:** defer until Phase 6/7; the licences are compatible and the crates are mature enough. Do not let PC/cloud work pull GPU or Wayland dependencies into the Indy build.

---

## 6. Rejected and licence-hazardous projects

### 6.1 Licence-incompatible or conditional

| Project | Licence | Reason |
|---|---|---|
| Linux kernel | GPL-2.0-only | Cannot be combined with GPL-3.0-only; also violates clean-room if copied. |
| `xfsprogs` / `libxfs` | GPL-2.0-only | Same; read the format docs, not the code. |
| `btrfs-cli` / `btrfs-mkfs` | GPL-2.0-only | Per-crate licence; do not assume the whole workspace is permissive. |
| OpenZFS | CDDL-1.0 | Not GPL-compatible. |
| `fish-shell` | GPL-2.0-only | Not GPL-3.0-compatible. |
| `git2` / libgit2 | MIT/Apache wrapper; libgit2 GPL-2.0-only with linking exception | The wrapper’s licence does not relicense libgit2; use `gix`. |
| `zerofs` / NineFS | AGPL-3.0 | Network copyleft; not acceptable. |
| Xpra / nx-libs | GPL-2.0 | Proxies, not servers; licence incompatible. |
| `unicorn` | GPL-2.0 | Emulator; external tool only, no code. |
| QEMU / `iris` | GPL-2.0 (verify) | Test tools; do not link or copy into the OS. |
| `mbedtls` | Apache-2.0 OR GPL-2.0-or-later | Could choose GPLv3, but it is C and not Rust-first; avoid. |
| EUPL-1.2 projects (`eza`) | EUPL-1.2 | Conditional, not a hard rejection: on the EUPL compatibility list but legal review required; prefer MIT alternatives. |

### 6.2 Licence-clear but rejected on portability or fit

| Project | Licence | Why not |
|---|---|---|
| `ring`, `aws-lc-rs` | Apache/ISC, ISC/Apache | C/assembly crypto; no MIPS III big-endian support; build systems are hostile to n32. |
| `wasmtime` | Apache-2.0 WITH LLVM-exception | JIT, 64-bit, threads, huge; host oracle only. |
| `libp2p` | MIT | Too heavy, std/tokio, many protocols; not the Indy mesh transport. |
| `nushell` | MIT | Large, async, std; not a 32–256 MiB target shell. |
| `smithay`, `wayland-rs`, `winit`, `cosmic-comp` | MIT / GPL-3.0 | Wayland/DRM/GBM/libinput/GPU assumptions; no Indy path. |
| `pixels`, `vello`, `wgpu` | MIT/Apache | GPU required; Newport has no GPU. |
| `bincode` | MIT | Archived 2025-08; use `postcard`. |
| `rustybuzz` | MIT | Archived 2026-07-26; use `harfrust`. |
| `pci-ids` | MIT | Archived 2025-10; snapshot `pci.ids` instead. |
| `scsi` 0.2.1 | Apache-2.0 | Last release 2019; borrow command definitions. |
| `capstone` | MIT wrapper / BSD-3 C | C dependency; host disassembly only. |
| `littlefs2` | Apache/MIT wrapper / BSD-3 C | C dependency; no Indy need. |
| `cpal` | Apache-2.0 | Linux ALSA/ASIO/CoreAudio; not HAL2. |
| `probe-rs` | MIT/Apache | No MIPS support. |

### 6.3 Unlicensed or unclear

| Project | Status | Consequence |
|---|---|---|
| `danifunker/rust-irixlibstd` | No LICENSE file at the repository root; GitHub reports no licence | All rights reserved; cannot copy code, tests or `unwinding-mips`. |
| `atomchild411/rust-irix` + `atomchild411/libc` (irix branch) + `atomchild411/rust` (irix branch) | No LICENSE file at the port repository root; the upstream rust/libc licences do not automatically cover the author’s modifications | Cannot copy the IRIX `libc` module, std support or target patches. Ask the author to relicense, or reimplement clean-room. |
| `rabbitizer` crate metadata | “non-standard” on crates.io, MIT in the repository | Verify the repository `LICENSE` before vendoring; the GitHub metadata is MIT. |
| `redoxfs` crate metadata | “non-standard” on crates.io, MIT in the repository | Verify the repository `LICENSE`; the GitHub metadata is MIT. |
| `xfs-core` / `lamxfs` / `rust-fs-erofs` | Permissive declared | Provenance audit required: their subject matter has a GPL-2.0-only reference implementation. A permissive declaration is not a clean-room certificate. |

**Explicit rejections required by the brief:** GPL-2.0-only, AGPL, SSPL, proprietary and unclear licences are not candidates. The unlicensed IRIX ports are the most painful case because they are technically the best prior art; they must be treated as **behavioural evidence**, not as source.

---

## 7. Top 10 priority shortlist

Ranking counts the open epics/gates where the candidate is on the critical path, and gives extra weight to a hard M1–M4 gate rather than a compatibility epic; ties are broken by whether the work is on the M1/M2 path. “Adopt” is a dependency, “fork” is vendored modification, “borrow” is a clean-room reimplementation of the design.

| Rank | Candidate | Licence | Unblocks | Verdict | First step |
|---:|---|---|---|---|---|
| 1 | **Tooling gate suite** — `cargo-deny`, `cargo-audit`, `cargo-cyclonedx`, `cargo-about`, `cargo-call-stack`, `cargo-bloat`, `proptest`, `cargo-fuzz`, `nextest` | MIT/Apache-2.0, Apache-2.0, NCSA | #12, #13, #15, #17, #11 | Adopt | Add `deny.toml` with a GPL-3.0-only allowlist, SBOM and stack/size jobs. |
| 2 | **`foca`** | MPL-2.0 | #3, #4, #5 (all three mesh gates) | Adopt | Wrap it over smoltcp UDP + a timer source; test 2/8/128-node churn. |
| 3 | **`x11rb` + `yserver`** | MIT OR Apache-2.0; MIT | #8, #7, #6, #11 | Adopt/fork `x11rb`; borrow `yserver` | Build an MSBFirst-only core server skeleton with `x11rb-protocol`; port XTS ideas. |
| 4 | **`uutils/coreutils`** | MIT | #6, #10, #11, #2 | Fork subset | Port the read-only core utilities against relibc; reuse their tests. |
| 5 | **`cosmic-text` + `harfrust` + `tiny-skia`** | MIT OR Apache-2.0; MIT; BSD-3-Clause | #7, #8, #194 | Adopt/fork | Build a `no_std`/32-bit/big-endian text-and-raster spike; render a golden frame. |
| 6 | **`unwinding` + clean-room `libc` IRIX module** | MIT OR Apache-2.0; MIT OR Apache-2.0 | #10, #11, #6 | Adopt + port | Add MIPS DWARF unwinding; add `irix7` `libc` module from POSIX facts. |
| 7 | **`lamxfs` + `xfs-core`** | MIT OR Apache-2.0; Apache-2.0 | #9, #6, #11 | Fork/port + reference | Read clean IRIX v4 images; add v1 dirs/ACLs; test dirty-log refusal. |
| 8 | **`object` + `xmas-elf` + `gdbstub`** | Apache/MIT; Apache/MIT; MIT/Apache | #6, #10, #2 | Adopt host tools; fork `gdbstub` | Wire `gdbstub` MIPS arch to the trap frame; use `object` for image inspection. |
| 9 | **`rmcp`** | Apache-2.0 | #2, #15, #13 | Adopt host/native cell | Build a conformance suite against the in-kernel MCP prototype. |
| 10 | **`iced` + `libcosmic`** | MIT; MPL-2.0 | #7, #194 | Fork (long-range) | Prototype an `iced_tiny_skia` + native-surface backend; keep `libcosmic` at arm’s length. |

**Next tier (not in the top 10):** `virtio-drivers` (#5), `bootloader`/`limine`/`uefi`/`acpi`/`x86_64` (#5), `gix` (#2/#9/#11, host/home cell), `fuser` (#9), `alacritty_terminal` (#7/#10), `fatfs` (#9), `sntpc`/`dhcproto`/`simple-dns` (#2 test oracles), `tock-registers`/`volatile` (#10/#16), `relibc` (already own), `smoltcp`/`embedded-tls`/`wasmi` (already adopted).

**Effort order (rough, not a commitment).** Tooling: days to two weeks. `foca`: two to four weeks including the smoltcp adapter and churn tests. `x11rb`/`yserver`: months for the server, plus a separate 20–30k-line Xlib/Xt/Motif ABI. `uutils`: two to four person-months for the read-only subset. Text/raster: a two-to-four-week big-endian spike, then two to three person-months of integration. `unwinding`/`libc`: two to four person-months, dominated by the MIPS unwinder and signal/TLS ABI. XFS: four to six person-months for read-only, plus three to five for dirty-log replay. ELF/GDB: two to six weeks. `rmcp`: one to two weeks for host conformance, later for a native cell. `iced`/`libcosmic`: two to four person-months for the backend prototype, six to twelve for a usable libcosmic fork.

**Why tooling is first.** It is the only candidate that touches five epics, it is host-side (no MIPS port), and it is a prerequisite for trusting every other dependency. The licence gate is also the mechanism that prevents the GPL-2.0-only and unlicensed-code hazards from entering the tree.

**Why `foca` is second.** It is the only mature `no_std` SWIM implementation found, and membership/failure detection is a hard gate from M2 onward. It is also small enough to audit under MPL-2.0.

---

## 8. Verified versus needs a spike

### 8.1 Verified on 2026-10-25

- Licences and repository identities for the candidates in §4, via crates.io and the GitHub API.
- `foca` 2.0.0: MPL-2.0, `no_std` + `alloc`, MSRV 1.88, active; SWIM+Inf.+Susp.
- `cosmic-text` 0.19.0: `MIT OR Apache-2.0`, explicit `no_std` feature, depends on `harfrust` 0.5 (not archived `rustybuzz`).
- `harfrust` 0.15.0: MIT, `no_std`/`libm`, active; `rustybuzz` is archived.
- `tiny-skia` 0.12.0: BSD-3-Clause, `no_std` + `no-std-float`, SIMD optional.
- `x11rb-protocol` 0.14.0: `std` optional, `no_std` build; `x11rb` 0.14.0 MIT/Apache.
- `gdbstub` 0.7.10: MIT/Apache, `no_std`, MIPS32/64 architecture modules present.
- `lamxfs` 0.1.3: MIT/Apache, `no_std`, clean XFS v4 and v5, refuses dirty v4 log, no IRIX-specific support found.
- `xfs-core` 0.1.6: Apache-2.0, std, read-only v4/v5, no log replay.
- `rust-fs-erofs` 0.5.0: MIT, full reader/writer claim, one star, new.
- `unwinding` 0.2.10: MIT/Apache; MIPS support is not in the pinned version.
- `object` 0.40.0: Apache/MIT; `xmas-elf` 0.10.0: Apache/MIT, `no_std`.
- `rmcp` 3.7.0: Apache-2.0; `cargo-deny`/`cargo-cyclonedx`/`cargo-call-stack`/`cargo-bloat`/`proptest`/`cargo-fuzz` licences.
- `rust-irixlibstd` and `rust-irix` have **no licence file**; `rustybuzz`, `bincode` and `pci-ids` are archived.
- The in-tree versions: `smoltcp` 0.13, `embedded-tls` 0.19, `wasmi` 2.0, `blake3` 1.8, `ed25519-dalek` 2.1.

### 8.2 Needs a spike

| Claim to test | Why it is not proven | Suggested test |
|---|---|---|
| `cosmic-text`/`harfrust`/`swash` build and shape correctly on 32-bit big-endian MIPS | `no_std` feature exists; endianness of font tables and shaping buffers unproven | Cross-build for `mips3-sgi-irix7-native-n32`; shape a fixed string; compare glyph IDs/positions with host. |
| `tiny-skia` rasterises correctly on big-endian MIPS | Pixel storage, `bytemuck` casts and PNG byte order | Render a golden 320×200 image; compare bytes under QEMU/iris. |
| `x11rb` can serve MSBFirst clients | It is a client library; server dispatch/window tree is new | Parse a recorded IRIX `Setup`/request stream; reply byte-for-byte. |
| `yserver` architecture can be reduced to Newport | DRM/KMS/Vulkan/little-endian assumptions | Read-only design review; extract XTS test list, not code. |
| `foca` over smoltcp at 128 nodes | `no_std` but no smoltcp adapter; gossip budget on 10 Mbit | 8-node iris/QEMU run; measure bytes/s and failure detection. |
| `lamxfs` reads IRIX v4 images | Supports clean v4 but not IRIX v1 dirs/ACLs/dirty log | Mount a clean IRIX 6.5 image; compare tree with `xfs_db`/IRIX. |
| `unwinding` MIPS | Missing from pinned crate; prior art unlicensed | Implement DWARF MIPS unwinder; `catch_unwind` + backtrace on iris. |
| `wasmi` `no_std` + MIPS | `std` used in `wasm-host`; `wat` needs std; interpreter performance | Build without `std`; run the `i7host` WAT module; measure fuel/s. |
| `uutils`/`brush` on relibc | Process/threads/signals/termios not present | Port `ls`/`cat`/`stat` first; then `cp`/`mv`; then shell. |
| `gix` on 32-bit | Memory/mmap/threads; large repo | Build for n32; clone a small repo; measure RSS. |
| `virtio-drivers` 32-bit | PCI/64-bit assumptions | QEMU x86_64 first; then a 32-bit MIPS PCI model if one exists. |
| `rustls` with a pure provider | Default providers are C/asm and lack MIPS | Build `rustls-rustcrypto`/`rustls-pure`; TLS 1.3 handshake on MIPS. |
| `libcosmic`/`iced` without winit | Defaults require winit, tokio, X11/Wayland; libcosmic’s vendored iced uses `tiny-skia`, but there is no native IRIX backend | Build `iced` with `tiny-skia` only; implement a null/native window backend; measure RSS. |
| `cargo-llvm-cov` on cross-MIPS | Coverage instrumentation may not support the target | Host coverage first; treat target coverage as optional. |

---

## 9. Largest licensing and portability risks

### 9.1 Licensing

1. **The best IRIX `std`/`libc`/unwinding prior art is unlicensed.** `danifunker/rust-irixlibstd` and `atomchild411/rust-irix` demonstrate exactly what the project needs, and neither has a licence file. Copying any of it would be an all-rights-reserved violation. This is the largest licensing risk because the temptation is highest and the technical value is greatest. Mitigation: clean-room reimplementation; ask the authors to relicense; record the decision.
2. **GPL-2.0-only contamination in storage.** Linux XFS, `xfsprogs`/`libxfs`, `btrfs-cli`/`btrfs-mkfs` and libgit2 are GPL-2.0-only. `git2` is a permissive wrapper around a GPL-2.0-only C library; the wrapper does not cure the combination. Mitigation: `gix` instead of `git2`; `lamxfs`/`xfs-core` with a provenance audit; never copy Linux code.
3. **Permissive-looking crates with unclear provenance.** `xfs-core`, `lamxfs`, `rust-fs-erofs` and `xfuse` declare permissive licences, but XFS/EROFS have large GPL-2.0-only reference implementations. A permissive declaration is not a clean-room certificate. Mitigation: provenance audit, independent format documents, differential tests, and a written clean-room record.
4. **MPL-2.0 file-level obligations.** `libcosmic`, `foca`, `ostd` and `symphonia` are compatible, but a fork must keep MPL files MPL and publish their source. A downstream `libcosmic` fork is not a pure GPL-3.0 fork. Mitigation: legal review, clear file ownership, `THIRD-PARTY-NOTICES.md`.
5. **EUPL-1.2 and CDLA-Permissive-2.0.** `eza` is EUPL-1.2 (compatible on the EUPL list but unfamiliar); `webpki-roots` is a data licence. Mitigation: prefer `lsd`/uutils `ls`; keep the CA bundle notice; avoid adding either to the kernel.
6. **Apache-2.0 `NOTICE` and patent terms.** Many recommended crates are Apache-2.0 (`rustls`, `embedded-tls`, `object`, `cargo-cyclonedx`, `rmcp`). The notices must survive, and the patent-termination clause is a real term. Mitigation: generate notices from `Cargo.lock` with `cargo-about` and gate them in CI.

### 9.2 Portability

1. **Big-endian is the default failure mode.** Most of the Rust ecosystem is developed and tested on little-endian x86-64/AArch64. The R4400 has no `wsbh`; byte swaps are shift/mask sequences. Any crate that uses `transmute`, `bytemuck` casts, packed pixel formats, network-order assumptions or `u32` pixel words must be tested on MIPS. Mitigation: explicit `from_be_bytes`/`to_le_bytes`, golden-byte tests, iris + real Indy CI.
2. **32-bit `usize` and memory.** `wasmtime`, `gix`, `libcosmic`, `iced`, `nushell`, `tokio` and `rustls` are comfortable with 64-bit address spaces; the Indy has 32–256 MiB. Mitigation: `cargo bloat`/`cargo-call-stack`, heap budgets per cell, `no_std` + alloc where possible, and a hard “does it fit in 32 MiB?” gate.
3. **`no_std` claims are not target proof.** `cosmic-text`, `harfrust`, `tiny-skia`, `foca`, `lamxfs`, `x11rb-protocol` and `gdbstub` advertise `no_std`, but the feature combinations, `libm` path and allocator assumptions may not have been tested on 32-bit big-endian. Mitigation: a build matrix for `mips3-sgi-irix7-n32` and `mips3-sgi-irix7-native-n32`; run the tests, not just `cargo check`.
4. **No threads, TLS or process table yet.** `tokio`, `parking_lot`, `rayon`, `crossbeam` scoped threads, `std::thread`, `gix` mmap/thread pools and `alacritty_terminal` all assume threads. The native bring-up is single-threaded. Mitigation: keep host tools host-side; port userland only after `pthread_create`/TLS/processes land.
5. **Linux APIs are everywhere.** `rustix`, `nix`, `libc`, `calloop`, `tokio`, `fuser`, `winit`, `smithay`, `uutils` and `brush` use epoll/eventfd/signalfd/inotify/statx/copy_file_range/splice or Linux `ioctl` numbers. The native libc deliberately does not provide epoll/eventfd/inotify/statx. Mitigation: port to POSIX `poll`/`read`/`write`/`fcntl`; reject crates that cannot be ported without a Linux compatibility layer.
6. **GPU/Wayland assumptions.** `iced`’s default features include `wgpu`/`winit`; `libcosmic` uses a vendored iced with `tiny-skia` but still defaults to winit/tokio/X11/Wayland; `smithay`, `wayland-rs`, `winit`, `pixels`, `vello` and `wgpu` are GPU/Wayland-shaped. The Indy has a 2D framebuffer and no GPU. Mitigation: `iced` with `tiny-skia`, a custom native display backend, and no `cosmic-comp`/Wayland on the Indy.
7. **C dependencies and cross-compilation.** `ring`, `aws-lc-rs`, `zstd`, `lz4`, `littlefs2`, `capstone`, `git2`, `mbedtls` and `bzip2` need C toolchains, CMake/Perl/NASM or platform assembly. Cross-compiling these for n32 big-endian MIPS is a project in itself. Mitigation: pure-Rust alternatives (`rustls-rustcrypto`, `ruzstd`, `lz4_flex`, `miniz_oxide`, `gix`); keep C tools host-side.
8. **MSRV and the GCC backend.** Many crates require Rust 1.85–1.95; the pinned nightly is 1.101, so the LLVM path is fine, but `rustc_codegen_gcc` may lag on edition 2024/new features. Mitigation: test the adopted set with both backends; keep a per-crate MSRV record.
9. **Maintenance and single-maintainer risk.** `rustybuzz`, `bincode` and `pci-ids` are archived; `scsi` is from 2019; `lamxfs`, `xfs-core`, `rust-fs-erofs` and `lamfold-erofs` are very new and low-star. Mitigation: avoid critical dependencies on new crates; keep a fork/patch queue; prefer crates with tests and an active maintainer.

---

## 10. Sources

### 10.1 Project documents

- `docs/plan/milestones.md`, `testing.md`, `release.md`, `security.md`, `hardware-lab.md`, `interfaces.md`, `risks.md`, `github.md`, `community.md`, `cost.md`, `legal.md`.
- `docs/spec/system.md`, `architecture.md`, `boot-protocol.md`, `elf.md`, `modules.md`, `packages.md`, `mcp.md`, `posix.md`, `native-abi.md`, `native-libc.md`, `driver-interface.md`, `erofs.md`, `userland-services.md`, `console.md`, `overlay.md`, `mcp-apps.md`.
- `docs/research/rust-fs-research.md`, `erofs-big-endian-research.md`, `rust-x11-research.md`, `native-window-system-research.md`, `rust-os-design-research.md`, `plan9-rust-research.md`, `rust-std-irix7.md`, `rust-std-build-spike.md`, `std-hello-bringup.md`, `relibc-n32-bringup.md`, `compressed-ram.md`, `indy-hardware.md`.
- `README.md`, `Cargo.toml`, `docs/map.md`, `docs/plan/interfaces.md`.

### 10.2 Candidate repositories and crates

Kernel and `no_std`:

- <https://github.com/asterinas/asterinas> (Ostd, MPL-2.0)
- <https://github.com/theseus-os/Theseus> (MIT)
- <https://github.com/redox-os/kernel> and <https://github.com/redox-os/redoxfs> (MIT)
- <https://github.com/rcore-os/rCore>, <https://github.com/rcore-os/zCore>, <https://github.com/rcore-os/arceos> (MIT/Apache-2.0)
- <https://github.com/oxidecomputer/hubris> (MPL-2.0)
- <https://github.com/tock/tock> (Apache-2.0 OR MIT)
- <https://crates.io/crates/linked_list_allocator>, <https://crates.io/crates/buddy_system_allocator>, <https://crates.io/crates/rlsf>, <https://crates.io/crates/talc>
- <https://crates.io/crates/spin>, <https://crates.io/crates/lock_api>, <https://crates.io/crates/critical-section>, <https://crates.io/crates/portable-atomic>, <https://crates.io/crates/heapless>, <https://crates.io/crates/hashbrown>
- <https://crates.io/crates/embassy-executor>, <https://crates.io/crates/embassy-time>, <https://crates.io/crates/embassy-sync>
- <https://crates.io/crates/tock-registers>, <https://crates.io/crates/volatile>, <https://crates.io/crates/embedded-hal>, <https://crates.io/crates/embedded-dma>

MIPS, ELF and debug:

- <https://crates.io/crates/object>, <https://crates.io/crates/xmas-elf>, <https://crates.io/crates/elf>, <https://crates.io/crates/goblin>, <https://crates.io/crates/gimli>, <https://crates.io/crates/addr2line>
- <https://crates.io/crates/zerocopy>, <https://crates.io/crates/bytemuck>, <https://crates.io/crates/endian-num>, <https://crates.io/crates/plain>, <https://crates.io/crates/byteorder>
- <https://github.com/KarpelesLab/rsemu> (`rsemu`, MIT; not a MIPS system model)
- <https://github.com/daniel5151/gdbstub> (MIPS arch under `gdbstub_arch/src/mips`)
- <https://github.com/Decompollaborate/rabbitizer> (MIT)
- <https://github.com/capstone-rust/capstone-rs> (MIT wrapper; Capstone BSD-3)
- <https://github.com/danifunker/rust-irixlibstd> (no licence)
- <https://github.com/atomchild411/rust-irix> (no licence)

Boot, crypto and networking:

- <https://crates.io/crates/lz4_flex>, <https://crates.io/crates/crc>, <https://crates.io/crates/crc32c>, <https://crates.io/crates/sha2>, <https://crates.io/crates/blake3>
- <https://crates.io/crates/ed25519-dalek>, <https://crates.io/crates/x25519-dalek>, <https://crates.io/crates/curve25519-dalek>
- <https://github.com/smoltcp-rs/smoltcp> (0BSD)
- <https://github.com/drogue-iot/embedded-tls> (Apache-2.0)
- <https://github.com/rustls/rustls>, <https://github.com/rustls/webpki>, <https://github.com/rustls/webpki-roots>, <https://github.com/rustls/rcgen>
- <https://github.com/bluecatengineering/dhcproto>, <https://github.com/balliegojr/simple-dns>, <https://github.com/nlnetlabs/domain>, <https://github.com/vpetrigo/sntpc>
- <https://github.com/quinn-rs/quinn>, <https://github.com/n0-computer/iroh>, <https://github.com/libp2p/rust-libp2p>, <https://github.com/aws/s2n-quic>
- <https://crates.io/crates/httparse>, <https://crates.io/crates/http>, <https://crates.io/crates/ureq>, <https://crates.io/crates/reqwest>
- Optional 9P: <https://github.com/sminez/ninep>, <https://github.com/google/rust-p9>, <https://github.com/rs9p/rs9p>, <https://github.com/pdfinn/styx9p>

Storage:

- <https://github.com/Dreamacro/erofs-rs> (MIT/Apache-2.0)
- <https://github.com/antimatter-studios/rust-fs-erofs> (MIT)
- <https://github.com/lamco-admin/lamfold> (`lamfold-erofs`, MIT/Apache-2.0)
- <https://github.com/lamco-admin/lamxfs> (MIT/Apache-2.0)
- <https://github.com/SecurityRonin/xfs-forensic> (`xfs-core`, Apache-2.0)
- <https://github.com/antimatter-studios/rust-fs-xfs> (MIT)
- <https://github.com/KhaledEmaraDev/xfuse> (BSD-2-Clause)
- <https://github.com/redox-os/redoxfs> (MIT)
- <https://github.com/rafalh/rust-fatfs> (MIT)
- <https://github.com/cberner/fuser> (MIT)
- <https://github.com/GitoxideLabs/gitoxide> (`gix`, MIT/Apache-2.0)
- <https://github.com/rust-lang/git2-rs> (wrapper; libgit2 GPL-2.0-only)
- <https://github.com/composefs/tar-rs>, <https://github.com/zip-rs/zip2>, <https://github.com/jcreekmore/cpio-rs>, <https://github.com/mdsteele/rust-ar>
- <https://github.com/KillingSpark/zstd-rs> (`ruzstd`, MIT), <https://github.com/Frommi/miniz_oxide>, <https://github.com/pseitz/lz4_flex>
- <https://crates.io/crates/littlefs2>, <https://crates.io/crates/ext4-view>, <https://crates.io/crates/flate2>, <https://crates.io/crates/zstd>, <https://crates.io/crates/crc32fast>

Drivers and media:

- <https://crates.io/crates/tock-registers>, <https://crates.io/crates/volatile>, <https://crates.io/crates/embedded-hal>, <https://crates.io/crates/embedded-dma>, <https://crates.io/crates/bitfield>
- <https://github.com/pepyaka/pcics> (`pcics`, MIT), <https://github.com/lienching/pci-ids.rs> (`pci-ids`, archived)
- <https://github.com/ischeinkman/scsi-rs> (`scsi`, Apache-2.0), <https://github.com/pdh11/cotton> (`cotton-scsi`, CC0-1.0), <https://github.com/maboroshinokiseki/scsir> (`scsir`, MIT/Apache-2.0)
- <https://github.com/pdeljanov/Symphonia> (MPL-2.0), <https://github.com/HEnquist/rubato> (MIT/Apache-2.0), <https://github.com/rustaudio/dasp> (MIT/Apache-2.0), <https://github.com/ruuda/hound> (Apache-2.0), <https://github.com/RustAudio/lewton> (MIT/Apache-2.0), <https://github.com/awxkee/yuvutils-rs> (`yuv`, BSD-3/Apache-2.0), <https://github.com/RustAudio/cpal> (Apache-2.0)

POSIX and libc:

- <https://github.com/irix7/relibc> (MIT fork)
- <https://github.com/rust-lang/libc> (MIT/Apache-2.0)
- <https://github.com/bytecodealliance/rustix> (Apache-2.0/MIT)
- <https://github.com/nix-rust/nix> (MIT)
- <https://github.com/nbdd0121/unwinding> (MIT/Apache-2.0)
- <https://github.com/rust-lang/rustc-demangle>, <https://github.com/RazrFalcon/memmap2-rs>, <https://github.com/rust-random/getrandom>

Mesh, actors, CAS and Wasm:

- <https://github.com/caio/foca> (MPL-2.0)
- <https://github.com/quickwit-oss/chitchat> (MIT)
- <https://github.com/databendlabs/openraft> (MIT/Apache-2.0)
- <https://github.com/wasmi-labs/wasmi> (MIT/Apache-2.0)
- <https://github.com/bytecodealliance/wasm-tools> (`wasmparser`, `wasm-encoder`, `wasmprinter`)
- <https://github.com/bytecodealliance/wasmtime> (Apache-2.0 WITH LLVM-exception)
- <https://github.com/jamesmunns/postcard>, <https://github.com/serde-rs/serde>, <https://github.com/rkyv/rkyv>, <https://github.com/enarx/ciborium>
- <https://github.com/BLAKE3-team/BLAKE3>, <https://github.com/dalek-cryptography/curve25519-dalek>

Desktop, display and X11:

- <https://github.com/psychon/x11rb> (MIT/Apache-2.0)
- <https://github.com/joske/yserver> (MIT)
- <https://github.com/systemrustd/x12> (MIT)
- <https://github.com/PerikiyoXD/rxserver> (Apache-2.0)
- <https://github.com/pop-os/cosmic-text> (MIT/Apache-2.0)
- <https://github.com/harfbuzz/harfrust> (MIT)
- <https://github.com/linebender/tiny-skia> (BSD-3-Clause)
- <https://github.com/iced-rs/iced> (MIT)
- <https://github.com/pop-os/libcosmic> (MPL-2.0)
- <https://github.com/dfrg/swash> (Apache-2.0/MIT)
- <https://github.com/RazrFalcon/fontdb> (MIT)
- <https://github.com/Smithay/smithay>, <https://github.com/smithay/wayland-rs>, <https://github.com/Smithay/calloop>, <https://github.com/rust-windowing/winit>, <https://github.com/rust-windowing/softbuffer>
- <https://github.com/linebender/vello>, <https://github.com/parasyte/pixels>, <https://github.com/linebender/resvg>, <https://github.com/linebender/kurbo>, <https://github.com/nical/lyon>
- <https://github.com/pop-os/cosmic-comp> (GPL-3.0; not for the Indy)

Userland:

- <https://github.com/uutils/coreutils> (MIT)
- <https://github.com/reubeno/brush> (MIT)
- <https://github.com/nushell/nushell> (MIT)
- <https://github.com/fish-shell/fish-shell> (GPL-2.0-only)
- <https://github.com/BurntSushi/ripgrep> (Unlicense/MIT)
- <https://github.com/sharkdp/fd>, <https://github.com/sharkdp/bat>
- <https://github.com/eza-community/eza> (EUPL-1.2)
- <https://github.com/alacritty/alacritty> (`alacritty_terminal`, Apache-2.0)
- <https://github.com/alacritty/vte> (Apache-2.0/MIT)

Tooling:

- <https://github.com/EmbarkStudios/cargo-deny> (MIT/Apache-2.0)
- <https://github.com/rustsec/rustsec> (`cargo-audit`, Apache/MIT)
- <https://github.com/CycloneDX/cyclonedx-rust-cargo> (Apache-2.0)
- <https://github.com/EmbarkStudios/cargo-about> (MIT/Apache-2.0)
- <https://github.com/nextest-rs/nextest> (Apache/MIT)
- <https://github.com/taiki-e/cargo-llvm-cov> (Apache/MIT)
- <https://github.com/japaric/cargo-call-stack> (MIT/Apache-2.0)
- <https://github.com/RazrFalcon/cargo-bloat> (MIT)
- <https://github.com/proptest-rs/proptest> (MIT/Apache-2.0)
- <https://github.com/rust-fuzz/cargo-fuzz> (MIT/Apache-2.0)
- <https://github.com/rust-fuzz/arbitrary> (MIT/Apache-2.0)
- <https://github.com/model-checking/kani> (MIT/Apache-2.0)
- <https://github.com/modelcontextprotocol/rust-sdk> (`rmcp`, Apache-2.0)
- <https://github.com/crate-ci/typos> (MIT/Apache-2.0)
- <https://github.com/lycheeverse/lychee> (Apache-2.0), <https://github.com/rust-lang/mdBook> (MPL-2.0)

PC/cloud:

- <https://github.com/rust-osdev/bootloader> (MIT/Apache-2.0)
- <https://github.com/robotman2412/limine-boot-rust> (MIT/Apache-2.0)
- <https://github.com/rust-osdev/uefi-rs> (MIT/Apache-2.0)
- <https://github.com/rust-osdev/acpi> (MIT/Apache-2.0)
- <https://github.com/rust-osdev/x86_64> (MIT/Apache-2.0)
- <https://github.com/rcore-os/virtio-drivers> (MIT)

### 10.3 Retrieval note

All GitHub and crates.io figures were read through the GitHub REST API and the crates.io API on **2026-10-25**. The repository `pushed_at` dates are not release dates; a low-star repository can still be active, and an active repository can have an unmaintained crate. Where the README and the crate metadata disagreed, both are reported.

---

*This report is research input, not an ADR. It does not change the architecture, the licence policy or any product code.*
