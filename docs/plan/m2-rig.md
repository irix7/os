# M2 two-node rig — runbook

**Status:** working
**Scope:** M2-0 foundation. The rig starts two independent kernels, proves both reach
`BOOT-READY` with distinct identities, and proves a two-way TCP exchange through the
chosen path. It does not implement join, membership, route cache, migration or
capabilities.

The decisions behind the rig are [M2-ADR-0001](../adr/m2-0001-two-node-rig.md),
[M2-ADR-0002](../adr/m2-0002-transport.md), [M2-ADR-0003](../adr/m2-0003-authority.md)
and [M2-ADR-0004](../adr/m2-0004-actor-runtime.md).

## Prerequisites

- The pinned Rust nightly with `rust-src` (see `rust-toolchain.toml`).
- An `iris` binary with instance support (`--instance`, `--print-instance`; the
  `0.1.7+irix7` tree has it).
- The matching `iris-ci` binary.
- The repository's `targets/mips3-sgi-irix7-n32.json` and the kernel/ROM/MCP sources.
- No root, no `CAP_NET_RAW` and no pcap feature are required.

The harness is a standalone Rust crate. It is deliberately **not** a workspace member, so
a plain `cargo build` at the repository root does not cross-build it.

## Build

```sh
source /path/to/toolchain/env.sh   # rustup nightly-2026-10-03 + rust-src
tools/m2-rig/build.sh
```

`build.sh` writes:

| Artefact | Purpose |
|---|---|
| `target/m2-rig/irix7-rom.bin` | PROM with the kernel embedded (must be ≤ 524288 bytes). |
| `target/m2-rig/mcp.module` | Fresh MCP module (must be ≤ 2 MiB). |
| `target/m2-rig/SHA256SUMS` | Digests of both. |

If the host `cc` wrapper is stale (a Nix store path that no longer exists), point `CC`
or `PATH` at a working compiler before running `build.sh`. The kernel/ROM target uses
`rust-lld`; only host build scripts need the C compiler.

The rig defaults to a PROM and module under `<work>`; `--rom` and `--module` override
them. It verifies their sizes and records their SHA-256 digests.

## Run

The short form:

```sh
cargo run --manifest-path tools/m2-rig/Cargo.toml -- run \
  --iris /path/to/iris --iris-ci /path/to/iris-ci
```

`run` starts both nodes, waits for readiness, probes both directions, stops the nodes and
writes `target/m2-rig/evidence/`.

The lifecycle commands:

| Command | What it does |
|---|---|
| `up` | Start both nodes, wait for readiness, leave them running. |
| `status` | Show `iris-ci` state and the boot markers for both nodes. |
| `probe` | Probe both directions against running nodes and write evidence. |
| `down` | Send `iris-ci quit` to both and wait for the processes. |
| `run` | `up` + `probe` + `down` + evidence. |
| `print-config` | Print the derived instance/port table and the NVRAM keys without starting. |
| `clean` | Remove the work directory. Stop the nodes first. |

Useful options: `--work DIR`, `--base-port N`, `--timeout SECS`, `--keep` (with `run`,
leave the nodes running), `--verbose`.

`--base-port` moves the whole block:

| Node | Monitor | Serial A/B | Telnet | MCP HTTP | Mesh |
|---|---|---|---|---|---|
| `n0` | `base` | `base+1/+2` | `base+600` | `base+601` | `base+602` |
| `n1` | `base+100` | `base+101/+102` | `base+610` | `base+611` | `base+612` |

The default base is `9100`.

## Network path

```text
guest n0 (192.168.0.2)
  -> n0 NAT gateway 192.168.0.1:<relay>
  -> host 127.0.0.1:<relay>          (Rust byte relay)
  -> host 127.0.0.1:9711             (n1's port forward)
  -> guest n1 (192.168.1.2):80       (MCP status page)

guest n1 (192.168.1.2)
  -> n1 NAT gateway 192.168.1.1:<relay>
  -> host 127.0.0.1:<relay>          (Rust byte relay)
  -> host 127.0.0.1:9701             (n0's port forward)
  -> guest n0 (192.168.0.2):80       (MCP status page)
```

The relay is a transparent byte pump. It is not a directory, membership service or
protocol translator. The direct NAT-to-forward path (without the relay) resets or loses
data that arrives before the peer forward's guest-side connection is ready; see
M2-ADR-0001.

There is no broadcast, multicast or ARP between the two guests. Each guest can reach only
its own NAT gateway; the relay and the peer's forward are the seam.

## NVRAM keys

The rig writes a DS1386 image per node. The keys are:

| Key | Meaning |
|---|---|
| `mcp.url` | The node's own gateway plus the fixture port: `http://192.168.<n>.1:<port>/mcp.module`. |
| `mcp.sha256` | SHA-256 of the fresh module. |
| `mesh.peer` | The peer's mesh host port, reached through this node's gateway: `192.168.0.1:9712` for `n0`, `192.168.1.1:9702` for `n1`. |
| `mesh.authority` | Node 0's MAC; the 1-of-1 authority root. |
| `mesh.epoch` | Initial confirmed epoch, `1`. |
| `console` | `serial0`. |

