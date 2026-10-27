# M3-ADR-0005 — eight module TCP slots for a multi-peer transport

**Status:** implemented; transport prerequisite only
**Date:** 2026-10-27
**Issue:** <https://github.com/irix7/os/issues/249>
**Related:** [M2-ADR-0002](m2-0002-transport.md), [M2-ADR-0010](m2-0010-direct-peer-transport.md), ABI 15 `tcp_slots`

## Context

The M2 mesh module has one listener and one logical link.  The host hub can multiplex
several members onto that link, but the module cannot hold a connection to each peer; a
root that fans in eight members is a star, not a peer transport.  #249's aggregated
membership needs at least a root with one live connection per member, and the Version 1
target is four to eight real nodes.  The earlier #249 analysis named this as the first
schedule risk: one slot and one listener cannot do peer gossip.

The kernel previously provided two module TCP slots, each with one socket and 16 KiB
receive and transmit rings.  `tcp_listen` and `tcp_connect` both took a slot index, so the
protocol already had the shape; the limit was the kernel array and the fixed local port
used by every connect.

## Decision

1. **Eight module TCP slots.**  `MODULE_TCP_SLOTS` is eight.  Each slot owns one socket
   and its own 16 KiB RX/TX rings.  Slot 0 remains the MCP HTTP slot and slot 1 the mesh
   listener; slots 2..7 are free for peer links.
2. **Ask, do not assume.**  ABI 15 adds `tcp_slots()`.  A module that needs several peer
   links asks the kernel and refuses or degrades when the count is lower.  The count is
   not a compile-time constant duplicated in every module.
3. **One ephemeral local port per slot.**  A connect in slot `n` uses `49152 + n`, so
   several connects can be live at once.  The previous fixed port made the second
   concurrent connect collide.
4. **A socket set with room for the control plane.**  The shared smoltcp `SocketSet` grows
   from 12 to 20 entries: eight module sockets plus DHCP, telnet, the MCP listener and the
   userland client fit without eviction.
5. **Keep the ring clamp.**  `SeeqDevice::capabilities` sets `max_burst_size = Some(7)`,
   matching the eight-descriptor ring.  The buffers/batching change (#256) replaced the
   old `Some(1)` assignment with a comment that described the new value but never set it.
   Restoring the assignment keeps the advertised TCP window below what the ring can hold.
6. **The probe paces its bursts.**  `modules/ports` keeps all eight sockets connected at
   once, then sends and echoes one line at a time.  The eight-descriptor Seeq ring cannot
   absorb a twelve-frame burst of host ACKs and echoes; the probe tests the socket count,
   not an unrealistic burst.

## Evidence

- `modules/ports` on one real `iris` kernel: `ports: connected sockets=8`, one echo per
  slot, `ports: pass sockets=8`; the host fixture records all eight lines.
- `cargo test --manifest-path tools/m2-rig/Cargo.toml` covers the fixture's eight
  independent listeners and the missing-slot case.
- The M2 two-node gate and the M1/M2 markers are unchanged; slot 0/1 use is identical to
  the ABI 14 layout.
- Clean build at the change: PROM 480,068 bytes (with the hash-chain LZ4 encoder);
  `ports.module` 3,544 bytes.

## Consequences

- A module can hold eight simultaneous peer sockets, which is enough for a root star of
  seven members plus the control plane, or for a member with a handful of peers.
- The **mesh link state is still single-link**: `Mesh` has one decoder, send queue,
  heartbeat and join link.  This ADR provides the transport; per-link state and the
  signed gossip exchange are the remaining #249 work.
- Eight slots are not a 128-node transport.  Larger clusters still need the podcast/
  gossip layer to bound fan-out or a UDP service; this ADR does not claim either.
- The 20-entry socket set is a fixed budget.  A module that wants more than eight data
  sockets needs a slot manager, which remains future work (#217).
- The ring clamp can reduce a single bulk stream's in-flight window to seven segments;
  that matches the ring and the #247 bulk path's bounded chunks.

## Alternatives

- **UDP service instead of slots.**  A good fit for SWIM probes, but it replaces the
  stream transport with a new ABI (bind/send/recv) and does not by itself give the
  request/response join path.  Slots are the smaller Version 1 prerequisite.
- **One listener with a kernel-side accept queue.**  smoltcp gives one socket per slot;
  a kernel accept-multiplexer would move connection ownership into the kernel and add a
  new module API.  Eight slots reuse the existing calls.
- **Keep the hub star.**  It is a rig-only switch and cannot be the production topology;
  #249 explicitly rejects a central directory or hub claim.
