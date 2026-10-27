# M2-ADR-0002 — TCP transport and `MeshEnvelope` framing

**Status:** accepted for M2
**Date:** 2026-10-26
**Deciders:** M2-0, for review at the M2 gate
**Related:** [M2-ADR-0001](m2-0001-two-node-rig.md), [M2-ADR-0003](m2-0003-authority.md),
[`../plan/m2-rig.md`](../plan/m2-rig.md)

## Context

`sys-primitives` already defines the wire vocabulary:

- `MeshHeader`, a fixed 128-byte routing header with magic `I7MH`, version 1, source,
  destination, home, route hint, four epochs, request/reply IDs, TTL, hop count, priority,
  `frame_len` and a CRC32C over bytes 0..124;
- `MeshEnvelope`, a `MeshHeader` plus one `MessageFrame`;
- `MessageFrame`, the fabric frame with payload descriptors, tags, completions and trace
  context.

What does not exist is a transport. The M1 kernel constructs no `MeshHeader` and sends
zero mesh frames. M2-1 must put an envelope on a wire, frame it, enforce deadlines, and
cancel in-flight work.

The M2-0 rig established the only available path between two independent iris guests:
each guest has its own NAT, each NAT maps a connection to its own gateway onto host
loopback, and a peer is reached through a host forward or a transparent Rust relay. The
guests cannot share memory; they cannot ARP each other; they cannot broadcast. The host
loopback path is real TCP between two independent kernels, but it is not a shared L2
network.

The kernel has a TLS 1.3 **client** for HTTPS and module fetches. It has no TLS server,
no certificate store for peer identities, and no accept-side TLS state machine. A mesh
peer has to accept inbound connections, so a TLS transport would require a new server
implementation, a peer-authentication story and a certificate/trust configuration. That
is not M2 work.

## Decision

1. **TCP is the M2 mesh transport.** It runs over the rig's host-forward/relay path. The
   transport is a byte stream; the mesh layer owns framing.

2. **Framing.** Each record on the stream is:

   ```text
   u32 little-endian length L
   L bytes of MeshEnvelope
   ```

   `L` counts only the envelope bytes. `MeshEnvelope::decode` already validates the
   header magic, version, header length, CRC32C, `frame_len` and the embedded
   `MessageFrame`; the length prefix only restores record boundaries. The receiver must
   apply a bound (M2: **64 KiB**) before allocating, and must reject `L` smaller than the
   minimum envelope. The header remains the authority for `frame_len`; the length prefix
   and `header.frame_len` must agree.

   All mesh metadata is little-endian, as the system spec requires. Encode field by
   field; never send a raw `#[repr(C)]` struct, because the kernel is big-endian MIPS and
   the wire is little-endian.

3. **One logical link per peer.** The link is full-duplex: both nodes may send envelopes
   on the same TCP connection. To avoid two simultaneous connections after a restart,
   the **lower `NodeId` initiates** (byte-wise comparison of the 16-byte IDs) and the
   higher listens on guest TCP port **7000**. A reconnect uses exponential backoff with
   jitter; the higher side keeps its listener armed. The rig gives both nodes a listener
   and a peer address, so either role is testable without changing the harness.

4. **Peer configuration is explicit; discovery is not in M2.** Each node's DS1386
   environment carries:

   ```text
   mesh.peer=<own gateway>:<peer mesh host port>
   mesh.authority=<node 0 MAC>
   mesh.epoch=<initial epoch>
   ```

   The guest's own gateway is the only host address its NAT can reach. The rig writes
   `mesh.peer` as `192.168.0.1:10112` for `n0` and `192.168.1.1:10102` for `n1` after the
   eight-node port-block move; the key's presence is what enables the listener. A future
   rendezvous or gossip layer can replace the static key; M2 must not depend on
   broadcast.

