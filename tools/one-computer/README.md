# One computer — M2 object/actor transcript

This is the M2 demonstration that the mesh is the computer, not a cluster with
remote calls.  Two independent iris kernels run the real MCP module and one
prompt drives both through the mesh policy.  It is the **M2 object/actor
proof**: the bounded L0 actor fixture from M2-5 and the capability route from
M2-4.  It is not fork/exec over cells, process migration or a POSIX session;
that path lands after #326/#327.

## Run

```sh
nix develop
export CC=cc
tools/one-computer/run.sh
```

The runner builds `tools/m2-rig/build.sh` artefacts and the `m2-rig` harness if
they are missing, then runs `m2-rig demo`.  The transcript is
`target/one-computer/demo/transcript.txt`; `summary.md` carries the checks and
the exact node, object and actor identities.  `IRIS`, `IRIS_CI`, `M2_RIG_BIN`,
`ROM`, `MODULE`, `BUILD_DIR`, `BASE_PORT` and `TIMEOUT` override the defaults.
A sample pass is in [`transcript.txt`](transcript.txt).

## The prompt

The operator sees one prompt and plain words:

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

No command names a node, a route, a transport, a port or an HTTP path.  Node
identities, home epochs, values, fences and route attempts appear only in the
output.  `stat <object>` and `move <object>` use
`GET /mesh/route?op=stat&object=...` and `?op=move&object=...`; `move` moves the
named object to the node that answers the console.  `move actor` uses the
existing `GET /mesh/migrate?step=2|7` surface and chooses the direction from the
actor's current host.  `fail` sends SIGKILL to the member process; `recover`
restarts it through the rig's process helpers and rejoins it through the seeded
mesh relay.

## What the transcript proves

- Two separate iris processes with distinct NodeIds, MACs and NAT subnets.
- A signed two-node join at epoch 2; both kernels serve the same rendezvous
  home table.
- A named object homed on the peer is read remotely at its home.
- The same object moves to the console's node through the root-signed home
  lease; a later read hits the stale home, consumes exactly one `MOVED` and
  retries at the new home.
- The bounded L0 actor moves root to member and back at a message boundary with
  the same canonical state hash, queued message and capability handle.
- Killing the member leaves the console running.  A read of an object that was
  homed on the dead member fails closed as unavailable; it is not invented
  locally.  The survivor reports suspect, failed and removed, advances the
  epoch once, fences the cached routes that named the removed node and serves
  the object from its recomputed local home.
- The restarted member sends a fresh join request and converges with the
  survivor at epoch 4.  Both nodes serve the same two-member table and digest.
- The actor's state hash is unchanged on the survivor.
- The final prompt is byte-for-byte the prompt used before the failure.

## CI

The `mesh-real` job in `.github/workflows/ci.yml` runs `m2-rig demo` after the five M2
fault modes, with the same built PROM and MCP module, and uploads the transcript under
`mesh-real-evidence/demo`.

## Limits

- This is an internal lab transcript, not a general shell.  The object set is
  the M2 home fixture (8 objects derived from `IRIX7-M2-HOME-OBJECT-v1`); the
  actor is the M2-5 stackless fixture.  There is no user object store, no
  fork/exec, no cell, no POSIX session and no live-state migration.
- The mesh link is the host relay, not a direct guest-to-guest TCP path.  The
  guest modules still encode, decode and policy-check every mesh record.
- The actor state is not persisted across a member crash.  The demonstration
  moves the actor back to the survivor before the failure so the surviving
  state hash is meaningful; actor crash recovery is later work.
- `move <object>` moves to the console's node.  A general placement policy and
  a remote target argument are not part of this M2 proof.
