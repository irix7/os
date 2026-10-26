# M2 two-node rig — runbook

**Status:** working; M2-1 transport, M2-2 join/epoch, M2-3 rendezvous homes and the M2-4
remote capability route
**Scope:** M2-0 foundation, the M2-1 framed mesh transport, the M2-2 join, the M2-3
rendezvous homes and the M2-4 remote capability route. The rig starts two independent
kernels, proves both reach `BOOT-READY` with distinct identities, proves a two-way HTTP
exchange through the chosen path, connects the two guest mesh listeners so the kernels
exchange framed `MeshEnvelope` records, drives the signed join handshake until both nodes
report the same membership epoch and snapshot, and fetches the rendezvous home table from
both kernels. In the default route mode it then issues a capability at n0, serves the
remote stat at n1, performs a controlled root-signed home-lease move, consumes one
`MOVED` and retries exactly once in each direction, and fetches the route report from both
kernels. It then stops and restarts the member and requires it to rejoin at the **same**
epoch with the **same** table. It does not implement L0 migration, live TLS/socket/fd
migration, leave/removal, failure detection, general home moves or a persistent route
directory.

The decisions behind the rig are [M2-ADR-0001](../adr/m2-0001-two-node-rig.md),
[M2-ADR-0002](../adr/m2-0002-transport.md), [M2-ADR-0003](../adr/m2-0003-authority.md),
[M2-ADR-0004](../adr/m2-0004-actor-runtime.md),
[M2-ADR-0005](../adr/m2-0005-join-epoch.md) and
[M2-ADR-0006](../adr/m2-0006-homes.md) and
[M2-ADR-0007](../adr/m2-0007-route.md).

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
framing probe, waits for the join and the member's acknowledgement, fetches the rendezvous
home table from both kernels and checks the stale-epoch negative, drives the five route
steps (`/mesh/route?step=1..5`) and fetches the route report from both kernels, then stops
and restarts **only n1**, waits for it to rejoin at the same epoch, fetches the tables
again, stops both nodes and writes `target/m2-rig/evidence/`.

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
leave the nodes running), `--verbose`, and
`--fault none|no-peer|wrong-authority|no-route`.

### Fault modes

The fault modes are negative evidence, not extra features:

| Mode | Setup | Expected result |
|---|---|---|
| `none` | Full two-node configuration, including `mesh.route=1`. | Join converges; both kernels serve the same home table; the remote stat completes and one wire `MOVED` plus one local `MOVED` retry succeed; n1 rejoins at the same epoch with the same table; `?epoch=1` is refused as stale. |
| `no-peer` | No `mesh.*` key is written. | MCP keeps its two slots, no mesh listener, no join, `/mesh/homes` is `404 disabled`; `mesh=single` and `mesh=0/0/0` are preserved. |
| `wrong-authority` | n1 gets a valid but wrong `mesh.pub`. | n0 signs; n1 logs `mesh-join: reject reason=bad-signature`, stays at its seed view and serves `409 pending` with no homes. |
| `no-route` | The join and homes keys are written, but `mesh.route` is absent. | The join and homes pass exactly as in M2-3; both kernels log `mesh-route: disabled reason=config`, `/mesh/route` is `404 disabled`, and no route step or `MOVED` runs. |

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

## Rendezvous homes (M2-3)

The module computes homes from the **confirmed** membership snapshot with
`mesh-core::home_for`; it does not carry a second hash or placement model. A member that
has not installed a confirmation serves `409 pending` and no rows.

The fixture object set is deterministic and configured by `mesh.obj` (1–32, default 8):

```text
object[i] = BLAKE3-256("IRIX7-M2-HOME-OBJECT-v1" || i u32 LE)
```

The MCP endpoint is `GET /mesh/homes`:

```text
mesh-homes: state=current object_set=m2-v1 epoch=2 members=2 digest=<32 hex> count=8
mesh-home: object=<64 hex> home=<32 hex> epoch=2 digest=<32 hex>
...
```

Every row repeats the epoch and digest. The module logs the same header and rows at
convergence. `GET /mesh/homes?epoch=N` answers `409 stale` with `count=0` and no object
rows when `N` is not the confirmed epoch; it never relabels the old table as current.

