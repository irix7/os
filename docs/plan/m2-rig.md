# M2 two-node rig — runbook

**Status:** working; M2-1 transport probe added
**Scope:** M2-0 foundation plus the M2-1 transport slice. The rig starts two independent
kernels, proves both reach `BOOT-READY` with distinct identities, proves a two-way HTTP
exchange through the chosen path, and connects the two guest mesh listeners so the
kernels exchange framed `MeshEnvelope` records in both directions. It does not implement
join, membership, route cache, migration or capabilities.

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

`build.sh` defaults the kernel opt level to `z`, matching CI: the M1 remediation no
longer fits the 512 KiB PROM window at `opt-level=s`. `M2_RIG_OPT_LEVEL` can override it.
It writes:

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

`run` starts both nodes, waits for readiness, probes both directions, runs the mesh
framing probe, stops the nodes and writes `target/m2-rig/evidence/`.

The lifecycle commands:

| Command | What it does |
|---|---|
| `up` | Start both nodes, wait for readiness, leave them running. |
| `status` | Show `iris-ci` state and the boot markers for both nodes. |
| `probe` | Probe both directions (HTTP and mesh framing) against running nodes and write evidence. |
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

## Mesh transport path (M2-1)

The combined MCP+mesh module listens on guest port 80 for HTTP (module TCP slot 0) and
guest port 7000 for the mesh (module TCP slot 1) when NVRAM contains `mesh.peer`. The guest
has no TCP connect primitive, so **both nodes listen** and the host relay initiates both
sides:

```text
guest n0 (192.168.0.2):7000
  <- n0 forward 127.0.0.1:9702
  <- Rust mesh relay
  -> 127.0.0.1:9712 (n1 forward)
  -> guest n1 (192.168.1.2):7000
```

The relay is still a dumb byte pump. It waits until it has read first bytes from both
guests before writing in either direction, which avoids the early-data race in the iris
port forward. Its idle timeout is 120 s and the modules send a 5 s heartbeat, so it is
separate from the 5 s HTTP probe relay.

The modules exchange `Request/Ping` and `Response/Ping` on connect, then `Control/Ping`
heartbeats. The wire is the M2-ADR-0002 contract: a 32-bit little-endian envelope length
followed by exactly that many `MeshEnvelope` bytes, with a 64 KiB bound. Each envelope body
is the source's 16-byte `NodeId`; a node rejects a body that does not match the header.
The pass line names the peer `NodeId`, frame/byte counters and `rejects=0`.

This listen-only bootstrap is a documented M2-1 deviation from the lower-`NodeId`-initiates
rule; the guest cannot connect until a kernel/module connect service exists. See the
implementation note in M2-ADR-0002.

## NVRAM keys

The rig writes a DS1386 image per node. The keys are:

| Key | Meaning |
|---|---|
| `mcp.url` | The node's own gateway plus the fixture port: `http://192.168.<n>.1:<port>/mcp.module`. |
| `mcp.sha256` | SHA-256 of the fresh module. |
| `mesh.peer` | The peer's mesh host port, reached through this node's gateway: `192.168.0.1:9712` for `n0`, `192.168.1.1:9702` for `n1`. Its presence enables the guest mesh listener. M2-1 learns the peer `NodeId` from the first envelope; M2-2 will use the address for join. |
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

The M2-1 mesh probe adds these serial lines (the listening line can follow the `sash>`
prompt):

```text
mesh-transport: enabled
mesh-transport: listening on port 7000
mesh-transport: connected
mesh-transport: peer=<16-byte peer NodeId>
mesh-transport: pass peer=<peer NodeId> rx_frames=N tx_frames=N rx_bytes=N tx_bytes=N rejects=0 connects=N
```

The rig requires both nodes to log `listening` and `pass`, verifies that each pass line
names the other node's derived `NodeId`, and requires non-zero frame counts and
`rejects=0`.