The image is 8 KiB; the environment starts at byte `0x10`, the MAC is at byte `314`, and
the environment must end before the MAC. The harness checks the bound and records the
parsed environment in the evidence bundle.

## Readiness and evidence

`BOOT-READY v=1 ... network=up mcp=listening mesh=single` is the readiness marker. The rig
also requires:

- the `network: mac` line to match the derived MAC;
- the `mesh: single node` line to match the derived 16-byte NodeId;
- the DHCP line to contain the derived guest address;
- `mcp: listening on port 80`.

The rig waits 10 s after both `BOOT-READY` lines before it opens the first host
connection. The module arms its first listener on a later idle tick, and the emulated
MIPS CPU is slow enough that a shorter settle made the probe flaky. This is a rig
settle, not a kernel readiness contract.

The evidence bundle contains:

| File | Content |
|---|---|
| `summary.md` | Human-readable results, node table, boot lines, NVRAM, probes, relay logs. |
| `run.json` | Machine-readable version of the same. |
| `serial-n0.log`, `serial-n1.log` | Full serial transcripts. |
| `stdout-n0.log`, `stderr-n0.log`, etc. | Emulator output. |
| `probe-n0-to-n1.{txt,raw}`, `probe-n1-to-n0.{txt,raw}` | Telnet transcripts, clean and raw. |
| `relay-n0-to-n1.log`, `relay-n1-to-n0.log` | Connection and byte counts. |
| `fixture.log` | Module fetches by both guests. |

A successful probe line is:

```text
sash> http 192.168.0.1:<relay>/
http: fetching
sash> http: 200 38 bytes
sha256: ...
```

The 38-byte body is the MCP status page (`IRIX 7 module HTTP server\nuptime: N s\n`). The
digest changes with the uptime; the status and byte count are the pass criteria.

## Known limitations

- **Plain TCP, no TLS.** The mesh transport decision defers TLS; the rig runs on host
  loopback. Do not present it as a secure multi-node deployment.
- **The probe relay has a 5 s quiet-period close.** It is suitable for the MCP HTTP/1.0
  probe, not for a persistent mesh link. M2-1 needs a mesh relay mode with a long idle
  timeout and a heartbeat, or the direct forward once the early-data race is fixed.
- **The relay is a host single point of failure.** It is a laboratory seam, not the
  production data path.
- **The checked-in v0.1.3 MCP module is not used.** It loads and prints `mcp: listening`
  but did not answer `GET /` here; a module built from the current tree does. The rig
  builds and serves the current module.
- **Fixed ports.** Another rig run can collide. Use `--base-port` and stop the previous
  run.
- **The guests still print `mesh=single`.** That is the M1 bootstrap view. M2-2 adds the
  join and the epoch; M2-0 deliberately does not fake a two-node membership view.

## M2-1 handoff

M2-1 can start against the running rig:

1. Read `mesh.peer`, `mesh.authority` and `mesh.epoch` from NVRAM (the kernel's
   `nvram::get` is the accessor).
2. Add a TCP listener on guest port 7000 and a connect path to `mesh.peer`. The host
   forwards for port 7000 already exist (`base+602` and `base+612`).
3. Frame `MeshEnvelope` with a 4-byte little-endian length prefix. Enforce a 64 KiB
   record bound, the per-record deadline and the send deadline. Do not rely on connection
   close to delimit a record.
4. Use the lower `NodeId` as the initiator (byte-wise compare) and the higher as the
   listener. Reconnect with exponential backoff.
5. Add counters for frames, bytes, connects, disconnects, deadlines and cancelled tags;
   the M2 gate wants observable mesh traffic, not a silent path.
6. Extend the rig with a mesh relay mode (long idle timeout) or use the direct forward and
   document the early-data result. Keep the 5 s probe relay separate.
7. Do not implement join, epoch confirmation, homes, capabilities or migration in M2-1;
   those are M2-2 through M2-5.

## Troubleshooting

- If `up` fails before `BOOT-READY`, read `target/m2-rig/n0/stdout` and
  `target/m2-rig/n0/serial.log` (and `n1`). The harness prints the same tails on failure.
- If a port is busy, run `down` first, then check `ss -ltnp` for the block.
- If the fixture fetch fails, check `target/m2-rig/evidence/fixture.log` and the guest
  serial `autoload:` lines. The module digest in NVRAM must match the served file.
- If a probe times out, inspect the relay log: `relay down idle after N bytes` means the
  peer answered and the relay closed after the quiet period; `relay target ... failed`
  means the peer forward was not listening; `relay up closed after 0` means the guest did
  not send a request.
