# M2 two-node rig — runbook

**Status:** working; M2-1 transport, M2-2 join/epoch, M2-3 rendezvous homes, the M2-4
remote capability route, M2-5 L0 actor migration and the M2-6 failure/fence/partition slice
**Scope:** M2-0 foundation, the M2-1 framed mesh transport, the M2-2 join, the M2-3
rendezvous homes, the M2-4 remote capability route, the M2-5 two-way L0 stackless-actor
migration and the M2-6 failure, lease-fencing and partition modes. The rig starts two independent kernels, proves both reach `BOOT-READY`
with distinct identities, proves a two-way HTTP exchange through the chosen path, connects
the two guest mesh listeners so the kernels exchange framed `MeshEnvelope` records, drives
the signed join handshake until both nodes report the same membership epoch and snapshot,
and fetches the rendezvous home table from both kernels. In the default route mode it then
issues a capability at n0, serves the remote stat at n1, performs a controlled root-signed
home-lease move, consumes one `MOVED` and retries exactly once in each direction, and
fetches the route report from both kernels. It then stops and restarts the member and
requires it to rejoin at the **same** epoch with the **same** table. On the clean
`mesh.route=2` pair it then seeds a stackless actor, moves it n0→n1, resumes the queued
message, refuses a stale delivery at the old root, duplicate-delivers at the member, queues
the next message, moves the actor back n1→n0, resumes it, refuses a stale wire delivery at
the old member, duplicate-delivers at the root and refuses a tampered capability. In
`kill-peer` it
stops the member without a clean close and asserts the bounded suspect/failed/removal
transition, the recomputed homes, the leased-home fence and the survivor's local data
path. In `partition` it cuts only the mesh seam: the root continues under a new epoch while
the live member keeps its last view, refuses a mutable commit and still serves a read from
its unexpired lease; restoring the seam makes it rejoin at a new epoch. It does not
implement live TLS/socket/fd migration, L2 safepoint migration, general home moves, a
general actor scheduler, gossip, a persistent route directory or linearisable writes
across a partition.

The decisions behind the rig are [M2-ADR-0001](../adr/m2-0001-two-node-rig.md),
[M2-ADR-0002](../adr/m2-0002-transport.md), [M2-ADR-0003](../adr/m2-0003-authority.md),
[M2-ADR-0004](../adr/m2-0004-actor-runtime.md),
[M2-ADR-0005](../adr/m2-0005-join-epoch.md),
[M2-ADR-0006](../adr/m2-0006-homes.md),
[M2-ADR-0007](../adr/m2-0007-route.md),
[M2-ADR-0008](../adr/m2-0008-failure-fence.md) and
[M2-ADR-0009](../adr/m2-0009-actor-migration.md).

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
steps (`/mesh/route?step=1..5`) and fetches the route report from both kernels, stops and
restarts **only n1** (the default), then drives the eleven migration steps
(`/mesh/migrate?step=1..11`) and fetches both migration reports. It stops both nodes and
writes `target/m2-rig/evidence/`. In `kill-peer` and `partition` it runs the failure
scenario instead of the member restart; migration is not driven in those modes.

The lifecycle commands:

| Command | What it does |
|---|---|
| `up` | Start both nodes, wait for readiness, leave them running. |
| `status` | Show `iris-ci` state and the boot/join markers for both nodes. |
| `probe` | Probe both directions (HTTP, mesh framing, join) against running nodes and write evidence. It does not restart a node. |
| `down` | Send `iris-ci quit` to both and wait for the processes. |
| `run` | `up` + probe + member restart/rejoin (or the failure/partition scenario) + `down` + evidence. |
| `print-config` | Print the derived instance/port table and the NVRAM keys without starting. |
| `clean` | Remove the work directory. Stop the nodes first. |

Useful options: `--work DIR`, `--base-port N`, `--timeout SECS`, `--keep` (with `run`,
leave the nodes running), `--verbose`, and
`--fault none|no-peer|wrong-authority|no-route|kill-peer|partition`.

### Fault modes

The fault modes are negative evidence, not extra features:

