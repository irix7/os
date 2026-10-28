# M3-ADR-0006 — module UDP services and a non-blocking Seeq transmit path

**Status:** implemented; transport prerequisite only
**Date:** 2026-10-29
**Issue:** <https://github.com/irix7/os/issues/255>
**Related:** [M3-ADR-0005](m3-0005-multi-connection-transport.md),
[`../research/quic-primary.md`](../research/quic-primary.md), ABI 16 `udp_*`

## Context

The QUIC study ([`docs/research/quic-primary.md`](../research/quic-primary.md)) returns a
**conditional GO** for QUIC on native flows and makes the decision depend on measurements
that do not exist yet.  Its spike issue (#255) lists five gates, two of which are transport
foundations rather than protocol work:

- a UDP path that two real kernels can exercise, with event-driven receive; and
- a Seeq transmit path that does not block the kernel for the duration of a frame.

Neither existed.  The module ABI exposed TCP only (`tcp_open`-style listen/connect, send,
receive, close, events and slots at ABI 15), and the Seeq transmit token polled the HPC3
`ACTIVE` bit up to ten million times inside the service call.  A module that sent a frame
therefore stalled the console, the idle loop, every other module and the receive path until
the engine finished with the descriptor.

A QUIC profile cannot be evaluated on this machine until a datagram can leave one kernel and
arrive at another.  This ADR records that foundation.  It does **not** adopt QUIC, and it
does not change the transport of any existing flow.

## Decision

1. **ABI 16 appends the module UDP services.**  `udp_open(slot, port)`, `udp_send`,
   `udp_recv`, `udp_close`, `udp_slots` and `udp_state` are appended to the service table;
   the n32 offsets are pinned by compile-time assertions in the shared `module-abi` crate
   (SDK v0.11.0).  `udp_recv` reports the sender so a reply needs no second lookup, and
   `udp_state` reports the bound local endpoint.  `udp_open` rejects port zero because the
   in-tree stack has no ephemeral UDP bind.
2. **The kernel owns the socket; the module owns the protocol.**  Eight slots, one socket
   per slot, a 2 KiB payload ring each way and eight packet metadata entries.  There is no
   retransmission, congestion control, framing or demultiplexing in the kernel: that is the
   module's job, and it is what a QUIC profile would replace.
3. **`udp_send` is explicitly backpressured.**  It returns the length queued, `0` when the
   socket ring is full, and `-1` for an invalid argument.  A module retries after the next
   `poll`; it never spins in the kernel.
4. **Receive is level-triggered by construction.**  `udp_recv` returns one datagram per
   call and `-2` when the caller's buffer was too small (the datagram is dropped, not
   truncated).  There is no edge queue and no lost notification.
5. **The Seeq transmit path is non-blocking.**  One frame is outstanding at a time.  The
   token fills the buffer, programmes the descriptor, kicks the engine and returns; the next
   `transmit`/`receive` reaps completion when `ACTIVE` clears.  While a frame is in flight
   the device hands out no token, so smoltcp backpressures instead of the kernel spinning.
   `receive` also checks the gate before taking a frame, so it never hands out a transmit
   token it cannot honour; the frame stays in the descriptor ring for the next poll.
6. **The socket set grows from 20 to 28 entries.**  Eight module TCP slots, eight module UDP
   slots and the control-plane sockets (DHCP, telnet, MCP listener, mesh, userland client)
   fit without eviction.
7. **The probe is a gate, not a product.**  The MCP module carries a `netprobe` mode that
   activates only when `netprobe.peer` is present in the DS1386 environment.  It sends
   `PING <name> <seq>` to a peer and answers with `PONG <name> <seq>`, so the gate exercises
   send, receive, the sender address and the reply path on two real kernels.

## Evidence

Measured on 2026-10-29 with the pinned CI emulator (`iris` v0.1.10+irix7) and the
two-node rig at `tools/m2-rig`, `--fault udp`:

- **Artefacts.**  PROM 480,072 bytes (limit 524,288); MCP module 414,424 bytes (limit
  524,288, so 109,864 bytes free).  The module grew by 3,616 bytes over the pre-netprobe
  build.  `rom=480072 module=414424` is recorded in the bundle's `manifest.json`.
- **Two real kernels.**  `n0` and `n1` both reached `BOOT-READY ... mcp=listening`, opened
  the probe socket (`netprobe: up name=n0 port=7001 peer=192.168.0.1:41049`), sent a
  `PING`, received the peer's `PING` (`netprobe: ping-rx ... peer=n1`) and received a
  `PONG` back across the relay (`netprobe: pong from=192.168.0.1:41049 peer=n1 seq=3` on
  `n0`, `netprobe: pong from=192.168.1.1:41049 peer=n0 seq=1` on `n1`).  The relay
  forwarded 5 datagrams between the two flows (received 9, learned 2, dropped 2) with no
  inbound port forward on either node.
- **No regression.**  The same artefacts pass the default M2 mode (`--fault none`): join,
  homes, route/MOVED, L0 migration, member restart, signed-epoch restore and root restart.
  The M2-ADR-0002 TCP path and the M3-ADR-0004 readiness loop are unchanged in behaviour.
- **Bundle.**  `<work>/evidence/{summary.md,run.json,udp-relay.log,serial-n0.log,serial-n1.log}`
  in the `udp` mode of the two-kernel CI job (`$RUNNER_TEMP/m2-evidence/udp`).

This is a transport prerequisite, not a QUIC result.  #255's remaining acceptance
criteria — guest crypto throughput, the linked n32 QUIC/TLS footprint, and the CID-routed
relay design — are still open, and the GO/NO-GO ADR has not been written.

## Consequences

- A module can move datagrams between kernels, which is the precondition for the QUIC
  profile and for any future UDP-based fabric.
- Ordinary TCP traffic is unchanged in behaviour.  The transmit change removes a blocking
  wait; it does not change framing, ordering or the ring clamp
  (`max_burst_size = Some(7)`).
- One outstanding transmit frame bounds transmit throughput to one frame per poll.  The
  receive path already worked this way; a real Indy measurement is still needed to show
  whether that bound matters at 10 Mbit.
- The UDP service is **not** a QUIC stack.  Encryption, stream multiplexing, loss recovery,
  connection migration and congestion control remain module work gated on #255.
- The 2 KiB receive buffer holds one path-MTU datagram.  A module that wants to queue
  several datagrams in userspace must copy them out; there is no zero-copy path.

## Alternatives

- **A UDP service inside the MCP module.**  The module has no raw device access, so it
  cannot build or parse Ethernet/IP/UDP frames; the kernel must own the interface.
- **smoltcp's `udp` socket exposed directly as a file descriptor.**  That would bind the
  module ABI to smoltcp's socket type and give up the versioned C table the whole module
  system depends on.
- **Keeping the blocking transmit and adding a timeout.**  A timeout does not make the call
  non-blocking; it only chooses how long the kernel stalls.  The existing ten-million-spin
  loop was already a latent corruption risk: after it expired the buffer could be rewritten
  while the engine still owned the descriptor.
- **Porting quinn, quiche, s2n-quic or neqo now.**  The study measured rustls alone at
  ~482 KiB against ~113 KiB of free module space.  The spike must produce a code-size
  measurement before any stack is chosen.

## Evidence standard

This ADR is a transport prerequisite.  It does not move the M2 or M3 marker evidence, and it
does not satisfy any #255 acceptance criterion on its own: the crypto throughput, the linked
n32 QUIC/TLS footprint and the CID-routed relay design remain open.
