# Changelog

All notable changes to IRIX 7 are recorded here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the project uses
[Semantic Versioning](https://semver.org/).

## [Unreleased]

### Added

- Raw PROM netboot path: an n32/MIPS III ROM executes directly from `0xBFC00000`, sizes
  and tests DRAM stacklessly, brings up the Seeq 8003 over HPC3 DMA, obtains DHCP and NTP,
  fetches a kernel over HTTP, verifies SHA-256 and jumps to the hand-off address.
- One canonical `targets/mips3-sgi-irix7-n32.json` target for both the ROM and the kernel.
- `rom/linker.ld` and the `rom/src/boot/` reset, memory-sizing and memory-test assembly.
- Research note on Meta CRAM and compressed RAM (`docs/research/compressed-ram.md`);
  compressed RAM is rejected for the Indy in favour of the read-only EROFS root.
- Advanced architecture specification (`docs/spec/architecture.md`), including the D1–D31
  decision register, hybrid SAS/MMU isolation, the native capability ABI, the IRIX
  compatibility container, R2-first boot, EROFS root, X11, the native display server and the
  in-OS MCP control plane.
- Project plan (`docs/plan/`): milestones, GitHub operating model, risk register, interface
  index, hardware lab, testing, security, release, legal, community and cost.
- Interface specifications: boot protocol, MCP control plane, driver `ctl`/`data`/`status`.
- Research reports (`docs/research/`): Indy hardware, Rust X11, native window systems, Rust
  filesystems, Plan 9 in Rust, big-endian EROFS and the broader Rust OS design survey.
- GitHub configuration: issue forms, PR template, CODEOWNERS, Dependabot, release notes,
  labeler and workflows for CI, security, licence, docs, release, project automation, stale
  items, hardware-in-the-loop and nightly builds.
- Community and governance files: contributing, code of conduct, security policy, support,
  governance and maintainers.
- Root Cargo workspace (`Cargo.toml`, `Cargo.lock`) so CI runs the Rust jobs; `kernel-test`
  is the freestanding `no_std` tracer bullet.
- `-Z build-std=core,alloc` cross-builds for the n32 `mips3-sgi-irix7-n32` target
  (compiles `core`/`alloc` from source).
- Nix devshell (`flake.nix`) pinning rustc, cargo, rustfmt, clippy, gcc and binutils.

### Changed

- Consolidated the ROM and kernel on n32/MIPS III and removed the misnamed
  `mips2-sgi-irix7-o32` target and its duplicate spec.
- The ROM now links text at the PROM window (`0xBFC00000`) and writable data at 7 MiB
  (`0xA8700000`), with an 8 MiB stack top, matching the original IP22 PROM layout.
- CI and release scripts build the kernel and ROM from the same target; the ROM build
  passes `rom/linker.ld`.
- README links the architecture spec and the plan.
- `docs/map.md` records the advanced decisions and the research index.
- CI cross-build job (`.github/workflows/ci.yml`) is now the M0 gate: `continue-on-error`
  removed, pinned nightly + `rust-src`, host `cc`, `rust-lld`, `-Zjson-target-spec`, and
  `Swatinem/rust-cache` (`m0-cross`).

### Fixed

- Seeq 8003 receive/transmit over HPC3 PDMA, including descriptor self-references that
  must be taken after the device is in its final location.
- DHCP, NTP and HTTP now share one `Interface`, `SocketSet` and `SeeqDevice`; NTP queries
  the DHCP router instead of the guest's own address.
- HTTP response parsing across packet boundaries and connection-close handling.
- The fetched kernel image is copied to the hand-off address before jumping.
- `kernel-test` `panic_handler` is now `#[cfg(not(test))]` so the host test harness (which
  links std) no longer conflicts with the freestanding `no_std` crate.

## [0.0.0] — 2026-10-20

- Initial repository state: toolchain targets, tracer-bullet scratch code, licence and
  project map.