5. **Deadlines and cancellation are transport-visible.** M2-1 must define:

   - a connect deadline (M2: 5 s) and a bounded reconnect backoff;
   - a per-record receive deadline (M2: 30 s, matching the heartbeat window because the
     module is single-threaded) and a maximum record size;
   - a send deadline for backpressured streams;
   - a heartbeat or keepalive so an idle link is distinguishable from a dead peer;
   - cancellation as a fabric operation, not a TCP RST: a cancelled request is answered
     with a terminal completion and its tag is released. A TCP reset is a link failure,
     not a request cancellation.

   The exact values are M2-1's to choose and test; they must be in the codec/transport
   module, not scattered through the kernel.

6. **TLS is deferred; it is not rejected.** M2 runs plain TCP on host loopback. The mesh
   header carries a CRC32C, which detects accidental corruption but is not a MAC. There is
   no confidentiality, no peer authentication and no replay window in M2. This is
   acceptable only because the M2 rig is a single host and the loopback path is not a
   network. The upgrade path is a TLS 1.3 server with a peer certificate/PSK trust store,
   or a Noise/QUIC handshake; the `MeshHeader` and `MeshEnvelope` are transport-neutral
   and stay unchanged. The transport interface should be a trait so the TCP implementation
   can be replaced.

7. **The probe relay is not the mesh link.** The `tools/m2-rig` relay closes a
   target-to-client direction after a 5 s quiet period because the M2 probe speaks
   HTTP/1.0 to MCP. A persistent mesh link must not inherit that policy. M2-1 has two
   honest choices:

   - use the guest 7000 forward directly and fix/avoid the early-data race (see
     M2-ADR-0001); or
   - add a **mesh relay mode** with a long idle timeout and a heartbeat, and keep the
     5 s probe mode separate.

   Either way, the relay stays a dumb byte pump. It does not parse `MeshHeader`, assign
   node IDs, confirm epochs or route frames.

8. **Shared memory is out of scope for this rig.** The two guests are separate iris
   processes with separate emulated RAM and no shared-memory device. A shared-memory
   local transport is a Phase 3 item for same-host native cells and for a future
   co-located deployment; it is not the M2 two-instance path.

## Consequences

- M2-1 can start immediately against a TCP socket with a known peer address and a real
  two-kernel path. It does not need TLS, a directory service or a new emulator device.
- The framing is simple enough to fuzz and to test on the host: length prefix, envelope
  decode, CRC, `frame_len` agreement, oversize rejection, partial reads, short writes.
- The link is a single TCP connection per peer, so a lost connection is visible and
  reconnect policy is explicit. Head-of-line blocking is the normal TCP behaviour; M2
  does not need multiple streams yet.
- Plain TCP is a **documented security limitation** of M2. The rig must not be presented
  as a secure multi-node deployment. The M2 gate is about join, routing and migration,
  not transport security.
- Static peer configuration means a node restart does not need discovery, but a changed
  peer address needs a new NVRAM image. M2-2 can add a join handshake that exchanges
  addresses once a bootstrap link exists.
- The 64 KiB record bound and the 30 s receive/send deadline are M2 starting points; large
  payloads belong in CAS or shared memory, not in a mesh frame.  The deadline is not a
  liveness check: a dead peer is still detected by the 30 s heartbeat timeout.

## Alternatives considered

| Alternative | Why it lost |
|---|---|
| TLS 1.3 now | No TLS server, no peer trust store, no certificate provisioning; M2 would spend its time on transport security instead of join/routing/migration. |
| QUIC | Same server-side and trust work, plus a large dependency and no kernel support. |
| UDP with a custom reliability layer | TCP already provides ordered, reliable bytes on the host path; a custom layer adds reassembly, congestion and retransmission work with no M2 benefit. |
| Shared memory | The guests are separate processes with separate RAM; no shared-memory device exists. |
| `mesh-sim` | Policy only; it cannot exercise the real kernel transport. |
| A central message broker | Would add a directory/authority in the data path, contrary to the M2 "without a central directory" criterion. The relay is a byte pump, not a broker. |

## Evidence

