# Closed-issue audit: mesh, network and modules

**Status:** audit report (read-only; no production change)
**Date:** 2026-11-02
**Issue:** [irix7/os#330](https://github.com/irix7/os/issues/330)
**Worktree:** `audit-mesh`, branch `audit-mesh`, base `cac1a5a` (`origin/main`)
**Scope owner:** mesh slice of the 39 closed issues

## 1. Scope and method

### 1.1 Issues audited

Six closed issues are audited here:

| # | Title | Family |
|---|---|---|
| 3 | M2 — Two-node join and migration | M2 gate |
| 245 | net: direct peer transport for the mesh | M2 transport |
| 246 | net: multiplexed mesh streams | M2 transport |
| 247 | net: larger mesh buffers and batching | M2 transport |
| 248 | kernel: event-driven mesh I/O | M2 transport |
| 249 | net: aggregated and gossip membership | M3 membership |

The final partition of all 39 closed issues is core 16, mesh 6 (this report) and
platform 17. Core owns #1, #2, #33, #43, #115, #116, #117, #118, #119, #204,
\#205, #206, #213, #214, #217 and #271. Platform owns #266, #287, #288, #291,
\#295, #296, #301, #302, #305, #306, #309, #310, #313, #314, #317, #318 and
\#321. This report does not audit either set; two core verdicts are cross-checked
in Appendix A.

### 1.2 Method

For each issue I read the body, every acceptance criterion and the closure
comment; the merged PR body and merge commit; the relevant commits; the live
code, tests and CI job. The GitHub reads were
`gh issue list --repo irix7/os --state closed --limit 1000`, `gh issue view N`,
`gh pr view N` and `gh api repos/irix7/os/issues/N/timeline`; the history reads
were `git log --all --oneline --grep '#N'` and `git show`. I ran only targeted
tests:

```sh
# Toolchain was reached through the repository devshell because `cargo` is not on
# the default PATH in this environment.
nix develop --command cargo test -p mesh-join -p mesh-stream \
  -p mesh-transport --offline
# 51 + 6 + 21 = 78 tests passed.

nix develop --command cargo test -p mesh-route -p mesh-service -p mesh-core \
  -p mesh-migrate -p mesh-sim -p fabric --offline
# All suites passed. crates/mesh-route/tests/trace_propagation.rs: 2 passed.

nix develop --command cargo test --manifest-path tools/m2-rig/Cargo.toml --offline
# 80 passed.
```

No two-kernel run was repeated: this audit had no `iris` binary or guest images,
and the existing CI evidence is identified per issue. I read the CI definitions
rather than inferring coverage.

### 1.3 Verdict vocabulary

- **implemented** — every acceptance criterion is met by code and evidence, with
  any limits explicitly recorded.
- **partial** — the main behaviour exists, but at least one acceptance criterion
  is unmet, unverified, or met only by a model/fixture rather than the stated
  target.
- **divergent** — the implementation has a different shape from the issue's
  stated design, even if some outcomes improve.
- **contract-only** — the behaviour exists in specs/types but not in a live path.

Evidence grade is one of: **real two-kernel gate** (CI or a recorded two-`iris`
run), **real multi-kernel** (more than two, with topology recorded), **unit
test**, or **docs only**.

## 2. Summary

| # | Title | Verdict | Foundation risk | Follow-up |
|---|---|---|---|---|
| 3 | M2 — two-node join and migration | partial | high | Amend the one-node-failure criterion or add root-failure recovery; provision the epoch store; record fixture limits |
| 245 | Direct peer transport | partial | medium | Membership-checked peer identity; direct mode in CI |
| 246 | Multiplexed mesh streams | partial | high | Wire the stream scheduler into the live path; CI cancellation/control bundle; test `mesh-stream` in CI |
| 247 | Larger buffers and batching | partial | medium | Correct the 16 KiB/32 KiB record; reconcile the 4.52x and 2.81x runs; make the benchmark reproducible |
| 248 | Event-driven mesh I/O | divergent | high | Measure the cold path; add the Seeq RX interrupt; stop calling readiness polling event-driven |
| 249 | Aggregated and gossip membership | partial | high | #264 is the remainder; reopen #249 or record an explicit amendment; direct four-node CI |

The single highest-risk pattern is not a broken feature: it is a **closure
pattern**. #249 was closed with "this issue is not complete" in its own body and
\#264 open carrying the remainder. #246 was closed with its cancellation bundle
recorded as an open evidence item. #248 was closed as "event-driven" when the
mechanism is a readiness poll with a hot-path shortcut. These are status-integrity
gaps that will mislead M3/M4 planning more than any single missing test.

## 3. Per-issue evidence

### #3 — M2 two-node join and migration

**Verdict: partial. Foundation risk: high. Evidence: real two-kernel gate.**

What is implemented:

- Two `iris` kernels join, converge on one signed epoch and digest, compute the
  same rendezvous homes, route a remote capability with exactly one `MOVED`
  retry, migrate a stackless actor both ways, remove a failed member and refuse
  a mutable commit on the isolated side of a partition. The CI job runs
  `none`, `no-peer`, `kill-peer`, `partition` and `udp` modes
  (`.github/workflows/ci.yml:1435-1525`).
- The epoch store is real, not a host shadow: `EpochStore`, `read_epoch_store`,
  `write_epoch_store` and `restore_epoch_store` are in
  `modules/mcp/src/mesh.rs:4158-4365`; the write is read back and compared
  before success (`mesh.rs:4222-4265`). Member and root restart are in the
  default rig scenario (`docs/research/m2-remediation.md:80-100`).
- Acceptance item 8 now has host conformance tests added by PR #244:
  cancellation-before-completion, completion-before-cancellation, interleaved
  races, queue backpressure, stale completion after tag reuse and 10,000
  distinct/outstanding tags (`crates/fabric/tests/conformance.rs:27,57,79,135,167,190,217`),
  slow-consumer flow control over the real `SendQueue`/`StreamDecoder`
  (`crates/fabric/tests/flow_control.rs:119,161`), and a frame-path trace test
  across two in-process nodes (`crates/mesh-route/tests/trace_propagation.rs:198`).
  The live module still has an 8-entry pending table
  (`modules/mcp/src/mesh.rs:126`); the 10,000-tag result is explicitly a fabric
  model result, not a live-module capacity.

What is missing or narrower than the issue:

- **Only member failure is exercised.** `tools/m2-rig/src/failure.rs:501-517`
  calls `kill_peer` and the comment says "stopping the member". There is no
  scenario that fails the root. On the survivor, `AuthorityLost` sets
  `mutable=0` (`modules/mcp/src/mesh.rs:1617-1624`): the data path is safe, but
  the failed node is not removed and its homes are not re-homed. Issue #3 says
  "one-node failure" without qualifying which node; the M2 gate is one-node
  failure only for the member.
- **The epoch store depends on a provisioned scratch target.** The store is SCSI
  target 3 (`mesh.rs:143-155`), and the code itself says "A production image
  must provision the same reservation before persistence is claimed"
  (`mesh.rs:145-149`). A boot without that target logs "persistence not
  configured" and falls back to the seed (`mesh.rs:4279-4290`). The restart
  criterion is met in the rig, not by a released image.
- **The `MOVED` path is a lab fixture.** The root-signed `HOME_MOVE` in
  `crates/mesh-route` is a controlled one-object move, not a general
  membership-change home-migration policy; the milestone text says so
  (`docs/project/milestones.md:198-215`).
- **The actor is a deterministic native fixture**, not a general actor runtime
  or a Wasm actor. The issue permits an L0 proof, but M3 cannot treat this as an
  actor scheduler.
- **The single root is a single point of failure.** A member restart is
  recovered; a root crash requires the root to come back with its store. No
  other node can confirm an epoch. This is documented as a 1-of-1 lab authority
  limit, but it is not in the #3 acceptance text or in the closure comment's
  short list.

Evidence grade: real two-kernel CI, plus host fabric tests. The M2 behaviours
listed in `docs/internals/system.md:216` are met; the unqualified "one-node failure"
and the persistence provisioning are not.

### #245 — direct peer transport

**Verdict: partial. Foundation risk: medium. Evidence: real two-kernel manual
run; not CI.**

What is implemented:

- ABI `tcp_connect(slot, address, port)` and a shared module socket factory;
  the lower `NodeId` initiates, with 3 s settle, five 500 ms attempts, then a
  logged listener fallback and bounded reconnect
  (`modules/mcp/src/mesh.rs:1681-1712`, `docs/research/direct-peer.md:10-19`).
- The direct path carries join, homes, one wire `MOVED`, L0 migration both ways,
  root restart and bulk on two real `iris` kernels with no relay
  (`docs/research/direct-peer.md:21-40`). n1 (lower id) initiated; n0 listened;
  `connect_ms=21`.
- `--transport relay|direct|direct-fail` exists and `direct-fail` observes
  `mesh-transport: direct fallback listen`
  (`tools/m2-rig/src/main.rs:85-90,137`).

What is missing:

- **Peer identity is configuration, not confirmed membership.** The address and
  peer id come from `mesh.direct`/`mesh.peer_id` in NVRAM
  (`mesh.rs:366-380`). More importantly, `learn_peer` accepts any source
  `NodeId` on the link and makes the first source the route and migration peer
  without checking it against the joined membership (`mesh.rs:3017-3049`). The
  issue's criterion "membership remains the source of peer identity" is not
  enforced on the data path.
- **Direct is not in CI.** The two-kernel job runs relay modes only
  (`.github/workflows/ci.yml:1516-1525`), and the rig restricts direct to
  `run --fault none/no-route` (`tools/m2-rig/src/main.rs:85-90`). The evidence
  is a manual PR run; nothing prevents it rotting.
- One listener and one connection per peer. The PR says a full peer mesh above
  two nodes needs #246/#248 or the hub. After the initial connect attempts fail,
  the node listens and does not retry direct until a link is established and
  then lost; that is a reasonable lab policy but should be documented as such.

Plain TCP with no TLS or peer authentication is recorded as a lab boundary in
`docs/research/direct-peer.md:42-49`. That is allowed by the issue, but it means
the source `NodeId` is self-asserted. For a two-node lab this is acceptable;
for the "one logical computer" claim it is a boundary, not a solved identity.

### #246 — multiplexed mesh streams

**Verdict: partial. Foundation risk: high. Evidence: real two-kernel credit
run; cancellation bundle not evidenced.**

What is implemented:

- `crates/mesh-stream` has a 16-entry table, initiator/responder parity, 64 KiB
  credit and queue bounds, close/reset and a control-first round-robin
  scheduler (`crates/mesh-stream/src/lib.rs:25-32,329-373`). Its six unit tests
  pass in this audit.
- The live module uses stream 5 for bulk with an initial grant of two chunks
  and `WindowUpdate` grants after each consumed pair
  (`modules/mcp/src/mesh.rs:116-117,3152-3183,3317-3328`), and stream 5
  `Cancel` resets the receiver (`mesh.rs:682-697,3186-3196`).
- A real direct two-kernel credit run transferred 64 KiB in 16 × 4 KiB chunks
  at 201-213 ms against a 768 ms 1 KiB baseline (PR #258).

What is missing or divergent:

- **The live path does not use the stream scheduler.** The module imports only
  `Side, StreamTable` (`mesh.rs:61`), inserts streams 5 and 6 at start-up
  (`mesh.rs:479-485`), and never calls `Scheduler`, `open`, `close_local`,
  `reserve` or `release`. Stream 6 is inserted but latency still uses
  `StreamId(1)` via `queue_ping`. The stream IDs are fixed labels for control,
  join, route, migrate, bulk and latency (`mesh.rs:2285,2313,3002,3074`), not a
  general multiplexing layer.
- **Only stream 5 has a window.** Control, join, route and migration frames do
  not use per-stream credit. Control priority comes from the `SendQueue`
  control reserve (`crates/mesh-transport/src/lib.rs:507-508,558-565`) and from
  the call order in `tick`, not from the stream scheduler.
- **Cancellation/control-during-bulk is not evidenced end to end.** The PR
  closure comment says "the control-during-bulk/cancel end-to-end bundle
  remains an open evidence item, not a pass". The rig code can run it
  (`tools/m2-rig/src/bulk.rs:288-380`), but the CI job does not invoke `bulk`.
- **`mesh-stream` is not in the CI test list.** `.github/workflows/ci.yml:92-100`
  includes `mesh-transport`, `mesh-migrate` and `fabric`, but not
  `mesh-stream`. Its six tests run only when someone runs them by hand.

The outcome is a working single bulk stream with credit and reset, not the
"one connection carries interleaved control RPCs and bulk data with per-stream
flow control and fair scheduling" the issue describes. The send queue does
prevent bulk from occupying the control reserve, so the starvation outcome is
partly achieved by a different mechanism.

### #247 — larger mesh buffers and batching

**Verdict: partial. Foundation risk: medium. Evidence: real two-kernel manual
bulk run; host model.**

What is implemented:

- The Seeq advertised window is clamped to seven segments
  (`kernel/src/net.rs:255-265`), the module socket buffer is 16 KiB per
  direction (`kernel/src/module.rs:739-748`), and the receive copy is one
  `copy_nonoverlapping` rather than a per-byte volatile loop (documented in
  `docs/research/bulk-batching.md:18-24`).
- `SendQueue` is a fixed-capacity coalescing queue: records are appended and
  flushed with one sink write over the contiguous buffer
  (`crates/mesh-transport/src/lib.rs:500-643`), with an 8 KiB control reserve.
- A 64 KiB logical record is streamed as eight chunks over the existing
  offset/length/MORE/EOS fields, and the rig's `bulk` command verifies the
  received byte count and chunk count
  (`docs/research/bulk-batching.md:55-82`, `tools/m2-rig/src/bulk.rs:340-410`).

What is missing or overstated:

- **The buffer is 16 KiB, not the 32 KiB claimed in the PR and issue comment.**
  `kernel/src/module.rs:748` is `MODULE_TCP_BUFFER = 16384`. The PR body says
  "module TCP RX 32 KiB / TX 32 KiB"; the merged code and
  `docs/research/bulk-batching.md:22` say 16 KiB. The acceptance asks for an
  increase beyond 2 KiB/16 KiB: RX is 8× larger, TX is unchanged.
- **The 4× acceptance is not reproducible.** The recorded run is 4.52×
  (0.072 → 0.326 MiB/s), but the same document records a later run at 2.81×
  under load average 6.75 (`docs/research/bulk-batching.md:84-86`). A single
  4.52× run does not establish "at least 4×" as a property of the path.
- **The benchmark is manual.** `bulk` is not in CI, so the throughput and the
  64 KiB completion are not regression-gated.
- Buffers are compile-time constants, not runtime-configurable, and there is no
  per-peer fairness because the live link is single-peer. The issue's "document
  memory ceilings and per-peer fairness" is only partly answered.

The bulk path is a benchmark opcode, not a general data plane; the document says
so. The implementation is useful and the coalescing is real; the quantitative
claim and the 32 KiB/16 KiB inconsistency are the reasons for `partial`.

### #248 — event-driven mesh I/O

**Verdict: divergent. Foundation risk: high. Evidence: real two-kernel manual
latency run.**

What is implemented:

- ABI 14 `tcp_events(slot)` returns a level-triggered mask recomputed from the
  live socket: RX, TX-space, CLOSED, LISTEN and ESTABLISHED
  (`crates/tcp-slots/src/lib.rs:167-188`). There is no event queue.
- `module::poll` checks every slot, runs ticks immediately when a slot is
  ready, and otherwise keeps the 10 ms gate
  (`kernel/src/module.rs:221-273`).
- The idle loop parks in `wait`; after a ready socket or recent module work it
  parks for about 0.5 ms instead (`kernel/src/main.rs:738-745`,
  `kernel/src/idle.rs:58-66`).
- A real two-kernel `/mesh/latency` run reports p50 2 ms, p95 3 ms, min 1 ms,
  max 7 ms against a 30 ms p50 for the tick-only path (PR #259).

Why this is divergent, not implemented:

- **There is no event primitive and no RX interrupt.** `tcp_events` is a
  readiness query called from the idle loop, not a wakeup source. The issue
  asked for "the smallest kernel/module event or async primitive" and "wake
  mesh work on network/ring events rather than only the idle tick". This is a
  poll of socket state, with a shorter wait when the poll says ready.
- **The cold path is still tick-bound.** After 50 ms without module work, the
  loop uses the normal 10 ms `wait`. A first request that arrives just after
  the wait begins is not observed until the timer fires. The p50 2 ms is a hot
  burst: `/mesh/latency` sends every 20 ms (`modules/mcp/src/mesh.rs:3149`) and
  `recently_active` holds the short-wait mode for 50 ms
  (`kernel/src/module.rs:239-243`). The probe traffic itself keeps the node in
  the fast path. A single control RPC after idle is not measured and can be up
  to 10 ms.
- **No p99 and no idle-CPU measurement is in the tree.** The issue asks for
  p50/p99 and idle CPU; the recorded run is p50/p95. The PR says idle CPU is
  unchanged, but there is no checked-in measurement.
- The acceptance says the result must be "not merely a 1 ms poll". A 0.5 ms
  observation loop is not better in kind; it is a poll with a shorter period
  while active.

The latency improvement on a hot link is real. The "event-driven" description
and the cold-path guarantee are not.

### #249 — aggregated and gossip membership

**Verdict: partial. Foundation risk: high. Evidence: real four-kernel manual
run over the host hub; host model.**

What is implemented:

- The join library now has `MemberSummary` (unsigned) and a root-signed
  `EpochSummary` binding every member `(NodeId, incarnation)` to one epoch and
  digest; the root publishes one aggregate per change set
  (`crates/mesh-join/src/gossip.rs:249-520`, `crates/mesh-join/src/node.rs:714-790`).
- The live module sends a summary to the root every refresh and the root sends
  one aggregate to each known peer (`modules/mcp/src/mesh.rs:1852-1860,2504-2525`).
- Batch removal advances the epoch once per change set; the failure detector is
  the `mesh-service` state machine; root restart restores the signed epoch
  (`modules/mcp/src/mesh.rs:1604,4266-4365`).
- The host model covers 2/8/128 nodes with deterministic digests and bounded
  per-node records (`crates/mesh-join/src/model.rs:573-653`); 51 mesh-join tests
  passed in this audit.
- Four real `iris` kernels converged on one epoch over the bounded host hub;
  the full shard/route bundle was recorded for three because the fourth member's
  HTTP slot stalled (`docs/project/m3-rig.md:1-5,173-179`).

What is missing or wrong with the closure:

- **The issue was closed while its own body says it is not complete.** The
  #249 body (fetched 2026-11-02) says: "This issue is not complete. The
  membership layer it asks for remains: per-link mesh state (the module still
  has one decoder/queue/join link), signed summaries and bounded gossip, root
  aggregation, epoch persistence/floor, batched removals, and 4/8-node real
  evidence. That work is tracked in #264." #264 is still open and repeats
  "per-link mesh state... split them into a bounded link table" as item 1.
  Closing #249 did not close #264.
- **The live root still has one decoder, one send queue and one link.** The
  `Mesh` struct has a single `decoder` and `queue` for all sources
  (`mesh.rs:207-221`); the `peers` array records source IDs but does not give
  each peer its own join state, heartbeat or send path. The comment at
  `mesh.rs:211-215` says the M3 hub multiplexes member links onto the root's
  single mesh slot. This is a star through a host process, not per-peer links.
- **There is no live gossip fan-out.** The `Follower`/`Aggregator` fan-out code
  is exercised by the host model, but the module does not use it: it sends a
  member summary to the root and the root sends one aggregate to every peer
  (`mesh.rs:1852-1860,2504-2525`). That is O(N) records but O(N²) aggregate
  bytes per change set at the root, and it is root fan-out, not gossip.
- **The four-node evidence is hub-mediated and not in CI.** The hub is a host
  link switch that forwards whole envelopes by destination
  (`docs/project/m3-rig.md:100-140`); `--nodes 3..8` requires `--hub`
  (`docs/project/m3-rig.md:73`). It is not a directory, but it is a central
  process required for more than two nodes. Eight real kernels are explicitly
  not achieved (`docs/project/m3-rig.md:173-180`). There is no four-node CI job.
- **#264's item 3 is stale.** It lists "epoch persistence and floor" as
  remaining, but `restore_epoch_store` and the CI root-restart scenario already
  implement it (`modules/mcp/src/mesh.rs:4266-4365`,
  `docs/research/m2-remediation.md:80-100`). Leaving it open muddles the real
  remainder.

The protocol and library work is substantial; the closure is not. A reader of
\#249 would believe per-link state and gossip exist. They do not.

## 4. Cross-cutting findings

### 4.1 Membership is a single-root star, not gossip

`docs/internals/system.md:148-150` says membership uses "SWIM-style failure detection
and a bounded gossip fan-out". The failure detector is real. The gossip fan-out
is not in the live path: the member sends a summary to the root
(`mesh.rs:1852-1860`), the root signs one aggregate and sends a copy to every
peer (`mesh.rs:2504-2525`), and the module has one decoder and one queue for all
sources (`mesh.rs:207-221`). The four-kernel run depends on the host hub
(`docs/project/m3-rig.md:100-140`). A root failure is a single point of authority
failure; a member cannot take over because the root key is on the root only.

This is the largest gap between the M2/M3 narrative and the code. It is also the
one the project already knows about (#264), which makes the #249 closure the
process failure rather than the technical one.

### 4.2 "Event-driven" is hot-path readiness polling

`tcp_events` is a level-triggered mask recomputed from the socket
(`crates/tcp-slots/src/lib.rs:167-188`). `module::poll` calls it for every slot
on every idle-loop pass (`kernel/src/module.rs:221-273`). There is no event
queue, no interrupt and no async primitive. The idle loop shortens its wait to
0.5 ms only while a socket is ready or a module worked in the last 50 ms
(`kernel/src/main.rs:738-745`, `kernel/src/module.rs:239-243`). The 2 ms p50 is
measured by a probe that keeps the node in that window; the cold request remains
tick-bound. This does not satisfy "event-driven" or "not merely a 1 ms poll",
even though it improves a hot link.

### 4.3 Multiplexing is fixed stream labels plus one bulk window

The live module inserts streams 5 and 6 and never opens, closes, reserves or
schedules another stream (`mesh.rs:479-485`). All control traffic uses
`StreamId(1)`; join, route and migration use fixed IDs 2-4
(`mesh.rs:2285,2313,3002,3074`). Only stream 5 has credit and reset
(`mesh.rs:3152-3196`). The `Scheduler` in `crates/mesh-stream` is a reference
model, not a live scheduler. The send queue's control reserve is what keeps
bulk from starving control. This is a working benchmark path, not a general
stream layer.

### 4.4 Failure and epochs: member failure is real, root failure is not

The 6 s write lease is shorter than the 12 s failure window
(`mesh.rs:76-80`), which is the right safety ordering. The failure scenario
stops the member, not the root (`tools/m2-rig/src/failure.rs:501-517`). The
epoch store is real and verified, but it lives on a scratch SCSI target that a
production image must provision (`mesh.rs:143-155,4279-4290`). There is no
replicated epoch log. A root crash is a safe stop for mutable writes, not a
recovery path.

### 4.5 Capability issuance is root-only

`JoinSigner` signs with the authority key (`mesh.rs:4038-4060`), and
`mesh-route` builds a `TrustStore` with that one key
(`crates/mesh-route/src/lib.rs:881-895`) and issues capabilities with
`issuer: self.authority_key` (`mesh-route/src/lib.rs:1990,2021`). The
`cap-engine` threshold and home-certificate machinery exists at host-test level,
but the live mesh cannot verify a capability issued by a non-root member. The
M2 route test therefore proves a root-issued capability, not the system.md
model of delegated home certificates. This is a contract-only behaviour.

### 4.6 A node cannot forward: the two-hop rule is contract-only

The receive path rejects any frame whose destination is neither zero nor the
local node (`mesh.rs:2006-2009`). There is no forwarding branch. The M2 and
M3-hub paths are one hop (the hub is a link switch, not a mesh router). The
system.md invariant "at most two hops in the normal case"
(`docs/internals/system.md:53`) and the M4 gate are not implemented; the live design
is one logical link per node or a star. This should be stated as an M3/M4 design
gap, not assumed from the `MeshHeader.ttl` field.

### 4.7 Homes and routing are real; general home movement is not

Rendezvous homes are computed from the confirmed snapshot and agree across real
kernels (`crates/mesh-core/src/lib.rs:30,115-157`, M2-3 evidence). The route
cache, capability check and one `MOVED` retry are real (`crates/mesh-route`).
But the `MOVED` is driven by a controlled root-signed `HOME_MOVE` in the rig, not
by a membership change or a general `DRAINING` handover
(`docs/project/milestones.md:198-215`). Failure removal recomputes homes and fences
the dead node, which is a different path. The system.md `DRAINING`/forwarding
grace behaviour is a lab fixture.

### 4.8 One logical computer, but per-node control surfaces

Every mesh inspection and test control is a per-node HTTP endpoint:
`/mesh/homes`, `/mesh/shard`, `/mesh/route`, `/mesh/migrate`, `/mesh/latency`
and `/mesh/bulk` (`modules/mcp/src/main.rs:1047-1077`). `/mesh/shard` returns
only the local node's assignments (`modules/mcp/src/mesh.rs:918-930`), and the
rig drives each node separately. `mesh.reconnect` is a lab tool in the
production MCP tool list (`main.rs:1438,1533-1538`). There is no single
system-level mesh view, and the four-node topology requires the host hub. These
surfaces are appropriate for evidence, but they are a cluster-of-nodes
management pattern, not the "one logical computer" control plane. If they are
kept in the shipped MCP module, they should be labelled as lab surfaces and
excluded from the product API.

### 4.9 CI coverage does not match the claims

The two-kernel CI job runs relay join, no-peer, kill-peer, partition and UDP
(`.github/workflows/ci.yml:1516-1525`). It does not run:

- the direct transport (#245);
- the 64 KiB bulk benchmark (#247);
- control-during-bulk or cancel (#246);
- the latency probe (#248);
- a four-node cluster (#249).

The `Test` job also omits `mesh-stream` (`ci.yml:92-100`), so the six stream
tests are not gated. The 10,000-tag fabric tests are in `fabric`, which is in
the job. The `m2-rig` host tests are in the job. The result is that the
functional M2 gate is protected, while the transport-efficiency and
multi-node-membership claims decay unless someone reruns a manual command.

### 4.10 No fabricated real-kernel evidence, but closure language blurs model and target

I found no case of `mesh-sim` being presented as a real-kernel result. The M2
and M3 documents consistently label host models, and the two- and four-kernel
runs are real `iris` processes. The problem is the closure comments: #249 says
"complete" while #264 is open; #246 says "implemented" while its cancellation
bundle is an "open evidence item"; #248 says "event-driven" for a poll. Item 8
of #3 is host-model evidence for 10,000 tags, correctly labelled in the PR, but
easy to read as a live capacity. These are reporting defects, not fabricated
runs.

### 4.11 What the mesh guarantees today

- **Homes:** deterministic rendezvous over the confirmed membership; two real
  kernels agree; four kernels agree over the hub. Real.
- **Routing:** one remote capability invoke to the home with a bounded cache and
  one `MOVED` retry. Real, root-issued only, one hop.
- **Membership:** signed join and epoch confirmation, summaries, one root-signed
  aggregate per change set. Real, single root, star topology, no live gossip.
- **Failure:** member failure removal, home recomputation and lease fencing.
  Real on two kernels. Root failure is a safe stop, not recovery.
- **Epochs:** signed record on a provisioned scratch target, verified readback,
  member and root restart. Real in the rig, not provisioned by a release.
- **Streams:** one bulk stream with credit and reset; no general multiplexing.
- **Batching:** coalesced send queue and a 64 KiB logical bulk record; 4.52× in
  one run, 2.81× in another.
- **Contract-only:** gossip fan-out, root failover, threshold/delegated
  capability issuance, two-hop forwarding, general `DRAINING` home migration,
  CAS fetch-by-OID, NodeSummary placement and the global namespace. These are
  M3/M4 work, not M2 facts.

## 5. Recommended follow-ups

Ranked by risk to the one-logical-computer foundation.

| Rank | Action | Carrier | Why |
|---|---|---|---|
| 1 | Reopen #249 or record an explicit amendment that transfers its remaining acceptance to #264; make #264 the single membership tracker and correct its stale epoch-persistence item | #249 / #264 | The closed issue's own body says it is incomplete; the current tree cannot form a multi-node mesh without the host hub |
| 2 | Add per-link mesh state and live gossip fan-out, then a direct four-node CI job | #264 | One decoder/queue and root fan-out are the real M3 blocker; a host hub is a central process in the data path |
| 3 | Measure a cold RPC after more than 50 ms of idle; add the Seeq RX interrupt / `EXC_INT` path or rename the feature; record idle CPU and p99 | #248 (reopen) | The 2 ms p50 is a hot-burst result; the first request after idle is still tick-bound |
| 4 | Wire `mesh-stream` into every live stream, add the control-during-bulk/cancel two-kernel run to CI, and add `mesh-stream` to the `Test` job | #246 (reopen) | The stream scheduler is unused; the cancellation bundle is an admitted open item and the six tests are ungated |
| 5 | Amend the M2 acceptance to name member failure, or add root-failure recovery; provision the epoch store in the release image | #3 / M2-ADR-0003 / M2-ADR-0005 | The issue says "one-node failure" without qualification; only member failure is tested, and persistence needs a scratch target |
| 6 | Bind peer identity to the confirmed membership before `learn_peer`, and add a direct-mode CI run | #245 follow-up | `learn_peer` trusts the header source; direct is manual-only |
| 7 | Correct the buffer claim to 16 KiB, record the 2.81× run beside the 4.52× run, and decide whether the 4× target is a gate or an observation | #247 follow-up | The merged code contradicts the PR text and the quantitative criterion is not stable |

The first four are the foundation items; the last three are coverage and
measurement debt.

## Appendix A — Cross-check notes for core-owned issues

The core report owns both verdicts below. These notes record the evidence that
made the mesh audit question them, so the core owner can reconcile or keep the
current verdict. No competing verdict is asserted here.

### A.1 #115 — unified console

**Core verdict: implemented (authoritative).**

The implementation routes one byte stream to the graphics console, the MCP
telemetry ring, serial and the telnet queue
(`kernel/src/main.rs:184-195`); native `write(fd=1, ...)` goes through
`write_fd` and `write_bytes` (`kernel/src/user.rs:585-606`); the ring is exposed
through `console_read` and MCP `serial.read`
(`kernel/src/main.rs:138-180`, `modules/mcp/src/main.rs:1451,1695-1701`).

The evidence to reconcile is the acceptance test. CI boots `userelf` and checks
`hello from user` on serial (`.github/workflows/ci.yml:1215`), but it does not
assert the same bytes on telnet or through MCP, and it has no mid-line
interleaving or escape case. `docs/project/testing.md:172-175` records the sash as a
manual check on both transports. If the core verdict treats the routing as
sufficient, the missing cross-surface assertion is still worth recording as a
test gap rather than reopening #115.

### A.2 #213 — Wasm policy as the live policy

**Core verdict: partial (authoritative).**

The evidence for a live path is real: `adopt_wasm` installs the committed
bridge and adopts the guest actor (`kernel/src/logical.rs:127-149`), and
`read_file`/`file_size`/`device` route `/etc/motd` and `/dev/null` through the
guest when the bridge is present (`kernel/src/logical.rs:151-217`). CI waits for
`m1-live: wasm-policy adopted` before running the std image
(`.github/workflows/ci.yml:1070`).

The core objection is also visible in the same file: the bridge is optional,
`Local.root` and the native capability table remain native, and `forget_wasm`
drops the adopted policy when slot B is cleared or replaced, falling back to the
native policy (`kernel/src/logical.rs:115-125`). That is the distinction the
core verdict turns on: if the criterion is a general live authority rather than
a bridged read path, the native fallback and the reload drop are decisive. The
core owner should decide whether that distinction is in #213's acceptance or
only in the Phase 5 follow-up.
