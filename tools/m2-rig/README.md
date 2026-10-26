# m2-rig — two independent IRIX 7 kernels

`m2-rig` starts **two independent `iris` processes**, each with its own state directory,
NVRAM, MAC, NAT subnet, CI socket, monitor and serial log. It waits for both kernels to
print `BOOT-READY`, proves a two-way HTTP exchange through a Rust host relay, connects the
two guest mesh listeners so the kernels exchange framed `MeshEnvelope` records, and drives
the M2-2 signed join until both nodes report the same membership epoch and snapshot. It
then fetches the M2-3 rendezvous home table from both kernels, checks the stale-epoch
negative, drives the M2-4 remote capability route and its one-`MOVED` retry in both
directions, restarts the member and requires it to rejoin at the same epoch with the same
table, and then drives the M2-5 L0 actor migration n0→n1→n0 with the same canonical state
hash at each boundary. `--fault kill-peer` and `--fault partition` replace the
restart with the M2-6 failure scenario: a bounded suspect/failed/removal transition,
locally recomputed homes, lease fencing and the mutable-object partition rule.

The `shard` command is the first M3 slice: two real kernels converge on the same
rendezvous table and serve `/mesh/shard`, while the 1/2/8/128-node shard policy is a host
gate.  Eight live kernels are not claimed; see the M3 runbook.

- Runbook: [`../../docs/plan/m2-rig.md`](../../docs/plan/m2-rig.md)
- Decisions: [`../../docs/adr/README.md`](../../docs/adr/README.md)
  ([M2-ADR-0005](../../docs/adr/m2-0005-join-epoch.md) is the join/epoch design,
  [M2-ADR-0006](../../docs/adr/m2-0006-homes.md) is the homes/evidence design and
  [M2-ADR-0007](../../docs/adr/m2-0007-route.md) is the route/`MOVED` design and
  [M2-ADR-0008](../../docs/adr/m2-0008-failure-fence.md) is the failure/fence/partition
  design and [M2-ADR-0009](../../docs/adr/m2-0009-actor-migration.md) is the L0 actor
  migration design, and [M3-ADR-0001](../../docs/adr/m3-0001-shard-slice.md) is the first
  shard slice and its real-kernel limit)
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
| `run` | Start both, wait for `BOOT-READY`, probe HTTP, mesh framing, join and homes, drive the five route steps, restart the member (default), drive the eleven migration steps, stop, write evidence. In `kill-peer`/`partition` it runs the failure scenario instead of the restart and does not drive migration. |
| `shard` | Start `--nodes` kernels (two by default), join them through the M2 relay, verify `/mesh/shard`, `/mesh/homes`, the stale-epoch negative and one remote stat, stop, write the M3 evidence bundle. `--hub` enables the experimental multi-node link switch; see [`../../docs/plan/m3-rig.md`](../../docs/plan/m3-rig.md). |
| `up` | Start both and leave them running. |
| `status` | Show CI state and the serial boot/join/home markers. |
| `probe` | Probe HTTP, mesh framing, join, homes and the route against running nodes; no restart. |
| `down` | Stop both through their CI sockets. |
| `print-config` | Print the derived instance, port and NVRAM table. |
| `clean` | Remove the work directory (stop the nodes first). |

Options: `--work DIR`, `--repo DIR`, `--iris PATH`, `--iris-ci PATH`, `--rom PATH`,
`--module PATH`, `--base-port N`, `--nodes N`, `--hub`, `--timeout SECS`,
`--fault MODE`, `--keep`, `--verbose`.  Host forward ports now start at `base+1000` so
the eight-node monitor block (`base+600`) cannot collide.

`--fault` selects a negative-evidence mode:

| Mode | What changes | Expected result |
|---|---|---|
| `none` (default) | Full `mesh.*` configuration with `mesh.route=2`. | The join converges at epoch 2; both kernels serve the same homes; the member restarts and rejoins at the same epoch with the same table; `?epoch=1` is `409 stale` with no rows; the actor moves n0→n1→n0 with the same state hash, the queued message resumes, stale old homes are refused and duplicates are idempotent. |
| `no-peer` | No `mesh.*` key at all. | No mesh listener and no join; `/mesh/homes` is `404 disabled`; the HTTP probe passes and the M1 `mesh=single` / `mesh=0/0/0` markers are preserved. |
| `wrong-authority` | n1 has a valid but wrong `mesh.pub`. | n0 signs; n1 logs `mesh-join: reject reason=bad-signature`, keeps its seed view and serves `409 pending` with no homes. |
| `no-route` | The join/homes keys are written, but `mesh.route` is absent. | The join and homes pass exactly as in M2-3; both nodes log `mesh-route: disabled reason=config` and `mesh-migrate: disabled reason=config`, `/mesh/route` and `/mesh/migrate` are `404 disabled`, and no route step, `MOVED` or migration runs. |
| `kill-peer` | The full route configuration (`mesh.route=1`); n1 is killed with `SIGKILL` after steps 1–5. | n0 removes n1 after `suspect` (6 s) and `failed` (12 s), moves to epoch 3 with one member, recomputes every home, fences the stale routes and leases, refuses a replayed stale frame with `stale-epoch`, and commits and reads locally with no quorum and no timeout. |
| `partition` | The full route configuration (`mesh.route=1`); only the mesh relay is cut, so both kernels stay alive. | The root follows the same removal. The member keeps epoch 2 and two members, reports `authority state=lost mutable=0`, refuses a commit with `no-authority`, reports a remote home as `no-peer` and serves a read from its unexpired lease. Restoring the seam rejoins it at epoch 4 with the two-member table and a successful commit. |

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
- A real rendezvous mapping: both kernels compute the same object-to-home rows from the
  confirmed snapshot with `mesh-core`; the rig recomputes the fixture IDs, winners and
  digest independently, compares the MCP body with the serial group, and checks the same
  table before and after the member rejoin.  The after-rejoin check reads the serial
  group because the iris host forward can retain a half-open guest connection across the
  restart; the MCP body is checked before the restart.
