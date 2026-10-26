# Wire efficiency: IRIX 7 custom protocols, transport paths and a v2 design space

**Status:** research / design input. No production wire format was changed.
**Scope:** the M2 mesh record protocol (`sys-primitives` + `mesh-transport` + the M2/M3
crates), the MCP control plane that carries it (`modules/mcp`), the kernel services and
socket path underneath, and the host rig paths (`tools/m2-rig`).
**Benchmark:** [`tools/protocol-bench`](../../tools/protocol-bench) — reproducible host
measurements of the shipped framing and of the transport paths around it.
**Companion decisions:** [M2-ADR-0002](../adr/m2-0002-transport.md) (TCP transport and
framing), [M2-ADR-0003](../adr/m2-0003-authority.md) (authority), [M3-ADR-0001](../adr/m3-0001-shard-slice.md)
(shard slice).

Evidence grades used throughout:

| Grade | Meaning |
|---|---|
| **[M]** | Measured on this host with `tools/protocol-bench`, reproducible with the commands in Appendix A. |
| **[C]** | Read directly from the code (constant, branch, call path, buffer size). |
| **[D]** | Derived arithmetic or a model built from [M] and [C]. |
| **[H]** | Hypothesis or expectation that this slice did **not** verify. |

Nothing here is a guest measurement. The guest is an emulated 50 MHz-class R4000; every
host number is a lower bound and the document says so wherever it matters.

---

## 1. Executive summary

1. **The per-message envelope is 324 bytes before any payload.** 4 bytes of record length
   + 128-byte `MeshHeader` + 128-byte `FrameHeader` + 64-byte `PayloadDescriptor`
   **[C]** (`crates/mesh-transport/src/lib.rs:46-55`,
   `crates/sys-primitives/src/lib.rs:944,1152,1468`), confirmed byte-for-byte by the
   shipped encoders **[M]**. A keepalive is 340 bytes on the wire to carry 16 bytes: 95.3 %
   overhead. An `Invoke` is 656 bytes: 324 of envelope, 320 of inline capability token and 12 of
   operation fields.
2. **Dispatch latency, not bytes, is the dominant cost.** The kernel's service loop is
   gated by the 10 ms CP0 timer tick **[C]** (`kernel/src/idle.rs:19`,
   `kernel/src/module.rs:222`), and the Seeq driver is explicitly polled **[C]**
   (`kernel/src/net.rs:170,606`). A request/response exchange between two ticked ends
   measures **10.0 ms p50** against **0.030 ms p50** when both ends block on the socket
   **[M]** — a **~330×** latency penalty. On the guest every hop that lands in a module
   costs a multiple of 10 ms.
3. **The MCP control plane costs four ticks per call.** The module answers `HTTP/1.0`
   with `Connection: close`, one request per connection, one listening slot, and the
   phase machine `Arm -> WaitRequest -> WaitClose -> Closing -> Arm` **[C]**
   (`modules/mcp/src/main.rs:486,596-830,1189-1210`). Modelled at the kernel cadence that
   is **40.0 ms p50 per tool call (25 req/s)** **[M]**, and it is also why the M3 rig
   retries `fetch_shard` every 500 ms **[C]** (`tools/m2-rig/src/shard.rs:349`).
   Keep-alive alone is 4.6× faster on the host **[M]**.
4. **The M3 hub's writer is a hard 2 ms-per-128-bytes tax.** `tools/m2-rig/src/hub.rs:235-240`
   writes every record in 128-byte chunks with a 2 ms sleep after each. Measured
   round-trip cost added by that forwarder: **8.5 ms** for a 340-byte record, **14.8 ms**
   for 656 bytes, **45.3 ms** for 2500 bytes **[M]**. It is rig-only code, but it is in
   every M3 measurement path, so M3 latency numbers do not describe the protocol.
5. **Bulk transfer is capped at roughly one TCP segment per round trip.** The Seeq device
   advertises `max_burst_size = Some(1)` **[C]** (`kernel/src/net.rs:251`), which makes
   smoltcp clamp the advertised receive window to `MTU - IP - TCP ≈ 1474` bytes **[C]**
   (`smoltcp-0.13.1/src/iface/packet.rs:155-162`). With a tick-bound RTT that is
   **≈147 KB/s** per connection; the module's RX ring is only 2 KiB and its TX ring 16 KiB
   **[C]** (`kernel/src/module.rs:684-685`).
6. **The CRC is a measurable CPU tax and covers less than it appears to.** `crc32c` is
   table-free, 8 dependent shift rounds per byte **[C]**
   (`crates/sys-primitives/src/lib.rs:1856`), and is computed four times per round trip
   (mesh header and frame header, on encode and on decode). On this host a 128-byte
   bitwise CRC costs **2.0 µs** and a 1024-byte one **20.5 µs**; a 256-entry table costs
   2.1 µs and 10.2 µs, i.e. **2× at 1 KiB** **[M]**. The CRC covers the 128-byte header
   only — the payload descriptor and body are **not** mesh-checksummed **[C]**.
7. **Membership is single-root fan-in with a signature per member per 5 s.** Every
   member refresh (5 s) makes the root build and Ed25519-sign a full member list
   **[C]** (`crates/mesh-join/src/node.rs:52`, `crates/mesh-join/src/authority.rs:258-296`).
   At 128 members that is 25.6 signatures/s at the root and 128 × 2500-byte
   confirmations per 5 s, ~64 KB/s, on the wire **[D]**. The signed payload is identical
   for all members except three fields, so this is O(N) work where O(1) per epoch would
   do.
8. **The v2 wins are large and come from three places, in order:** collapse the 324-byte
   envelope to a ~39-byte record (**8.7×** on a keepalive), carry capabilities by handle
   instead of by value (**656 → ~87 B** for an invoke, **7.5×**), and remove the 10 ms
   poll gate (**~330×** on dispatch latency). The first two are wire changes with a
   version negotiation; the third is a kernel change and belongs to a later slice.

### 1.1 Prioritised table

| # | Change | Owner | Effect | Evidence | Compatibility |
|---|---|---|---|---|---|
| 1 | Event-driven RX (Seeq interrupt or short adaptive poll while a socket is active) instead of the 10 ms `wait` gate | kernel | 10.0 ms → ~0.03–0.5 ms per exchange (20–330×) | **[M]** 10.0 ms vs 0.030 ms | none on the wire |
| 2 | Remove the `max_burst_size = Some(1)` window clamp | kernel | bulk 1.4 KB/RTT → up to the 16 KiB TX ring per RTT (~11×) | **[C]** smoltcp clamp, **[D]** | none on the wire |
| 3 | HTTP keep-alive (or a length-prefixed control channel) on the MCP slot | module | 40 ms → ~10 ms per tool call (4×), 25 → 100 req/s | **[M]** 40.0 ms vs 10.0 ms tick | server-initiated; no client change needed |
| 4 | v2 compact record header (32 B) with optional TLVs | wire | 324 → 39 B/record; keepalive 340 → 39 B (8.7×); invoke 656 → 87 B with a capability handle (7.5×) | **[C]** exact v1 layout, **[D]** | version nibble + magic, negotiated in `Hello` |
| 5 | Sign the epoch snapshot once per epoch, not per member | join | O(N) → O(1) signatures per refresh | **[C]** `authority.rs:295` | new confirmation body version |
| 6 | Batch/coalesce: several records per `tcp_send`, one flush per tick, pipelined tags | module | removes per-message syscalls; enables N-deep pipelining | **[M]** tick dominates | none |
| 7 | Table-driven CRC32C (same polynomial, same value) | sys-primitives | 2–4× on header validation | **[M]** 20.5 → 10.2 µs per KiB | **none** — identical CRC value |
| 8 | Remove the M3 hub's 128 B / 2 ms chunk writer | host rig | 8.5–45 ms → <1 ms per forwarded record | **[M]** | none |
| 9 | 16 KiB module RX ring (currently 2 KiB) and credit-based flow control | kernel | stops window stalls on bulk streams | **[C]** `module.rs:684` | none |
| 10 | Peer authentication (Noise/TLS-PSK) and a replay window | transport | security, not speed | **[C]** M2-ADR-0002 §6 | new transport |

The ranking is by effect per unit of risk: 7 and 8 are free, 3 and 6 are module-only, 1
and 2 are one-file kernel changes with the largest payoff, and 4/5/10 are the protocol v2
programme.

---

## 2. Scope and method

### 2.1 What was read

- `crates/mesh-transport` (framing, decoder, send queue, deadlines, tags, stats).
- `crates/sys-primitives` (`MeshHeader`, `MeshEnvelope`, `MessageFrame`, `FrameHeader`,
  `PayloadDescriptor`, `Capability`, `Completion`, `TraceContext`, `crc32c`).
- `crates/mesh-join`, `mesh-route`, `mesh-migrate`, `mesh-core`, `mesh-service`.
- `modules/mcp/src/mesh.rs` and `main.rs` (socket state machines, HTTP/JSON-RPC, buffers).
- `kernel/src/module.rs`, `net.rs`, `http.rs`, `tls.rs`, `idle.rs`, `autoload.rs`.
- `tools/m2-rig/src/{relay,hub,shard,rig}.rs`.
- CI: `.github/workflows/ci.yml` mesh/module gates, and the M1/M2/M3 evidence contracts in
  `docs/plan/m2-rig.md` and `docs/plan/m3-rig.md`.

