# M2 two-node rig — runbook

**Status:** working; M2-1 transport and the M2-2 join/epoch slice
**Scope:** M2-0 foundation, the M2-1 framed mesh transport and the M2-2 join. The rig
starts two independent kernels, proves both reach `BOOT-READY` with distinct identities,
proves a two-way HTTP exchange through the chosen path, connects the two guest mesh
listeners so the kernels exchange framed `MeshEnvelope` records, and drives the signed
join handshake until both nodes report the same membership epoch and snapshot. It then
stops and restarts the member and requires it to rejoin at the **same** epoch. It does not
implement homes, remote capabilities, migration, leave/removal or failure detection.

The decisions behind the rig are [M2-ADR-0001](../adr/m2-0001-two-node-rig.md),
[M2-ADR-0002](../adr/m2-0002-transport.md), [M2-ADR-0003](../adr/m2-0003-authority.md),
[M2-ADR-0004](../adr/m2-0004-actor-runtime.md) and
[M2-ADR-0005](../adr/m2-0005-join-epoch.md).

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

`build.sh` defaults the kernel opt level to `z`, matching CI. `M2_RIG_OPT_LEVEL` can
override it. It writes:

| Artefact | Purpose |
|---|---|
| `target/m2-rig/irix7-rom.bin` | PROM with the kernel embedded (must be ≤ 524288 bytes). |
| `target/m2-rig/mcp.module` | Fresh MCP+mesh module (must be ≤ 2 MiB). |
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

`run` starts both nodes, waits for readiness, probes both HTTP directions, runs the mesh
framing probe, waits for the join and the member's acknowledgement, stops and restarts
**only n1**, waits for it to rejoin at the same epoch, stops both nodes and writes
`target/m2-rig/evidence/`.

The lifecycle commands:

| Command | What it does |
|---|---|
| `up` | Start both nodes, wait for readiness, leave them running. |
| `status` | Show `iris-ci` state and the boot/join markers for both nodes. |
| `probe` | Probe both directions (HTTP, mesh framing, join) against running nodes and write evidence. It does not restart a node. |
| `down` | Send `iris-ci quit` to both and wait for the processes. |
| `run` | `up` + probe + member restart/rejoin + `down` + evidence. |
| `print-config` | Print the derived instance/port table and the NVRAM keys without starting. |
| `clean` | Remove the work directory. Stop the nodes first. |

Useful options: `--work DIR`, `--base-port N`, `--timeout SECS`, `--keep` (with `run`,
leave the nodes running), `--verbose`, and `--fault none|no-peer|wrong-authority`.

### Fault modes

The fault modes are negative evidence, not extra features:

| Mode | Setup | Expected result |
|---|---|---|
| `none` | Full two-node configuration. | Join converges; n1 rejoins at the same epoch after restart. |
| `no-peer` | No `mesh.*` key is written. | MCP keeps its two slots, no mesh listener, no join; `mesh=single` and `mesh=0/0/0` are preserved. |
| `wrong-authority` | n1 gets a valid but wrong `mesh.pub`. | n0 signs; n1 logs `mesh-join: reject reason=bad-signature` and stays at its seed view. |

`run --fault no-peer` deliberately skips the M2-1 mesh probe (there is no listener) and
passes on the HTTP probe plus the M1/M2-0 markers. `probe` and `run` write the mode into
the evidence.

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

The relay is still a dumb byte pump. On the first session it waits until it has read first
bytes from **both** guests before writing in either direction, which avoids the early-data
race in the iris port forward. Later sessions (for example after the member restart) pump
each direction as soon as it has bytes, so a live guest's join retry is not held behind a
peer whose listener is still arming. Its idle timeout is 120 s and the modules send a 5 s
heartbeat.

The modules exchange `Request/Ping` and `Response/Ping` on connect, then `Control/Ping`
heartbeats. The wire is the M2-ADR-0002 contract: a 32-bit little-endian envelope length
followed by exactly that many `MeshEnvelope` bytes, with a 64 KiB bound. A Ping body is the
source's 16-byte `NodeId`; a node rejects a body that does not match the header. The
transport pass line names the peer `NodeId`, frame/byte counters and `rejects=0`.

