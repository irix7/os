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

The toolchain lives in `irix7/project`. The short version:

```sh
# Nix is the supported environment
nix develop

# GitHub CLI, authenticated with project/repo/workflow scopes
gh auth status

# Build and test (once the workspace exists)
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
```

The emulator is `irix7/iris`; the rig scripts are in `irix7/project/scripts/rig/`.
The MCP development loop is described in [`docs/spec/mcp.md`](docs/spec/mcp.md).

## Clean-room rules

IRIX 7 is a clean-room project. The private `irix7/irix6` repository contains SGI-derived
source and decompilation.

- Do **not** copy SGI source, decompiled output or proprietary expression into this repo.
- Read the behavioural specification, not the private source, unless you are a clean-room
  reviewer.
- Every kernel PR is screened by `irix7/harness`; a similarity hit blocks the merge.
- If you are unsure, ask a clean-room reviewer before writing code.

See [`docs/plan/legal.md`](docs/plan/legal.md) and ADR-0009/0014/0015.

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

- Architecture: [`docs/spec/architecture.md`](docs/spec/architecture.md).
- Plan: [`docs/plan/`](docs/plan/).
- Research: [`docs/research/`](docs/research/).
- Every decision becomes an ADR in `irix7/project/docs/adr/`.

## Hardware testing

The lab is described in [`docs/plan/hardware-lab.md`](docs/plan/hardware-lab.md). Do not
modify a real Indy's NVRAM, disks or firmware without recording the original state.

## Security

Do not report vulnerabilities in public issues. See [`SECURITY.md`](SECURITY.md).

## Getting help

- GitHub Discussions: Q&A, hardware, research.
- Issues: tracked work.
- The monthly update in Discussions: what shipped and what is next.