### 2.2 How the measurements were made

`tools/protocol-bench` encodes and decodes with the **shipped** functions
(`mesh_transport::encode_record`, `decode_record`, `SendQueue`, and the
`mesh-join`/`mesh-route`/`mesh-migrate` body encoders). It never re-implements the wire
format. The network sections use real TCP sockets on loopback and reproduce the loop
shape of the code under test:

| Bench path | Reproduces | Source |
|---|---|---|
| `blocking` | an event-driven floor (both ends in a blocking read) | — |
| `tick10` | 10 ms kernel tick, `READ_CHUNK = 1024`, `READS_PER_TICK = 8`, one flush per tick, both ends | `kernel/src/idle.rs:19`, `modules/mcp/src/mesh.rs:86-87` |
| `relay` | two TCP hops, 8 KiB pumps, `TCP_NODELAY` | `tools/m2-rig/src/relay.rs:126,288-400` |
| `hub` | record-oriented forwarder, 128-byte chunks, 2 ms sleep per chunk, both directions | `tools/m2-rig/src/hub.rs:235-240` |
| `http close` | HTTP/1.0, one request per connection, one listening slot | `modules/mcp/src/main.rs:1189-1210` |
| `http tick` | the `Arm -> WaitRequest -> WaitClose -> Closing -> Arm` phase machine at 10 ms | `modules/mcp/src/main.rs:596-830` |

Host: Intel Core i5-8500T @ 2.10 GHz, 6 vCPU, Linux 6.18.55, rustc 1.99.0, release build.
The host is shared and noisy: repeat runs of the codec section vary by 2–3× in the tail.
Percentile tables are from a single run; the qualitative ratios reproduced across runs.

### 2.3 What this slice did not do

No guest run, no `m2-rig` run, no packet capture, no Ed25519 timing on the guest, no real
Indy. The M1/M2/M3 CI gates were not re-run here; §7 states the gates that must stay green
when the recommendations are implemented.

---

## 3. Current-state inventory

### 3.1 Two stacks, one of which is on the wire

There are two things called "mesh" in this repository:

- **The M1 kernel mesh counters** (`kernel/src/mesh.rs`, `m1: mesh=0/0/0`) — a
  single-node counter set that never puts a byte on a wire. The `BOOT-READY ... mesh=single`
  marker is this view.
- **The M2/M3 mesh transport** — `MeshEnvelope` records over TCP, implemented inside
  `modules/mcp` (module TCP slot 1, guest port 7000) and driven by the host rig's relay or
  hub. This document is about the second one only.

The kernel contributes no mesh framing of its own: it provides generic per-slot TCP
services (`kernel/src/module.rs:697-800`) and the module owns the protocol. That is why
every wire change in scope is either a shared-crate change (`sys-primitives`,
`mesh-transport`, `mesh-join`, `mesh-route`, `mesh-migrate`, `modules/mcp`) or a host-rig
change.

### 3.2 Record framing

```text
u32 little-endian length L        (4 bytes; L counts the envelope only)
L bytes of MeshEnvelope
  ├─ MeshHeader        128 bytes   magic "I7MH" (0x49 0x37 0x4D 0x48), version 1
  ├─ FrameHeader       128 bytes   magic "7FM1", version 1
  ├─ PayloadDescriptor  64 bytes
  └─ inline body       inline_len bytes
```

Source: `crates/mesh-transport/src/lib.rs:20-32,46-55` and `crates/sys-primitives/src/lib.rs:1415-1470`.
The prefix is little-endian even though the guest CPU is big-endian MIPS; every field is
encoded byte-by-byte (`put_u32`/`get_u32` in `sys-primitives`), never by writing a
`#[repr(C)]` struct. The decoder enforces `L == 128 + header.frame_len`, so a prefix/header
disagreement is a hard error, and it refuses `L > 65536` before allocating
(`mesh-transport/src/lib.rs:375-393`).

Bounds **[C]**:

| Constant | Value | Where |
|---|---|---|
| `LENGTH_PREFIX_LEN` | 4 | `mesh-transport/src/lib.rs:46` |
| `MAX_RECORD_LEN` | 65 536 | `mesh-transport/src/lib.rs:49` |
| `MIN_ENVELOPE_LEN` | 320 | `mesh-transport/src/lib.rs:52` |
| `MAX_QUEUED_BYTES` | 65 540 | `mesh-transport/src/lib.rs:55` |
| `StreamDecoder` initial capacity | 4 KiB | `mesh-transport/src/lib.rs:265` |
| `SendQueue` initial capacity | 4 KiB | `mesh-transport/src/lib.rs:468` |

### 3.3 Envelope byte map

`MeshHeader` (128 bytes) **[C]** (`sys-primitives/src/lib.rs:1415-1470`):

| Offset | Size | Field | Notes |
|---|---|---|---|
| 0 | 4 | magic `I7MH` | |
| 4 | 1 | version = 1 | |
| 5 | 1 | flags | FORWARD, REPLY, MOVED, PROBE, MIGRATE, DROP |
| 6 | 2 | header_len = 128 | |
| 8 | 16 | `src` NodeId | |
| 24 | 16 | `dst` NodeId | |
| 40 | 16 | `home` NodeId | |
| 56 | 16 | `route_hint` NodeId | |
| 72 | 4 | src_epoch | |
| 76 | 4 | dst_epoch | |
| 80 | 4 | home_epoch | |
| 84 | 4 | object_epoch | |
| 88 | 8 | request_id | |
| 96 | 8 | reply_to | |
| 104 | 1 | ttl (≤ 8) | |
| 105 | 1 | hop_count | |
| 106 | 1 | priority | |
| 107 | 1 | reserved = 0 | |
| 108 | 4 | frame_len | |
| 112 | 12 | reserved = 0 | |
| 124 | 4 | CRC32C over bytes 0..124 | |

**64 of the 128 bytes are four 16-byte NodeIds**, and 16 bytes are reserved. The
`FrameHeader` adds another 128 bytes, of which 32 are the `ObjectId`, 24 are trace
context, 8 are a capability handle, and 12 are trailing/reserved. The
`PayloadDescriptor` is 64 bytes, of which 32 are the CAS `ObjectId` and 20 are
offset/length/kind that are zero for an inline frame.

For a keepalive (`Control/Ping`, 16-byte body) **324 of the 340 bytes** are metadata; the
useful fraction is 4.7 %.

### 3.4 Per-message wire inventory [M]

Produced by `protocol-bench frames` from the shipped encoders (Appendix B.1 has the full
output):

| Message | Body | Record | Overhead |
|---|---:|---:|---:|
| `Control/Ping` (heartbeat) | 16 | 340 | 95.3 % |
| `Request/Ping` (probe) | 16 | 340 | 95.3 % |
| `Request/Hello` `JOIN_REQUEST` | 64 | 388 | 83.5 % |
| `Response/Hello` `EPOCH_CONFIRM` (2 members) | 160 | 484 | 66.9 % |
| `Response/Hello` `EPOCH_CONFIRM` (8 members) | 256 | 580 | 55.9 % |
| `Response/Hello` `EPOCH_CONFIRM` (128 members) | 2 176 | 2 500 | 13.0 % |
| `Completion/Hello` `JOIN_ACK` | 56 | 380 | 85.3 % |
| `Response/Hello` `EPOCH_REFUSED` | 40 | 364 | 89.0 % |
| `Request/Invoke` (capability + op) | 332 | 656 | 49.4 % |
| `Response/Invoke` `RESULT` | 100 | 424 | 76.4 % |
| `Response/Invoke` `MOVED` | 96 | 420 | 77.1 % |
| `Request/RouteQuery` `HOME_MOVE` | 176 | 500 | 64.8 % |
| `Response/RouteQuery` `MOVE_ACK` | 64 | 388 | 83.5 % |
| `Response/*` `ERROR` | 48 | 372 | 87.1 % |
| `Request/Migrate` `AUTHORISE` | 172 | 496 | 65.3 % |
| `Request/Migrate` `PREPARE` | 206 | 530 | 61.1 % |
| `Response/Migrate` `READY` | 204 | 528 | 61.4 % |
| `Request/Migrate` `STATE` (248 B) | 420 | 744 | 43.5 % |
| `Response/Migrate` `COMMIT` | 204 | 528 | 61.4 % |
| `Completion/Migrate` `ACK` | 204 | 528 | 61.4 % |
| `Request/Migrate` `DELIVER` | 389 | 713 | 45.4 % |
| `Response/Migrate` `RESULT` | 109 | 433 | 74.8 % |
| `Request/Migrate` `RESUME` | 348 | 672 | 48.2 % |
| `Response/Migrate` `ERROR` | 34 | 358 | 90.5 % |
| inline payload at the 64 KiB bound | 65 216 | 65 540 | 0.5 % |

Two structural facts fall out of the table:

- The **320-byte inline capability** (`Capability::LEN`, `sys-primitives:514`) is the
  largest single field in `INVOKE`, `DELIVER` and `RESUME`, and it is re-sent on every
  operation even though the same token authorises every call in its validity window.
