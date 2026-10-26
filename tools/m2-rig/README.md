# m2-rig — two independent IRIX 7 kernels

`m2-rig` starts **two independent `iris` processes**, each with its own state directory,
NVRAM, MAC, NAT subnet, CI socket, monitor and serial log. It waits for both kernels to
print `BOOT-READY`, proves a two-way HTTP exchange through a Rust host relay, connects the
two guest mesh listeners so the kernels exchange framed `MeshEnvelope` records, and drives
the M2-2 signed join until both nodes report the same membership epoch and snapshot. The
default `run` then restarts the member and requires it to rejoin at the same epoch.

- Runbook: [`../../docs/plan/m2-rig.md`](../../docs/plan/m2-rig.md)
- Decisions: [`../../docs/adr/README.md`](../../docs/adr/README.md)
  ([M2-ADR-0005](../../docs/adr/m2-0005-join-epoch.md) is the join/epoch design)
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
| `run` | Start both, wait for `BOOT-READY`, probe HTTP, mesh framing and join, restart the member and require the same epoch, stop, write evidence. |
| `up` | Start both and leave them running. |
| `status` | Show CI state and the serial boot/join markers. |
| `probe` | Probe HTTP, mesh framing and join against running nodes; no restart. |
| `down` | Stop both through their CI sockets. |
| `print-config` | Print the derived instance, port and NVRAM table. |
| `clean` | Remove the work directory (stop the nodes first). |

Options: `--work DIR`, `--repo DIR`, `--iris PATH`, `--iris-ci PATH`, `--rom PATH`,
`--module PATH`, `--base-port N`, `--timeout SECS`, `--fault MODE`, `--keep`,
`--verbose`.

`--fault` selects a negative-evidence mode:

| Mode | What changes | Expected result |
|---|---|---|
| `none` (default) | Full `mesh.*` configuration. | The join converges at epoch 2; the member restarts and rejoins at the same epoch. |
| `no-peer` | No `mesh.*` key at all. | No mesh listener and no join; the HTTP probe passes and the M1 `mesh=single` / `mesh=0/0/0` markers are preserved. |
| `wrong-authority` | n1 has a valid but wrong `mesh.pub`. | n0 signs; n1 logs `mesh-join: reject reason=bad-signature` and keeps its seed view. |

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
  little-endian length, and decodes the peer's records.
- A real signed join: the member sends a versioned `JoinRequest`; the root returns a full
  member list and epoch signed with Ed25519; the member installs it and acknowledges. Both
  nodes print `mesh-join: converged` with the same two member IDs, epoch and digest, and
  the rig recomputes that digest independently.
- A clean member restart: n1 stops, restarts from the seed epoch, and the root re-confirms
  the same epoch and snapshot; the rig checks the first and rejoin transcripts.
- Fail-closed alternatives: `no-peer` keeps the M1/M2-0 path, and `wrong-authority` proves
  the member rejects an unauthenticated epoch.

The kernel's `BOOT-READY` still says `mesh=single`: that is the M1 bootstrap view printed
before any module runs. The confirmed membership is the module's `mesh-join: converged`
line. The rig never uses `mesh-sim` as evidence.

## NVRAM keys

| Key | Meaning |
|---|---|
| `mcp.url`, `mcp.sha256` | Module fixture URL and digest. |
| `mesh.peer` | Peer mesh port through this node's gateway; enables the listener and join. |
| `mesh.authority` | The root's MAC-derived `NodeId`. |
| `mesh.epoch` | Seed epoch (`1`). |
| `mesh.pub` | Root's Ed25519 public key, on the member. |
| `mesh.key` | Root's Ed25519 seed, on the root only; a lab fixture, redacted in evidence. |
| `console` | `serial0`. |

The DS1386 environment has 298 bytes before the MAC slot, so the root stores only the
seed and derives its public key. `--fault no-peer` omits every `mesh.*` key.

## Tests

```sh
cargo test --manifest-path tools/m2-rig/Cargo.toml
cargo clippy --manifest-path tools/m2-rig/Cargo.toml --all-targets -- -D warnings
```

The unit tests cover SHA-256, the DS1386 environment layout and bound, the deterministic
authority key, instance-report parsing, the HTTP relay, the mesh relay's first-byte guard,
the fixture, the probe transcript parser, the join-line parser, the independent snapshot
digest, the join/restart oracle and JSON escaping. The codec/authority/state-machine tests
are in `crates/mesh-join`. The end-to-end test is the `run` command above; it needs iris
and the built artefacts.

## Layout

| File | Role |
|---|---|
| `src/main.rs` | Commands, probes, member restart, evidence wiring. |
| `src/rig.rs` | Instance derivation, NVRAM, process control, serial facts. |
| `src/join.rs` | Join/epoch oracle, independent snapshot digest, restart checks. |
| `src/relay.rs` | HTTP relay and the persistent mesh relay. |
| `src/evidence.rs` | `summary.md` and `run.json`. |
| `src/fixture.rs` | Loopback HTTP module fixture. |
| `src/nvram.rs` | DS1386 image layout. |
| `src/probe.rs` | Telnet/HTTP transcript probe. |
| `src/sha256.rs` | Dependency-free SHA-256. |
| `build.sh` | PROM and MCP module build. |
