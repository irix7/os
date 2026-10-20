# Changelog

All notable changes to IRIX 7 are recorded here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the project uses
[Semantic Versioning](https://semver.org/).

## [Unreleased]

### Added

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
- `-Z build-std=core,alloc` cross-builds for `mips3-sgi-irix7-n32` and
  `mips2-sgi-irix7-o32` (compiles `core`/`alloc` from source).
- Nix devshell (`flake.nix`) pinning rustc, cargo, rustfmt, clippy, gcc and binutils.

### Changed

- README links the architecture spec and the plan.
- `docs/map.md` records the advanced decisions and the research index.
- CI cross-build job (`.github/workflows/ci.yml`) is now the M0 gate: `continue-on-error`
  removed, pinned nightly + `rust-src`, host `cc`, `rust-lld`, `-Zjson-target-spec`, and
  `Swatinem/rust-cache` (`m0-cross`).

### Fixed

- `kernel-test` `panic_handler` is now `#[cfg(not(test))]` so the host test harness (which
  links std) no longer conflicts with the freestanding `no_std` crate.

## [0.0.0] — 2026-10-20

- Initial repository state: toolchain targets, tracer-bullet scratch code, licence and
  project map.