- Overhead is only material at small sizes, but **small sizes are the protocol's steady
  state**: heartbeats, join refreshes, lease renewals, route probes and `MOVED`
  redirects are all sub-600-byte records.

### 3.5 Exchange and round-trip counts [C]

| Flow | Records | Round trips | Bytes (2-member case) |
|---|---|---:|---:|
| M2-1 connect probe | `Request/Ping` + `Response/Ping` | 1 | 680 |
| M2-2 join | `JOIN_REQUEST` + `EPOCH_CONFIRM` + `JOIN_ACK` | 1.5 | 1 252 |
| M2-2 refresh (per member, every 5 s) | `JOIN_REQUEST` + `EPOCH_CONFIRM` | 1 | 968 |
| M2-3 homes | `GET /mesh/homes` over HTTP | 1 HTTP request | body ~ N × 100 B |
| M2-4 remote stat | `Request/Invoke` + `Response/Invoke` | 1 | 1 080 |
| M2-4 stat through one `MOVED` | above + `MOVED` + retry | 2 | 2 160 |
| M2-4 home move | `HOME_MOVE` + `MOVE_ACK` | 1 | 888 |
| M2-5 migration | `AUTHORISE`, `PREPARE`, `READY`, `STATE`, `COMMIT`, `ACK` | 3 | 3 354 |
| M2-6 failure | heartbeats only until `SUSPECT_AFTER_MS` (6 s) / `FAIL_AFTER_MS` (12 s) | — | 340 per 5 s |
| MCP tool call | HTTP request + response | 1 | 333 + response + TCP setup/teardown |

The join handshake is the cheapest part of the protocol. The expensive parts are the
*control plane* around it (HTTP) and the *periodic* traffic (refresh signatures and
heartbeats).

### 3.6 Connection model

- **One TCP connection per peer link.** The link is full-duplex; both sides may send.
- The ADR says the lower `NodeId` initiates and the higher listens on 7000
  (`docs/adr/m2-0002-transport.md` §3), but the guest has **no TCP connect service**
  (`kernel/src/module.rs` exposes only `tcp_listen`, `tcp_state`, `tcp_recv`, `tcp_send`,
  `tcp_close`, `tcp_abort`). The shipped M2-1 therefore makes **both** nodes listen and
  the host relay dials both **[C]** (`modules/mcp/src/mesh.rs:1-20`).
- Module TCP slots: **2** (`kernel/src/module.rs:681`). MCP keeps slot 0 for HTTP and
  takes slot 1 for the mesh listener when `mesh.peer` is configured
  (`modules/mcp/src/mesh.rs:80-84`). With the mesh enabled there is **exactly one HTTP
  connection at a time**.
- M3 (host hub): each guest still has one listening socket; the hub connects to every
  guest and multiplexes member→root traffic onto the root's single connection. The root
  therefore sees several `NodeId` sources on one byte stream
  (`modules/mcp/src/mesh.rs:107-113`).
- Keepalive: smoltcp TCP keepalive 60 s (`kernel/src/module.rs:718`) **plus** a mesh
  heartbeat every 5 s. With failure detection enabled the mesh heartbeat timeout is
  raised to 300 s and the 12 s membership detector is the real failure path
  (`modules/mcp/src/mesh.rs:208-212`).

### 3.7 Queues, buffers and limits

| Buffer | Size | Where | Consequence |
|---|---:|---|---|
| Module TCP RX ring per slot | 2 048 B | `kernel/src/module.rs:684` | smaller than one full window; a 1460-byte segment plus a second segment can stall |
| Module TCP TX ring per slot | 16 384 B | `kernel/src/module.rs:685` | 11× the advertised window, so the socket, not the ring, is the limit |
| Kernel HTTP client RX/TX | 4 096 / 2 048 B | `kernel/src/http.rs:17-18` | module fetch only |
| TLS record read buffer | 16 640 B | `kernel/src/tls.rs:440` | one full TLS record |
| MCP request buffer | 2 048 B | `modules/mcp/src/main.rs:49` | an oversized request with no header terminator is dispatched as-is |
| MCP response buffer | 16 384 B | `modules/mcp/src/main.rs:504` | **silently truncates**: `Out::push` clamps |
| Mesh send queue | 65 540 B | `mesh-transport/src/lib.rs:55` | one maximum record |
| Mesh decoder buffer | ≤ 65 540 B | `mesh-transport/src/lib.rs:265,271` | one record, then a `Full` error until consumed |
| Join RX/TX scratch | 4 096 B each | `modules/mcp/src/mesh.rs:139-141` | every received body is copied here |
| Pending tag table | 8 | `modules/mcp/src/mesh.rs:88` | one in-flight request per link, mostly |
| M3 hub per-link queue | 256 records | `tools/m2-rig/src/hub.rs:38` | drop on full (counted as backpressure) |

`Out::push` clamping is a real correctness edge: `fs.read` can return up to 8 KiB of file
bytes, `json_string` expands `"`, `\`, `\n`, `\r`, `\t` by 2×, and the response buffer is
16 KiB, so a file of mostly control bytes can silently produce a shorter body than the
file. JSON `Content-Length` still matches the truncated body, so the client sees a
well-formed but truncated response **[C]** (`modules/mcp/src/main.rs:1702-1761`).

### 3.8 Polling cadence

```text
CP0 timer, 10 ms  ──> idle::wait() returns
                      └─> idle_loop: gfx flush, net::poll(), module::poll(), telnet::poll()
                          ├─ net::poll(): one iface.poll() pass, Seeq RX/TX polled
                          └─ module::poll(): each module tick if now - last >= 10 ms
                              └─ mcp::tick -> mesh::tick (arm / wait / connected)
