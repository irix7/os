# The x86 backend for the two-node rig

**Status:** design, no code. This is the plan for running the M2 gate with two
x86 QEMU guests instead of two iris guests.

## Machine selection

`m2-rig` grows a machine kind: `--machine iris|x86`, default `iris`. The kind
selects the launcher, the console probe, the node identity source and the disk
layout. Everything above that (join, homes, route, migration, restart checks,
evidence) stays shared.

## Launch

One QEMU process per node:

```text
qemu-system-x86_64 -M q35 -m 256M -kernel <kernel-x86.elf> \
  -display none -serial file:<work>/nN.serial -monitor none -no-reboot \
  -device isa-debug-exit,iobase=0x501,iosize=0x04 \
  -netdev user,id=netN,hostfwd=tcp:127.0.0.1:<http>-:80,hostfwd=tcp:127.0.0.1:<mesh>-:7000 \
  -device virtio-net-pci,netdev=netN,disable-modern=on,mac=<mac> \
  -drive file=<work>/nN.img,if=none,id=bootN,format=raw \
  -device virtio-blk-pci,drive=bootN,disable-modern=on
```

- Guest port 80 is the MCP HTTP server; port 7000 is the mesh listener. Both are
  forwarded to the rig's existing per-node host ports.
- There is no telnet console on x86. The guest port 23 forward is not used.
- `-no-reboot` and `isa-debug-exit` make a guest exit observable; the rig kills
  the process for a restart.

## Boot disk

The machine's version-1 boot configuration lives in disk sectors 100-103 and the
epoch store in sectors 4096 onwards. The rig writes the image before boot:

| Sector | Content |
|---|---|
| 1 | The disk marker the block self-test reads |
| 8 | MCP module header (magic, length, SHA-256) |
| 9 | MCP module URL |
| 10 | wasm module header |
| 11 | wasm module URL |
| 100-103 | `key=value` configuration |
| 4096+ | Epoch store (module target 3) |

The module images are not on the disk; the guest fetches them from the rig's
HTTP fixture.

The configuration must carry what the Indy rig puts in NVRAM:

- `mesh.peer=10.0.2.2:<relay-port>` for the relay transport, where 10.0.2.2 is
  QEMU's host alias inside the user network;
- the node identity keys the join uses (`mesh.peer_id`, authority key), copied
  from the same lab authority the iris rig uses;
- the MCP token when the run sets one.

The exact key names come from `tools/m2-rig/src/nvram.rs`; the x86 writer reads
them from there rather than duplicating the list.

## Node identity

The guest's NodeId is derived from the NIC MAC (`services.info(1)` returns
`mac: ...`). QEMU's `mac=` parameter gives each node a distinct address, and the
rig derives the NodeId it expects from the same MAC. This replaces the iris NVRAM
MAC.

## Console

The x86 machine has a serial console and no shell. Consequences:

- Readiness uses `BOOT-READY` in the serial log, as now.
- The join and homes checks use HTTP (`/mesh/join`, `/mesh/homes`) and need no
  console.
- The two-way HTTP probe that asks a guest to fetch its peer uses the Indy shell.
  For x86, replace it with:
  - a host-side fetch of each guest's `/mcp` (liveness and control plane), and
  - optionally a mesh-level probe once the two nodes are joined.
- Any check that types a command must be skipped or rewritten for x86.

## Restart

Member and root restarts kill the QEMU process and start it again with the same
disk image, MAC and forwarded ports. The epoch store persists in the disk image,
so `store_restored=true` remains a real check.

## Evidence

`run.json` records the machine kind. Serial logs are copied as today, with the
restarted member's transcript kept as `serial-n1-boot2.log`.

## Risks

- **Slirp reachability.** The relay listens on the host; the guest reaches it at
  10.0.2.2. Whether slirp allows that connection to a loopback-bound listener
  must be tested before the gate depends on it.
- **Timing.** The x86 guest boots in seconds and has a real timer, but QEMU on a
  loaded runner still changes ordering. The gate's waits stay generous.
- **No shell.** The existing guest-to-guest probe cannot be reused; replacing it
  must not weaken the control-plane check.
- **MAC collisions.** Two nodes must never share `52:54:00:12:34:56`, the QEMU
  default, or both derive the same NodeId.

## Slices

1. Launcher, boot-disk writer, readiness, `up`/`down`/`status` for one x86 node;
   verify by fetching `/mcp` from the host.
2. Two nodes, mesh configuration, join through the relay; verify `/mesh/join`
   and `/mesh/homes` on both.
3. The full M2 mode: route, migration, member and root restart.
4. Mixed run: one iris and one x86 node in the same mesh.