This listen-only bootstrap is a documented M2-1 deviation from the lower-`NodeId`-initiates
rule; the guest cannot connect until a kernel/module connect service exists. See the
implementation note in M2-ADR-0002.

## Join and membership epoch (M2-2)

The member sends a versioned `JoinRequest` in a `Request/Hello` frame; the root answers
with a signed `EpochConfirm` in `Response/Hello`; the member installs the full member list
and sends a `JoinAck` in `Completion/Hello`. The body layout, signature domain and
fail-closed rules are in [M2-ADR-0005](../adr/m2-0005-join-epoch.md). The root is the only
epoch writer. The member retries the same request every 5 s, at most five sends, then logs
a timeout and keeps its last view.

The rig requires both nodes to print the same:

```text
mesh-join: converged role=root   epoch=2 members=2 digest=<32 hex> snapshot=<n0>,<n1>
mesh-join: converged role=member epoch=2 members=2 digest=<32 hex> snapshot=<n0>,<n1>
```

It recomputes the snapshot digest from the canonical bytes independently of the module,
requires the two node IDs in the snapshot, requires the root's `mesh-join: ack from=...`
and the member's `mesh-join: ack-sent ...`, and requires `rejects=0` in the M2-1 pass
line. The epoch must advance past the seed (2 in the two-node case).

### Restart and rejoin

`run` stops n1 (the member), copies its transcript to `serial-n1-boot1.log`, waits for n0
to notice the dead link and re-arm its listener, restarts n1 with the same NVRAM image,
and starts a fresh mesh relay. The member starts at the seed epoch, sends
`last_epoch=1`, and the root re-confirms its current epoch and list. The rig requires:

- the rejoin epoch and digest equal the first epoch and digest;
- the root's epoch did not advance across the member restart;
- the rejoin request's `last_epoch` is below the confirmed epoch (the member was
  promoted by the root, not self-confirmed);
- the root logged a second `mesh-join: confirm epoch=2`.

A **root** restart is different: the root would start at the seed and the member's higher
epoch would be refused with `epoch-regression`, because the module cannot persist the
confirmed epoch (the ABI has `config_get` only). Failing closed is deliberate; see
M2-ADR-0005 §4 and the host test `root_restart_refuses_to_lower_a_member_epoch`.

## NVRAM keys

The rig writes a DS1386 image per node. The keys are:

| Key | Meaning |
|---|---|
| `mcp.url` | The node's own gateway plus the fixture port: `http://192.168.<n>.1:<port>/mcp.module`. |
| `mcp.sha256` | SHA-256 of the fresh module. |
| `mesh.peer` | The peer's mesh host port, reached through this node's gateway: `192.168.0.1:9712` for `n0`, `192.168.1.1:9702` for `n1`. Its presence enables the guest mesh listener and the join. |
| `mesh.authority` | Node 0's MAC; the 1-of-1 authority root. |
| `mesh.epoch` | Initial seed epoch, `1`. |
| `mesh.pub` | The root's Ed25519 public key (64 hex characters). Written to the member. |
| `mesh.key` | The root's 32-byte Ed25519 seed (64 hex characters). Written only to the root; a lab fixture, redacted in the evidence bundle. |
| `console` | `serial0`. |

The image is 8 KiB; the environment starts at byte `0x10`, the MAC is at byte `314`, and
the environment must end before the MAC. The harness checks the bound and records the
parsed environment in the evidence bundle. The environment limit is 298 bytes; the root
stores only `mesh.key` and derives its public key so both 64-hex keys fit. `--fault
no-peer` omits every `mesh.*` key.

## Readiness and evidence

`BOOT-READY v=1 ... network=up mcp=listening mesh=single` is the readiness marker. The
`mesh=single` field is the **kernel's M1 bootstrap view** and does not change: the kernel
prints it before any module runs. The module's join view is the separate `mesh-join:`
evidence. The rig also requires:

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

The M2-2 join adds:

```text
mesh-join: enabled role=root|member seed_epoch=1 authority=<NodeId> key=<64 hex>
mesh-join: local role=... epoch=1 members=1 digest=<32 hex> snapshot=<NodeId> state=confirmed|pending
mesh-join: request from=<member> incarnation=N last_epoch=1 last_digest=<32 hex> nonce=N attempt=1
mesh-join: confirm epoch=2 members=2 digest=<32 hex> snapshot=<n0>,<n1>
mesh-join: converged role=... epoch=2 members=2 digest=<32 hex> snapshot=<n0>,<n1>
mesh-join: ack from=<member> epoch=2 digest=<32 hex>
mesh-join: ack-sent epoch=2 digest=<32 hex>
mesh-join: duplicate epoch=2 members=2 digest=<32 hex>
mesh-join: refused reason=... root_epoch=N
mesh-join: reject reason=bad-signature|wrong-root|epoch-regression|...
mesh-join: retry attempt=N
mesh-join: timeout attempts=N state=pending
```

The evidence bundle contains:

| File | Content |
|---|---|
| `summary.md` | Human-readable results, node table, boot lines, NVRAM, probes, join, relay logs. |
| `run.json` | Machine-readable version of the same. |
| `serial-n0.log`, `serial-n1.log` | Final serial transcripts. |
| `serial-n1-boot1.log` | The member's transcript before the restart (when `run` restarted it). |
| `stdout-n0.log`, `stderr-n0.log`, etc. | Emulator output. |
| `probe-n0-to-n1.{txt,raw}`, `probe-n1-to-n0.{txt,raw}` | Telnet transcripts, clean and raw. |
| `relay-n0-to-n1.log`, `relay-n1-to-n0.log` | HTTP probe connection and byte counts. |
| `relay-mesh.log` | Mesh relay connections, first-byte guard, byte counts and reconnects. |
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
  probe, not for a persistent mesh link. The mesh relay has a 120 s idle timeout and a
  first-byte guard on the cold session.
- **The guest cannot connect.** Both modules listen and the host relay initiates. The
  lower-`NodeId`-initiates rule is not exercised until a connect service or fixed forward
  path exists.
- **The mesh listener owns module TCP slot 1.** When `mesh.peer` is present MCP uses only
  slot 0 for HTTP; without it the original two-slot MCP behaviour is unchanged.
- **The relay is a host single point of failure.** It is a laboratory seam, not the
  production data path.
- **The checked-in v0.1.3 MCP module is not used.** The rig builds and serves the current
  module.
- **Fixed ports.** Another rig run can collide. Use `--base-port` and stop the previous
  run.
- **No persisted epoch.** A module cannot write NVRAM. A member restart rejoins at the
  same epoch while the root is up; a root restart fails closed on a higher member epoch;
  both restart deterministically. See M2-ADR-0005.
- **1-of-1 authority.** No threshold, no key rotation, no revocation and no production
  key custody. The key is a lab fixture.
- **No leave/removal and no epoch push.** A third-node join, failure detection, homes,
  capability routes and migration remain M2-3..M2-6/M3.
- **The guests still print `mesh=single`.** That is the kernel's M1 bootstrap view; the
  module's confirmed view is `mesh-join: converged`.

## M2-2 status and handoff

M2-2 (join/epoch) is implemented in `crates/mesh-join`, `crates/mesh-service`,
`crates/cap-engine`, `modules/mcp` and `tools/m2-rig`:

- the join codec, signed confirmation, authority verification, retry/timeout policy,
  replay/equivocation checks, restart rules and malformed-body tests are host-tested;
- the MCP module carries the handshake over the unchanged M2-1 framing and binds the join
  body to the frame kind;
- the rig runs the two-kernel join, recomputes the snapshot digest, asserts the two-node
  membership, restarts the member and asserts the same epoch on rejoin, and records the
  negative `no-peer` and `wrong-authority` modes.

What remains for M2: rendezvous homes and a remote capability route (M2-3), L0 migration
in both directions (M2-5), and failure/partition with lease fencing (M2-6). M2-2 does not
claim the full M2 gate.

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
- If the join does not converge, look for `mesh-join: enabled`, `local`, `request`,
  `confirm` and `reject` lines. `disabled reason=...` means the authority/key NVRAM keys
  are missing or malformed. `reject reason=bad-signature` means the member's `mesh.pub`
  is not the root's `mesh.key`. `refused reason=epoch-regression` means the root's seed is
  below the member's confirmed epoch (the root restarted).
- If the member restart fails, check `serial-n1-boot1.log` and the current
  `serial-n1.log`. The harness waits for n0 to leave its old link before restarting n1;
  `mesh-transport: listen retry` on n0 means its listener could not re-arm.