```

- Timer interval `330_000` Count ticks = 10 ms at iris's 33 MHz Count rate
  (`kernel/src/idle.rs:19`).
- Module tick gate: `if now.saturating_sub(last) < 10 { continue; }`
  (`kernel/src/module.rs:222`).
- The Seeq comment is explicit: "interrupts left to polling" (`kernel/src/net.rs:170`),
  and `net::poll()` is called only from the idle loop (`kernel/src/net.rs:606`).
- `idle::wait()` parks until the next interrupt; the timer is the only guaranteed
  source, so the loop period is **10 ms** unless another device interrupts **[C]**.

Consequences **[D]**:

- A record that arrives just after a poll waits ~10 ms before it is even read.
- A two-node exchange spans **1–2 ticks** depending on the phase of the two tick
  loops: measured p50 **10.0 ms**, p90 **20.0 ms** (§4.2, [M]).
- The MCP HTTP phase machine needs up to **4 ticks per request** ([M], §4.3).

The kernel's own HTTP/TLS client is the exception: it busy-polls `net::poll()` inside the
fetch loop (`kernel/src/http.rs:170-260`, `kernel/src/tls.rs:56-62`), so a module download
is not tick-gated — but it is window-gated (§4.4).

### 3.9 Copies and syscalls per message [C]

Send path in the module (`modules/mcp/src/mesh.rs:1969-2010`):

1. `MeshHeader::new` + `MessageFrame::inline` — structs only.
2. `MeshHeader::encode()` → a 128-byte stack array (materialise #1).
3. `MeshEnvelope::encode` → copy into the queue slice (copy #1).
4. `FrameHeader::encode()` → 128-byte stack array (materialise #2), copied into the queue
   (copy #2).
5. `PayloadDescriptor::encode()` → 64-byte stack array (materialise #3), copied (copy #3).
6. Body copied into the queue (copy #4).
7. `SendQueue::flush` → `tcp_send` → smoltcp `send_slice` memcpy into the TX ring
   (copy #5).
8. smoltcp → `TxToken::consume` → copy into `tx_buf` (copy #6), then one HPC3 DMA.

Receive path:

1. HPC3 DMA writes the frame into an uncached RX buffer.
2. `SeeqDevice::receive` copies it **byte by byte with `read_volatile`** into `rx_frame`
   (`kernel/src/net.rs:279-285`) — up to 1514 uncached reads per packet.
3. smoltcp copies the payload into the socket RX ring (copy #2).
4. `tcp_recv` copies into the module's 1024-byte stack chunk (copy #3).
5. `StreamDecoder::push` copies into the decoder vector (copy #4).
6. `decode_record` is zero-copy over the decoder buffer.
7. `mesh::feed` copies the body into `JOIN_RX` for **every** message, not just join
   bodies (`modules/mcp/src/mesh.rs:1256`).

So one exchange is **~5–6 copies and one syscall per direction** (plus the per-byte
volatile copy on receive). The copies are not the latency problem at 340 bytes — the 10 ms gate is
— but they are the throughput ceiling once the gate is removed.

### 3.10 Failure, retry, deadlines

| Mechanism | Value | Where | Behaviour |
|---|---|---|---|
| Connect deadline | 5 s | `mesh-transport:58` | constant defined; the guest never connects |
| Receive deadline | 10 s | `mesh-transport:61` | partial record → disconnect (`mesh.rs:1113-1122`) |
| Send deadline | 10 s | `mesh-transport:64` | blocked queue → disconnect |
| Heartbeat interval | 5 s | `mesh-transport:67` | `Control/Ping`, 340 B |
| Heartbeat timeout | 30 s (300 s with failure detection) | `mesh-transport:74`, `mesh.rs:208-212` | link torn down |
| Reconnect backoff | 250 ms → 10 s, ±25 % jitter | `mesh-transport:77-80` | implemented but **unused**: the guest can only listen |
| Join retry | 5 s, 5 attempts | `mesh-join/src/node.rs:38-40` | then `Tick::TimedOut`, stuck until link-down |
| Join refresh | 5 s per converged member | `mesh-join/src/node.rs:52` | full signed confirmation each time |
| Suspect / failed | 6 s / 12 s | `mesh-service:47-50` | membership removal, home recompute |
| Route retry | exactly one after `MOVED` | `mesh-route/src/lib.rs:111` | a second `MOVED` fails closed |
| HTTP close timeout | 5 s | `modules/mcp/src/main.rs:486` | abort and reclaim the single slot |

Two failure-behaviour gaps:

- **A link loss discards in-flight work without a terminal completion.**
  `disconnect` clears the send queue and rebuilds the pending table
  (`modules/mcp/src/mesh.rs:2080-2107`); the route/migrate state machines see
  `on_link_down`, but a client waiting on a tag gets nothing at the fabric level. For
  v2 this needs an explicit `Status::Broken` completion per tag.
- **The M3 hub does not reconnect.** `hub.rs` dials each guest once (`connect`, line 212)
  and the reader thread exits on EOF; the link stays in the registry with a dead writer,
  so a restarted guest is unreachable until the hub restarts **[C]**.

### 3.11 Observability

`Stats` (`mesh-transport/src/lib.rs:574-613`) counts connects, disconnects, frames,
bytes, short reads/writes, receive/send/connect deadlines, oversize/short/header/protocol
rejects, heartbeats, heartbeat timeouts and cancelled/expired tags. The module logs a
single `mesh-transport: pass peer=... rx_frames=... tx_frames=... rx_bytes=... tx_bytes=...
rejects=... connects=...` line once per link (`mesh.rs:2016-2040`). There is **no
per-opcode latency histogram, no queue-depth gauge and no per-peer byte counter**; the M3
hub has per-source counters (`HubStats`) but the guests do not. For v2, tag-level
timestamps are the missing measurement that would make the ticket claims falsifiable.

### 3.12 Host rig paths

| Path | Hops | Copies | Notes |
|---|---|---|---|
| `tools/m2-rig` mesh relay | guest → host forward → relay → host forward → guest | 4 TCP copies + 2 × 8 KiB pump buffers | waits for first bytes from both guests before pumping (`relay.rs:369-395`), idle timeout 120 s, nodelay |
| `tools/m2-rig` M3 hub | guest → hub → guest (+ root fan-in) | 2 reads + 1 alloc + 2 memcpys + mpsc per record | 128-byte chunks, 2 ms sleeps, 256-deep queue, no reconnect |
| HTTP probe | `reqwest`-style client → host forward → guest port 80 | — | HTTP/1.0 close per request; retried every 500 ms in the shard rig |

The hub's per-record work `[C]` (`hub.rs:307-390`): `read_exact(4)` (syscall), `read_exact(body)`
(syscall), `Vec::with_capacity(4+len)` (allocation), two `extend_from_slice` (copies),
`try_send` on a bounded channel, `recv` in the writer, then `write_all` × chunks with
2 ms sleeps. It is a correct byte pump and an expensive one.

---

## 4. Measurements and calculations

### 4.1 Codec cost [M]

`protocol-bench codec`, one record encoded and decoded with the shipped code, median of
five batches:

| Payload | Record | Round trip | ns/byte | Send-queue path | Round trips/s |
|---:|---:|---:|---:|---:|---:|
| 0 | 324 | 11.62 µs | 35.9 | 4.88 µs | 86 073 |
| 16 | 340 | 10.03 µs | 29.5 | 7.82 µs | 99 671 |
| 332 | 656 | 10.95 µs | 16.7 | 4.08 µs | 91 324 |
| 1 024 | 1 348 | 7.32 µs | 5.4 | 6.93 µs | 136 574 |
| 8 192 | 8 516 | 12.88 µs | 1.5 | 4.30 µs | 77 621 |
| 65 212 | 65 536 | 18.98 µs | 0.29 | 17.98 µs | 52 684 |

The sub-2 KiB cases are dominated by **four** table-free CRC32C computations of 128 bytes
each (mesh header and frame header, on both encode and decode), not by the payload copy.
The CRC micro-benchmark:

| Variant | 128 B | 1 024 B |
|---|---:|---:|
| shipped bitwise | 2.036 µs | 20.527 µs |
| 256-entry table | 2.111 µs | 10.181 µs |

At 128 bytes the two are indistinguishable because the table build and cache miss offset
the win; at 1 KiB the table is 2× faster. A 128-byte bitwise CRC is ~40 dependent
cycles/byte; the 1 KiB table fits the guest's 8–16 KiB D-cache. **A table changes only the
implementation, not the CRC value, so the wire is unaffected.**

For the guest, scale this by the CPU ratio (unknown, order 10–50×) **[H]**. Even at 50×,
a 656-byte round trip is ~0.5 ms of CPU — material against a 10 ms tick but not dominant.
It becomes dominant once the tick is removed (§6.6).

### 4.2 Round-trip latency matrix [M]

`protocol-bench net`, one request record and one response record, 2000 iterations for
blocking paths and 300 for ticked paths:

| Path | Record | p50 | p90 | p99 | Serial rate (p50) |
|---|---:|---:|---:|---:|---:|
| blocking (event-driven) | 340 | **0.030 ms** | 0.050 | 2.232 | 33 037/s |
| two-hop host relay | 340 | 0.053 ms | 0.072 | 2.547 | 18 735/s |
| 10 ms tick, both ends | 340 | **10.005 ms** | 11.687 | 19.561 | 100/s |
| relay + 10 ms tick | 340 | 10.002 ms | 10.252 | 12.090 | 100/s |
| blocking | 656 | 0.028 ms | 0.046 | 0.207 | 35 420/s |
| two-hop host relay | 656 | 0.055 ms | 0.075 | 0.241 | 18 347/s |
| 10 ms tick, both ends | 656 | 10.083 ms | 20.023 | 20.789 | 99/s |
| relay + 10 ms tick | 656 | 10.029 ms | 20.002 | 20.143 | 100/s |
| M3 hub writer | 340 | 8.492 ms | 9.262 | 11.142 | 118/s |
| M3 hub writer | 656 | 14.838 ms | 16.371 | 20.286 | 67/s |
| M3 hub writer | 2 500 | 45.275 ms | 53.574 | 65.956 | 22/s |

Readings:

- **The tick is ~330× the network cost.** A 340-byte exchange needs 30 µs of real work on
  this host and 10 000 µs of waiting.
- **The relay adds ~23 µs** when nothing else dominates (0.030 → 0.053 ms), i.e. one
  extra host round trip plus two pumps. It is not the problem.
- **The hub writer adds `chunks × 2 ms` per direction**: 3 chunks → ~8.5 ms,
  6 chunks → ~14.8 ms, 20 chunks → ~45.3 ms. The M3 measurement path is dominated by a
  test-harness sleep, not by the protocol, so **no M3 latency claim should be made from
  those runs**.

### 4.3 MCP control plane [M]

`protocol-bench http`, a server shaped exactly like `modules/mcp/src/main.rs`:

| Response size | close-per-request | keep-alive | close + 10 ms tick |
|---:|---:|---:|---:|
| 291 B (status) | 0.107 ms | 0.023 ms | **39.996 ms** |
| 527 B (`resources/list`-like) | 0.113 ms | 0.024 ms | 39.994 ms |
| 1 192 B (`block.read`, 1 KiB hex) | 0.113 ms | 0.023 ms | 40.011 ms |
| 2 350 B (`tools/list`, measured 2 258 B body) | 0.101 ms | 0.022 ms | 39.995 ms |
| 8 392 B (`fs.read` at the print cap) | 0.118 ms | 0.029 ms | 40.002 ms |

- Connection reuse is **4.6×** on the host and does not depend on body size.
- The module's phase machine costs **exactly 4 ticks = 40 ms**, i.e. **25 requests/s**,
  regardless of payload. That is the ceiling for every `/mesh/*` control action, every
  MCP tool call and every rig step.
- The rig's own retry (`fetch_shard` every 500 ms) is a symptom of this ceiling.

The 40 ms is a model of the phase machine, not a guest trace; the number of ticks is what
the code requires: `Arm`→`WaitRequest` (accept), read+handle+send, `WaitClose` (peer FIN),
`Closing`→`Arm` (`main.rs:596-850`). Any client that keeps the connection open until a
timeout adds the 5 s `CLOSE_TIMEOUT_MS`.

### 4.4 Bulk throughput: the window clamp [C]+[D]

`SeeqDevice::capabilities` sets `max_transmission_unit = 1514` and
`max_burst_size = Some(1)` (`kernel/src/net.rs:250-251`). smoltcp uses the burst size to
clamp the advertised window:

```rust
// smoltcp-0.13.1/src/iface/packet.rs:155
if let Some(max_burst_size) = caps.max_burst_size {
    let mut max_segment_size = caps.max_transmission_unit;
    max_segment_size -= ip.header_len();
    max_segment_size -= tcp.header_len();
    let max_window_size = max_burst_size * max_segment_size;   // 1 × ~1474
    if tcp.window_len as usize > max_window_size {
        tcp.window_len = max_window_size as u16;
    }
}
```

So every TCP socket in the kernel advertises **~1474 bytes**, i.e. one segment, whatever
the buffer size. Steady-state throughput is then `MSS / RTT`:

| RTT | Window | Ceiling |
|---|---:|---:|
| 0.1 ms (host loopback, busy-polled) | 1 460 B | 14.6 MB/s |
| 1 ms | 1 460 B | 1.46 MB/s |
| 10 ms (tick-bound) | 1 460 B | **0.146 MB/s** |
| 20 ms (both ends ticked) | 1 460 B | 0.073 MB/s |

Worked examples **[D]**:

- A 3.9 MB `screen.bmp` (1280×1024×3) is streamed one 3840-byte row per tick
  (`main.rs:715-780`), so it cannot finish in less than **1024 ticks = 10.2 s** even with
  an infinite window. With the 1460-byte window it is worse.
- A 256 KiB module fetch (the module slot size) needs ~180 window-limited exchanges. On
  the busy-polled TLS path at ~0.1 ms per exchange that is ~18 ms of network plus the
  guest's TLS/SHA-256 CPU; on a tick-gated path it would be ~1.8 s.

### 4.5 Where the bytes and the milliseconds go for one tool call

A `tools/call` for `status`:

| Component | Bytes | Notes |
|---|---:|---|
| TCP handshake | 3 segments, 0 payload | SYN, SYN-ACK, ACK |
| HTTP request | 333 | 200-byte header block (Host, Accept, Connection, Authorization Bearer 64, Content-Length) + 88-byte JSON |
| HTTP response header | ~92 | `HTTP/1.0 200 OK`, `Content-Type`, `Connection: close`, `Content-Length` |
| JSON-RPC response body | 291 | `{"jsonrpc":"2.0","id":1,"result":{"content":[{"type":"text","text":"..."}]}}` |
| TCP teardown | 3–4 segments | FIN/ACK each way, or RST on the 5 s timeout |
| **Total** | **~716 bytes, 10–11 segments** | versus **~316 bytes, 4 segments** with keep-alive |

Latency on the guest: 1 tick to observe and read the connection, 1 tick for
handle+send, 1–2 ticks for the close path, plus the client's own round trip. Measured
model: **40 ms**, amortised to ~10 ms with keep-alive.

### 4.6 What the numbers do not include

- Guest CPU (emulated R4000) for encode/decode, CRC, Ed25519 and JSON.
- The kernel's per-byte volatile copy of every received frame.
- The `_start`/module-tick cost of the MCP module itself (JSON parse, EROFS, overlay).
- Any host scheduling noise beyond the p99 shown.

Every one of these makes the guest slower, never faster.

---

## 5. Bottleneck ranking

Starting hypothesis (from the brief) versus what the code and measurements say.

| Rank | Hypothesis | Verdict | Evidence |
|---|---|---|---|
| 1 | 10 ms tick dominates latency | **Confirmed, dominant** | 10.0 ms vs 0.030 ms **[M]**; gate at `module.rs:222` **[C]** |
| 2 | Host relay adds latency | **Refuted as a bottleneck** (23 µs, 1.8×) but it is a *single point of failure and a fan-in choke* | 0.030 → 0.053 ms **[M]** |
| 3 | M3 hub chunk writer | **Confirmed for M3 runs only**: 8.5–45 ms added per record | **[M]**; `hub.rs:235-240` **[C]** |
| 4 | HTTP `Connection: close` per request | **Confirmed**, 4.6× on host, 4 ticks on guest | **[M]** |
| 5 | JSON/base64 overhead | **Partly confirmed**: JSON header + hex doubles `block.read` (512 → 1024 chars), base64 is only on the screen path; JSON itself is ~90 bytes for a tool call | **[C]** |
| 6 | 2 KiB RX / 16 KiB TX | **Confirmed for bulk, not for latency**: RX ring is smaller than one window; TX is 11× the window so it never binds | **[C]** |
| 7 | 324-byte envelope | **Confirmed for bytes, not for latency**: 8.7× on heartbeats, but a 340-byte message still costs 30 µs on a host and 10 ms on the guest | **[M]** |
| 8 | Per-peer connection | **Not a problem at 2 nodes; a fan-in problem at 8**: the root multiplexes all members on one 2 KiB-RX connection | **[C]** |
| 9 | Single-root fan-in + refresh signatures | **Confirmed as a scaling risk**, invisible at 2 nodes | **[C]**/**[D]** |
| 10 | TLS/HTTP on the wire | **Not on the mesh wire**: the mesh is plain TCP; TLS is only the module-fetch path | **[C]** |
| 11 | Poll-to-event | **Confirmed as the highest-value structural fix** | **[M]**/**[C]** |

Throughput ranking (what limits bytes/s today):

1. **Window clamp** (~1.4 KB in flight) — [C]/[D].
2. **Tick-gated dispatch** (one poll per 10 ms) — [C]/[M].
3. **Module RX ring 2 KiB** — [C].
4. **Copies and the per-byte volatile RX copy** — [C].
5. Wire overhead (324 B) — [M]; last, because it matters only once 1–4 are fixed.

Latency ranking:

1. 10 ms tick (~330×) — [M].
2. MCP HTTP phase machine, 4 ticks (40 ms) — [M].
3. Hub chunk writer, 8.5–45 ms (rig only) — [M].
4. Join retry/refresh intervals, 5 s — [C].
5. Encode/decode, 7–19 µs host (≈0.4–1 ms guest at 50×) — [M]/[H].

---

## 6. Protocol v2 design space

The goal is a wire that is cheap for the steady state (heartbeats, refreshes, invokes),
bounded for bulk, and honest about its security. The v1 format is frozen for M1/M2/M3
evidence; v2 is a negotiated addition, not a replacement.

### 6.1 Framing and header

**Problem.** 324 bytes per record, four 16-byte NodeIds, a 32-byte reserved field, and
two independent 128-byte headers with two CRCs.

The v2 record below is counted **including** its 3-byte length prefix and 4-byte
trailing CRC, so it is directly comparable with the v1 "record" column of §3.4 (which
includes the 4-byte prefix).

**Proposal A (recommended): one compact header with optional TLVs.**

```text
record:
  u8   len_prefix_kind    0 = u16 length follows, 1 = u32 length follows
  u16/u32 length          total bytes after this field
  header (32 bytes):
    +0   u8   ver_kind    version 2 (4 bits) | MessageKind (3 bits) | has_tlv (1 bit)
    +1   u8   flags       REPLY, MOVED, PROBE, MIGRATE, TRACE, CAP_HANDLE, CAP_INLINE
    +2   u8   opcode      0xFF = escape (u16 follows in the TLV area)
    +3   u8   hdr_words   header length in 4-byte words (8..15)
    +4   u16  src_idx     membership index, 0xFFFF = inline NodeId TLV
    +6   u16  dst_idx
    +8   u16  home_idx
    +10  u16  route_idx   0xFFFF = none
    +12  u32  tag_stream  tag:16 | stream:16
    +16  u32  request_id  (omitted when flags.NEW_REQUEST = 0)
    +20  u32  reply_to    (omitted when flags.REPLY = 0)
    +24  u32  epochs      home_epoch:10 | object_epoch:10 | ttl:4 | hop:4 | prio:4
    +28  u32  body_len
  TLV area (4-byte aligned, each: u16 kind, u16 len, payload):
    0x0001 object      32 B   present for object-scoped ops
    0x0002 cap_handle   4 B   index into the link's installed-token table
    0x0003 cap_inline 320 B   only before the handshake installs a handle
    0x0004 trace       24 B   trace_id 16 + span_id 8
    0x0005 deadline     8 B
    0x0006 inline_node 16 B   when an index is not known yet
  body
  u32 crc32c over the whole record (header + TLVs + body)
```

Fields marked "omitted" are simply absent; `hdr_words` records the shorter header, so a
reply carries no `request_id` and an unsolicited frame carries no `reply_to`.

Sizes **[D]** from the v1 inventory. Each row states its assumption: the join rows keep
their v1 body, `INVOKE` moves the capability into a `cap_handle` TLV and the object into
an `object` TLV, and `RESULT`/`STATE` keep their v1 bodies.

| Message | v1 record | v2 (handle) | Ratio |
|---|---:|---:|---:|
| keepalive (no body needed) | 340 | 39 | 8.7× |
| join request (64 B v1 body) | 388 | 103 | 3.8× |
| join confirm, 2 members (160 B v1 body) | 484 | 199 | 2.4× |
| `INVOKE` (capability by handle, object TLV) | 656 | 87 | 7.5× |
| `INVOKE` (capability inline, pre-handshake) | 656 | 391 | 1.7× |
| `RESULT` (object TLV) | 424 | 111 | 3.8× |
| migration `STATE` (248 B state) | 744 | 451 | 1.6× |
| 64 KiB bulk record | 65 540 | 65 255 | 1.004× |

The bulk case barely moves because overhead is already 0.5 %; that is the point — v2 wins
on the steady state and is neutral on bulk. The join confirmation is dominated by the
64-byte Ed25519 signature and the member list, which is why §6.7 attacks it at the
protocol level rather than the framing level.

The capability handle is installed by the handshake: the first `INVOKE` on a link may
carry `cap_inline` (320 B); the receiver validates it against the same subject, object and
home epoch as today, stores it in a small per-link table, and answers with a `cap_handle`
assignment. Later invokes carry 4 bytes. A handle is scoped to the link and the epoch and
is dropped on reconnect, so no revocation state crosses a link failure.

**Membership-indexed NodeIds** are the single biggest structural saving: 64 bytes of
NodeId per record become 6–8 bytes. The index is only valid inside a confirmed epoch, so
it must be scoped by the `home_epoch`/membership epoch that the join handshake installs;
a stale index must fall back to the inline TLV, which the receiver detects with
`route_idx == 0xFFFF` or a non-current epoch.

**One CRC over the whole record** (not two over the headers) is both cheaper (one pass)
and stronger (the body and the capability token are covered).

**Proposal B (minimal, no membership indices): keep the 128-byte `MeshHeader` but drop
the 128-byte `FrameHeader` and 64-byte descriptor into a 24-byte frame prefix + TLVs.**
324 → ~156 bytes, no index negotiation, no epoch scoping. Lower risk, roughly half the
win. Useful as a step 1 if indices are contentious.

**Proposal C (do nothing on the wire): fix only the tick, HTTP and hub.** The measured
latency win (330×) is far larger than any byte win, so this is defensible for M3. It
leaves the 95 % overhead on heartbeats and the 320-byte capability re-send.

### 6.2 Version negotiation

- Record magic stays distinct (`I7M1` for v1, `I7M2` for v2) so a v1 decoder that sees a
  v2 record fails with `BadMagic` and poisons the link rather than misparsing.
- Both sides advertise `framing = [1, 2]` in a v1 `Hello` (the existing body has flags
  spare: `wire.rs:36-37` prefix has a u16 `flags` field fixed to zero). The first record
  on a link is always v1; v2 starts on the first record after both sides ack.
- A node that only speaks v1 keeps working unchanged; the v2 peer simply never upgrades.
- Everything versioned separately: the record header (v2), the join body (`I7J2`), the
  route body (`I7R2`) and the migration body (`I7M1`→`I7M2`). A v2 record can carry a v1
  body; that is the migration path.

### 6.3 Transport: direct peer versus relay-first

| Option | Latency | Complexity | Where it fails |
|---|---|---|---|
| Direct peer TCP (lower-NodeId dials) | 1 RTT, no host hop | needs a `tcp_connect` module service | the guest has no connect primitive today **[C]** |
| Relay/hub (shipped M2/M3) | +1 host hop (~23 µs measured) | rig-only code | single point of failure; fan-in choke at the root |
| Hub as a routed overlay | 1 hop per frame | needs routing and backpressure | head-of-line blocking per link |
| UDS/shared memory for co-located guests | ~µs | needs the emulator to provide a shared device | M2-ADR-0002 §8 rules it out for the current rig |

**Recommendation:** keep the relay for the lab, but add a **kernel `tcp_connect` service**
so the production path is direct peer-to-peer and the lower-`NodeId` rule in ADR M2-0002
§3 can actually run. The relay then becomes a diagnostic tool, not a dependency. The
kernel service is small (one `TcpSocket::connect`) and unlocks the backoff code that
already exists in `mesh-transport`.

### 6.4 Multiplexed streams, batching and coalescing

- **Multiple streams over one connection.** `FrameHeader.stream` exists and is always
  `StreamId(1)` in the module **[C]** (`mesh.rs:1979`). Using it would let a 64 KiB
  migration `STATE` record share a link with heartbeats without head-of-line blocking.
  Cost: a per-stream ordering rule and a fair scheduler in the module's tick.
- **Batching.** `SendQueue` already supports multiple records per flush
  (`ends: Vec<usize>`); the module pushes at most a few per tick. Batching N records per
  `tcp_send` removes N−1 syscalls and N−1 smoltcp copies. Expected win on bulk:
  proportional to N; on latency: small.
- **Coalescing heartbeats with data.** A heartbeat is only needed when the link is idle;
  `Heartbeat::due()` already checks `last_tx`, but it is checked every tick regardless of
  queued data. Suppressing a heartbeat when a record was queued this tick saves 340 B per
  5 s per link — trivial in bytes, but it removes a periodic wake-up from the peer.
- **Pipelining.** `PendingTable` capacity is 8 but the module uses one outstanding
  request. Pipelining `Invoke`s to 8 deep would raise throughput 8× on the route path
  without any wire change (tags already exist).

### 6.5 Buffers and backpressure

- **Module RX 2 KiB → 16 KiB.** The RX ring should be at least one window plus one
  segment, and with the window clamp removed it should be at least 16 KiB. Cost: 2 × 14 KiB
  of RAM per slot (2 slots) = 28 KiB. That is affordable in the 2 MiB module area.
- **Credit-based flow control.** `OpCode::WindowUpdate` and `flags::LINK`/`NO_REPLY` exist
  in the vocabulary and are unused **[C]**. A simple credit scheme (receiver grants N
  records or N bytes; sender blocks at zero) gives backpressure without a TCP stall and
  without the 10 s send-deadline disconnect.
- **Bulk by reference.** `PayloadKind::Cas`/`Ring`/`Dma` exist and are unused **[C]**. A
  bulk transfer should be a small `CAS` frame (object + range) with the bytes fetched out
  of band, keeping the mesh link for control. The kernel already has EROFS and a block
  cache; a module-to-module CAS fetch is a later-layer item but the descriptor is ready.

### 6.6 Poll to event / tickless work

This is the largest single win and it is in the kernel.

Options, cheapest first:

1. **Adaptive poll while a socket is active.** Keep the 10 ms timer for idle, but when a
   module has an established TCP socket with pending data, shorten the wait to ~250 µs
   (`Count` + 8 250) for a bounded window. Cost: a few lines in `idle_loop`/`module::poll`;
   no new interrupt wiring; CPU cost only while traffic flows.
2. **Seeq RX interrupt.** The HPC3 has an ENET interrupt; the driver currently leaves
   interrupts to polling (`net.rs:170`). Wiring ENET to an IPL and calling `net::poll()`
   from the handler removes the 10 ms gate entirely for inbound data.
3. **Tickless module scheduling.** `module::poll()` already has a per-module `LAST_TICKS`
   and returns a `tick` callback with no deadline hint. Extending `ModuleVtable` with a
   `next_deadline_ms` (ABI v11) lets a module ask for an earlier wake. This is the clean
   long-term design but needs an ABI bump.

Expected effect **[D]**: `blocking` measured 0.030 ms; a 250 µs adaptive poll would land
at ~0.3 ms (33× better than 10 ms); an interrupt path at ~0.05–0.1 ms (100–200×).

### 6.7 Membership: fan-in versus gossip

Today: single root, every member refreshes every 5 s with a full signed list, the root
signs each confirmation, and the hub funnels all member traffic onto the root's single
connection.

Problems at N members **[D]**:

- Root signing: N signatures per 5 s (25.6/s at N = 128). Ed25519 on the guest is
  unmeasured here but is not cheap; this is the first thing that will saturate.
- Root bandwidth: at 8 members it is 8 × 968 B per 5 s ≈ 1.5 KB/s, trivial. At 128
  members each confirmation is 2500 B, so the root sends 128 × 2500 = 320 KB per 5 s
  (64 KB/s) and receives 128 × 388 = 50 KB per 5 s (10 KB/s); small in link terms, but
  320 KB of encode work per refresh on a 50 MHz core is not.
- Head-of-line blocking: one 2 KiB RX ring serves N peers.

Improvements, in order of value:

1. **Sign once per epoch.** The signed payload is the epoch, root, member list and
   membership digest. Only `subject`, `request_nonce` and `incarnation` vary per member.
   Sign the snapshot once, then send per-member confirmations that carry the signature
   plus a cheap MAC (or simply the unsigned per-member fields, since the snapshot
   signature already binds the list). Root signing becomes O(1) per epoch.
2. **Push, not poll.** The root already knows the new epoch; broadcast it to members
   instead of waiting up to 5 s for each refresh. One message per member per *change*
   instead of per 5 s.
3. **Delta refresh.** Send the epoch and a digest, and only the member-list delta when it
   changes. A no-change refresh becomes ~80 bytes instead of 580.
4. **Gossip** (each member forwards the signed snapshot to k peers) removes the root
   fan-in for *data* but not for *authority*: the epoch writer must stay single (ADR
   M2-0003). Gossip is a distribution mechanism, not a second authority. Worth doing only
   after 1–3.

### 6.8 Security and peer authentication

Shipped state: plain TCP, CRC32C for header integrity, no MAC, no confidentiality, no
replay window, no peer identity **(M2-ADR-0002 §6)**. The join confirmation is
Ed25519-signed, so membership is authenticated; everything else is not.

Options:

| Option | Cost | Notes |
|---|---|---|
| Noise IK / XX (Rust: `snow`) | small dependency, ~1 RTT handshake | static key = the node's existing Ed25519 key converted to X25519; gives mutual auth + AEAD + replay window |
| TLS 1.3 with PSK | needs a server implementation | `embedded-tls` is client-only today (`kernel/src/tls.rs`) |
| Signed records (per-record Ed25519) | ~64 B + verify per record | too slow on the guest; use for control only |
| MAC'd records with a session key | 16 B + cheap | after a Noise handshake; this is the right steady state |

Recommendation: **Noise IK over the existing TCP link**, with the static key derived from
the node key that the join already trusts. It adds one round trip at connect and 16 bytes
per record (Poly1305 tag), and it removes the "plaintext mesh" caveat that blocks any
non-lab deployment. The record CRC then becomes redundant for integrity and can be
demoted to a debug flag.

### 6.9 Observability and fault behaviour

- Per-tag latency: record `queued_at`, `sent_at`, `acked_at` and export a histogram in
  the pass line or a `/mesh/stats` body. Without it, every claim in §4 is a model.
- Per-peer counters in the module (the hub has them; the guests do not).
- Terminal completion for abandoned tags on link loss (`Status::Broken`), so a client
  never waits forever.
- Hub reconnect with backoff, and a rig assertion that a restarted guest rejoins within a
  bounded time.
- Structured `Moved`/`Fence`/`Refuse` counters already exist in the route report; expose
  them over `/mesh/stats` instead of the 5-line text report.

### 6.10 Recommendation summary

| Recommendation | Expected win | Risk | When |
|---|---|---|---|
| Adaptive poll / RX interrupt | 33–200× latency | kernel change | next kernel slice |
| Remove window clamp | up to 11× bulk | kernel change | next kernel slice |
| HTTP keep-alive + pipelining | 4× control plane | module only | now |
| v2 compact header + capability handle | 7–9× bytes on steady state | negotiated wire change | after M3 |
| Sign once per epoch + push | removes O(N) signing | protocol change | with v2 join |
| Table CRC | 2–4× codec | shared crate | now |
| Hub writer fix | 8–45 ms per record | rig only | now |
| Noise IK | security, ~1 RTT + 16 B | new transport | before any non-lab use |

---

## 7. Highest-value implementation sequence

Each phase is independently shippable and has measurable acceptance targets. Nothing in
phases 0–2 changes the wire format.

### Phase 0 — free wins (host rig + shared crate), no wire change

| Step | Change | Acceptance |
|---|---|---|
| 0.1 | Replace the table-free `crc32c` with a 256-entry table (same polynomial, same value) | `sys-primitives` tests unchanged; `protocol-bench codec` shows ≥1.8× at 1 KiB; no wire diff |
| 0.2 | Remove the hub's 128-byte/2 ms chunk writer; write each record with one `write_all` | `protocol-bench net` hub row < 1 ms; M3 rig evidence records the new latency |
| 0.3 | Add `tools/protocol-bench` + `tests/wire_sizes.rs` to CI | CI runs `cargo test --release --offline` in the tool; a wire change fails the gate |

Ownership: `sys-primitives` (kernel + modules), `tools/m2-rig` (host), `tools/protocol-bench`.

### Phase 1 — MCP control plane, module only

| Step | Change | Acceptance |
|---|---|---|
| 1.1 | HTTP/1.1 keep-alive on slot 0: read the next request without closing, keep the slot armed | `protocol-bench http` keep-alive row; rig tool-call latency p50 ≤ 12 ms (from 40 ms model) |
| 1.2 | Coalesce: one `tcp_send` per response (already), plus one flush per tick for the mesh | no change in serial logs; `frames_sent` unchanged |
| 1.3 | Pipelined `/mesh/*` step commands (one request, N steps) for the rig | M3 rig step phase ≤ 1 request per scenario |

Ownership: `modules/mcp`. No kernel, no wire change. Keep the `Connection: close` path for
clients that ask for it (`Connection: close` request header), so the change is additive.

### Phase 2 — kernel transport (largest latency win)

| Step | Change | Acceptance |
|---|---|---|
| 2.1 | Adaptive poll: when an established module socket has data pending, use a 250 µs wait for a bounded window | `m2-rig` mesh RTT p50 ≤ 1 ms (measured by a new pass line); no boot-time regression |
| 2.2 | Seeq RX interrupt → `net::poll()` | same target, CPU idle unchanged when idle |
| 2.3 | Remove `max_burst_size = Some(1)`; set a realistic burst (e.g. 8) and raise the module RX ring to 16 KiB | bulk transfer ≥ 2 MB/s on the loopback rig; `screen.bmp` ≤ 11 s with the current one-row-per-tick loop, < 3 s once rows are batched |
| 2.4 | `tcp_connect` module service + lower-`NodeId` dial | direct peer link in the rig; relay optional |

Ownership: kernel. Every step needs the M1 boot gate and the M2/M3 rig gates green.

### Phase 3 — wire v2 (negotiated)

| Step | Change | Acceptance |
|---|---|---|
| 3.1 | Define `I7M2` record + 32-byte header + TLV; add codec + fuzz tests | round-trip byte-exact; `wire_sizes.rs` extended |
| 3.2 | Negotiate in `Hello` flags; v1 stays the default | a v1-only peer completes join and route unchanged |
| 3.3 | Membership-index NodeIds, capability handles installed at handshake | keepalive ≤ 40 B, invoke ≤ 96 B |
| 3.4 | One CRC over the record; table implementation | header bytes do not change |
| 3.5 | Join v2: sign the snapshot once, push on change, delta refresh | root signatures per epoch = 1; refresh ≤ 100 B when unchanged |

Ownership: shared crates + module. Dual-stack for at least one release; the ADR must
record the v1 sunset rule.

### Phase 4 — security and observability

| Step | Change | Acceptance |
|---|---|---|
| 4.1 | Per-tag latency + per-peer counters in the pass line and `/mesh/stats` | numbers reproducible from the serial log |
| 4.2 | Terminal `Broken` completion on link loss | no tag leaks; route/migrate see the failure |
| 4.3 | Noise IK over TCP with the node key | handshake < 2 RTT; 16 B/record; replay window test |

### 7.1 No-regression gates

Every phase must keep these green:

| Gate | Command / marker | Why |
|---|---|---|
| M1 boot | `BOOT-READY ... mesh=single`, `m1-post: mesh=0/0/0` | no accidental mesh in a plain boot |
| M1 module load | `m2-mesh: entered interface=irix7.mesh.transport version=1 probe=ok`, `status=ok root=m2-root@1.0.0 nodes=2 signatures=ed25519` | the runtime dependency graph is unchanged |
| M2 join | `mesh-join: converged ... epoch=2 members=2`, same digest on both nodes | wire stays compatible |
| M2 route | the five `/mesh/route?step=N` steps, one `MOVED`, `ack-mismatch` and stale-epoch negatives | the `MOVED` contract |
| M2 migrate | eleven `/mesh/migrate?step=N` steps, same state hash at each boundary | the migration contract |
| M2 failure | `suspect` 6 s, `failed` 12 s, home recompute, lease fencing, partition rule | the failure contract |
| M3 shard | `/mesh/shard` on both kernels with the same epoch and digest; 1/2/8/128-node policy gate | the shard contract |
| Wire sizes | `cargo test` in `tools/protocol-bench` | a byte change is a protocol change |
| Clippy/fmt | `cargo fmt --check`, `cargo clippy` | repo hygiene |

---

## 8. Risks and limits

### 8.1 Security

- **No confidentiality or peer authentication on the mesh.** Anyone who can reach port
  7000 can send a well-formed `MeshEnvelope`; header CRC is not a MAC. The join signature
  protects membership, not traffic. This must not be presented as a secure deployment.
- **Replay.** `request_id` is not a window; the route layer keeps a per-peer replay guard
  (`Reason::Replay`, sequence 18) but the transport does not. A captured `Invoke` with a
  live capability can be replayed until the capability expires.
- **Capability replay on the wire.** The 320-byte token is a bearer token in plaintext.
  Moving to handles (§6.1) shortens the exposure window but does not encrypt it.
- **DoS by allocation.** `MAX_RECORD_LEN` is 64 KiB and the decoder pre-checks the
  prefix, but a peer can still pin 64 KiB per link. Bounded by the 8-slot pending table
  and the module's single send queue.

### 8.2 Memory

- Per module TCP slot: 2 KiB RX + 16 KiB TX = 18 KiB; two slots = 36 KiB.
- Mesh decoder + send queue: up to 2 × 64 KiB per link.
- `JOIN_RX`/`JOIN_TX`: 8 KiB of static.
- Raising RX to 16 KiB adds 28 KiB total; removing the window clamp needs at least that.
- The v2 header shrinking the *record* does not shrink the *buffers*; the 64 KiB bound is
  a policy choice, not a requirement.

### 8.3 Head-of-line blocking and fairness

- One TCP connection per peer + one 64 KiB record + one 2 KiB RX ring means a bulk
  transfer delays heartbeats. With a 10 ms tick and a 1.4 KB window, a 64 KiB record
  occupies the link for ~0.44 s. During that time the peer's heartbeat is delayed and the
  30 s timeout is the only backstop.
- At the M3 root, all members share one connection: one member's bulk record delays every
  other member's control traffic. Per-stream fairness (§6.4) or per-peer links will be
  needed before any real workload.

### 8.4 Migration

- The migration `STATE` record is 744 bytes for a 248-byte actor state — 66 % overhead on
  the one operation where payload size will grow. A v2 envelope would make it ~451 bytes
  (§6.1), and a `CAS`/`Ring` payload would make it a reference plus a hash.
- Migration is 6 records / 3 RTTs, all tick-gated: 30–60 ms plus signing on the guest.
  That is acceptable for a lab, not for a live actor under load.

### 8.5 ABI and versioning

- `module-abi` is pinned at `v0.7.0`; adding `next_deadline_ms` or `tcp_connect` is an ABI
  bump and a service-table layout change. Every module must be rebuilt (the compile-time
  offset assertions in `abi/` catch a mismatch).
- The wire version and the ABI version are independent; do not couple them.
- The `Stats` struct is a plain data type in `mesh-transport`; adding fields is a
  source-compatible change for the module but not for any external consumer that
  serialises it (none today).

### 8.6 Real-Indy constraints

- The Seeq is 10 Mbit/s and the HPC3 PDMA path copies per byte on receive; a real Indy
  will not exceed ~1 MB/s no matter what the window says.
- The R4000/R4400 have 8–16 KiB D-caches; a 1 KiB CRC table fits, a 64 KiB send queue
  does not (it streams through, but the copy cost is real).
- The 10 ms timer is a hardware choice in `idle.rs`; a shorter tick raises interrupt load
  on a machine that already polls several devices.

### 8.7 What cannot be fixed without a kernel/transport change

- The 10 ms dispatch quantisation (kernel).
- The 1.4 KB window clamp (kernel/smoltcp device caps).
- The 2 KiB module RX ring (kernel).
- The per-byte volatile RX copy (kernel).
- The absence of a connect primitive (kernel service).
- The absence of a TLS/Noise server (kernel or a new transport module).
- The single HTTP slot when the mesh is enabled (kernel: only two module TCP slots).

Everything else — header size, capability handles, batching, keep-alive, refresh signing,
hub chunking, CRC implementation — is module, shared-crate or rig work and can ship
without touching the PROM.

---

## Appendix A — commands

```sh
# toolchain used for these numbers
rustc 1.99.0 (b940084d7 2026-09-28)
cargo 1.99.0 (5f94df478 2026-08-27)

cd tools/protocol-bench
cargo test --release --offline           # pins every byte count in this document
cargo run --release --offline -- frames
cargo run --release --offline -- codec
cargo run --release --offline -- net
cargo run --release --offline -- http
cargo run --release --offline -- calibrate
```

Host: Intel Core i5-8500T @ 2.10 GHz, 6 vCPU, Linux 6.18.55.

## Appendix B — raw results

### B.1 `frames`

See §3.4; the full table is reproduced there verbatim from the tool.

### B.2 `codec`

```text
| payload B | record B | roundtrip us | roundtrip ns/B | queue us | roundtrip records/s |
| 0         | 324      | 11.62        | 35.86          | 4.88     | 86073                |
| 16        | 340      | 10.03        | 29.51          | 7.82     | 99671                |
| 332       | 656      | 10.95        | 16.69          | 4.08     | 91324                |
| 1024      | 1348     | 7.32         | 5.43           | 6.93     | 136574               |
| 8192      | 8516     | 12.88        | 1.51           | 4.30     | 77621                |
| 65212     | 65536    | 18.98        | 0.29           | 17.98    | 52684                |

CRC32C:
| variant         | 128 B  | 1024 B  |
| shipped bitwise | 2.036  | 20.527  |
| table-driven    | 2.111  | 10.181  |
```

### B.3 `net`

See §4.2.

### B.4 `http`

See §4.3.

### B.5 `calibrate`

```text
| bytes | copy us | GB/s |
| 16    | 0.0     | 2.7  |
| 1024  | 0.0     | 51.2 |
| 8192  | 0.3     | 32.1 |
| 65212 | 2.9     | 22.7 |
```

The copy calibration exists to prove the harness can see a memory copy (27–32 GB/s for
8–64 KiB); without it a 2 ns "encode" would be indistinguishable from an elided one.

## Appendix C — the M2 record, byte for byte

A 340-byte `Control/Ping` from node `11…11` to `22…22`, offsets from the first byte of the
record (the length prefix). The record is 4 + 128 + 128 + 64 + 16 = 340 bytes, so
`envelope length = 336` and `frame_len = 208`.

| Offset | Bytes | Field | Value / note |
|---:|---:|---|---|
| 0 | 4 | envelope length | 336, little-endian (`50 01 00 00`) |
| 4 | 4 | mesh magic | `49 37 4d 48` = "I7MH" |
| 8 | 1 | mesh version | 1 |
| 9 | 1 | mesh flags | `0x08` = PROBE |
| 10 | 2 | mesh header_len | 128 |
| 12 | 16 | `src` | `11…11` |
| 28 | 16 | `dst` | `22…22` |
| 44 | 16 | `home` | `22…22` |
| 60 | 16 | `route_hint` | zero |
| 76 | 4 | `src_epoch` | 0 |
| 80 | 4 | `dst_epoch` | 0 |
| 84 | 4 | `home_epoch` | 0 |
| 88 | 4 | `object_epoch` | 0 |
| 92 | 8 | `request_id` | 0 for a control heartbeat |
| 100 | 8 | `reply_to` | 0 |
| 108 | 1 | `ttl` | 8 |
| 109 | 1 | `hop_count` | 0 |
| 110 | 1 | `priority` | 0 |
| 111 | 1 | reserved | 0 |
| 112 | 4 | `frame_len` | 208 |
| 116 | 12 | reserved | zero |
| 128 | 4 | CRC32C | over record bytes 4..132, its own field zeroed |
| 132 | 4 | frame magic | `37 46 4d 31` = "7FM1" |
| 136 | 1 | frame version | 1 |
| 137 | 1 | `kind` | 5 = Control |
| 138 | 2 | frame flags | 0 |
| 140 | 2 | `opcode` | `0x32` = Ping |
| 142 | 2 | frame header_len | 128 |
| 144 | 4 | `stream` | 1 |
| 148 | 4 | `tag` | 0 |
| 152 | 4 | `parent` | 0 |
| 156 | 4 | `payload_len` | 80 (descriptor + body) |
| 160 | 4 | `status` | 0 |
| 164 | 8 | `cap` | 0 |
| 172 | 32 | `object` | zero |
| 204 | 8 | `offset` | 0 |
| 212 | 4 | `length` | 16 |
| 216 | 1 | `priority` | 0 |
| 217 | 1 | `trace_flags` | 0 |
| 218 | 2 | reserved | 0 |
| 220 | 8 | `deadline_ns` | 0 |
| 228 | 16 | `trace_id` | zero |
| 244 | 8 | `span_id` | zero |
| 252 | 4 | `inline_len` | 16 |
| 256 | 4 | CRC32C | over record bytes 132..260, its own field zeroed |
| 260 | 1 | payload kind | 0 = Inline |
| 261 | 1 | payload flags | 0 |
| 262 | 2 | reserved | 0 |
| 264 | 4 | `inline_len` | 16 |
| 268 | 8 | `length` | 16 |
| 276 | 8 | `offset` | 0 |
| 284 | 32 | payload `object` | zero |
| 316 | 4 | `ring_id` | 0 |
| 320 | 4 | `dma_token` | 0 |
| 324 | 16 | body | source `NodeId` |
| 340 | — | end | |

Two things are visible in the map: 324 bytes of framing for 16 bytes of meaning, and two
independent CRCs that each cover only their own 128-byte header (the descriptor and the
body are outside both).

## Appendix D — references

- `crates/mesh-transport/src/lib.rs` — framing, decoder, send queue, deadlines, tags, stats.
- `crates/sys-primitives/src/lib.rs` — headers, payload descriptor, capability, CRC.
- `crates/mesh-join/src/{wire,node,authority}.rs` — join bodies, state machine, signing.
- `crates/mesh-route/src/{wire,lib}.rs` — route bodies, `MOVED`, leases.
- `crates/mesh-migrate/src/{wire,node}.rs` — migration bodies and steps.
- `crates/mesh-core/src/lib.rs`, `crates/mesh-service/src/lib.rs` — homes, shards, failure detector.
- `modules/mcp/src/mesh.rs`, `modules/mcp/src/main.rs` — the socket state machines.
- `kernel/src/{module,net,http,tls,idle,autoload}.rs` — services, device, client, timer.
- `tools/m2-rig/src/{relay,hub,shard,rig}.rs` — host paths.
- `docs/adr/m2-0002-transport.md`, `m2-0003-authority.md`, `m3-0001-shard-slice.md`.
- `docs/plan/m2-rig.md`, `docs/plan/m3-rig.md` — evidence contracts.
