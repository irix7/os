# Contributing to IRIX 7

Thank you for helping build a clean-room, Rust-native successor to IRIX 6.5.

## Code of conduct

By participating, you agree to the [Code of Conduct](CODE_OF_CONDUCT.md).

## Ways to contribute

- Fix a bug or implement a feature from the [Project](https://github.com/orgs/irix7/projects).
- Write or improve documentation, specs, ADRs or tests.
- Test on a real Indy and report the result.
- Fuzz a parser, profile a subsystem, or improve the emulator.
- Triage issues and answer questions in Discussions.

Good first issues are labelled `good first issue`. If you are unsure where to start, open a
Discussion or comment on an issue.

## Development setup

The toolchain is pinned by `rust-toolchain.toml` and supplied by the Nix devshell;
[`docs/installation/toolchain.md`](docs/installation/toolchain.md) has the full setup and cross-build commands.
The short version:

```sh
# Nix is the supported environment
nix develop

# GitHub CLI, authenticated with project/repo/workflow scopes
gh auth status

# Build and test
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test
```

The emulator is `irix7/iris`, driven by `iris` and `iris-ci`; the two-kernel rig is
[`tools/m2-rig`](tools/m2-rig/README.md) and the one-prompt M2 transcript is
[`tools/one-computer`](tools/one-computer/README.md). The MCP development loop is
described in [`docs/using/mcp.md`](docs/using/mcp.md).

## Pre-release compatibility

IRIX 7 has no stable base or deployed users. Internal interfaces are Version 1 and may be
replaced without compatibility shims, dual-stack paths or migration code. Update every
in-tree caller and delete the old path; keep only safety/security invariants, explicit
external standards and M1/M2/M3 marker evidence. See
[`docs/administration/pre-release.md`](docs/administration/pre-release.md) before adding any compatibility or
version-negotiation code.

## Clean-room rules

IRIX 7 is a clean-room project. The private `irix7/irix6` repository contains SGI-derived
source and decompilation.

- Do **not** copy SGI source, decompiled output or proprietary expression into this repo.
- Read the behavioural specification, not the private source, unless you are a clean-room
  reviewer.
- Every kernel PR is screened by `irix7/harness`; a similarity hit blocks the merge.
- If you are unsure, ask a clean-room reviewer before writing code.

See [`docs/project/legal.md`](docs/project/legal.md) and ADR-0009/0014/0015.

## Developer Certificate of Origin

Every commit must be signed off:

```sh
git commit -s
```

This certifies that you have the right to contribute the code under GPL-3.0 (DCO 1.1). There
is no CLA.

## Pull requests

1. Open or pick an issue.
2. Branch from `main`; keep the branch short-lived.
3. Write tests and docs with the code.
4. Use conventional commit prefixes (`feat:`, `fix:`, `docs:`, `spec:`, `adr:`, `ci:`).
5. Fill in the PR template.
6. Ensure CI is green.
7. Request review from the area owner.

Review expectations:

- docs/typos: 1 approval;
- feature/bug in one area: 1 area reviewer;
- kernel, `unsafe`, capability or security: 2 approvals, one maintainer;
- clean-room-sensitive: 1 clean-room reviewer.

## Style

- Rust: `rustfmt`, Clippy with `-D warnings`, no `unsafe` outside the framework/drivers
  without a written invariant.
- Prose: **Australian English** (behaviour, colour, centre, organisation, optimise,
  prioritise, licence, catalogue, modelling).
- Markdown: `.markdownlint.json`; wrap prose at ~100 columns.
- Commit messages: imperative mood, conventional prefix.

## Tests

- Unit tests live beside the code.
- Integration and conformance tests live in `test/`.
- A bug fix includes a regression test.
- A new interface includes a conformance test.
- Fuzz targets live in `fuzz/`.

## Documentation

- Architecture: [`docs/internals/architecture.md`](docs/internals/architecture.md).
- Plan: [`docs/project/`](docs/project/).
- Research: [`docs/research/`](docs/research/).
- Every decision becomes an ADR in `irix7/project/docs/adr/`.

## Hardware testing

The lab is described in [`docs/project/hardware-lab.md`](docs/project/hardware-lab.md). Do not
modify a real Indy's NVRAM, disks or firmware without recording the original state.

## Security

Do not report vulnerabilities in public issues. See [`SECURITY.md`](SECURITY.md).

## Getting help

- GitHub Discussions: Q&A, hardware, research.
- Issues: tracked work.
- The monthly update in Discussions: what shipped and what is next.
