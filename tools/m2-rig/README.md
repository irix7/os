# m2-rig — two independent IRIX 7 kernels

`m2-rig` is the M2-0 foundation harness. It starts **two independent `iris` processes**,
each with its own state directory, NVRAM, MAC, NAT subnet, CI socket, monitor and serial
log; waits for both kernels to print `BOOT-READY`; proves a two-way TCP exchange through a
Rust host relay; and writes a self-contained evidence bundle.

It does **not** implement join, membership, route cache, migration or capabilities. The
NVRAM keys `mesh.peer`, `mesh.authority` and `mesh.epoch` are the handoff to M2-1.

- Runbook: [`../../docs/plan/m2-rig.md`](../../docs/plan/m2-rig.md)
- Decisions: [`../../docs/adr/README.md`](../../docs/adr/README.md)
- Harness crate: `Cargo.toml`, `src/`
- Build helper: `build.sh`

## Quick start

```sh
source /path/to/toolchain/env.sh
tools/m2-rig/build.sh
cargo run --manifest-path tools/m2-rig/Cargo.toml -- run \
  --iris /path/to/iris --iris-ci /path/to/iris-ci
```

The default work directory is `<repo>/target/m2-rig`. The evidence bundle is written to
`<work>/evidence/`, with `summary.md` as the human-readable entry point.

## Commands

| Command | Purpose |
|---|---|
| `run` | Start both, wait for `BOOT-READY`, probe both directions, stop, write evidence. |
| `up` | Start both and leave them running. |
| `status` | Show CI state and the serial boot markers. |
| `probe` | Probe both directions against running nodes. |
| `down` | Stop both through their CI sockets. |
| `print-config` | Print the derived instance, port and NVRAM table. |
| `clean` | Remove the work directory (stop the nodes first). |

Options: `--work DIR`, `--repo DIR`, `--iris PATH`, `--iris-ci PATH`, `--rom PATH`,
`--module PATH`, `--base-port N`, `--timeout SECS`, `--keep`, `--verbose`.

## What it proves

- Two separate `iris` processes, not one kernel pretending to be two.
- Distinct MACs and 16-byte `NodeId`s, derived through `iris --print-instance` and checked
  against the guest's `mesh: single node` line.
- Distinct NAT subnets and guest addresses (`192.168.0.2` and `192.168.1.2` by default).
- `BOOT-READY ... network=up mcp=listening mesh=single` on both.
- A real two-way TCP exchange: each guest's kernel HTTP client fetches the peer's MCP
  status page and receives `HTTP 200` through its own NAT, the relay and the peer's port
  forward.

The two guests remain at `mesh=single`: that is the M1 bootstrap view, not a failed join.
M2-2 adds the join and the confirmed epoch.

## Tests

```sh
cargo test --manifest-path tools/m2-rig/Cargo.toml
cargo clippy --manifest-path tools/m2-rig/Cargo.toml --all-targets -- -D warnings
```

The unit tests cover SHA-256, the DS1386 environment layout, instance-report parsing, the
relay, the fixture, the probe transcript parser and JSON escaping. The end-to-end test is
the `run` command above; it needs iris and the built artefacts.

## Layout

| File | Role |
|---|---|
| `src/rig.rs` | Options, instance derivation, NVRAM, process control, readiness. |
| `src/relay.rs` | Transparent TCP relay and shared log. |
| `src/fixture.rs` | Local HTTP fixture that serves the MCP module. |
| `src/probe.rs` | Telnet-driven HTTP probe and transcript parser. |
| `src/evidence.rs` | Evidence bundle (`run.json`, `summary.md`, logs, transcripts). |
| `src/nvram.rs` | DS1386 image layout. |
| `src/sha256.rs` | Dependency-free SHA-256 for the PROM/module digests. |
| `build.sh` | Cross-builds the kernel, ROM and fresh MCP module. |

## Limitations

- Plain TCP over host loopback; no TLS. Do not present the rig as a secure deployment.
- The relay's probe mode closes after a 5 s quiet period; it is not a persistent mesh
  transport. M2-1 needs a mesh relay mode with a long idle timeout, or the direct forward
  once the early-data race is fixed.
- The relay is a host single point of failure. It is a laboratory seam, not a production
  path.
- The checked-in v0.1.3 MCP module is not used; the rig builds a fresh one. The release
  module loaded and printed `mcp: listening` but did not answer `GET /` here.
- Fixed host ports; use `--base-port` to move the block.