The rig independently recomputes the fixture IDs, the rendezvous winners and the
membership digest. It requires both kernels to match that table, to match each other, and
to keep the same table across the member rejoin. It also fetches `?epoch=1` on both nodes
and requires `409 stale` with no rows. This is a **rejoin at the same epoch**, not a
membership change: leave/removal and a third-node epoch advance do not exist in this
slice.

The before-restart table and the stale-epoch negative are fetched from `/mesh/homes` on
the clean control plane. The after-rejoin comparison uses the module's `mesh-homes:`
serial group, which carries the same header and rows. The iris host forward can keep a
half-open guest connection across the member restart, so a second MCP fetch from the
root is not a reliable gate primitive; the serial group is produced by both kernels and
is compared against the independent table as well. The guest-level HTTP probe runs after
the MCP fetch and before the restart, so the control-plane fetch is not behind a probe
connection.

## Remote capability route (M2-4)

The route is the M2-3 rendezvous home. The sender computes it with
`mesh-core::home_for` over the confirmed snapshot, caches it in the bounded
`mesh-core::RouteCache`, and carries a `cap-engine`-signed `Capability` to that home. There
is no central directory and no second placement model.

`mesh.route=1` is the opt-in. When it is absent the module logs
`mesh-route: disabled reason=config` and keeps the M2-1/M2-2/M2-3 behaviour; when
`mesh.peer` is absent the mesh listener and the route do not exist. The route does not
change `/mesh/homes`: that surface remains the confirmed rendezvous membership mapping,
while the route report carries the per-object lease movement.

The MCP endpoint is `GET /mesh/route`:

```text
mesh-route: state=enabled view_epoch=2 members=2 digest=<32 hex>
mesh-route: move old=<64 hex|-> new=<64 hex|->
mesh-route: result object=<64 hex> home=<32 hex> home_epoch=3 value=<64 hex> attempts=2 moved=1
mesh-route: counters completes=1 retries=2 rejects=0 timeouts=0 results=3 moved_local=1 moved_remote=1
```

A disabled node answers `404` with `mesh-route: state=disabled reason=config|peer`. The
report is in memory only; it is not persisted and it is not a directory.

`GET /mesh/route?step=N` is the rig-driven control surface. The rig chooses the order and
the route policy chooses the first fixture object with the required confirmed home:

| Step | Effect |
|---|---|
| 1 | n0 issues a capability for the first n1-homed fixture object and n1 serves the stat. |
| 2 | n0 signs a root-authorised move of that object from n1 to n0; n1 enters `DRAINING`. |
| 3 | n0 repeats the stat on its stale n1 route; n1 answers `MOVED`; n0 retries once locally and completes. |
| 4 | n0 signs a move of the first n0-homed fixture object to n1. |
| 5 | n0 repeats the stat on its stale n0 route; the local old home answers `MOVED`; n0 retries once at n1 and completes. |

The MCP HTTP slot keeps its close handshake for up to 5 s. The rig waits 10 s after the
join/homes serial group before the first control-plane fetch and 6 s before each
non-idempotent route command, so a half-open host forward cannot replay a step. A lost
command response is reported, not silently repeated; the operator restarts the pair.

The `MOVED` answer is bounded: it names exactly `old_home_epoch + 1`, a confirmed member,
the remaining lease and the remaining forwarding grace. The sender checks that it came
from its cached old home, validates the object, epochs, member and bounds, replaces the
cache entry and retries exactly once. A second `MOVED`, a lower or unknown epoch, a wrong
home, a non-member target or a malformed frame fails closed with `repeated-moved`,
`stale-epoch`, `unknown-epoch`, `wrong-home`, `not-home` or `malformed`. A node that is not
the current home and holds no draining lease answers `not-home`; it never invents a
redirect.

The rig recomputes the fixture object IDs, rendezvous winners, epoch pairs and the
deterministic value independently. It requires:

- both reports to carry the join-confirmed epoch, member count and digest;
- n1 to serve the remote stat (one completion) and n0 to verify the value;
- n0's `move new` and n1's `move old` to name the wire object;
- n1 to emit one `moved source=remote`, n0 to retry once locally and complete at n0;
- n0's `move old` and n1's `move new` to name the local object;
- n0 to emit one `moved source=local`, retry once at n1 and complete at n1;
- the three result rows to name the expected object, home, home epoch, attempt count,
  `moved` flag and independently recomputed value;
- `rejects=0` and `timeouts=0` on both nodes.