| Mode | Setup | Expected result |
|---|---|---|
| `none` | Full two-node configuration, including `mesh.route=2`. | Join converges; both kernels serve the same home table; the remote stat completes and one wire `MOVED` plus one local `MOVED` retry succeed; n1 rejoins at the same epoch with the same table; `?epoch=1` is refused as stale; the actor moves n0→n1→n0 with the same state hash at each boundary, the queued message resumes, stale old homes are refused and duplicate deliveries are idempotent. |
| `no-peer` | No `mesh.*` key is written. | MCP keeps its two slots, no mesh listener, no join, `/mesh/homes` is `404 disabled`; `mesh=single` and `mesh=0/0/0` are preserved. |
| `wrong-authority` | n1 gets a valid but wrong `mesh.pub`. | n0 signs; n1 logs `mesh-join: reject reason=bad-signature`, stays at its seed view and serves `409 pending` with no homes. |
| `no-route` | The join and homes keys are written, but `mesh.route` is absent. | The join and homes pass exactly as in M2-3; both kernels log `mesh-route: disabled reason=config` and `mesh-migrate: disabled reason=config`, `/mesh/route` and `/mesh/migrate` are `404 disabled`, and no route step, `MOVED` or migration runs. |
| `kill-peer` | The full route configuration (`mesh.route=1`); n1 is `SIGKILL`ed after steps 1–5. | n0 logs `mesh-fail: suspect`, `failed` and `removed`; the epoch advances 2→3 with a one-member table; every object is recomputed to n0; a fence row names n1 as the stale home; step 6 completes at n0 with one attempt, no send and no timeout; step 7 is `stale-epoch`; step 8 commits; step 9 reads. |
| `partition` | The full route configuration (`mesh.route=1`); the mesh relay is paused (frames dropped, sessions kept). | The root follows the same removal path; the live member keeps epoch 2 and two members, reports `authority state=lost mutable=0 reads=lease`, refuses step 8 with `no-authority` and reports a remote home in step 6 as `no-authority`. Resuming the seam rejoins it at epoch 4 with the two-member table; the root then commits a peer-homed object (step 10) and the recovered member serves it. |

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

`mesh.route=1` or `2` is the opt-in (`2` also enables the M2-5 migration). When it is
absent the module logs `mesh-route: disabled reason=config` and keeps the M2-1/M2-2/M2-3
behaviour; when `mesh.peer` is absent the mesh listener and the route do not exist. The route does not
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

## Failure, lease fencing and partition (M2-6)

The failure detector is the existing membership state machine, not a second model. Any
well-formed frame from the confirmed peer (the 5 s `Control/Ping`, a join message or a
route frame) refreshes the peer's liveness record. A silent peer becomes **suspect** at
6 s and **failed** at 12 s. A suspect peer stays in the routing snapshot; a failed one
does not. Duplicate failures are idempotent, a stale-incarnation heartbeat is ignored and
a failed peer is re-admitted only by a fresh signed join.

The authority root removes a failed member with exactly one epoch advance and installs the
full remaining member list. The member side never removes the root and never advances the
epoch: losing the authority is not a membership change.

`RouteNode::on_snapshot` fences every cached route and home lease that named a node no
longer in the view. The fence is local: the object is re-routed from the new view with
`mesh_core::home_for`, no message goes to the removed node and no quorum is consulted. A
cached route to a non-member is never used, and the removed node is never asked for a
redirect.

There are two leases:

| Lease | Lifetime | Renewed by | Bounds |
|---|---|---|---|
| Read home lease | 120 s | The root from its own confirmed view; a member by an accepted frame from the authority | Read availability |
| Authority write lease | 6 s | Any accepted frame from the authority, a confirmation, or the root itself | Mutable commits |

The write lease plus one heartbeat (5 s + 6 s = 11 s) is shorter than the 12 s failure
window; the module carries a compile-time assertion. After the seam is cut the old home
stops accepting commits before the root can re-home the object. A read is still served
from the unexpired read lease because that is exactly what the existing `HomeLease` model
supports; nothing here adds a second read path.

The exact availability rule is in
[M2-ADR-0008](../adr/m2-0008-failure-fence.md) §5:

| State | Read | Mutable commit |
|---|---|---|
| Current view, current leases | Available | Available |
| Current view, read lease expired | `not-current` | `not-current` |
| Isolated member, read lease valid, write lease expired | Available to a client with a valid capability; the member does not issue one itself | `no-authority` |
| Isolated member, read lease expired | `not-current` | `no-authority` |
| Root after removal | Available at the survivor | Available, no quorum |
| Non-home without a lease | `not-home` | `not-home` |