- The M2-0 rig demonstrates a real TCP exchange between the two kernels in both
  directions: each guest's HTTP client fetches the peer's MCP status page and receives
  `HTTP 200 38 bytes`; the transcripts and relay byte counts are in the M2-0 evidence
  bundle.
- `sys-primitives` host tests round-trip `MeshHeader`/`MeshEnvelope`, including CRC and
  field offsets. The M2-1 work should extend those tests with a stream framing layer and
  a short-read/short-write harness.
- `kernel/src/tls.rs` exposes only `get`/`get_to`/`get_range`/`get_range_to`; there is no
  server-side accept path.
- The M2-0 direct-path observation (early data reset) is recorded in M2-ADR-0001; it is
  the reason the probe uses the relay and the reason M2-1 must not assume a direct
  forward behaves like a normal listening socket.

## M2-1 implementation note — listen-only bootstrap

**Status:** implemented for M2-1; this note records a deliberate interim deviation.

The M2-1 transport ships inside the existing MCP module, not as a new PROM feature.
The kernel exposes only two module TCP slots and no client-connect service. MCP used
both slots to pre-arm the next HTTP connection. M2-1 resolves the ownership conflict as
follows:

- The combined MCP+mesh module keeps **HTTP on module TCP slot 0** and gives **module TCP
  slot 1 to a mesh listener on guest port 7000** when NVRAM contains `mesh.peer`.
- Without `mesh.peer`, the mesh listener is disabled and MCP keeps its original two-slot
  behaviour. A normal boot is unchanged.
- Both nodes listen. The host `tools/m2-rig` mesh relay connects to both guest-7000 host
  forwards and pumps bytes between them. This is the only supported M2-1 path because the
  guest has no TCP connect primitive. The lower-`NodeId`-initiates rule is therefore not
  exercised; it becomes relevant again when a connect service or a fixed early-data path
  exists. The relay is still a dumb byte pump, not a directory or membership service.
- The relay waits until it has read first bytes from **both** guests before writing either
  direction. That avoids the iris port-forward early-data race recorded in M2-ADR-0001.
  Its idle timeout is 120 s, and the modules generate a heartbeat every 5 s, so the 5 s
  HTTP probe relay is not reused for the mesh link.
- The M2-1 probe is `Request/Ping` followed by `Response/Ping`; an idle link carries
  `Control/Ping`. The 16-byte body is the source `NodeId`, and the rig checks that each
  node's pass line names the other node's derived `NodeId`.
- Module TCP slot 1 is now a shared resource when the mesh is enabled: a future module that
  wants to use it must coordinate with MCP or replace this combined module. Runtime module
  dependencies (#217) are not available, so M2-1 does not claim a general slot manager.

## #247 implementation note — buffers, batching and a 64 KiB logical record

**Status:** implemented; no wire-format change.

The M2-1 transport kept a 2 KiB module RX ring and `max_burst_size = Some(1)`,
so every socket advertised about 1,474 bytes and each 64 KiB transfer needed
45 window-limited ticks.  #247 changes the transport and leaves the record
format alone:

- Seeq RX ring 16, `max_burst_size = Some(15)`, module RX 16 KiB / TX 32 KiB,
  Nagle off and a 1 ms delayed ACK;
- one `copy_nonoverlapping` from the uncached DMA buffer instead of a
  per-byte `read_volatile` loop;
- fixed-capacity decoder and send queue with an 8 KiB control reserve, and one
  flush per module tick;
- a 64 KiB **logical** record carried as four 16 KiB chunks using the existing
  `FrameHeader.offset`/`length` and `flags::MORE`/`EOS`.

The module linker asserts image + `.bss` stays below the next boot slot.  A
single 64 KiB wire record is still accepted by `mesh-transport` (the 64 KiB
bound is unchanged), but the module deliberately streams it: slot A has about
50 KiB of headroom after this change, and one record would head-of-line block
control for the whole transfer.  See
[`docs/research/bulk-batching.md`](../research/bulk-batching.md) for the host
model, the real two-kernel numbers and the remaining limits.