The controlled move is a **root-signed lab lease move**, not a membership change. M2 has
no leave/removal, so the stale path cannot be driven by a real epoch advance. The move is
scoped to the confirmed membership epoch/digest, one object and one `home_epoch` step; it
is not a general home-migration policy. See M2-ADR-0007.

## NVRAM keys

The rig writes a DS1386 image per node. The keys are:

| Key | Meaning |
|---|---|
| `mcp.url` | The node's own gateway plus the fixture port: `http://192.168.<n>.1:<port>/mcp.module`. |
| `mcp.sha256` | SHA-256 of the fresh module. |
| `mesh.peer` | The peer's mesh host port, reached through this node's gateway: `192.168.0.1:9712` for `n0`, `192.168.1.1:9702` for `n1`. Its presence enables the guest mesh listener and the join. |
| `mesh.authority` | Node 0's MAC; the 1-of-1 authority root. |
| `mesh.epoch` | Initial seed epoch, `1`. |
| `mesh.obj` | Number of M2 home-fixture objects, `1`–`32` (the rig uses the default `8` and omits the key so the route opt-in fits). |
| `mesh.route` | `1` enables the M2-4 remote capability route. Absent preserves the M2-3 behaviour. |
| `mesh.pub` | The root's Ed25519 public key (64 hex characters). Written to the member. |
| `mesh.key` | The root's 32-byte Ed25519 seed (64 hex characters). Written only to the root; a lab fixture, redacted in the evidence bundle. |
| `console` | `serial0`. |

The image is 8 KiB; the environment starts at byte `0x10`, the MAC is at byte `314`, and
the environment must end before the MAC. The harness checks the bound and records the
parsed environment in the evidence bundle. The environment limit is 298 bytes; the root
stores only `mesh.key` and derives its public key so both 64-hex keys fit, and
`mesh.obj` uses the short name because only a few bytes remain. The route opt-in is
`mesh.route=1`; the rig omits `mesh.obj` when it is the default eight so the full route
environment still fits. The `nvram` host test builds the full two-node environment at the
worst-case five-digit fixture port and requires it to fit. `--fault no-peer` omits every
`mesh.*` key.

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

The M2-3 homes surface adds:

```text
mesh-homes: configured set=m2-v1 count=8
mesh-homes: state=current object_set=m2-v1 epoch=2 members=2 digest=<32 hex> count=8
mesh-home: object=<64 hex> home=<32 hex> epoch=2 digest=<32 hex>
```

and, on a stale query, the HTTP body (not a serial line):

```text
mesh-homes: state=stale requested_epoch=1 epoch=2 members=2 digest=<32 hex> count=0
```

The M2-4 route adds the opt-in marker:

```text
mesh-route: enabled role=root|member objects=8
mesh-route: disabled reason=config|peer
```

The detailed route evidence is the `GET /mesh/route` body:
`state=enabled view_epoch=2 members=2 digest=<32 hex>`, the `move old=/new=` rows, one
`result` row per operation with object, home, home epoch, value, attempts and `moved`, and
the `counters` row. A disabled node answers `state=disabled reason=config|peer`.

The evidence bundle contains:

| File | Content |
|---|---|
| `summary.md` | Human-readable results, node table, boot lines, NVRAM, probes, join, rendezvous homes, remote capability route, relay logs. |
| `run.json` | Machine-readable version of the same, including every home table and the route reports. |
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
  slot 0 for HTTP; without it the original two-slot MCP behaviour is unchanged. A client
  that keeps its side of an HTTP connection open is aborted after 5 s so the single slot
  cannot wedge the control plane; the response has already been written.
- **The relay is a host single point of failure.** It is a laboratory seam, not the
  production data path.
- **The host forward can keep a half-open guest connection.** The after-rejoin homes
  check therefore uses the serial group. The MCP endpoint is still fetched and validated
  before the restart; this is a harness limitation, not a second home model.
- **The checked-in v0.1.3 MCP module is not used.** The rig builds and serves the current
  module.
- **Fixed ports.** Another rig run can collide. Use `--base-port` and stop the previous
  run.
- **No persisted epoch.** A module cannot write NVRAM. A member restart rejoins at the
  same epoch while the root is up; a root restart fails closed on a higher member epoch;
  both restart deterministically. See M2-ADR-0005.
- **1-of-1 authority.** No threshold, no key rotation, no revocation and no production
  key custody. The key is a lab fixture.