`GET /mesh/route` gains the failure surface:

```text
mesh-route: authority state=current|lost epoch=E mutable=0|1 write_lease_ms=N
mesh-route: fence object=<64 hex> stale_home=<32 hex> stale_home_epoch=N home=<32 hex> home_epoch=N reason=left-view|lease-left-view
mesh-route: result ... op=stat|commit ...
mesh-route: counters ... fences=N commits=N replays=N last_replay=stale-epoch
```

The partition mode pauses the relay without closing the guest sessions: the iris port
forward cannot reliably reopen a half-open guest connection, so tearing the relay down
would test the forward instead of the partition rule. With failure detection enabled the
transport heartbeat timeout is a 300 s backstop and the 12 s membership detector is the
failure path; a member that lost authority asks for a fresh signed join on the next
authority frame, so the same session carries the rejoin. Step 10 is used only after
recovery: the root commits an object homed at the recovered member and the member serves
it.

Step 6 re-routes the object that was homed at the removed node (the last object the root
put on the wire); on the isolated member it reports an object homed on the other side as
`no-authority` instead of inventing a local home. Step 7 replays the saved stale frame through
the receive path and requires `stale-epoch`; the exercise is a successful control action
whose *result* is a `409` refusal, so the response carries the same reason and the report's
`replays`/`last_replay` counters increment. The epoch fence is checked before the
request-id replay check. Step 8 issues the mutable commit; step 9 the read.

The default `run` restarts the member for the M2-2 rejoin check only in the `none` and
`no-route` modes. `kill-peer` and `partition` use the failure scenario instead, and the
evidence bundle records the scenario, node reports, expected views and every check.

## L0 stackless-actor migration (M2-5)

`mesh.route=2` opts in to both the route and migration. `mesh.route=1` keeps the M2-4
route only; absent keeps M2-3. No new NVRAM key is needed, so the full route+failure
environment still fits the 298-byte DS1386 area.

The policy lives in `crates/mesh-migrate`. The actor is a native Rust state machine, not a
thread and not a captured stack. It is polled one message at a time, so a message boundary
is the only suspension point. Its canonical state is a fixed 248-byte little-endian value
(schema 1) carrying the actor id, rendezvous home, current host, placement epoch, applied
count, highest message id, logical digest, one queued message, one unacknowledged message
and one serialised capability handle. The state hash is BLAKE3 over the logical fields;
placement (`host`, `epoch`) is excluded so the source and destination must produce the
**same** hash for the move to be meaningful.

The fixture is deterministic:

```text
actor[0]    = BLAKE3-128("IRIX7-M2-ACTOR-v1" || 0 u32 LE)
home        = mesh-core actor rendezvous home over the confirmed view
message[i]  = BLAKE3-256("IRIX7-M2-ACTOR-MSG-v1" || i u32 LE), id 10+i
logical0    = BLAKE3-256("IRIX7-M2-ACTOR-INIT-v1" || actor)
logical'    = BLAKE3-256("IRIX7-M2-ACTOR-STEP-v1" || logical || id u64 LE || payload)
```

The move is an explicit root-signed lease:

```text
root -> source:        AUTHORISE(ActorMove)          (or the root is the source)
source:                freeze the actor; no further step
source -> destination: PREPARE(directive, state_len, hash)
destination -> source: READY(directive, hash)
source -> destination: STATE(directive, canonical bytes)
destination:           decode, verify schema/length/hash/identity/epochs
destination -> source: COMMIT(directive, hash)
source:                fence the old instance, discard the live state
source -> destination: ACK(directive, hash)
destination:           activate the actor
```

`ActorMove` names the confirmed membership epoch/digest, actor, home, old/new host and
old/new placement epoch; it is Ed25519-signed by the authority root and verified before
any field is trusted. Both ends require the current confirmed view, confirmed members,
the source/destination role, a one-epoch advance and a bounded lease. A delivery carries a
root-issued `cap-engine` capability with the actor principal as subject, the actor object as
object, `INVOKE`, and the current home/home-epoch/object-epoch; a tampered or stale token
is refused before the actor advances.

The old host keeps a fence record (actor, old/new host and epoch, canonical hash). A
delivery naming the old host or an old epoch answers `stale-epoch`/`stale-home`/`not-home`;
it never invents a redirect. A duplicate message id at or below the highest applied id is
idempotent and does not advance the actor.

