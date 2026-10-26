# m2-rig — two independent IRIX 7 kernels

`m2-rig` is the M2-0 foundation harness with the M2-1 transport probe. It starts **two
independent `iris` processes**, each with its own state directory, NVRAM, MAC, NAT subnet,
CI socket, monitor and serial log; waits for both kernels to print `BOOT-READY`; proves a
two-way HTTP exchange through a Rust host relay; connects the two guest mesh listeners so
the kernels exchange framed `MeshEnvelope` records in both directions; and writes a
self-contained evidence bundle.

It does **not** implement join, membership, route cache, migration or capabilities. The
NVRAM keys `mesh.peer`, `mesh.authority` and `mesh.epoch` remain the handoff to M2-2.

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
| `run` | Start both, wait for `BOOT-READY`, probe HTTP and mesh framing both ways, stop, write evidence. |
| `up` | Start both and leave them running. |
| `status` | Show CI state and the serial boot markers. |
| `probe` | Probe HTTP and mesh framing both ways against running nodes. |
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
- A real two-node framing exchange: each guest's mesh listener accepts a host-relay
  connection on guest port 7000, encodes `MeshEnvelope` records with a 4-byte
  little-endian length, and decodes the peer's records. Each pass line names the other
  node's derived `NodeId`, with non-zero frame counts and `rejects=0`.

The two guests remain at `mesh=single`: that is the M1 bootstrap view, not a failed join.
M2-2 adds the join and the confirmed epoch. The guest has no TCP connect primitive, so
both nodes listen and the host mesh relay initiates both sides; this is the documented
M2-1 deviation from lower-`NodeId`-initiates.

## Tests

```sh
cargo test --manifest-path tools/m2-rig/Cargo.toml
cargo clippy --manifest-path tools/m2-rig/Cargo.toml --all-targets -- -D warnings
```

The unit tests cover SHA-256, the DS1386 environment layout, instance-report parsing, the
HTTP relay, the mesh relay's first-byte guard, the fixture, the probe transcript parser and
JSON escaping. The codec/send-queue/deadline/tag tests are in
`crates/mesh-transport`. The end-to-end test is the `run` command above; it needs iris and
the built artefacts.

## Layout

| File | Role |
|---|---|
| `src/rig.rs` | Options, instance derivation, NVRAM, process control, readiness. |
| `src/relay.rs` | Transparent HTTP relay and the persistent mesh relay. |
| `src/fixture.rs` | Local HTTP fixture that serves the MCP module. |
| `src/probe.rs` | Telnet-driven HTTP probe and transcript parser. |
| `src/evidence.rs` | Evidence bundle (`run.json`, `summary.md`, logs, transcripts). |
| `src/nvram.rs` | DS1386 image layout. |
| `src/sha256.rs` | Dependency-free SHA-256 for the PROM/module digests. |
| `build.sh` | Cross-builds the kernel, ROM and fresh MCP module. |

## Limitations

- Plain TCP over host loopback; no TLS. Do not present the rig as a secure deployment.
- The HTTP relay's probe mode closes after a 5 s quiet period; the separate mesh relay
  has a 120 s idle timeout, a first-byte guard and a module-generated 5 s heartbeat.
- The guest has no connect service: both nodes listen and the host relay initiates. The
  lower-`NodeId`-initiates rule is not exercised until a connect service or a fixed
  forward path exists.
- The mesh listener owns module TCP slot 1 when `mesh.peer` is set; MCP uses slot 0 only.
  Without `mesh.peer`, MCP keeps its original two-slot behaviour. With one HTTP slot, a
  back-to-back HTTP client can be refused during the close-handshake re-arm gap; the M2-0
  probe's one request per direction is unaffected.
- The relay is a host single point of failure. It is a laboratory seam, not a production
  path.
- The checked-in v0.1.3 MCP module is not used; the rig builds a fresh one. The release
  module loaded and printed `mcp: listening` but did not answer `GET /` here.
- Fixed host ports; use `--base-port` to move the block.
