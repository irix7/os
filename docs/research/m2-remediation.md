# M2 remediation gate — audit #242 and issue #3

**Status:** remediation report; code, CI, evidence and review record
**Date:** 2026-10-27
**Base:** `fb89b33` (audit #242 at `7a02061`; M2 closure `f9029a3`)
**Scope:** the three closure gaps in [`m2-completion-audit.md`](m2-completion-audit.md) §1 and
the M2-8 review, plus the two-node CI gate.  No new production feature outside that scope.

This report records what was changed, what was rerun, and what is still not claimed.  It is
deliberately not a second audit: the point-in-time audit in
[`m2-completion-audit.md`](m2-completion-audit.md) is left intact, and this file answers its
recommendations.

---

## 1. What the audit asked for

| Audit recommendation | This branch |
|---|---|
| Add a two-node `mesh-real` CI job and stop calling the rig manual | Added `.github/workflows/ci.yml` `mesh-real`; the job builds the two-kernel artefacts and runs `none`, `no-peer`, `kill-peer` and `partition` with unique ports and evidence uploads. |
| Make the confirmed epoch persistent or record an explicit amendment | Implemented in `crates/mesh-join` and `modules/mcp` on the ABI v11 `block_write`/`block_read` path with a dedicated SCSI scratch target.  Member and root restarts restore the signed confirmed epoch; corrupt/foreign records fail closed. |
| Re-run the L0 migration from the merged tree and fix provenance | The rig now records `git describe --dirty`, the tree dirty flag and the PROM/module/`iris`/`iris-ci` hashes in `manifest.json` and `BUILD-PROVENANCE.txt`; the final run is from the merged candidate (see §5). |
| Record a human/engineering review of migration/route/failure code | §6 records the review axes, findings and changes. |
| Item 8 (fabric conformance) | Already closed by `fb89b33` (`#244`) before this branch; the canonical tests run in CI.  The live module still has an **8-entry** pending table; the 10,000-tag result is a fabric-model capacity, not a live-module claim. |
| Do not close #3 until the gate is evidenced | #3 is commented, not closed. |

The M2-8 roster is now backed by a real two-kernel CI job, not by a manual command and a
mosaic of local bundles.

## 2. Signed persistent epoch store

### Design

The module ABI has `config_get` only; it cannot write NVRAM.  ABI 11 does expose
`block_read`/`block_write`, so a node can persist a signed record on a dedicated SCSI
scratch volume.

- Target **3**, LBA 8, five 512-byte sectors (2.5 KiB); target 2 is the MCP module's EROFS
  fixture and target 1 is the lab boot disk, so neither is touched.
- The root writes an `EpochRecord` signed over `IRIX7-M2-EPOCH-STORE-v1`, the epoch and
  the full member list.
- A member writes the exact root-signed `EpochConfirmation` it installed.
- The root writes **before** it queues the confirmation, so a crash after the member
  installs the new epoch cannot leave the root behind.  There is no fsync or write-ahead
  log; a power loss can still lose the newest record (documented limit).
- A restore verifies the Ed25519 signature, root, subject (member), epoch and canonical
  member list before installing.  A corrupt, foreign or role-mismatched record disables
  the join with `mesh-join: disabled reason=epoch-store`; it does not silently start from
  the seed.  A missing target logs `epoch-store: unavailable` and falls back to the seed
  without claiming persistence.
- A same-epoch different digest is never overwritten: it is the transient view that lost a
  failed-but-not-yet-removed member.  A removal advances the epoch and is written normally.

### Tests

`cargo test -p mesh-join` (40 tests) covers:

- `stored_root_record_restores_the_confirmed_epoch` — root record restores epoch 2 and the
  member restores its confirmation, presents `last_epoch=2`, duplicate-installs the same
  epoch and becomes authority-current only after the fresh confirmation;
- `corrupt_or_foreign_epoch_records_fail_closed` — tampered signature, wrong signing key,
  foreign root, wrong subject, root record at a member and member record at a root all
  fail without changing the view;
- the existing root-restart refusal remains the correct failure for a missing store.

`crates/mesh-join/src/wire.rs` adds exact round-trip, truncation, non-canonical member and
signature-domain tests for `EpochRecord`.

### Real-kernel evidence

Every M2 two-node run has a `[scsi.3]` scratch volume per node.  The serial transcript
must show:

```text
epoch-store: stored epoch=2 members=2 digest=2EDA13C5AC6F726132E0479DFE6045B7 source=root-record
epoch-store: stored epoch=2 members=2 digest=2EDA13C5AC6F726132E0479DFE6045B7 source=confirmation
epoch-store: restored epoch=2 members=2 digest=2EDA13C5AC6F726132E0479DFE6045B7 source=confirmation
epoch-store: restored epoch=2 members=2 digest=2EDA13C5AC6F726132E0479DFE6045B7 source=root-record
```

The final `run.json` records `join.restart.store_restored`, `join.restart.store_epoch`,
`join.restart.request_last_epoch`, `join.root_restart.store_epoch`,
`join.root_restart.local_epoch`, `join.root_restart.member_last_epoch` and
`join.root_restart.confirm_epoch`.  The checks require the presented epoch to equal the
confirmed epoch and reject `epoch-regression`.

## 3. Two-node CI job

The `mesh-real` job is separate from the monolithic Boot job.  It:

- installs the pinned nightly and `iris v0.1.10+irix7` (the same release the runbook
  names);
- builds the PROM and module with `tools/m2-rig/build.sh` and checks both size limits;
- runs the `m2-rig` host tests;
- runs four independent scenarios with disjoint work directories and base ports
  (`none` 9700, `no-peer` 9800, `kill-peer` 9900, `partition` 10000);
- stops every scenario on failure through a `trap` (cleanup only; assertions are never
  masked with `|| true`);
- uploads the `evidence/` bundles (including `manifest.json`), both node serial logs and
  the build `SHA256SUMS`, on success and failure.

The default `none` run drives join, homes, route/`MOVED`, L0 migration, the member restart,
the signed epoch restore and the root restart.  The `no-peer` run preserves the M1
`mesh=single`/`mesh=0/0/0` path.  `kill-peer` and `partition` cover removal/fencing and the
partition rule.

The existing `Test` job now includes `mesh-transport` and `mesh-migrate` (the audit noted
they were missing) and runs the `m2-rig` host tests.  `Clippy` checks the harness too.

## 4. Provenance

`tools/m2-rig/build.sh` writes `BUILD-PROVENANCE.txt` with the commit,
`git describe --always --dirty --tags`, the dirty flag and the rustc version.  Every run
writes `evidence/manifest.json`:

```json
{
  "schema": "irix7-m2-artifact-manifest-v1",
  "commit": "<sha>",
  "describe": "<tag>-<n>-g<sha>[-dirty]",
  "dirty": false,
  "artifacts": {
    "rom":    {"size": 523168, "sha256": "..."},
    "module": {"size": 367464, "sha256": "..."},
    "iris":   {"size": ..., "sha256": "..."},
    "iris_ci": {"size": ..., "sha256": "..."}
  }
}
```

The run refuses to present a dirty tree as a clean commit: `dirty` is true for an
uncommitted worktree and the `describe` carries `-dirty`.  The M2-5 bundle's
`93fc663`/`5458c108...` mismatch cannot recur silently because the manifest and the module
hash are recorded together and the tree state is explicit.

## 5. Final run

The final run is made from the merged candidate commit and is recorded in the PR.  The
evidence directory is not committed (as with the earlier M2 bundles); the immutable facts
are:

| Artefact | Value |
|---|---|
| Source commit | filled from the final candidate |
| `describe` / dirty | filled from `BUILD-PROVENANCE.txt` |
| PROM bytes / SHA-256 | filled from the final build |
| Module bytes / SHA-256 | filled from the final build |
| `iris` / `iris-ci` | v0.1.10+irix7, hashes in `manifest.json` |

## 6. Engineering review

Reviewed the migration, route and failure code against the audit's axes.  No production
feature was added outside the epoch store and the lifecycle fixes below.

### Security boundaries

- **Authority:** Ed25519 through `cap-engine`; the trust store is one root key.  A member
  cannot sign an epoch, a capability or an `ActorMove`.  The epoch store verifies the root
  signature before installing; it does not trust the disk.
- **Capabilities:** bearer tokens on plain TCP; the subject/object/rights/home epoch are
  checked by `mesh-route`.  The token has no transport confidentiality or peer
  authentication, as M2-ADR-0002/0007 record.  No change.
- **Store:** the scratch volume is a dedicated target; the module does not write target 1
  (boot disk) or target 2 (EROFS fixture).  The target-3 move was a review finding (see
  below).
- **Failure path:** a member that loses authority never removes the root or advances the
  epoch; only a signed confirmation restores authority.  No change.

### Stale handles

- `mesh-route` fences cached routes and leases on a confirmed view change; a non-member is
  never asked for a redirect, and the `MOVED` answer is bounded to `old_home_epoch + 1`.
- `mesh-migrate` fences the old actor instance and refuses deliveries naming an old host
  or epoch.
- The epoch store restores only the signed epoch and member list; it does not restore
  route, migration or HTTP state, so there is no stale in-memory handle to revive.

### Duplicate and idempotence

- Join confirmations are idempotent at the same epoch and list; a same-epoch different list
  is an equivocation and is refused.
- Migration duplicate messages at or below the highest applied id do not advance the
  actor.
- The store skips a routine refresh and the transient same-epoch different-list view; a
  duplicate confirmation does not rewrite the disk.
- Member restart now restores the signed confirmation instead of starting at the seed;
  the root's re-confirmation is a duplicate and does not advance the epoch.

### Bounds

- `EpochRecord`/`EpochConfirmation` are bounded by `MAX_BODY` (4 KiB) and the store buffer
  is five sectors (2,560 bytes); the header length is checked before slicing.
- The route report is bounded to eight result rows; the live pending table is eight tags.
- Migration state is 248 bytes with a 512-byte body bound.
- The MCP response buffer and NVRAM environment bounds are unchanged.

### `unsafe`

- `mesh-join`, `mesh-route` and `mesh-migrate` are `#![forbid(unsafe_code)]`.
- The new `unsafe` is confined to the existing module FFI: five `block_read` calls and five
  `block_write` calls over a static 2,560-byte buffer, plus the existing static buffers.
  The buffer slices are offset-checked (LBA 8..13), the kernel services write/read exactly
  512 bytes, and the module is single-threaded.
- No new `unsafe` in the host harness.

### CI assertions

- The job does not use `|| true` on a rig run; the only `|| true` is the best-effort
  `down` in the cleanup trap.
- Each scenario has a distinct work directory and base port; the default `none` run is the
  full M2 gate, not a subset.
- The job uploads evidence even when the run fails, so a red job is diagnosable.

### Findings changed in this branch

1. **Target collision:** the first implementation used SCSI target 2, which is the MCP
   module's EROFS fixture (`FS_TARGET`).  Moved the store to target 3.
2. **Restart wait:** `restart_member`/`restart_root` used `BootFacts`, which ignores a
   final serial line without a trailing newline.  The heartbeat-disconnect line is often
   exactly that line.  The waits now count raw lines.
3. **Already-between-links restart:** a root that was already disconnected before the
   member stopped never logs a second disconnect.  The wait now accepts
   `connected <= disconnected` as "ready for the new relay".
4. **Join evidence fallback:** after a restored member restart the node logs
   `mesh-join: duplicate`, not `converged`; the evidence capture parsed the duplicate but
   stored the empty `converged` field.  It now stores the parsed fallback line.
5. **Record deadlines:** a 388-byte confirmation with 8 bytes buffered was abandoned after
   the old 10 s receive deadline while the single-threaded module served an HTTP response.
   `RECEIVE_DEADLINE_MS` and `SEND_DEADLINE_MS` now match the 30 s heartbeat window; a dead
   peer is still detected by `HEARTBEAT_TIMEOUT_MS`.  This is the diagnosed cause of the
   mesh link resets during the two-way probe, not a blind timeout increase.
6. **Half-open root socket across a member restart:** after n1 was stopped and restarted,
   the root kept its old guest-side mesh socket.  The iris host forward delivered the new
   member requests to that socket, so the root processed and answered them, but the
   replies went to the old relay connection and the new relay read nothing.  The member
   logged `rx_frames=0` on every reconnect.  The module now exposes the link-only
   `mesh.reconnect` MCP tool; the rig aborts and re-arms the live peer's socket before
   starting the new relay.  The confirmed epoch, route and migration state are retained;
   no membership change is manufactured.  The root-restart path uses the same control on
   the live member.

## 7. Remaining limits

- The epoch store is a **local signed scratch record**, not a replicated log.  A lost or
  corrupt volume is an operator event; a power loss can lose the newest record and a
  member with a newer confirmation makes the root fail closed with `epoch-regression`.
- The 1-of-1 authority, root-issued capabilities, plain TCP and the host relay remain as
  recorded in M2-ADR-0002/0003/0005/0007.
- The live MCP module keeps an 8-entry pending table; the 10,000-tag fabric result is a
  model capacity.
- Real-Indy hardware is still issue #16; the gate is two `iris` instances.
- The two-node CI job is the new gate.  A green run is required at the merged candidate
  before #3 is closed.