`GET /mesh/migrate` is the bounded in-memory report:

```text
mesh-migrate: state=enabled role=root|member view_epoch=2 members=2 digest=<32 hex>
mesh-migrate: actor id=<32 hex> home=<32 hex> state=active|provisional|fenced host=<32 hex> epoch=N applied=N last=N
mesh-migrate: actor hash=<64 hex> logical=<64 hex> queued=<id>:<64 hex>|- unacked=<id>:<64 hex>|- handle=<object>:<rights>:<home>:<home_epoch>:<object_epoch>|-
mesh-migrate: move actor=<32 hex> old=<32 hex> new=<32 hex> old_epoch=N new_epoch=N hash=<64 hex>
mesh-migrate: counters migrations=N incoming=N outgoing=N resumes=N duplicates=N stale=N capability=N rejects=N aborted=N
mesh-migrate: last stage=... reason=...
```

A disabled node answers `404` with `state=disabled reason=peer|config`. `GET
/mesh/migrate?step=N` is the rig-driven control surface; the rig chooses the order and the
policy chooses the actor, hosts and message ids:

| Step | Effect |
|---|---|
| 1 | Seed at n0: apply message 10, leave message 11 queued and message 10 unacknowledged. |
| 2 | Move n0 -> n1; both nodes must report the same canonical state hash. |
| 3 | Resume the queued message 11 at n1. |
| 4 | Refuse a stale delivery at the old n0 host (local fence). |
| 5 | Duplicate-deliver message 11 at n1; the actor must not advance. |
| 6 | Queue message 12 at n1. |
| 7 | Move n1 -> n0 carrying the queued message. |
| 8 | Resume message 12 at n0. |
| 9 | Refuse a stale delivery at the old n1 host on the wire. |
| 10 | Duplicate-deliver message 12 at n0. |
| 11 | Refuse a tampered delivery capability at n0. |

The rig recomputes the actor identity, home, message chain, queued/unacknowledged rows,
capability handle and canonical hash independently of the module. It requires:

- both nodes carry the join-confirmed epoch and digest;
- the same actor id and rendezvous home on both nodes;
- the same state hash at the n0->n1 and n1->n0 boundaries;
- the queued/unacknowledged message and capability handle cross each boundary;
- the destination resumes each queued message exactly once and the logical digest matches
  the independent chain;
- the old root and old member refuse stale deliveries (one local, one wire);
- duplicate delivery leaves the count and hash unchanged;
- the tampered capability is refused without advancing the actor;
- one outgoing and one incoming migration on each node and no aborted move.

The migration report, the steps, the independent checks and both raw reports are recorded
in `run.json` and `summary.md`. `--fault no-peer`, `no-route` and `wrong-authority`
exercise the disabled marker and preserve the earlier M1/M2-1/M2-2/M2-3/M2-4 evidence;
`kill-peer` and `partition` keep `mesh.route=1` and the M2-6 scenario.

## NVRAM keys

The rig writes a DS1386 image per node. The keys are:

| Key | Meaning |
|---|---|
| `mcp.url` | The node's own gateway plus the fixture port: `http://192.168.<n>.1:<port>/mcp.module`. |
| `mcp.sha256` | SHA-256 of the fresh module. |
| `mesh.peer` | The peer's mesh host port, reached through this node's gateway: `192.168.0.1:9712` for `n0`, `192.168.1.1:9702` for `n1`. Its presence enables the guest mesh listener and the join. |
| `mesh.authority` | Node 0's MAC; the 1-of-1 authority root. |
| `mesh.epoch` | Initial seed epoch, `1`. The rig omits it at the module default of 1. |
| `mesh.obj` | Number of M2 home-fixture objects, `1`–`32` (the rig uses the default `8` and omits the key so the route opt-in fits). |
| `mesh.route` | `1` enables the M2-4 remote capability route; `2` enables the route **and** the M2-5 L0 migration. Absent preserves the M2-3 behaviour. |
| `mesh.fail` | `1` enables the M2-6 failure detector. Absent preserves the M2-2 same-epoch rejoin behaviour. |
| `mesh.pub` | The root's Ed25519 public key (64 hex characters). Written to the member. |
| `mesh.key` | The root's 32-byte Ed25519 seed (64 hex characters). Written only to the root; a lab fixture, redacted in the evidence bundle. |
| `console` | `serial0`. |

