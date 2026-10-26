# M2 completion audit — issue #3

**Status:** audit report (docs only, no production change)
**Date:** 2026-10-27
**Audited revision:** `6905740` (`origin/main`); M2 closure `f9029a3`; M2-6 `d3b8298`
**Issue:** <https://github.com/irix7/os/issues/3>; closure comment
<https://github.com/irix7/os/issues/3#issuecomment-6425534591>; M2-8 roster
<https://github.com/irix7/os/issues/3#issuecomment-6403421732>
**Scope:** issue #3 body, its closure comment and the M2-8 roster comment; issue #16;
`docs/spec/system.md` §5/§9; `docs/plan/milestones.md`; `docs/plan/m2-rig.md`;
`docs/plan/testing.md`; ADR m2-0001..0009; `crates/mesh-*`, `cap-engine`, `fabric`,
`sys-primitives`; `modules/mcp/src/mesh.rs`; `tools/m2-rig`; `.github/workflows/ci.yml`;
the evidence bundles under `/tmp/opencode`.

**Method:** read the implementation and tests; ran the host test suites; rebuilt the MCP
module from `f9029a3`; compared the rebuilt image with the image recorded in the M2-5
evidence; audited the CI job list and the evidence provenance; attempted a fresh
two-kernel `iris` run of the f9029a3 build. This is an adversarial audit: the claim was
treated as unproven until the code, tests or evidence forced it.

**Not done:** no real-Indy run (issue #16 is still open and #3 explicitly allows two
`iris` instances); no independent review of M3. The audit does not modify M2/M3 code.

---

## 1. Executive verdict

**Closing #3 was defensible for the six two-node behaviours the project actually built,
but not defensible for the issue as written.**

The positive case is real: two independent `iris` kernels boot with distinct MACs and
NodeIds, exchange signed join frames, converge on one epoch and one member digest, compute
the same rendezvous home table, complete a remote Ed25519-authenticated stat with one
`MOVED` retry, run one L0 actor n0→n1→n0 with equal source/destination state hashes, remove
a failed peer and fence its routes, and refuse a mutable commit on the isolated side of a
partition. Those are not `mesh-sim` results; they are serial transcripts from two kernels
with independently recomputed digests and hashes.

Three things prevent an unqualified "M2 complete":

1. **Issue #3's acceptance item 8 is not met at all.** There is no cancellation-race,
   flow-control-exhaustion, 10k-outstanding-tag or two-node trace-context test. The
   `fabric` crate is a standalone reference model that no production crate links; the
   live MCP module has an **8-entry** pending table, not 10k. The milestone doc's own
   Phase 3 exit still lists these items ([`docs/plan/milestones.md`](../plan/milestones.md)
   lines 246-247), and the closure comment does not defer them.
2. **The epoch is not persistent.** The member restart box is ticked, but the recorded
   rejoin request starts at `last_epoch=1`, not the confirmed epoch 2; it is the root's
   in-memory table that re-confirms epoch 2. A real root restart fails closed with
   `epoch-regression` and needs an operator to re-seed NVRAM. That is a documented limit
   in M2-ADR-0005, but it is not in the closure comment and it narrows "survive a restart
   with the same epoch".
3. **The final real-kernel migration evidence does not tie to the merged tree.** The M2-5
   bundle names commit `93fc663` and module `5458c108…` (344,328 bytes). The image
   contains a string that only exists in the merged `f9029a3` tree, a clean `f9029a3`
   build is 356,792 bytes (`b1bcc694…`), and a clean `93fc663` build lacks that string
   altogether. The run is probably genuine, but its own provenance metadata is
   inconsistent and the merged module was not re-run.

Two process facts reinforce the verdict. The serial M2 roster in issue #3 makes **M2-8
"two-node CI, final gate and review"** the last step; there is no two-node CI job in
`.github/workflows/ci.yml`, and no human review is recorded on PR #240 (only GitHub
Advanced Security comments). The final gate was declared without the step the project had
itself specified.

**Recommendation to the main agent:** do not treat M2 as closed. Either reopen #3 or file
an explicit follow-up that records (a) item 8 as deferred to Phase 3/M3 with a named
owner, (b) the non-persistent epoch as a known limit that excludes root restart, and
(c) a two-node `mesh-real` CI job before M3 work depends on M2. If the project's
definition of M2 is only the six behaviours in `docs/spec/system.md` §9, then amend the
issue's acceptance list to say so; the current text says more.

