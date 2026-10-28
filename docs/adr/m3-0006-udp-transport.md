# M3-ADR-0006 — module UDP services and the two-kernel datagram gate

**Status:** implemented and gated, but **blocked from landing**: the ABI 16 kernel fails the
M1 Wasm-autoload boot (see *A second failure, not yet explained*)
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
the engine finished with the descriptor.  Both were attempted in this slice; the datagram
path landed and the transmit rework did not, for a reason worth writing down (§Decision 5).

A QUIC profile cannot be evaluated on this machine until a datagram can leave one kernel and
arrive at another.  This ADR records that foundation.  It does **not** adopt QUIC, it does
not change the transport of any existing flow, and it leaves the transmit rework open.

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
5. **The single-descriptor transmit path is unchanged, and the non-blocking variant was
   rejected on evidence.**  The HPC3 clears `ACTIVE` when the transfer completes but
   publishes its descriptor writeback — `ENET_BC_TXD` into the descriptor's byte-count
   field — *after* that, from the Seeq thread.  A driver that reuses the one transmit
   descriptor as soon as `ACTIVE` clears races that writeback.  A non-blocking transmit was
   implemented, measured and withdrawn: it corrupted received data (a module download
   failed its SHA-256 check) and then kernel memory, and the same boot passes three times
   out of three with the original blocking path restored.  A replacement needs a transmit
   ring whose descriptors are each reused only after their own writeback has landed; that
   is a separate slice with its own evidence, not part of this ABI change.
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

### The rejected transmit rework

The M1 gate boots the kernel, fetches a 1.3 MiB Wasm policy module over HTTP, loads it into
slot B and runs it.  With the non-blocking transmit in place that boot failed, and the
failure moved with the code: a TLB refill trap after userland returned, a trap during the
module run, `autoload: wasm failed / module hash mismatch` (the *downloaded bytes* were
wrong), and a kernel panic in `receive` with `rx_index` holding `0x574242FF` instead of
`0..8`.  The last value was stable across runs.

The same boot passed on the unmodified tree three times out of three.  Bisecting by file
isolated the transmit change: with only `SeeqTxToken` changed to return without waiting
(completion reaped from `ACTIVE`), the boot failed two runs out of two with an identical
trap; with the original transmit path restored, it passed three runs out of three with the
Wasm policy adopted.

The emulator's HPC3 model explains why.  `set_active(false)` happens inside the DMA write,
and the descriptor writeback (`ENET_BC_TXD` into the descriptor's byte-count field) is
applied afterwards by the Seeq thread.  Reusing the descriptor on `ACTIVE` therefore races
a writeback that is still addressed to it, and the engine can be restarted against a
descriptor that is being rewritten.  The non-blocking transmit was withdrawn rather than
shipped with a known race.

### A second failure, not yet explained

The ABI 16 kernel also fails the M1 Wasm-autoload boot, and that failure is **not** the
transmit race above: it persists with the original transmit path restored.  It is
timing/memory-layout sensitive and it corrupts memory:

- with the ABI 16 kernel (UDP services present, slots set to zero, transmit untouched) the
  boot traps three runs out of three, jumping into hex text (`0x30643439` is ASCII `0d49`
  from the policy digest, `0x30303030` is `0000`);
- the same *code* with ~1 KiB of unrelated trap-handler diagnostics added passes three runs
  out of three, and the GDB stub changes the symptom to a module hash mismatch — the
  downloaded bytes themselves are wrong;
- `origin/main` passes seven runs out of seven, including with a 40 KiB `.bss` probe, a
  changed boot string, and a 16-cycle delay in the hot network path, so it is not simply
  "any perturbation".

Bisection so far: the ABI 16 `Services` table and the UDP services are the remaining
difference from `main` (the transmit change is withdrawn, `SOCKET_SLOTS` alone passes, the
ABI version *value* does not matter, and the failure reproduces with the JIT disabled).  The
corruption reaches downloaded module bytes and kernel memory, so it looks like a latent
race in the receive path that a different code layout resolves differently — but that is a
hypothesis, not a diagnosis, and the slice does not land until it is settled.

This is a transport prerequisite, not a QUIC result.  #255's remaining acceptance
criteria — guest crypto throughput, the linked n32 QUIC/TLS footprint, the CID-routed
relay design and a race-free transmit ring — are still open, and the GO/NO-GO ADR has not
been written.

## Consequences

- A module can move datagrams between kernels, which is the precondition for the QUIC
  profile and for any future UDP-based fabric.
- Ordinary TCP traffic is unchanged: same transmit path, same framing, same ring clamp
  (`max_burst_size = Some(7)`).
- The transmit path still spins, bounded, inside the transmit callback.  A module that
  sends a frame can block the kernel for up to one frame time.  That was true before this
  slice and remains true after it; the next transport slice owns the fix.
- The transmit completion handshake is now a known, documented hazard rather than an
  assumption: `ACTIVE` is necessary but not sufficient, and the descriptor writeback is the
  real signal.
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
- **Non-blocking transmit keyed on `ACTIVE`.**  Implemented and rejected on evidence: the
  descriptor writeback lands after `ACTIVE` clears, so the reuse races it and corrupts
  memory.  Keying on the writeback instead, or on a ring with one frame per descriptor,
  is the candidate design for the next slice.
- **Keeping the blocking transmit and adding a timeout.**  A timeout does not make the call
  non-blocking; it only chooses how long the kernel stalls.
- **Porting quinn, quiche, s2n-quic or neqo now.**  The study measured rustls alone at
  ~482 KiB against ~113 KiB of free module space.  The spike must produce a code-size
  measurement before any stack is chosen.

## Evidence standard

This ADR is a transport prerequisite.  It does not move the M2 or M3 marker evidence, and it
does not satisfy any #255 acceptance criterion on its own: the crypto throughput, the linked
n32 QUIC/TLS footprint and the CID-routed relay design remain open.
