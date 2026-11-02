# M2-ADR-0011 — one prompt over the M2 object and actor surfaces

**Status:** accepted for M2, limited
**Date:** 2026-11-03
**Deciders:** M2 demonstration track, for review at the M2 gate
**Related:** [M2-ADR-0006](m2-0006-homes.md), [M2-ADR-0007](m2-0007-route.md),
[M2-ADR-0008](m2-0008-failure-fence.md), [M2-ADR-0009](m2-0009-actor-migration.md),
[`../spec/system.md`](../spec/system.md) §1 and §5,
[`../plan/milestones.md`](../plan/milestones.md) §2, [`../plan/m2-rig.md`](../plan/m2-rig.md),
[`../../tools/one-computer/README.md`](../../tools/one-computer/README.md)

## Context

The M2 rig proved join, homes, the remote capability route, L0 migration and the
failure/fence/partition rule as separate scripted steps.  The evidence was correct but the
operator-facing shape was still a test harness: the route steps chose their own objects,
the migration steps were numbered lab controls, and the failure scenario was a separate
`--fault` run that did not exercise the same prompt end to end.

The M2 demonstration gate needs one console, one prompt and plain commands over two real
kernels.  The command text must not name a node, a route, a port or an HTTP path.  The
object move must be a named object, not "the first peer-homed object the step policy
picked", and the failure and rejoin must happen in the same session as the successful
reads and moves.

## Decision

### 1. Two named-object operations on the route surface

`mesh-route` gains `move_object`, and the MCP module accepts:

```text
GET /mesh/route?op=stat&object=<64 hex>
GET /mesh/route?op=move&object=<64 hex>
```

`stat` uses the existing `begin_operation` path and the capability/lease checks.  `move`
moves the named object to the node that received the request; the existing root-signed
home-lease directive, `MOVED` and one-retry path are unchanged.  The fixed
`GET /mesh/route?step=N` lab control remains for the M2-4 rig; the named operations are
the console surface, not a second policy.

The route report is unchanged.  The console learns the current move from `move old/new`,
the completed result rows and the fence rows; it does not create a second directory.

### 2. A `m2-rig demo` transcript runner

`tools/m2-rig/src/demo.rs` owns the prompt.  It starts two real iris processes through the
existing `start_node`, `kill_node`, `stop_nodes`, `wait_serial` and relay helpers, joins
them through the existing signed handshake, and prints:

```text
irix7> nodes
irix7> homes
irix7> stat <object>
irix7> move <object>
irix7> actor
irix7> move actor
irix7> fail
irix7> nodes
irix7> homes
irix7> stat <object>
irix7> recover
irix7> nodes
irix7> homes
irix7> actor
irix7> stat <object>
irix7>
```

`move actor` uses the existing `/mesh/migrate?step=2|7` surface and picks the direction
from the actor's current host.  `fail` sends SIGKILL to the member and drops the host relay
so the supervisor cannot keep reconnecting to the survivor's half-open guest forward.
`recover` starts the member, resets the survivor's guest link with the link-only
`mesh.reconnect` control, starts a fresh relay and waits for both ends to agree on the new
epoch and digest.

The transcript assertions are recorded in `demo/summary.md`: the same prompt before and
after the failure, no internal detail in any command line, the named object moved, the
actor moved at a message boundary, the unavailable read and the fence, and the rejoin
convergence on both nodes.

### 3. Module budget

The named operations add code to slot A.  The module was 523,416 bytes of image plus
`.bss` against a 524,288-byte slot; the new path fits by reducing the generic MCP response
buffer from 16 KiB to 14 KiB.  `FS_PRINT_MAX` remains 8 KiB and the JSON wrapper keeps its
escaping headroom.  The fixed 512 KiB slot and the PROM freeze are unchanged.

## Consequences

- The M2 object/actor proof now has a single human-facing artefact and a CI step in the
  `mesh-real` job.
- The console's object set is still the M2 home fixture and the actor is still the M2-5
  stackless fixture.  This is not a user object store, a cell, fork/exec or live-state
  migration.
- The route move target is the console's node.  General placement and a remote target
  argument are later work.
- The actor survives the member failure because the transcript moves it back to the
  survivor before the kill.  Actor crash recovery and state persistence are not claimed.
- The console is a host-side driver.  It proves the mesh policy and the module surfaces;
  it does not add a kernel prompt, a cell or a process table.

## Alternatives

- **Use only the fixed `?step=N` route surface.**  Rejected: the console would accept an
  object argument but silently operate on a different object.
- **Add a second object directory for the console.**  Rejected: the M2 homes and leases are
  the authority; a second directory would duplicate it.
- **Put the console in a new module.**  Rejected: the MCP module already owns the mesh
  frame loop, the HTTP surface and the failure detector, and the slot budget is fixed.
- **Keep the failure as a separate `--fault kill-peer` run.**  Rejected: the gate needs the
  same prompt, the same object and the same actor before and after the failure.

## Evidence

`m2-rig demo` on two real iris 0.1.13 kernels with the built PROM and MCP module.  The
`demo: pass` transcript is in the CI artefact `mesh-real-evidence/demo` and the sample in
[`tools/one-computer/transcript.txt`](../../tools/one-computer/transcript.txt).
The host tests in `crates/mesh-route` cover the named move, the already-home refusal and
the non-root refusal; `cargo test --manifest-path tools/m2-rig/Cargo.toml` covers the
report parsers and the independent fixture oracle.
