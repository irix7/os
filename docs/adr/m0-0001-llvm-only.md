# M0-ADR-0001 — LLVM is the only Rust code generation backend

**Status:** accepted
**Date:** 2026-11-03
**Deciders:** M0 toolchain audit (#1), for review at the M0 gate
**Related:** ADR-0018 (GCC backend primary, LLVM parallel) and ADR-0019 (IRIX fork on
`rust-lang/gcc`), both in [`irix7/project/docs/adr`](https://github.com/irix7/project/tree/main/docs/adr);
ADR-0010 (Rust monolith, n32/MIPS III),
[`../toolchain.md`](../toolchain.md), [`../spec/architecture.md`](../spec/architecture.md) §3.2

## Context

The project ADR series records ADR-0018 as "GCC backend primary, LLVM parallel". That
decision assumed `rustc_codegen_gcc` would be built from the `irix7/gcc` fork with
libgccjit and would carry the kernel and userland, with LLVM as a cross-check.

Nothing in this repository uses it. The kernel, ROM, loadable modules, the native `std`
spike, the x86_64 kernel and the PowerBook target all build with rustc's LLVM backend and
`-Z build-std` against JSON target specs. `docs/toolchain.md` already said the GCC backend
was unavailable while `docs/spec/architecture.md` still called it primary, so the two
documents contradicted each other. The audit for #1 recorded the GCC backend as never
delivered: there is no `rustc_codegen_gcc`, libgccjit or dual-backend build path in the
tree, and the fork build was blocked on host disk space. CI proves the LLVM path on every
pull request, including the M0 cross-build and the iris boot.

Two facts removed the original reason for a GCC backend:

1. LLVM supports the native ABI the project actually ships. The kernel and ROM are n32,
   big-endian, MIPS III, soft-float and `noabicalls`; `targets/mips3-sgi-irix7-n32.json`
   expresses that and the resulting ELF is `EF_MIPS_ABI2` with `.mdebug.abiN32`. No
   target-spec field required GCC.
2. o32 is a **loader and runtime** problem, not a Rust codegen problem. The
   compatibility container runs existing IRIX 6.5 binaries; it does not ask rustc to
   emit o32. `tools/build-o32-hello.sh` is a personality probe that relabels an n32
   object, not a second compiler target.

The non-Rust GCC in §3.1 of the architecture spec keeps its role: `irix7/gcc` builds the
IRIX 6.5 system toolchain for the compatibility container, and the host `cc` builds
`compiler-builtins`. Neither is a Rust code generator.

## Decision

1. **rustc's LLVM backend is the only code generation backend for IRIX 7 Rust code.**
   Kernel, ROM, modules, userland, native `std` and the x86_64/PPC shims all use it.

2. **One toolchain pin.** `rust-toolchain.toml` names `nightly-2026-10-03` with
   `rust-src`, `rustfmt`, `clippy` and `llvm-tools-preview`. The Nix `#default` shell
   reads that file through `rust-overlay`, so `nix develop` supplies exactly the compiler
   CI proves. A rustup user gets the same channel from the same file.

3. **ADR-0018 is amended by this record.** In this repository the "GCC primary, LLVM
   parallel" ordering is withdrawn; LLVM is primary and sole. The project ADR series at
   `irix7/project/docs/adr/` should mark ADR-0018 superseded or amended to match. ADR-0019
   remains the IRIX GCC fork for the compatibility container, outside the Rust build.

4. **The M0 gate is the LLVM build.** The gate command cross-builds the kernel with
   `-Z build-std` and the n32 target spec, then links the ROM and boots the PROM image
   under iris. It does not require libgccjit, a GCC-backend rustc or a second sysroot.

## Consequences

- One codegen path, one set of codegen bugs, one target-spec format. A codegen defect is
  fixed in LLVM, worked around in the target spec, or blocked; it cannot be routed around
  by switching backends.
- The devshell is reproducible from the repository: pinned nightly plus `rust-src`, with
  no multi-gigabyte libgccjit build and no second compiler to keep in step with rustc.
- The documentation has one answer. `docs/spec/architecture.md` §3.2, §3.5,
  `docs/toolchain.md` and the install guide no longer describe a GCC-primary build.
- The compatibility container's GCC work is unaffected and remains a separate track.
- If a future requirement needs Rust code generated through GCC — for example a backend
  bug that blocks n32 or a supported ABI LLVM cannot express — this decision must be
  reopened with that concrete defect as evidence, not restored as a parallel path.

## Alternatives considered

| Alternative | Why rejected |
|---|---|
| Keep GCC primary, LLVM parallel (ADR-0018 as written) | No delivered backend, no build path and no evidence; the tree contradicts it on every build. Keeping the claim makes the spec false. |
| Keep both as supported backends | Doubles the toolchain, test matrix and codegen bug surface for a backend with no working build. Pre-release policy has no users to keep compatible. |
| GCC for the compatibility container and LLVM for Rust | This is the decision: GCC stays as the IRIX system toolchain for container binaries, not as a Rust codegen backend. |
| Add an o32 `-Z build-std` target | o32 belongs to the loader and compatibility container, which execute existing binaries. A second Rust target would not make IRIX 6.5 binaries run. |

## Evidence

- `flake.nix` default shell supplies `nightly-2026-10-03` with `rust-src` through
  `rust-overlay`; `rust-toolchain.toml` is the single pin.
- 2026-11-03 M0 run in that shell: kernel release build, ROM build and a
  `rom/tools/make_prom.py` image of 501,964 bytes, booted under iris
  `v0.1.13+irix7-1-ga054d50` to `BOOT-READY` (see the pull request for #1/#43).
- `.github/workflows/ci.yml` `Cross-build (M0 gate)` and `Boot (iris)` use the same
  pinned nightly and LLVM target spec.
- No `rustc_codegen_gcc`, libgccjit dependency or GCC-backend rustc exists in the
  workspace, the Nix shell or the CI workflow.
