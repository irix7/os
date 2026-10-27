# M2-ADR-0010 — direct peer TCP and the lower-NodeId connect rule

**Status:** accepted for the direct-transport slice; limited
**Date:** 2026-10-27
**Issue:** <https://github.com/irix7/os/issues/245>
**Related:** [M2-ADR-0002](m2-0002-transport.md), [M2-ADR-0003](m2-0003-authority.md),
[`../research/direct-peer.md`](../research/direct-peer.md)

## Context

Every M2/M3 lab link is `guest → host relay → peer port-forward → guest` because the
kernel exposes only `tcp_listen`, `tcp_state`, `tcp_recv`, `tcp_send`, `tcp_close` and
`tcp_abort`.  The relay is a dumb byte pump, but it is still a process in the data path,
a single point of failure, and the reason the lower-`NodeId`-initiates rule in
M2-ADR-0002 §3 has never run.

The wire-efficiency study measured the relay's host cost at about 23 µs and the 10 ms
module tick at 10.005 ms.  Removing the relay does not remove the tick, but it removes a
process, two copies and a fan-in point, and it makes the production path a direct TCP
connection between the two kernels.

## Decision

1. **Add one module service, `tcp_connect(slot, address, port)` (ABI 13).**  `address` is
   an IPv4 address in network byte order.  The kernel creates the slot with the same
   bounded 32 KiB RX / 32 KiB TX rings and the mesh socket policy as `tcp_listen`,
   chooses an ephemeral local port, and queues the SYN.  There is no general socket API,
   no bind, no non-blocking accept and no callback in the kernel.

2. **The lower `NodeId` initiates.**  Each node receives `mesh.direct=<gateway>:<peer
   host port>` and `mesh.peer_id=<peer NodeId>`.  If `self_id < peer_id` the module
   calls `tcp_connect`; otherwise it listens on guest port 7000.  The comparison is
   byte-wise over the 16-byte IDs, as M2-ADR-0002 specifies.  The direct target is the
   peer's host forward through this node's own NAT gateway, so the lab still has two
   isolated guests and one TCP connection per peer.

3. **Bounded connect and fallback.**  The initiator waits 3 s after boot for the listener
   to arm, then attempts one connect every 500 ms.  After five attempts or 5 s it logs
   `mesh-transport: direct fallback listen` and arms its own listener.  A successful link
   logs `mesh-transport: connected connect_ms=<n>`; a disconnect returns the initiator to
   the connect state with the same bounded backoff.  There is no unbounded retry and no
   relay dependency in direct mode.

4. **The relay remains a lab fallback, not a second wire format.**  `mesh.transport=relay`
   is the default for the M2 rig; `mesh.transport=direct` removes the relay from the path;
   `mesh.transport=direct-fail` points the initiator at a dead port and asserts the
   fallback listener with no join.  The evidence records which path ran.

5. **Trust boundary.**  Direct TCP is still plain TCP: no TLS, no peer authentication and
   no replay window.  The direct link is a lab path.  The join confirmation remains
   Ed25519-signed; the data path does not gain a security claim.

6. **PROM fit.**  Adding the TCP client state machine to the kernel costs about 1.6 KiB of
   compressed image.  The kernel TLS client keeps RSA and P-384; removing P-384 broke
   HTTPS module fetches in the Boot gate.  The direct slice fixes the ephemeral local port
   (one link per lifecycle, no counter) and leaves the PROM at 523,556 / 524,288 bytes
   with 732 bytes of headroom.  The full actor evidence remains on `m1` and `m1-post`.

## Evidence

- Two real `iris` kernels, direct mode, no host relay:
  - n1 (lower NodeId `08006969…`) `direct role=initiator`, n0 `direct role=listener`;
  - join epoch 2, homes, one wire `MOVED`, L0 migration n0→n1→n0, same state hashes;
  - connect latency 21 ms on the pass line, one connection;
  - mesh request p50 30 ms / p95 31–32 ms, against relay p50 31 ms / p95 52 ms;
  - 64 KiB bulk 0.404 MiB/s mean (5.21× the 1 KiB same-host baseline).
- Direct-fail: the initiator logs the fallback listener and no join occurs.
- The guest two-way HTTP probe is the pre-existing MCP HTTP/1.0 close race; the direct
  mesh sections are independent of it.  The rig records the probe result rather than
  hiding it.

## Consequences

- The production-shaped path is direct peer TCP; the relay is diagnostic only.
- A direct node needs the peer's NodeId and host forward in NVRAM.  Discovery is not
  added; membership remains the source of identity.
- The guest can still only listen on one port, so a full peer-to-peer mesh above two
  nodes needs multiplexed streams or a hub.  That is #246/#248 work, not this ADR.
- Plain TCP remains a documented security limit.