- A stale-epoch negative: `GET /mesh/homes?epoch=1` after convergence is `409` with
  `state=stale` and no object rows on both kernels.
- A real remote capability route: n0 issues a `cap-engine` capability for the first
  n1-homed fixture object, n1 verifies it and serves the stat, and the rig recomputes the
  value.
- A bounded stale-home path: n0 signs a root-authorised move of that object from n1 to n0,
  n1 enters `DRAINING` and answers `MOVED` with the next home epoch, and n0 retries exactly
  once locally. The reverse move (n0 to n1) produces a local `MOVED` and a remote retry.
- The route report: both kernels must carry the join-confirmed epoch, member count and
  digest; the three result rows must name the expected object, home, epoch, attempt count,
  `moved` flag and independently recomputed value; `rejects=0` and `timeouts=0`.
- A real L0 migration at a message boundary: one native stackless actor is seeded at n0
  with message 11 queued and message 10 unacknowledged, moved n0→n1, resumed, duplicate-
  delivered, queued again, moved back n1→n0 and resumed. Both nodes must report the same
  actor id, home, queued/unacknowledged rows, capability handle and canonical state hash at
  each move boundary. The old root and old member each refuse a stale delivery; a tampered
  capability is refused. The rig recomputes the actor identity, message chain and hash
  independently.
- A real bounded failure transition: any accepted frame from the peer refreshes liveness;
  6 s of silence is suspect (still routable), 12 s is failed. The root removes the member
  with one epoch advance and the whole remaining member list.
- A real lease fence: cached routes and leases that named the removed member are dropped
  and recomputed locally, with a `fence` row naming the stale and new home and epochs. The
  survivor's next read is local, one attempt, no send and no timeout. The old epoch's
  replayed frame is refused with `stale-epoch`.
- A real partition rule: the isolated member never removes the root, never advances the
  epoch and never re-homes an object. A commit fails closed with `no-authority`; a read is
  served only from the unexpired home lease. The 6 s write lease is shorter than the 12 s
  failure window, so the old home stops committing before the root can re-home the object.
- Fail-closed alternatives: `no-peer` keeps the M1/M2-0 path and has no homes or route
  endpoint, `wrong-authority` proves the member rejects an unauthenticated epoch and
  presents no unconfirmed homes, and `no-route` proves the route opt-in without weakening
  the M2-1/M2-2/M2-3 markers.

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
| `mesh.obj` | Number of home-fixture objects, 1–32 (rig default 8; omitted when default). |
| `mesh.route` | `1` enables the M2-4 remote capability route; `2` enables the route and the M2-5 L0 migration; absent preserves M2-3. |
| `mesh.pub` | Root's Ed25519 public key, on the member. |
| `mesh.key` | Root's Ed25519 seed, on the root only; a lab fixture, redacted in evidence. |
| `console` | `serial0`. |

The DS1386 environment has 298 bytes before the MAC slot, so the root stores only the
seed and derives its public key, and the rig omits `mesh.obj` at its default of eight so
the `mesh.route=1` opt-in fits. `--fault no-peer` omits every `mesh.*` key.

## Tests

```sh
cargo test --manifest-path tools/m2-rig/Cargo.toml
cargo clippy --manifest-path tools/m2-rig/Cargo.toml --all-targets -- -D warnings
```

The unit tests cover SHA-256, the DS1386 environment layout and bound (including the full
two-node mesh environment), the deterministic authority key, instance-report parsing, the
HTTP relay, the mesh relay's first-byte guard, the fixture, the probe transcript parser,
the join-line parser, the independent snapshot digest, the join/restart oracle, the homes
codec, independent fixture/rendezvous derivation, stale and pending bodies, before/after
comparison and JSON escaping. The route report parser, independent value oracle and
five-step driver are in `src/route.rs`; the migration report parser, independent actor/
message/hash oracle and eleven-step driver are in `src/migrate.rs`; the failure and
partition scenario, expected views and checks are in `src/failure.rs`. The
codec/authority/state-machine tests are in `crates/mesh-join`, the placement/mismatch tests
are in `crates/mesh-core`, the capability/lease/`MOVED` tests are in `crates/mesh-route`,
and the actor state/migration/duplicate/capability tests are in `crates/mesh-migrate`. The end-to-end test is the
`run` command above; it needs iris and the built artefacts.

## Layout

| File | Role |
|---|---|
| `src/main.rs` | Commands, probes, member restart, evidence wiring. |
| `src/rig.rs` | Instance derivation, NVRAM, process control, serial facts. |
| `src/join.rs` | Join/epoch oracle, independent snapshot digest, restart checks. |
| `src/homes.rs` | Home-table codec, independent fixture/rendezvous oracle and checks. |
| `src/route.rs` | Route report codec, independent value oracle and five-step driver. |
| `src/migrate.rs` | Migration report codec, independent actor/hash oracle and eleven-step driver. |
| `src/failure.rs` | M2-6 kill-peer/partition scenario, expected views and checks. |
| `src/relay.rs` | HTTP relay and the persistent mesh relay. |
| `src/evidence.rs` | `summary.md` and `run.json`. |
| `src/fixture.rs` | Loopback HTTP module fixture. |
| `src/nvram.rs` | DS1386 image layout. |
| `src/probe.rs` | Telnet/HTTP transcript probe. |
| `src/sha256.rs` | Dependency-free SHA-256. |
| `build.sh` | PROM and MCP module build. |