The evidence bundle contains:

| File | Content |
|---|---|
| `summary.md` | Human-readable results, node table, boot lines, NVRAM, probes, relay logs. |
| `run.json` | Machine-readable version of the same. |
| `serial-n0.log`, `serial-n1.log` | Full serial transcripts. |
| `stdout-n0.log`, `stderr-n0.log`, etc. | Emulator output. |
| `probe-n0-to-n1.{txt,raw}`, `probe-n1-to-n0.{txt,raw}` | Telnet transcripts, clean and raw. |
| `relay-n0-to-n1.log`, `relay-n1-to-n0.log` | HTTP probe connection and byte counts. |
| `relay-mesh.log` | Mesh relay connection, first-byte, byte-count and reconnect log. |
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
  probe, not for a persistent mesh link. M2-1 adds a separate mesh relay with a 120 s
  idle timeout, first-byte guard and module-generated heartbeat.
- **The guest cannot connect.** Both modules listen and the host relay initiates. The
  lower-`NodeId`-initiates rule is not exercised until a connect service or fixed forward
  path exists.
- **The mesh listener owns module TCP slot 1.** When `mesh.peer` is present MCP uses only
  slot 0 for HTTP; without it the original two-slot MCP behaviour is unchanged. The
  single HTTP slot cannot pre-arm the next connection during the close handshake, so a
  back-to-back HTTP client can be refused in that gap. The M2-0 probe uses one request per
  direction and is unaffected. Runtime module dependencies (#217) are not available, so
  this is a fixed share, not a slot manager.
- **The relay is a host single point of failure.** It is a laboratory seam, not the
  production data path.
- **The checked-in v0.1.3 MCP module is not used.** It loads and prints `mcp: listening`
  but did not answer `GET /` here; a module built from the current tree does. The rig
  builds and serves the current module.
- **Fixed ports.** Another rig run can collide. Use `--base-port` and stop the previous
  run.
- **The guests still print `mesh=single`.** That is the M1 bootstrap view. M2-2 adds the
  join and the epoch; M2-0 deliberately does not fake a two-node membership view.

## M2-1 status and handoff

M2-1 is implemented in `crates/mesh-transport`, `modules/mcp` and `tools/m2-rig`:

- the codec/framing, 64 KiB bound, short-read/short-write handling, deadlines, heartbeat,
  reconnect backoff, counters and request-tag cancellation are host-tested;
- the combined MCP+mesh module owns module TCP slot 1 only when `mesh.peer` is present;
- the rig runs a two-node framing probe through the 120 s mesh relay and records the
  pass lines and peer `NodeId`s in the evidence bundle.

What remains for M2-2 and later: join and epoch confirmation, membership convergence,
homes, remote capabilities, actor migration and failure/partition behaviour. M2-1 does
not construct a `MembershipView`, change the `mesh: single node` marker, or implement a
remote route.

## Troubleshooting

- If `up` fails before `BOOT-READY`, read `target/m2-rig/n0/stdout` and
  `target/m2-rig/n0/iris-serial.log` (and `n1`). The harness prints the same tails on
  failure.
- If a port is busy, run `down` first, then check `ss -ltnp` for the block.
- If the fixture fetch fails, check `target/m2-rig/evidence/fixture.log` and the guest
  serial `autoload:` lines. The module digest in NVRAM must match the served file.
- If a probe times out, inspect the relay log: `relay down idle after N bytes` means the
  peer answered and the relay closed after the quiet period; `relay target ... failed`
  means the peer forward was not listening; `relay up closed after 0` means the guest did
  not send a request.
- If the mesh probe fails, inspect `evidence/relay-mesh.log` and the guest
  `mesh-transport:` lines. No `listening` line means `mesh.peer` was absent or the module
  did not tick; no `pass` line means the peer `NodeId`/frame exchange did not complete;
  `rejects=` above zero means a framing or header check failed and the link was reset.