- **No leave/removal and no epoch push.** A third-node join, failure detection, general
  home moves, L0 migration and lease fencing across a partition remain M2-5/M2-6/M3.
  The M2-4 route uses a root-signed one-object lab lease move, not membership churn.
- **The route is not a directory.** A route is the confirmed rendezvous home plus a
  bounded lease and route cache. There is no persistent route table, no central lookup
  and no trusted redirector other than a home that holds the old lease.
- **The capability is a bearer token.** The M2 transport is plain TCP on host loopback,
  so the token has no confidentiality, peer authentication or replay window at the
  transport. The subject check is an application-layer binding, not a TLS identity.
- **The capability window uses the boot clock.** The module ABI has no wall-clock
  service; the route lease is the real time bound. A shared time service is later work.
- **The `MOVED` retry is exactly one hop.** Multi-hop forwarding, an arbitrary stale-route
  chase and a third-member retry are not implemented.
- **No membership change in the rejoin check.** The member restarts and rejoins at the
  same epoch. Leave/removal does not exist, so the rig cannot advance the epoch by
  changing the member set; the host tests cover that policy property.
- **The guests still print `mesh=single`.** That is the kernel's M1 bootstrap view; the
  module's confirmed view is `mesh-join: converged`.

## M2 status and handoff

M2-2 (join/epoch) is implemented in `crates/mesh-join`, `crates/mesh-service`,
`crates/cap-engine`, `modules/mcp` and `tools/m2-rig`:

- the join codec, signed confirmation, authority verification, retry/timeout policy,
  replay/equivocation checks, restart rules and malformed-body tests are host-tested;
- the MCP module carries the handshake over the unchanged M2-1 framing and binds the join
  body to the frame kind;
- the rig runs the two-kernel join, recomputes the snapshot digest, asserts the two-node
  membership, restarts the member and asserts the same epoch on rejoin, and records the
  negative `no-peer` and `wrong-authority` modes.

M2-3 (rendezvous homes) is implemented in `crates/mesh-core`, `modules/mcp` and
`tools/m2-rig`:

- `mesh-core` exposes the fixture object set and a `HomeTable` over the existing
  `home_for`; host tests cover deterministic placement, empty/single/two-node sets, epoch
  changes, distribution, mismatch detection and membership-change movement;
- the module serves `GET /mesh/homes` from a confirmed snapshot only, logs the same lines,
  and refuses a stale epoch with `409` and no rows;
- the rig fetches both kernels, recomputes the object IDs, homes and digest independently,
  checks before/after the member rejoin, and drives the stale-epoch negative.

M2-4 (remote capability route) is implemented in `crates/mesh-route`, `modules/mcp` and
`tools/m2-rig`:

- `mesh-route` defines the bounded `I7R1` bodies, the confirmed-view home lease, the
  `DRAINING`/`MOVED` state machine, the route-cache refresh, the `cap-engine` capability
  checks and the one-retry rule; host tests cover remote completion, wrong home, stale
  epoch, one-`MOVED` success, repeated-`MOVED` rejection, missing peer/partition,
  malformed bodies, capability mismatch, authority-signed moves and the rig step sequence;
- the module serves `GET /mesh/route` from an in-memory report and drives `?step=N`; the
  route code and report stay inside the 512 KiB slot A;
- the rig issues the capability at n0, serves the remote stat at n1, drives the two
  root-signed lease moves, consumes one wire `MOVED` with a local retry and one local
  `MOVED` with a wire retry, recomputes every value and epoch, and records both reports;
- `--fault no-route` proves the opt-in negative while the M2-1/M2-2/M2-3 markers still
  pass, and `--fault no-peer` keeps the M1 path.

What remains for M2: L0 migration in both directions (M2-5) and failure/partition with
lease fencing (M2-6). This slice does not claim the full M2 gate and does not implement
L0 migration, live TLS/socket/fd migration or a persistent route directory.

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
- If the homes check fails, fetch the endpoint by hand:
  `curl -sS http://127.0.0.1:9701/mesh/homes` and `...:9711/mesh/homes`. A
  `state=pending` member has not installed a confirmation; a `404 disabled` means
  `mesh.peer` was absent; a `state=stale` response is the expected answer for
  `?epoch=1` after the epoch advanced. Compare the `mesh-home:` rows with
  `summary.md`; the rig's independent table is in `run.json` under `homes`.