The image is 8 KiB; the environment starts at byte `0x10`, the MAC is at byte `314`, and
the environment must end before the MAC. The harness checks the bound and records the
parsed environment in the evidence bundle. The environment limit is 298 bytes; the root
stores only `mesh.key` and derives its public key so both 64-hex keys fit, and
`mesh.obj` uses the short name because only a few bytes remain. The clean run uses
`mesh.route=2`; the failure modes use `mesh.route=1`. The rig omits `mesh.obj` when it is
the default eight so the full route+failure environment still fits. The `nvram` host test
builds the full two-node environment at the worst-case five-digit fixture port and requires
it to fit. `--fault no-peer` omits every
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

The M2-6 failure surface adds:

```text
mesh-fail: suspect node=<32 hex> epoch=E
mesh-fail: failed node=<32 hex> epoch=E
mesh-fail: recovered node=<32 hex> epoch=E
mesh-fail: removed node=<32 hex> epoch=E members=N digest=<32 hex> snapshot=<ids>
mesh-fail: authority state=lost epoch=E root=<32 hex> mutable=0 reads=lease
mesh-fail: authority state=restored epoch=E mutable=1
```

The M2-4 route adds the opt-in marker:

```text
mesh-route: enabled role=root|member objects=8
mesh-route: disabled reason=config|peer
```

The M2-5 migration adds the opt-in marker and the actor lifecycle lines:

```text
mesh-migrate: enabled role=root|member
mesh-migrate: disabled reason=config|join|peer
mesh-migrate: seed actor=<32 hex> host=<32 hex> epoch=1 hash=<64 hex>
mesh-migrate: migrate actor=<32 hex> from=<32 hex> to=<32 hex> old_epoch=N new_epoch=N
mesh-migrate: moved actor=<32 hex> from=<32 hex> to=<32 hex> old_epoch=N new_epoch=N hash=<64 hex> role=source|destination
mesh-migrate: fence actor=<32 hex> old_host=<32 hex> new_host=<32 hex> old_epoch=N new_epoch=N hash=<64 hex>
mesh-migrate: stale actor=<32 hex> reason=stale-epoch expected_epoch=N got_epoch=N
mesh-migrate: duplicate actor=<32 hex> message=N applied=N
mesh-migrate: capability actor=<32 hex> reason=bad-signature
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
| `summary.md` failure section | The M2-6 scenario, expected views, `mesh-fail:` lines, steps, checks and node reports. |
| `summary.md` migration section | The M2-5 actor id/home, boundary and return hashes, the eleven steps, every independent check and both raw `/mesh/migrate` reports. |

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
  before the restart; this is a harness limitation, not a second home model. The rig waits
  12 s after the last control-plane fetch before the guest HTTP probe and retries the
  probe once, because the MCP server has one HTTP slot while the mesh is enabled and the
  module's 5 s close handshake can outlast a single probe attempt.
- **The checked-in v0.1.3 MCP module is not used.** The rig builds and serves the current
  module.
- **Fixed ports.** Another rig run can collide. Use `--base-port` and stop the previous
  run.
- **No persisted epoch.** A module cannot write NVRAM. A member restart rejoins at the
  same epoch while the root is up; a root restart fails closed on a higher member epoch;
  both restart deterministically. See M2-ADR-0005.
- **1-of-1 authority.** No threshold, no key rotation, no revocation and no production
  key custody. The key is a lab fixture.
- **Failure removal is root-initiated and two-node.** The root removes a failed member
  with one epoch advance; there is no epoch push, so a third connected member would learn
  the new epoch only by rejoining. That is M3 gossip.
- **Writes are not linearisable across a partition.** The 6 s write lease stops the old
  home committing before the 12 s failure window lets the root re-home the object, but a
  commit accepted just before the cut is not replicated to the new home. M2 does not claim
  multi-node transactions or a consensus round on the data path.
- **A partition and a failure look the same to the root.** A two-node 1-of-1 authority
  cannot distinguish a dead member from an unreachable one; the lease margin bounds the
  consequence. A malicious or buggy root can remove a healthy member.
- **Read availability on the isolated side is bounded.** A member cannot renew its 120 s
  read lease without a confirmation, so after that window its objects are unavailable even
  though the process is alive.
- **No live-state migration and no general actor scheduler.** M2-5 migrates one native
  stackless actor at a message boundary. TLS, sockets, file descriptors, threads,
  processes, timers and kernel module state are not migrated; L2 arbitrary-safepoint
  migration is not attempted. The M2-4 route uses a root-signed one-object lab lease move;
  M2-6 moves only the homes of a removed member by recomputing the rendezvous table.
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
- **The default rejoin check does not change the member set.** The member restarts and
  rejoins at the same epoch in `none`/`no-route`. The `kill-peer` and `partition` modes
  change it through failure and recovery (2 → 3 → 4); a third-node join remains M3.
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

M2-5 (L0 actor migration) is implemented in `crates/mesh-migrate`, `modules/mcp` and
`tools/m2-rig`:

- `mesh-migrate` defines the fixed 248-byte canonical actor state, the placement-independent
  BLAKE3 hash, the deterministic actor/message fixture, the root-signed `ActorMove`
  directive, the `I7M1` bodies and the `PREPARE/READY/STATE/COMMIT/ABORT` state machine;
  host tests cover codec bounds, hash equality, one-way and return migration, stale
  epoch/home, duplicate delivery, the message boundary, malformed payloads, tampered
  capabilities and a wrong-authority signature;
- the module carries `/mesh/migrate` as a bounded in-memory report and `?step=N` as the
  rig-driven control surface; the actor state, queue/acknowledgement and capability handle
  stay inside the 512 KiB slot;
- the rig drives both directions on two real kernels, compares the source and destination
  state hash at each move boundary, recomputes the identity, message chain and hash
  independently, resumes the queued messages, refuses the stale old homes and proves
  duplicate delivery idempotent.

M2-6 (failure, lease fencing and partition) is implemented in `crates/mesh-service`,
`crates/mesh-join`, `crates/mesh-route`, `modules/mcp` and `tools/m2-rig`:

- `mesh-service` adds the bounded, deterministic failure detector over the existing
  `MembershipTable`; host tests cover exact boundaries, a slow tick that crosses both
  boundaries, duplicate failure, stale-incarnation replay, suspect recovery and the rule
  that a failed node needs a join;
- `mesh-join` removes a failed member at the root with one epoch advance, records
  `AuthorityLost` on a member without touching its confirmed view or epoch, and restores
  authority only through a fresh signed confirmation; host tests cover removal, home
  movement, the isolated member, rejoin at a new epoch and suspect recovery;
- `mesh-route` fences cached routes and leases that named a removed member, refuses a
  stale frame with `stale-epoch`, makes a commit require a current authority and a fresh
  write lease, refuses an isolated home's commit with `no-authority`, renews only the
  root's read leases and reports `not-current` after expiry; host tests cover each path
  and the write-lease/failure-window margin;
- the module logs `mesh-fail:` transitions and serves the extended `/mesh/route` report;
- `--fault kill-peer` and `--fault partition` assert the transition, the recomputed homes,
  the fence, the stale-frame refusal and the mutable-object rule on two real kernels, and
  the partition mode heals and requires a rejoin at epoch 4.

The M2 gate is now covered end to end by this run: join, membership epoch, rendezvous
homes, remote capability route, L0 migration both ways and one-node failure/partition.
The run does not claim live TLS/socket/fd migration, L2 safepoint migration, a general
actor scheduler, gossip, a persisted epoch, a persistent route/migration directory or
linearisable writes across a partition.

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
- If the failure mode stalls, read the `mesh-fail:` lines on both nodes. No `suspect`
  line means the detector never ticked (the module is not running); `suspect` but no
  `removed` means the root side did not confirm the new epoch. If step 7 reports `replay`
  instead of `stale-epoch`, the saved frame came from the current epoch, so the failure
  happened before the M2-4 sequence completed.
- If the homes check fails, fetch the endpoint by hand:
  `curl -sS http://127.0.0.1:9701/mesh/homes` and `...:9711/mesh/homes`. A
  `state=pending` member has not installed a confirmation; a `404 disabled` means
  `mesh.peer` was absent; a `state=stale` response is the expected answer for
  `?epoch=1` after the epoch advanced. Compare the `mesh-home:` rows with
  `summary.md`; the rig's independent table is in `run.json` under `homes`.