---

## 2. Criterion-by-criterion audit

Confidence is the auditor's confidence in the verdict, not in the project.

| # | Criterion (issue #3) | Verdict | Evidence and reasoning | Confidence |
|---|---|---|---|---|
| 1 | Two independent kernels run the same core and membership policy; one confirmed epoch | **Verified** | `tools/m2-rig` starts two `iris` processes with separate state dirs, MACs, NATs and NodeIds (`tools/m2-rig/src/rig.rs`). Evidence: `/tmp/opencode/wt-m2-migrate/target/m2-rig/evidence/summary.md` lines 15-21 (two NodeIds), 119-134 (both converge epoch 2, digest `2EDA13C5…`), serial lines 144-155. The rig recomputes the digest. Caveat: the confirmed view lives in the MCP module; the kernel still prints `mesh: single node`, so this is a module-backed kernel policy, not a changed kernel `MembershipView`. | High |
| 2 | Join/epoch confirmed by the authority root; views converge; survive a restart with the same epoch | **Partially verified** | Signed Ed25519 confirmation and convergence are verified (summary lines 119-155; `crates/mesh-join` has 35 host tests, with the verifier in `src/authority.rs`). The member restart rejoin is verified (summary lines 135-141). But the recovery is not survival: `run.json` `join.restart.request_last_epoch = 1` while the confirmed epoch is 2, so the member forgot the epoch and the root re-confirmed it from memory. Root restart is a host test only (`crates/mesh-join/src/node/tests.rs:368-389`) and fails closed with `epoch-regression`; there is no real-kernel root-restart evidence. The epoch is not persisted because the module ABI has no write service (M2-ADR-0005 §4). | High for join; Medium for "survive a restart" |
| 3 | Rendezvous homes agree, including after a membership change | **Verified** | Epoch 2: both kernels match the independently recomputed table before and after the member rejoin (summary lines 158-225; stale `?epoch=1` is `409` with zero rows). Membership change: `kill-peer` removes n1 at epoch 3 and recomputes the one-member table (`/tmp/opencode/m2-final-kill2/evidence/summary.md` lines 301-348); partition recovery re-adds n1 at epoch 4 and **both** kernels match the recomputed two-member table (`/tmp/opencode/m2-final-part4/evidence/summary.md` lines 366-373). | High |
| 4 | A capability issued on A routes to a home on B without a central directory; stale route returns `MOVED`, sender retries once | **Verified** | `/tmp/opencode/m2-rig-run4/evidence/summary.md` lines 228-299: step 1 is a remote stat at B; steps 3 and 5 consume one `MOVED` with exactly two attempts; `rejects=0`, `timeouts=0`; values recomputed. Homes come from `mesh_core::home_for` over the confirmed snapshot; no directory RPC exists (`crates/mesh-core/src/lib.rs:115-157`; `modules/mcp/src/mesh.rs:288-290`). Caveats: the issuer is always the authority root (member-issued capabilities cannot verify because the trust store is one key), and the `MOVED` is driven by a root-signed **lab `HOME_MOVE`** (M2-ADR-0007 §5), not by a real membership change; the M2-6 path fences instead of `MOVED`. | High for the literal criterion; Medium for generality |
| 5 | L0 stackless actor migration both ways at a message boundary, matching source/destination state hashes | **Partially verified** | The transcript is strong: n0→n1→n0, queued message 11 and unacknowledged message 10 cross the boundary, source and destination rows have the same hash at both moves (`246A7909…` then `C0C3BD12…`), duplicates and stale homes are refused (summary lines 326-402). The rig independently recomputes identity, rendezvous home, message chain and hash (`tools/m2-rig/src/migrate.rs:561-628`). But: (a) the bundle names commit `93fc663` while the image contains `IRIX7-M2-CAP-NONCE-v1`, which only exists in `f9029a3` (`crates/mesh-migrate/src/node.rs:1860`); (b) the recorded module `5458c108…`/344,328 bytes does not reproduce (this audit built `f9029a3` as `b1bcc694…`/356,792 and `93fc663` as `5c0d645c…`/344,056; the `93fc663` image contains no `CAP-NONCE` string); (c) the actor is a single deterministic fixture in the MCP module, not a general actor runtime; (d) no CI job runs `mesh-migrate` or the rig. The run is probably genuine, but the merged artefact was not the one evidenced. | Medium |
| 6 | One-node failure removes the node, moves homes, fences a stale home with a lease | **Verified** | `/tmp/opencode/m2-final-kill2/evidence/summary.md` lines 293-371: `suspect` at 6 s, `failed` at 12 s, `removed` at epoch 3 with one member; six `mesh-route: fence` rows name the stale home and the new home; a replayed lease frame is refused `stale-epoch`; the survivor commits and reads with `mutable=1` and no timeout. The failure path is real code (`crates/mesh-service/src/lib.rs:47-48`, `modules/mcp/src/mesh.rs` failure config). Caveat: the summary records commit `5ada151` (the M2-4 base), not the merged `d3b8298`; the image itself is genuine — this audit rebuilt d3b8298 and reproduced module `002e3077…`/286,456 bytes exactly. It predates M2-5, so the final f9029a3 module was not re-run for `kill-peer`. The M2-5 PR claims the negative modes still pass, but no bundle with the M2-5 module (`5458…`) exists for `kill-peer`/`partition`. | High |
| 7 | Partition makes a mutable object unavailable rather than split-brain; normal path does not wait for quorum | **Verified** | `/tmp/opencode/m2-final-part4/evidence/summary.md` lines 310-378: the isolated member logs `authority state=lost epoch=2 mutable=0`, refuses the commit with `no-authority`, and creates no new home or self-fence; the root removes it at epoch 3, commits with `mutable=1` and no timeout, then the member rejoins at epoch 4 and the two kernels agree. The safety argument is explicit: 6 s write lease vs 12 s failure window, worst case 11 s of writes (`crates/mesh-route/src/tests.rs:1256-1304`). Reads remain available under the unexpired home lease, which the criterion permits; linearisable write handover is explicitly left open (`docs/plan/milestones.md:243-244`). | High |
| 8 | Fabric tests cover cancellation races, flow-control exhaustion, 10k outstanding tags and trace-context propagation across two nodes | **Overclaimed / not met** | No such test exists. `crates/fabric` has 7 unit tests (`crates/fabric/src/lib.rs:460-545`): tag uniqueness, 2-tag exhaustion, stream windows, completion FIFO, cancel set, wait graph. It has no cancellation-vs-completion interleaving, no 10k tag test and no trace field. No production crate links `fabric` (`grep -rn "fabric" crates/*/Cargo.toml modules/*/Cargo.toml kernel/Cargo.toml` returns only fabric's own manifest). The live MCP module caps pending tags at **8** (`modules/mcp/src/mesh.rs:88`). `trace_id` is never set outside `sys-primitives` wire tests (`grep -rn trace_id crates/mesh-* modules/mcp` is empty). The only flow-control tests are the transport's `send_queue_rejects_oversize_work` and `would_block_keeps_the_queue_until_retry` (`crates/mesh-transport/src/lib.rs:1143-1203`); they are host-level, not two-node. | High |
| 9 | M1 still passes unchanged; M2 is a real-kernel run, not `mesh-sim` alone | **Verified** | M1: CI at `f9029a3` is green, including `Boot (iris)`; the boot job asserts `m1-post: mesh=0/0/0` and the `mesh=single` marker (`.github/workflows/ci.yml:440-447`). M2: the evidence is two real `iris` kernels, not `mesh-sim`, and the rig rejects a single-process fake. The literal criterion is met. Separate process gap: there is no two-node CI job (the CI job list is fmt/dco/clippy/test/tools/std-surface/cross/boot/modules/docs/lint, and `docs/plan/testing.md:67` still says "the rig is a manual command until the M2-8 CI job wraps it"), and the evidence is a mosaic of runs rather than one run of `f9029a3`. | High |
| 10 | No hidden dependency that narrows the completion claim | **Documented, but under-stated** | Host relay (both nodes listen; no node-to-node connect) — M2-ADR-0002 implementation note. 1-of-1 authority and root-issued capabilities — M2-ADR-0003, M2-ADR-0007 §2. Plain TCP, no TLS/peer auth/replay window — M2-ADR-0002 §6. Non-persistent epoch — M2-ADR-0005 §4, **not in the closure comment**. Fixture actor and native `mesh-migrate` not using `agnostic-core` — M2-ADR-0009 §2/§6. Combined MCP+mesh module and `mesh.route=2` opt-in — M2-ADR-0002 note, `modules/mcp/src/mesh.rs`. External `iris` 0.1.7+ binary, version not recorded in the bundle — `docs/plan/m2-rig.md:42-44`. | High |

### Smallest experiments that would close the remaining gaps

- **Item 2 (restart).** Restart n0 on the two-kernel rig while n1 keeps epoch 2. Expect
  `epoch-regression` and no rejoin. Then either add a persisted epoch store (a
  `config_set`-style module write or a module-owned record) or record the operator
  recovery in the runbook and amend the criterion to "member restart".
- **Item 5 (migration provenance).** `tools/m2-rig/build.sh` at `f9029a3`, then
  `tools/m2-rig run` with that module and PROM; compare the `## L0 actor migration`
  section, the two hashes and the source/destination rows. This is the one experiment the
  audit could not finish (see §5).
- **Item 8 (fabric conformance).** Four tests: (a) cancel vs completion interleaving;
  (b) send-queue exhaustion against a slow peer; (c) 10k outstanding tags (raise
  `PENDING_CAPACITY` in `modules/mcp/src/mesh.rs` first); (d) set `trace_id` on a request
  and assert it on the response across the two nodes. Until these exist, item 8 remains
  unmet regardless of the M2 narrative.
- **Item 9 (real-kernel CI).** Add a `mesh-real`/`m2-rig` CI job that boots two kernels,
  joins, routes, migrates and runs the `no-peer` M1 regression. One green run at the
  merged commit would replace the current mosaic.
- **Items 1, 3, 4, 6, 7.** Already evidenced on real kernels. The only residual is a
  rerun of item 6 with the final module, which the proposed CI job covers.

### What is explicitly excluded by #3 and must not be counted as a failure

- **Real-Indy hardware.** #3 says two `iris` instances are acceptable and prefers an Indy
  once #16 has a second machine; #16 is open and the lab is uncharacterised. The closure
  correctly records this limitation.
- **Live TLS/socket/fd state.** #3 and M2-ADR-0004/0009 explicitly exclude it. The
  migration carries only the actor state, one queued message, one unacknowledged message
  and one serialised capability handle.
- **L2 arbitrary-safepoint migration, a general actor scheduler, gossip/epoch push,
  linearisable write handover.** M2-ADR-0009 and the milestone status record these as
  later work.

### What is not excluded by #3 and therefore is a gap

- Item 8 (fabric conformance) — no deferral is recorded anywhere.
- The root-restart half of item 2 — the closure says "restart/rejoin at the same epoch"
  without saying "member only".
- A reproducible merged-tree run for item 5 — the M2-5 bundle's commit/module does not tie
  to `f9029a3` by hash, so the final migration claim rests on a transcript whose
  provenance needs correcting. Item 9's literal "real kernels, not `mesh-sim`" is still
  met; this is a provenance gap, not a `mesh-sim` substitution.

---

## 3. Highest-risk claims and what would falsify them

### 3.1 "M2 is complete" (item 8 is covered)

**Risk:** the closure treats the gate as done while one of its own acceptance items has no
implementation. A reader of `docs/spec/system.md` §9 would not notice, because that table
does not list fabric conformance; a reader of issue #3 would.

**Falsifier:** run the four required tests. They do not exist; the live pending table is
8 tags, so a 10k test cannot even be written against the running module without changing
`PENDING_CAPACITY`. The smallest closing experiment is: (a) add a two-node trace-context
test that sets `trace_id` on a request and asserts it on the response; (b) add a
cancel-vs-completion interleaving test; (c) add a flow-control exhaustion test that fills
the send queue against a slow peer; (d) raise the live pending capacity and run 10k
outstanding tags. Until then, the honest summary is "six of the issue's nine acceptance
items are fully met, two are partial and one is not met".

### 3.2 "Two real kernels migrated an actor at the merged commit"

**Risk:** the M2-5 evidence is the only real-kernel proof of the final feature, and its
recorded commit/module does not match the merged tree. A future bisect or release cannot
reproduce the gate from the bundle.

**Falsifier:** `tools/m2-rig/build.sh` at `f9029a3`, then `run`. This audit built the
module (`b1bcc694…`, 356,792 bytes) but the full rig run did not complete in the audit
environment (see §5); a clean run that reproduces the migration section would close the
gap, and a failure would falsify the claim. The bundle's own `IRIX7-M2-CAP-NONCE-v1`
string proves the image is not the `93fc663` tree it names; the hash/size mismatch shows
it does not reproduce from a clean `f9029a3` build on this host either.

### 3.3 "Join and epoch survive a restart"

**Risk:** the member restart is really a rejoin from the seed epoch, and a root restart is
an outage. An operator who restarts node 0 loses the epoch and cannot recover without
re-seeding both NVRAM images. This is the most likely production surprise hidden behind a
ticked box.

**Falsifier:** restart n0 on the real rig while n1 keeps epoch 2. The root starts at epoch
1 and refuses the member's `last_epoch=2` with `epoch-regression`; the member keeps its
view and no new membership change is confirmed. The host test already predicts this
(`crates/mesh-join/src/node/tests.rs:368-389`); the missing experiment is the real-kernel
one. Closing it needs a persisted epoch (a `config_set`/module-owned record) or an
explicit operator recovery procedure in the runbook.

---

## 4. Hidden dependencies and scope limits

These are real and mostly documented; they narrow the claim rather than falsify it. The
two that should appear in the closure text are marked **closure gap**.

| Dependency | Where it is recorded | Effect on the claim |
|---|---|---|
| Host relay `tools/m2-rig` | M2-ADR-0002 §7 and implementation note; `docs/plan/m2-rig.md` | Both guests listen; the host relay is the only path. The lower-NodeId-initiates rule is not exercised. The relay is a byte pump, not a directory, but it is a central process in the data path. |
| 1-of-1 authority root | M2-ADR-0003; M2-ADR-0005 | Membership changes have one failure domain. No threshold, no Byzantine safety, no key rotation/revocation. |
| Root-issued capabilities only | M2-ADR-0007 §2; `/tmp/opencode/m2-rig-run4/.../summary.md:274` ("member initiated no operation") | Only the root can issue a routable capability; the member cannot delegate. The data path has no directory but the authority is central. |
| Plain TCP, no TLS/peer auth | M2-ADR-0002 §6 | Any host attacker can observe/inject. Not a production transport claim. |
| Non-persistent epoch | M2-ADR-0005 §4 | **Closure gap.** Member restart survives by root re-confirmation; root restart fails closed and needs operator reseeding. |
| Fixture actor | M2-ADR-0009 §2/§6 | One deterministic actor in the MCP module, not a general actor runtime. The rig chooses hosts and messages. |
| `mesh-migrate` does not use `agnostic-core` | `crates/mesh-migrate/Cargo.toml` (no `agnostic-core` dependency) | The portable actor table/run queue are not on the migration path; the ADR's "agnostic-core remains the policy layer" is not literally true. |
| Combined MCP+mesh module | M2-ADR-0002 implementation note; `docs/spec/modules.md:153-156` | Mesh shares slot A with MCP and owns TCP slot 1; a future module needs to coordinate or replace the combined module. |
| `mesh.route=2` opt-in | M2-ADR-0009 §6 | Migration is disabled unless NVRAM sets it; a normal boot does not exercise it. |
| External `iris` 0.1.7+ | `docs/plan/m2-rig.md:42-44` | The bundle records the binary path but not the version/commit; the audit host had 0.1.10+irix7 (`8da7ae2`). Reproducibility depends on an unpinned emulator. |
| PROM headroom | Evidence PROM 523,296 bytes vs 524,288 | 992 bytes of headroom at the M2-5 path; this audit's build is 523,168. The kernel is unchanged, but the budget is fragile and path-dependent (the evidence PROMs differ across worktrees). |

---

## 5. What was re-run in this audit

Host tests (all pass):

```text
cargo test -p mesh-transport -p mesh-join -p mesh-core -p mesh-service -p mesh-route \
           -p mesh-migrate -p fabric
  mesh-transport 17, mesh-join 35, mesh-core 19, mesh-service 15, mesh-route 31,
  mesh-migrate 10, fabric 7

cargo test --manifest-path tools/m2-rig/Cargo.toml
  52 passed
```

CI state: `gh api repos/irix7/os/commits/f9029a3/check-runs` reports every check green,
including `Boot (iris)`, `Test`, `Clippy`, `Cross-build (M0 gate)` and `Modules`. Green CI
does **not** exercise the M2 path (see item 9).

Build comparison (this host, nightly-2026-10-03, `tools/m2-rig/build.sh` flags):

| Source | Module size | SHA-256 | Contains `IRIX7-M2-CAP-NONCE-v1`? |
|---|---|---|---|
| M2-5 evidence bundle (`wt-m2-migrate`, recorded commit 93fc663) | 344,328 | `5458c10831ad124ca7dd7e66ac3bd65cdda79815515faa00ae576b850156e5c1` | yes |
| clean `f9029a3` (this audit) | 356,792 | `b1bcc694f904b187fb72924ca50d6a959b03da63b1bce2d8ffe0cc2b3f59343d` | yes |
| clean `93fc663` (this audit) | 344,056 | `5c0d645cfcf6425e152ff5353f5bb6ac971e9bab495806c22fd54ed57582dc79` | no |
| M2-6 evidence bundle (`m2-failure`, recorded commit 5ada151) | 286,456 | `002e3077bdc90e65a2fd005bb0d845f72c56fd19e52a723a066f7272c9764ce2` | no |
| clean `d3b8298` (this audit) | 286,456 | `002e3077bdc90e65a2fd005bb0d845f72c56fd19e52a723a066f7272c9764ce2` | no |

The module hash is path-stable in this environment (the same tree at two path lengths
produced the same hash), and the M2-6 image reproduces **exactly** from a clean d3b8298
build. That makes the M2-5 mismatch sharper: the same build procedure reproduces one
feature bundle and not the other, so the 344,328-byte M2-5 image does not reproduce from a
clean f9029a3 build on this host.

**Provenance is systematically mislabelled.** `tools/m2-rig/src/main.rs:1643-1644` records
`git rev-parse HEAD` and no dirty-tree marker. Every feature bundle was produced before
the feature was committed, so the recorded commit is the *base* commit, not the tree that
was built: M2-4 records `6313024` (M2-3), M2-6 records `5ada151` (M2-4) and M2-5 records
`93fc663` (a pre-review commit). The serial transcripts and module hashes are the real
evidence; the commit field is not. This is a fixable process defect (record
`git describe --dirty` or the module hash only), but it means a reviewer cannot use the
bundle's commit field to reproduce the run.

Fresh two-kernel run: this audit built the PROM (`523,168` bytes, `d1c1698e…`) and the
module (`356,792` bytes, `b1bcc694…`) from `f9029a3` and started `tools/m2-rig run` with
`/mnt/europa/iris/target/release/iris` 0.1.10+irix7. Both kernels reached `BOOT-READY`,
both armed the mesh listener and joined at epoch 2 with digest `2EDA13C5…`, and both
logged the same homes table. The run then stalled before the route/HTTP stage: the n0
serial log shows `mesh-transport: disconnected reason=heartbeat-timeout` after a period
and n1 retried the join to its five-attempt limit; a direct `curl` to the n0 MCP host
forward accepted the connection but received no response. The host was simultaneously
running a four-kernel M3 shard rig, so CPU starvation of the emulated MIPS core is a
plausible confounder and this run is **not** counted as a failure of M2. It is, however,
a reminder that the combined MCP+mesh module is a single-threaded tick and that a busy
HTTP path can starve the mesh heartbeat; there is no CI job to tell whether that is
environmental or a real regression. The partial transcript is at
`/mnt/europa/scratch/m2-rig-audit-run/`. The **boot and join** path reproduces on a clean
f9029a3 build; the route/migration-stage rerun remains the gap-closing experiment.

---

## 6. Reproducible commands

```sh
# Toolchain (this host)
source /mnt/europa/env.sh
# If the Nix cc wrapper is stale, point CC at a working gcc (this audit used
# /nix/store/z4c6k0mrlkwl3s4w9ysxc8vq1wylm3ms-gcc-wrapper-15.3.0/bin/gcc).

# Host tests
cargo test -p mesh-transport -p mesh-join -p mesh-core -p mesh-service \
           -p mesh-route -p mesh-migrate -p fabric
cargo test --manifest-path tools/m2-rig/Cargo.toml

# Source-only check of the evidence change
git diff 93fc663 f9029a3 -- crates/mesh-migrate/src/node.rs

# Rebuild the two-node artefacts (use a work dir with ~2 GB free; /tmp was full
# during this audit, so the build used /mnt/europa/scratch)
tools/m2-rig/build.sh /path/to/work

# Two-node run (ports must be free; the rig needs the iris and iris-ci binaries)
cargo run --manifest-path tools/m2-rig/Cargo.toml -- run \
  --work /path/to/run --rom /path/to/work/irix7-rom.bin \
  --module /path/to/work/mcp.module \
  --iris /path/to/iris --iris-ci /path/to/iris-ci \
  --base-port 9700 --timeout 240

# Failure and partition modes
cargo run --manifest-path tools/m2-rig/Cargo.toml -- run ... --fault kill-peer
cargo run --manifest-path tools/m2-rig/Cargo.toml -- run ... --fault partition

# CI evidence
gh api repos/irix7/os/commits/f9029a3/check-runs \
  --jq '.check_runs[] | "\(.name): \(.conclusion)"'
```

---

## 7. Evidence paths

| What | Path | Key lines |
|---|---|---|
| M2-5 two-kernel run (join, homes, route, migration) | `/tmp/opencode/wt-m2-migrate/target/m2-rig/evidence/summary.md` | 6 (commit), 12 (module), 119-155 (join), 135-141 (restart), 158-225 (homes), 227-299 (route), 326-402 (migration checks) |
| M2-5 raw report | `/tmp/opencode/wt-m2-migrate/target/m2-rig/evidence/run.json` | `join.restart.request_last_epoch = 1`, `migration.boundary_hash`, `migration.return_hash` |
| M2-5 serial transcripts | `/tmp/opencode/wt-m2-migrate/target/m2-rig/evidence/serial-n0.log`, `serial-n1.log`, `serial-n1-boot1.log` | `mesh-migrate:` lines 73-87 (n0), 52-59 (n1) |
| M2-5 build log | `/tmp/opencode/wt-m2-migrate/build-final3.log` | 1335-1340 (module 344,328, hash 5458…) |
| M2-6 kill-peer | `/tmp/opencode/m2-final-kill2/evidence/summary.md` | 293-371 |
| M2-6 partition | `/tmp/opencode/m2-final-part4/evidence/summary.md` | 293-378 (especially 360-378) |
| M2-4 route | `/tmp/opencode/m2-rig-run4/evidence/summary.md` | 228-299 |
| M2-2 join | `/tmp/opencode/m2-join-evidence/` (and the join section of the M2-5 bundle) | — |
| M2-1 transport | `/tmp/opencode/m2-1-evidence/` | — |
| Audit rebuild | `/mnt/europa/scratch/m2-rig-audit/` (`irix7-rom.bin`, `mcp.module`, `SHA256SUMS`) | — |
| Audit partial run | `/mnt/europa/scratch/m2-rig-audit-run/` | `n0/iris-serial.log`, `n1/iris-serial.log` |

The evidence bundles are not committed to the repository; the paths above are the review
artefacts referenced by the M2 PRs.

---

## 8. Recommendation

1. **Do not close M2 as fully complete.** Reopen #3 or open a follow-up that records the
   gap explicitly. If the project prefers to keep #3 closed, amend the issue body and the
   closure comment to say: item 8 is deferred to Phase 3/M3; item 2 covers member restart
   only; the epoch is not persisted; and the M2 run is manual.
2. **Add the two-node CI job** (`mesh-real` or `m2-rig`) before M3 depends on M2. At
   minimum it should boot two kernels, join at epoch 2, run one remote route and one
   migration, and assert the M1 `mesh=0/0/0` markers in the `no-peer` mode. Without it,
   regressions in `mesh-transport`/`mesh-migrate` are invisible: the CI `test` job does
   not even run those two crates.
3. **Fix the evidence provenance.** Re-run `tools/m2-rig run` from `f9029a3` with a
   clean build, record the module and PROM hashes, and commit the summary (or attach it to
   the PR). The current bundle names a commit that lacks the code in the image.
4. **Make the epoch persistent or document the operator recovery.** A `config_set`-style
   module write or a small module-owned record would let the root restart at the confirmed
   epoch. If that is deferred, the runbook must say that a root restart requires re-seeding
   both NVRAM images and that the member will refuse to rejoin until then.
5. **Implement item 8 or strike it.** The fabric conformance tests are a Phase 3 exit
   item; if they are not M2, remove them from #3's acceptance list and say where they
   live. Leaving them ticked-but-absent is the main reason this audit cannot call the
   completion defensible.
