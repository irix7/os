# QUIC as the primary IRIX 7 transport

**Status:** research / design input. No production code, ABI, ROM image or wire format was
changed by this slice.
**Question.** Should QUIC replace TCP as the primary network protocol of IRIX 7, and if so,
what exactly moves, what stays, and what would have to be true for it to fit an SGI Indy?
**Basis.** `origin/main` at `41f3f24` (2026-10-27), including the pre-release policy
([`docs/spec/pre-release.md`](../spec/pre-release.md)) and the wire-efficiency study
([`wire-efficiency.md`](wire-efficiency.md)). Measurements marked **[M]** were taken on the
host described in Appendix A; **[C]** is read from the tree at that commit; **[D]** is
derived arithmetic; **[H]** is an unverified hypothesis.

**Companion decisions and issues.** [M2-ADR-0002](../adr/m2-0002-transport.md) (TCP
transport), [M2-ADR-0003](../adr/m2-0003-authority.md) (authority),
[M3-ADR-0001](../adr/m3-0001-shard-slice.md) (shard slice),
[M3-ADR-0002](../adr/m3-0002-multi-node-hub.md) (hub), issues #245–#249.
The pre-release policy applies: every internal interface here is Version 1, there are no
deployed peers, and no compatibility path is proposed.

---

## 1. Executive verdict and the exact meaning of "primary"

### 1.1 Verdict

**Conditional GO.** QUIC should become the primary transport for IRIX 7's *native* flows —
the mesh, the MCP control plane, the module manager, and service RPC — but not by porting an
existing Rust QUIC stack, not inside the PROM, and not by removing TCP from the POSIX/IRIX
socket API.

The recommendation has three parts:

1. **Adopt QUIC as the only native fabric and control-plane transport** once a bounded,
   in-tree profile passes the gates in §6. TCP is then a bootstrap protocol and an external
   compatibility surface, not a peer of QUIC on the fabric.
2. **Do not port quinn, s2n-quic, quiche or neqo.** They are std-only, C-backed, or both;
   the measured rustls library alone is five times the free space in the MCP module slot,
   before any QUIC transport code. A purpose-built QUIC profile plus a minimal TLS identity
   layer is the only native option that can fit.
3. **Measure before committing.** The decisive unknown is not the protocol, it is the Indy:
   ChaCha/AES throughput, UDP packet rate, the Seeq transmit path, and the linked code size
   on n32 MIPS. A ten-day spike (§8.2) can settle all four. If it fails, the fallback is a
   QUIC gateway at the edge, not a half-native stack.

### 1.2 What "primary" means

"Primary" is a statement about **native flows**, not about every byte the machine can send.

> A native flow is a flow that belongs to the operating system's own fabric or control
> plane: membership, homes, routing, migration, capability invocation, actor/fabric RPC,
> the MCP JSON-RPC plane, module management, and CAS transfer between IRIX 7 nodes.
>
> **Primary** means: every native flow uses QUIC, and a native flow carried over TCP is a
> defect. It does **not** mean that TCP disappears from the socket API, the boot ROM, or
> the external management surface.

That distinction is forced by the product, not by taste. POSIX.1-2024 requires a socket
API; IRIX 6.5 applications expect `AF_INET`/`SOCK_STREAM`; external MCP clients speak
Streamable HTTP; the kernel fetches its first module over HTTPS because there is no other
trust anchor at boot. Removing those is a different product, and it must be an explicit ADR
(§1.3, §7).

### 1.3 Disposition of each protocol and path

| Flow / interface | Today | Proposed | Why | External contract? |
|---|---|---|---|---|
| Mesh link (join, epoch push, heartbeat) | TCP:7000, `MeshEnvelope` | **QUIC**, control stream | encryption, auth, one connection, no listener/dialer race | no |
| Homes / route / `MOVED` | mesh over TCP | **QUIC**, RPC streams | per-request streams, cancellation, no head-of-line blocking | no |
| Actor migration (`PREPARE/STATE/COMMIT`) | mesh over TCP | **QUIC**, migration stream | reliable ordered state, `RESET_STREAM` for abort | no |
| Capability invoke / completion | mesh over TCP | **QUIC**, RPC stream | capability handle survives connection migration; TLS protects bearer tokens | no |
| MCP JSON-RPC (internal) | HTTP/1.0, `Connection: close`, TCP:80 | **QUIC**, MCP stream binding | removes four-tick HTTP phase machine; long-lived streams | MCP spec allows custom transports |
| External MCP clients | Streamable HTTP over TCP | **HTTP/1.1 or HTTP/3 at a gateway**, not on the Indy | preserves standard clients without a guest HTTP/3 stack | yes |
| Module manager / service RPC | kernel HTTPS fetch + in-process calls | **QUIC** after the first module | one transport, streams, resumption | partial (HTTPS URLs) |
| Boot autoload / first module | HTTPS over TCP + TLS 1.3 | **HTTPS over TCP, unchanged** | no QUIC stack fits in the PROM; TLS is the boot trust anchor | yes (HTTPS) |
| POSIX/IRIX `socket()` | TCP/UDP (future userland) | **TCP/UDP, unchanged** | POSIX and IRIX application compatibility | yes |
| Telnet console | TCP:23 | **TCP, unchanged** | management, not fabric | no |
| DHCP / DNS / NTP | UDP | **UDP, unchanged** | bootstrap and time; no QUIC equivalent | yes |
| Host rig / relay | TCP forwards, TCP hub | UDP forwards + CID-routed relay | test and lab fan-in; iris supports UDP forwards | no |

The test for "primary" is a packet capture on the Indy's Ethernet: after boot, with no
application socket open, there is no TCP. The only permitted TCP is the boot fetch (which
has already completed) and traffic from an application that explicitly opened a socket.

### 1.4 Where QUIC is a poor fit — the premise, challenged

QUIC is not a speed upgrade on this machine. The honest comparison:

| Claim | Verdict |
|---|---|
| "QUIC is faster than TCP" | **Not on a 10 Mbit Seeq with a 50–100 MHz R4400.** QUIC adds AEAD, packet-number spaces, ACK processing and a large state machine. TCP with a compact record and a fixed window will be faster in bulk. |
| "QUIC saves bytes" | **Not against a compact mesh record.** A QUIC short header with an 8-byte CID, 2-byte packet number and 16-byte tag is ~27 B per packet, plus a 4–20 B STREAM frame and ACKs. A 39-byte compact record inside TCP+IPv4 costs 40 B of IP/TCP. QUIC's byte win is against the *current 324-byte envelope*, which should be fixed regardless. |
| "QUIC gives multiplexing" | **True, and it matters at 8+ nodes.** But QUIC's streams still share one packet-number space and one congestion controller; it removes byte-stream head-of-line blocking, not all packet-level blocking. A simpler stream layer over TCP would capture much of the win for far less code. |
| "QUIC gives security" | **True, and this is the strongest reason.** Today the mesh is plaintext with a header CRC; QUIC gives TLS 1.3 confidentiality, peer authentication and a replay-safe handshake. But Noise IK over TCP would give most of that with an order of magnitude less code (the wire study's own recommendation). QUIC is justified only if the project also wants the standard protocol, streams, migration and ecosystem. |
| "QUIC gives 0-RTT" | **True but dangerous here.** 0-RTT is replayable; capability invokes and migration commits are mutations. v1 should disable it. |
| "QUIC gives connection migration" | **True, but the Indy is a fixed Ethernet machine.** Migration matters for DHCP changes and NAT rebinding, not for a desktop. It is a future benefit, not a current one. |
| "QUIC is easy to add" | **False.** There is no production no_std QUIC stack, no MIPS crypto provider in ring/aws-lc, no room in the PROM, and ~95 KiB free in the MCP slot. This is a multi-quarter project. |

The strategic case for QUIC is therefore: **one standard secure multiplexed transport for
all native flows, with a credible path to interop, migration and a smaller long-term
protocol surface.** If the goal is only "make the mesh faster", the answer is no: fix the
10 ms tick, the window clamp and the 324-byte envelope first (wire-efficiency study). If the
goal is "the mesh must be secure and multiplexed, and IRIX 7 should speak the protocol the
rest of the world is moving to", the answer is a conditional yes.

### 1.5 The decision in one line

**GO to a measured native QUIC profile for native flows; NO-GO to a ported stack, QUIC in
the PROM, or the removal of POSIX TCP.**

---

## 2. Current baseline and constraints

### 2.1 Numbers that bound the design

| Item | Value at `41f3f24` | Evidence / source |
|---|---:|---|
| PROM image | **523,204 / 524,288 B** | **[M]** release build, `opt-level=z`, `scripts/release/build.sh` recipe |
| PROM headroom | **1,084 B** (0.21 %) | [D] |
| MCP module binary (flat, `z`) | **359,528 B** | **[M]** `llvm-objcopy -O binary`, slot-A linker script |
| MCP module ELF footprint (text+rodata+data+bss) | **428,965 B** | **[M]** `llvm-size -A`: `.text` 337,712, `.rodata` 16,356, `.data` 5,448, `.bss` 69,449 |
| Slot A | 512 KiB at `0x8840_0000` | [C] `kernel/src/module.rs:37-56` |
| **Real free space in slot A** | **95,323 B** | [D] 524,288 − 428,965; the flat binary size understates the footprint because `.bss` is in the slot |
| Slot B | 1.5 MiB at `0x8848_0000`; first 256 KiB manager, five 256 KiB arenas | [C] `kernel/src/module.rs:37-90`, `docs/spec/modules.md` §3 |
| Module area | 2 MiB, `0x0840_0000`–`0x0860_0000`; Seeq DMA follows at `0x0860_0000` | [C] `kernel/src/net.rs:36-41` |
| Module TCP slots | **2** | [C] `kernel/src/module.rs:681` |
| Module TCP RX / TX | **16 KiB / 16 KiB per slot** | [C] `kernel/src/module.rs:692-693`; the wire study's 2 KiB RX figure is stale at this commit |
| Kernel HTTP client RX / TX | 4 KiB / 2 KiB | [C] `kernel/src/http.rs:17-18` |
| Kernel TLS record read buffer | 16,640 B | [C] `kernel/src/tls.rs:440` |
| Module tick | **10 ms** | [C] `kernel/src/idle.rs:19` (330,000 Count ticks at 33 MHz), `kernel/src/module.rs:222` |
| Service loop | `net::poll` → `module::poll` → `telnet::poll` → `idle::wait` | [C] `kernel/src/main.rs:690-721` |
| TCP advertised window | **≈1,474 B** | [C] `kernel/src/net.rs:251` `max_burst_size = Some(1)`; smoltcp clamp |
| Mesh record overhead | **324 B** = 4 + 128 + 128 + 64 | [C] `crates/mesh-transport/src/lib.rs:46-55`, `crates/sys-primitives/src/lib.rs` |
| Keepalive `Control/Ping` | 340 B | [C] + **[M]** `tools/protocol-bench` |
| `Invoke` | 656 B | [C] + **[M]** |
| Module ABI | v0.7.0 / ABI **12**, `Services` 0x70 bytes | [C] `docs/spec/modules.md` §4, `os-sdk` tag `v0.7.0` |
| Module allocator | bump allocator, **no free** | [C] `modules/mcp/src/main.rs:67-79`, `kernel/src/heap.rs` |
| CPU | R4400, MIPS III, big-endian, 16 KiB I/D L1, 1 MiB L2; Indy models at 50/75/100 MHz, later R4400/R5000 variants faster. Published 200 MHz MIPS32 crypto numbers are used only as an upper bound, not as an Indy clock. | [C] `docs/research/indy-hardware.md`, `docs/spec/architecture.md` |
| RAM | 32–256 MiB depending on profile; the m2-rig uses 64 MiB (`banks = [64, 0, 0, 0]`) | [C] `docs/spec/architecture.md`, `tools/m2-rig/src/rig.rs:680` |
| NIC | Seeq 80C03, 10 Mbit, HPC3 PDMA | [C] `kernel/src/net.rs`, `docs/research/indy-hardware.md` |
| Kernel UDP | smoltcp `socket-udp` compiled; used by DNS/NTP | [C] `kernel/Cargo.toml`, `kernel/src/dns.rs:8`, `kernel/src/ntp.rs:13` |
| Module UDP | **none** | [C] `kernel/src/module.rs:705-909`; no `udp_*` in `Services` |
| Module connect | **none** | [C] same; only `tcp_listen` |
| Kernel TLS | embedded-tls 0.19, AES-128-GCM-SHA256, P-384/RSA verification, CA roots in `kernel/src/ca/` | [C] `kernel/src/tls.rs`, `kernel/src/ca/README.md` |
| TLS server | **none** | [C] |

### 2.2 The current TCP path in one diagram

```text
module (MCP + mesh)                      kernel                         Seeq
  tcp_recv 1024 B × 8/tick  ──►  smoltcp TCP socket (16 KiB RX)  ──►  RX ring 8 × 1520 B
  MeshEnvelope decode       ◄──  tcp_send (16 KiB TX)            ◄──  TX one descriptor,
  324 B envelope + body          10 ms poll / 10 ms module tick       spin until ACTIVE clears
```

The path is correct and measured, but it has four properties that matter for QUIC:

1. **The 10 ms tick is the latency floor.** A request/response between two ticked modules is
   10.0 ms p50 and 20.0 ms p99; a blocking pair is 0.030 ms p50 (wire study §4.2, **[M]**).
2. **The window is one segment.** `max_burst_size = Some(1)` clamps every TCP socket to
   ~1,474 B, so bulk is `MSS / RTT` (≈147 KB/s at 10 ms, wire study §4.4).
3. **The module RX ring is 16 KiB now, not 2 KiB.** The earlier 2 KiB number in the wire
   study was fixed; the current limit is the 1,474 B window, not the ring.
4. **The Seeq driver is synchronous.** `SeeqTxToken::consume` spins up to 10,000,000
   iterations until `ENET_CTRL.ACTIVE` clears (`kernel/src/net.rs:357`), and `receive`
   copies each frame byte by byte with `read_volatile` (`kernel/src/net.rs:283`). The wire
   study estimated **13 ms of spin per 16 KiB burst** on a 10 Mbit wire. Even if the HPC3
   clears `ACTIVE` when the Seeq FIFO accepts a frame rather than when the wire drains it,
   the FIFO is only a frame or two deep, so the steady-state send rate is still
   wire-limited and the CPU cannot encrypt the next packet while the previous one is being
   accepted. Measure the actual `ACTIVE` timing on hardware; do not assume the spin is free.

Point 4 is the single most important hardware constraint in this study, and it is not in
the current issue plans.

### 2.3 QUIC-specific constraints

| Constraint | Consequence |
|---|---|
| PROM headroom **1,084 B** | No QUIC in the ROM or kernel. Even a UDP module service may not fit without a size-reclamation slice. |
| Slot A free **95 KiB** (including `.bss`) | QUIC + MCP cannot coexist in 512 KiB with a Rust TLS stack. The module area must be repartitioned, or the TLS/QUIC code must be much smaller. |
| Module allocator has no free | Every connection and stream buffer must come from a fixed static pool or a reusable arena. A `Vec`-per-connection design leaks on every reconnect. |
| 10 ms module tick | QUIC needs 1 ms timers for pacing, ACK delay and PTO. The ABI has no deadline; `ModuleVtable.tick` is called "at most every 10 ms" (`os-sdk` ABI 12). |
| 10 ms service loop | QUIC at 10 Mbit is ~1,042 packets/s; a 10 ms poll must drain ~10 packets. The RX ring is 8. Event-driven RX is mandatory. |
| Synchronous TX spin | Pacing and crypto cannot overlap the wire. A non-blocking TX ring is mandatory. |
| No module UDP service | QUIC cannot open a socket. A UDP ABI is a prerequisite. |
| No TLS server | QUIC's TLS handshake needs a server. The kernel TLS client is not reusable. |
| No CSPRNG | QUIC requires unpredictable CIDs and TLS keys. The kernel seeds xorshift from CP0 Count; that is not a CSPRNG. |
| No member keys | Only the root has a signing seed; members hold the root's public key (#245 §4.8). Mutual QUIC authentication needs a member key seed. |
| UDP NAT/firewall | UDP is often blocked; the mesh has no STUN/TURN/ICE. Direct QUIC is a LAN feature; NAT needs a relay. |

---

## 3. Architecture options

### 3.1 Options matrix

| Option | What it is | Fit (code/RAM/CPU) | Security | Interop | Effort | Verdict |
|---|---|---|---|---|---|---|
| **A. Port quinn / quinn-proto** | Pure-Rust std stack, rustls + ring | **No**: std-only, ring has no MIPS, rlib text 678 KB on x86_64 | strong | full | medium | **Reject native**; use on host as interop peer |
| **B. Port s2n-quic** | AWS stack, tokio + aws-lc | **No**: std platform, internal transport crates, aws-lc has no MIPS | strong | full | high | **Reject native**; reference architecture |
| **C. Port quiche** | Rust bindings over BoringSSL C | **No**: C toolchain, BoringSSL, big-endian n32 unsupported | strong | full | high | **Reject** |
| **D. Port neqo** | Mozilla stack, NSS C | **No**: NSS, std, server "experimental", not on crates.io | strong | full | high | **Reject** |
| **E. In-tree minimal QUIC + rustls no_std + RustCrypto provider** | RFC 9000 subset, TLS via rustls QUIC API | **Tight**: rustls MIPS rlib 482 KB; needs repartition and a provider | strong if rustls/audit | full QUIC | very high | **Recommended if GO** |
| **F. In-tree minimal QUIC + minimal hand-written TLS 1.3** | Same transport, ~5k lines TLS | **Best size**: 120–180 KB target | high risk (custom TLS) | full QUIC | very high | **Fallback if E does not fit** |
| **G. QUIC only at the edge / gateway** | Host or LAN gateway terminates QUIC; Indy speaks a lean local protocol | **Best fit**: no guest QUIC code | strong to gateway | full external | medium | **Fallback**, and the external HTTP/3 path |
| **H. QUIC only for mesh/MCP, boot stays TLS/TCP** | Same as E/F but module fetch remains HTTPS | **Fit as E/F** | strong on fabric | partial | high | **Recommended scope**; boot is a named exception |
| **I. HTTP/3 for MCP** | QUIC + QPACK + HTTP semantics | **No room** for QPACK on the guest | strong | standard MCP/HTTP | very high | **Defer**; use custom MCP binding internally, HTTP/3 at gateway |
| **J. Custom stream layer over UDP (Noise + mux)** | Not QUIC; borrows stream/flow-control design | **Fits** in ~30–50 KB | strong (Noise) | none | medium | **Reject as "QUIC"**; note as cheaper alternative if only security/mux is wanted |
| **K. Dual-stack TCP + QUIC at runtime** | Both paths, negotiated | Doubles code/tests | — | — | high | **Reject** under pre-release; feature flag during development only |

### 3.2 Why the existing Rust stacks do not fit

Measured on this host, x86_64, release, `size -t` on the rlib `.text`:

| Crate / artefact | Version | `.text` | Notes |
|---|---:|---:|---|
| `quinn-proto` rlib (default `rustls-ring`) | 0.11.19 (2026-09-30) | **678,549 B** | std-only; `bytes`, `slab`, `thiserror`, `std::time::Instant`, `std` collections |
| `rustls` rlib (TLS 1.3 only, ring, no tls12/logging/webpki-roots) | 0.23.46 (2026-10-14) | **841,678 B** | host; x86_64 |
| `rustls` rlib, **MIPS n32**, `opt-level=z`, no default features | 0.23.46 | **482,113 B** | **[M]** compiled with the repository target; webpki is a mandatory dependency in 0.23.46 |
| Host binary using quinn-proto + rustls | — | **2,136,938 B** text | vs 267,053 B for a `println!` baseline |
| MCP module ELF footprint | — | **428,965 B** | includes 69,449 B `.bss` |
| **Free in slot A** | — | **95,323 B** | — |

Even the MIPS rustls rlib, which is before link-time dead-code elimination, is five times
the free space in slot A. A linked subset will be smaller, but not five times smaller with
webpki, X.509, TLS 1.2 and a full QUIC state machine. This is why "port quinn" is not a
plan: it is a rewrite with a large dependency graph attached.

The only no_std QUIC efforts visible in October 2026 are single-author prototypes with
single-digit star counts (`milli-http`, `minip2p`, `purecrypto`); none has a release, a
compliance suite or a MIPS build. There is no production no_std QUIC stack to adopt.

The MCP module is also close to the slot edge: at `opt-level=2` the same source builds to
**594,072 B** and does not fit slot A at all. `opt-level=z` is not a preference; it is a
build requirement. Any new code must be measured against the `z` footprint, and the
compiler's inlining choices can move the number by kilobytes.

The only native options that fit are E (rustls with a repartition) or F (a much smaller
hand-written TLS). Option G avoids the guest cost entirely.

### 3.3 Library and licence assessment

| Component | Version / date | Licence | no_std | MIPS n32 | Assessment |
|---|---|---|---:|---|---|
| `quinn-proto` | 0.11.19 (2026-09-30) | MIT OR Apache-2.0 | **no** (`no_std` issue #579 open since 2019) | no (ring) | Host interop only |
| `quinn` | 0.11.12 (2026-09-14) | MIT OR Apache-2.0 | no (tokio) | no | Host interop only |
| `s2n-quic` | 1.91.0 (2026-10-14) | Apache-2.0 | no (std/tokio) | no (aws-lc) | Reference only |
| `s2n-quic-core` | 0.91.0 | Apache-2.0 | **yes** (alloc) | maybe | Not a full transport; internal crate |
| `quiche` | 0.30.0 (2026-09-17) | BSD-2-Clause | no (C BoringSSL) | no | Reject |
| `neqo` | v0.34.0 (2026-10-22) | MIT OR Apache-2.0 | no (NSS) | no | Reject |
| `rustls` | 0.23.46 (2026-10-14) | Apache-2.0 OR ISC OR MIT | **yes** (`#![no_std]` + `alloc`) | **yes, compiles** | Recommended TLS, with a custom provider and a repartition |
| `ring` | 0.17.14 (2025-03-11) | Apache-2.0 AND ISC | — | **no** | Reject |
| `aws-lc-rs` | 1.18.2 (2026-10-12) | ISC AND (Apache-2.0 OR ISC) | — | **no** (CI: aarch64/ppc/x86; no MIPS) | Reject |
| `graviola` / `rustls-graviola` | 0.4.2 / 0.4.0 (2026-10) | Apache-2.0 OR ISC OR MIT-0 | — | **no** (aarch64/x86_64 only) | Reject |
| `rustls-rustcrypto` | 0.0.2-alpha (2024-04-24) | MIT OR Apache-2.0 | yes | probably | Stale; do not depend; write a small provider |
| `embedded-tls` | 0.19.0 (2026-06-01) | Apache-2.0 | yes | yes (already in kernel) | Client-only; no QUIC handshake API; no server |
| `ed25519-dalek` | 2.1 (in tree) / 3.0.0 (2026-07-06) | BSD-3-Clause | yes | yes (already linked in MCP) | Reuse |
| `x25519-dalek` | 3.0.0 (2026-07-06) | BSD-3-Clause | yes | yes | Add for TLS key exchange |
| `chacha20poly1305` | 0.11.0 (2026-08-05) | Apache-2.0 OR MIT | yes | yes (portable) | Preferred AEAD |
| `aes-gcm` | 0.11.1 (2026-08-21) | Apache-2.0 OR MIT | yes | yes (portable) | Optional; slower and timing-sensitive |
| `sha2` | 0.11.1 (2026-10-12) | MIT OR Apache-2.0 | yes | yes | TLS transcript/HKDF |
| `h3` / `h3-quinn` | 0.0.8 / 0.0.10 (2025-05-06) | MIT/Apache-2.0 | no | no | External gateway only |
| `iroh` | 1.4.1 (2026-10-21) | MIT OR Apache-2.0 | no | no | Reference for relay/migration design |

`quic-go` (Go) is mature and readable, but it is a Go runtime dependency and cannot be
embedded in a no_std module. It is useful as a second interop peer and as a design
reference for a small state machine; it is not a candidate.

### 3.4 Cipher cost on MIPS-class hardware

The wire is 10 Mbit ≈ **1.25 MB/s**; with 1,200-byte QUIC packets that is ~1,042
packets/s and ~1.18 MB/s of payload. The CPU must AEAD every byte and protect every
header.

Published software-crypto measurements on 32-bit embedded cores:

| Platform | AES | ChaCha20 / Poly1305 | SHA-256 | Source |
|---|---:|---:|---:|---|
| PIC32MZ (MIPS32) @200 MHz, wolfSSL software | AES-CBC 0.26 MB/s; AES-CTR 0.69 MB/s | — | 1.678 MB/s | wolfSSL benchmarks **[M-vendor]** |
| Cortex-M4 @120 MHz, wolfSSL | AES 0.642 MB/s | ChaCha 3.488 MB/s; Poly1305 6.104 MB/s | 1.744 MB/s | wolfSSL benchmarks |
| Cortex-M7 @216 MHz, wolfSSL | AES-GCM 2.553 MB/s | ChaCha-Poly 10.474 MB/s | — | wolfSSL benchmarks |
| Cortex-M0+ (kB/s column), BearSSL | AES-CBC ct 58.1 kB/s | ChaCha20 ct 550.7 kB/s | — | BearSSL speed page |

Derived CPU share at 1.18 MB/s **[D]** (rows are AEAD throughput, columns are CPU share):

| AEAD throughput | 0.64 MB/s | 1.0 MB/s | 2.5 MB/s | 5 MB/s | 10 MB/s |
|---|---:|---:|---:|---:|---:|
| CPU share needed for 1.18 MB/s | **184 %** | 118 % | 47 % | 24 % | 12 % |

Readings:

- **AES-GCM software is not viable at 10 Mbit** on a 200 MHz MIPS core unless the
  implementation is unusually fast. On the PIC32MZ (a much newer MIPS32 core) AES-CTR is
  0.69 MB/s, i.e. the link is already 1.8× faster than the cipher.
- **ChaCha20-Poly1305 is the only realistic AEAD** for the native profile. It is
  constant-time by construction, has no table lookups, and is 4–15× faster than software
  AES on these cores. At 200 MHz, expect 4–10 MB/s; the 1.18 MB/s link costs 12–30 % CPU,
  leaving room for packet processing.
- **On the emulated 50 MHz guest**, even ChaCha is marginal: 1–3 MB/s **[H]**, so the
  achievable QUIC rate is ~0.3–0.8 MB/s, not 1.18 MB/s. This must be measured, not assumed.
- **Handshakes are rare and can be slow.** X25519 and Ed25519 at 200 MHz are single-digit
  to tens of milliseconds; at 50 MHz they are 4× that. For long-lived mesh connections that
  is acceptable; for a per-request connection it is not (another reason to keep connections
  alive and avoid 0-RTT complexity).
- **SHA-256 is handshake-only.** At 1.7 MB/s it would be a problem if it ran per packet;
  QUIC does not.

### 3.5 Code and memory budgets

**Code.**

| Candidate | Estimated MIPS n32 `.text` at `z` | Fits 95 KiB slot A? | Fits 768 KiB slot A with MCP? |
|---|---:|---|---|
| quinn-proto + rustls + provider | 400–700 KB | no | no (with MCP 338 KB + bss) |
| in-tree QUIC + rustls + RustCrypto provider | 250–450 KB | no | maybe (338 + 250 + 70 = 658 KB) |
| in-tree QUIC + hand-written TLS 1.3 | 120–200 KB | no (with MCP) | **yes** (338 + 150 + 70 = 558 KB) |
| custom stream layer + Noise (not QUIC) | 30–60 KB | maybe | yes |
| gateway (no guest QUIC) | 0 | yes | yes |

These are estimates; the linked size is the spike's job. The rustls **rlib** measurement
(482 KB) is the only hard MIPS number and is already five times slot A's free space.

**Memory.**

| Item | Per unit | 8 units | 16 units | 128 units |
|---|---:|---:|---:|---:|
| QUIC connection state (sent packets 256×64 B, streams 32×128 B, TLS, buffers) | 24–48 KiB | 192–384 KiB | 384–768 KiB | 3–6 MiB |
| Kernel UDP buffers (2 slots × 32 KiB) | 64 KiB total | — | — | — |
| Seeq RX ring 32 × 1520 B | 48 KiB | — | — | — |
| MCP + QUIC static pool | — | 128–256 KiB | 256–512 KiB | pool capped |

The module's bump allocator never frees, so the QUIC module must use a **fixed pool** of
connection slots. A `Vec`-per-connection design would leak the entire heap on the first
reconnect.

**Module-area layout.** The 2 MiB area is fully committed today (512 KiB + 1.5 MiB). Adding a
native QUIC stack requires an ADR to repartition it:

| Layout | A | B (manager + arenas) | Wasm arenas | Comment |
|---|---:|---:|---:|---|
| Today | 512 KiB | 1.5 MiB = 256 KiB + 5×256 KiB | 5 | MCP alone, 95 KiB free |
| **Recommended first cut** | **768 KiB** | **1280 KiB = 256 KiB + 4×256 KiB** | **4** | MCP + QUIC fits; loses one arena |
| If QUIC ≤ 120 KB | 640 KiB | 1408 KiB = 256 KiB + 4×256 KiB + 128 KiB | 4 | Tight; requires bss trimming |
| If rustls-based QUIC ≥ 300 KB | 896 KiB | 1152 KiB = 256 KiB + 3×256 KiB + 128 KiB | 3 | Loses two arenas |
| Long-term | grow area to 3 MiB by moving Seeq DMA below the shadow | 1.5–2 MiB | 5+ | Memory-map change; separate ADR |

The long-term option: the Seeq DMA window is 1 MiB at `0x0860_0000`–`0x0870_0000`; the
framebuffer shadow ends at `0x08AC_0000`; the frame allocator starts at `0x08B0_0000`.
Moving the Seeq window to `0x08AC_0000` (256 KiB is enough) would free the 1 MiB below it
and grow the module area to 3 MiB. That is a kernel memory-map change with DMA/cache
implications; it is not part of the first cut.

---

## 4. Recommended architecture and API/ABI

### 4.1 Placement

**Recommended:** a single module image, `mcp-net`, in a slot A repartitioned to 768 KiB
(§3.5), containing the MCP JSON-RPC server and the QUIC transport. It owns one UDP socket,
the QUIC endpoints, TLS, the stream scheduler, and the mesh/MCP bindings.

Why one image:

- There is no spare boot slot. Slot B is fully committed to the manager and the Wasm
  arenas. A separate transport module would require the same repartition *plus* a
  cross-module ABI.
- Modules share the kernel address space; a separate module does not buy isolation. The
  security boundary is the kernel, not the module (MCP spec §1.1).
- The MCP HTTP parser and the 4-tick phase machine disappear when MCP moves to QUIC
  streams, offsetting part of the QUIC code.

The transport exports a small C stream API (`irix7_quic_*`) through the module export
table, so the manager and future runtime modules can use it without duplicating QUIC:

```c
/* Module-to-module export; all pointers are in the shared module address space. */
int32_t quic_connect(const uint8_t node_id[16], const uint8_t *alpn, uint32_t alpn_len);
int32_t quic_accept(uint32_t listener);
int32_t quic_open(uint32_t conn, uint32_t class, uint32_t *stream);
int32_t quic_send(uint32_t stream, const uint8_t *buf, uint32_t len, uint32_t fin);
int32_t quic_recv(uint32_t stream, uint8_t *buf, uint32_t len);
int32_t quic_reset(uint32_t stream, uint32_t code);
int32_t quic_close(uint32_t conn, uint32_t code);
int32_t quic_stats(uint32_t conn, QuicStats *out);
```

This is not a kernel ABI; it is an in-tree module export. The kernel ABI only carries UDP,
timers and entropy.

If the maintainers prefer a separate transport image, use slot B with a 512 KiB QUIC region
and three arenas; the rest of this document is unchanged.

### 4.2 Kernel prerequisites

QUIC cannot be primary on the current kernel. The prerequisites, in order:

1. **Non-blocking Seeq TX.** Replace the per-frame spin (`kernel/src/net.rs:357`) with a
   descriptor ring and completion (interrupt or a bounded poll), so the CPU can encrypt the
   next packet while the wire drains. Without this, the CPU can spend the whole inter-packet
   interval waiting for the descriptor, leaving no time for crypto; the wire itself caps
   throughput at 1.25 MB/s regardless.
2. **Bulk RX copy.** Replace the per-byte `read_volatile` loop (`kernel/src/net.rs:283`)
   with word copies; keep the uncached alias.
3. **RX ring 32.** At 10 Mbit and 1,200-byte packets, 8 descriptors hold 7.7 ms; a 10 ms
   poll loses frames. 32 descriptors hold ~31 ms and 48 KiB.
4. **Event-driven RX** (issue #248): Seeq/HPC3 interrupt sets a pending bit; the loop drains
   a bounded number of frames and computes per-slot readiness. The current trap vector
   treats every external interrupt as a timer (#248 finding 3); fix that first.
5. **1 ms timer and module deadlines.** `idle::INTERVAL` 330,000 → 33,000 (1 ms at 33 MHz),
   and `ModuleVtable` gains `next_deadline_ms()` so `module::poll` can call QUIC at its
   pacing/PTO deadline instead of the 10 ms gate. A 1 ms tick at 1,000 Hz is ~2 % of a
   50 MHz core; measure idle CPU.
6. **UDP module service** (ABI 13): bind, send, recv with source metadata, close, state,
   event mask. Two slots, 32 KiB RX/TX each.
7. **CSPRNG service** (ABI 13): `random(ptr, len)` backed by a seeded ChaCha20 DRBG, not
   xorshift. QUIC's CIDs and TLS keys depend on it.
8. **`clock_us`** (ABI 13): a 64-bit microsecond clock, or the host-side profiler, for
   handshake/RTT/tail measurements (the current `clock_ms` quantises to 1 ms).
9. **`module_load_image`** (ABI 14): load an already-fetched, digest-verified image, so the
   QUIC transport can fetch modules for the manager without the kernel's HTTPS client.

Items 1–4 are useful even if QUIC is rejected; they are the transport-hygiene slice the
wire study already recommends. Item 5 is the ABI change #248 anticipated.

### 4.3 UDP service ABI (proposed ABI 13)

Append-only to `Services` (offsets are illustrative; the SDK crate pins them):

```c
int32_t  udp_bind(uint32_t slot, uint16_t port);
uint32_t udp_state(uint32_t slot);          /* 0 closed, 1 bound, 2 error */
intptr_t udp_send(uint32_t slot, uint32_t addr_be, uint16_t port,
                  const uint8_t *ptr, size_t len);
intptr_t udp_recv(uint32_t slot, uint8_t *ptr, size_t len, UdpMeta *meta);
uint32_t udp_events(uint32_t slot);         /* READABLE | WRITABLE | CLOSED */
void     udp_close(uint32_t slot);
```

`UdpMeta` is `{ uint32_t addr_be; uint16_t port; uint16_t len; }`. `udp_send` returns bytes
accepted or `-1`; `udp_recv` returns bytes, `0` for none, `-1` for no socket. The service
must not block; a full TX ring returns `0`/`WouldBlock`, and the module waits for
`WRITABLE`.

Buffer sizes: 32 KiB RX and 32 KiB TX per slot. Two slots (QUIC and diagnostics). The
kernel BSS grows by 128 KiB, not PROM (BSS is NOLOAD), but the kernel image's compressed
payload does grow by the code; the 1,084 B headroom is the gate.

### 4.4 QUIC profile

**v1 profile (IRIX-to-IRIX and native control):**

- QUIC **version 1** only (RFC 9000). No v2, no version negotiation; unknown versions get a
  Version Negotiation packet or a close.
- TLS 1.3 (RFC 8446), **ChaCha20-Poly1305-SHA256** mandatory; AES-128-GCM-SHA256 optional
  for external peers only.
- **X25519** key exchange; **Ed25519** signatures; **raw public keys** (RFC 7250) rather
  than X.509. No RSA, no P-384 in the native profile.
- ALPN: `irix7-mesh/1`, `irix7-mcp/1`, `irix7-module/1`.
- 1,200-byte packets (safe minimum, RFC 9000 §14); 1,472 on an IPv4 LAN after PMTUD
  (1,500 − 20 IP − 8 UDP); no IP fragmentation; DF set.
- 8-byte connection IDs, routable (see §4.5).
- NewReno congestion control (RFC 9002 §7), pacing at 1 ms, initial cwnd 4–10 packets,
  max cwnd capped to the BDP of the 10 Mbit link.
- ACK every 2 ack-eliciting packets or 5 ms; no ACK-frequency extension in v1.
- Idle timeout 30 s; heartbeat/PING every 5 s when idle; PTO max 3.
- **0-RTT disabled** in v1.
- Connection migration enabled after handshake; path validation per RFC 9000 §8.2.
- Datagrams (RFC 9221) optional for heartbeats/hints; not for control state.
- Key update supported but not exercised at 10 Mbit.

**Explicit non-goals for v1:** HTTP/3, QPACK, multipath, QUIC-LB wire format, v2, GREASE
extensions, 0-RTT, connectionless retry beyond a stateless token, and any extension that
does not have a stable RFC.

#### 4.4.1 n32 MIPS details

- The target is **n32**: 32-bit pointers, 64-bit integer registers, big-endian, soft-float,
  `max-atomic-width = 64`, LL/SC atomics (`targets/mips3-sgi-irix7-n32.json`). `core::sync::atomic`
  works for the widths QUIC needs; `portable-atomic` is not required. The module is
  single-threaded, so atomics are for allocator/`Arc` compatibility, not concurrency.
- **QUIC is network byte order (big-endian)**, which matches MIPS; varints, packet numbers
  and TLS lengths parse without byte swaps. The mesh wire is little-endian, so the boundary
  between a QUIC stream and a `MeshEnvelope` is where conversion happens; do not blanket-swap
  the stream.
- 64-bit packet numbers and offsets fit n32's 64-bit integer registers; avoid `usize` for
  them, because `usize` is 32-bit.
- The module is linked at a fixed address with no relocation (`modules/mcp/linker.ld`); the
  QUIC pool and buffers must be statically sized or allocated from the kernel bump heap once
  at init.

### 4.5 Connection IDs, routing and relay

A connection ID is not a NodeId. It is a short, unpredictable routing handle:

```text
8 bytes:  epoch:u8 | node_index:u16 | hmac:u40
```

- `node_index` is the index in the confirmed membership snapshot (0..127); it is compact and
  epoch-scoped.
- `hmac` is truncated HMAC-SHA-256 over `(epoch, node_index, secret)`; the secret is a
  per-boot CSPRNG value shared with the relay out of band (or derived from the node key).
- The CID rotates on epoch change and on path migration.
- A blind UDP relay can route a datagram by its destination CID without terminating TLS:
  it maps CID prefix → node endpoint, forwards the datagram, and never sees plaintext.
- This is the *idea* of QUIC-LB (`draft-ietf-quic-load-balancers-22`, 2026-10-23), not its
  wire format; the draft is not an RFC and must not be claimed as compliance.

The relay replaces the M3 TCP hub. It is stateless per packet, stateful only in a
CID→endpoint table with a TTL. It removes the hub's 2 ms/128 B writer and its
head-of-line blocking, and it preserves end-to-end encryption (no double TLS).

QUIC has **no service discovery**. The mesh keeps configured peer addresses plus the
confirmed membership view; DNS-SD (RFC 6763) is not needed on the fabric, and the QUIC
address-discovery draft (`draft-ietf-quic-address-discovery-01`) only reports an endpoint's
observed address. STUN/TURN/ICE are not in v1; a node behind an endpoint-dependent NAT uses
the relay, and a node that cannot reach the relay is unsupported.

### 4.6 Stream mapping

| Stream class | QUIC stream | Contents | Lifetime |
|---|---|---|---|
| Control | one unidirectional stream per direction (IDs 2/3) | join, epoch push, heartbeat, epoch-change, GOAWAY | connection |
| MCP | one bidirectional stream per request | length-prefixed JSON-RPC request/response, or notifications on a server-initiated uni stream | request |
| RPC | one bidirectional stream per invoke | stream preface + `MeshEnvelope` (or compact record) + completion trailer | request |
| Migration | one bidirectional stream per migration | `PREPARE/STATE/COMMIT` or `ABORT`; `RESET_STREAM` for abort | migration |
| Bulk | one bidirectional stream per object/range | CAS bytes or file range | transfer |
| Datagram | RFC 9221 datagram | heartbeat hint, route hint, liveness | one packet |

The stream preface (RPC class) carries the fields the mesh header currently carries:

```text
u8   version
u8   opcode
u16  flags
u32  home_epoch
u32  object_epoch
u64  request_id
u32  deadline_ms
u16  cap_handle          (0 = inline capability follows)
u8   trace_id[16]
u8   span_id[8]
```

The trailer carries `{ status:u16, object_epoch:u32 }`. A cancelled request is
`RESET_STREAM` with an application error code; the peer answers `STOP_SENDING`. This maps
the existing completion model onto QUIC without inventing a second cancellation protocol.

The existing `mesh-transport` `ByteSink`/`ByteSource` traits can be implemented per stream;
the codec (`u32 length + MeshEnvelope`) is reused until the compact v2 record lands. QUIC
does **not** remove the need for the compact record; it only removes the TCP framing and
provides the mux.

### 4.7 TLS identity and key provisioning

- **Node identity:** the existing 16-byte `NodeId` plus a 32-byte Ed25519 public key. The
  private seed lives in NVRAM (`mesh.key`) and is never in the repository or a log.
- **Root identity:** the root's Ed25519 public key is pinned in each node's NVRAM
  (`mesh.pub`), as today. The root signs the epoch snapshot as today (M2-ADR-0003).
- **Member key provisioning:** every member needs a key seed. This is the prerequisite #245
  identified for Noise; QUIC forces it. The rig writes the seed into the DS1386 image; a
  real deployment needs a secure element or an operator-provisioned seed. The DS1386 is not
  secure storage; say so.
- **TLS authentication:** raw public keys (RFC 7250) with Ed25519. The root's key is the
  trust anchor for the join connection; the member's key is bound to its `NodeId` by the
  root-signed epoch confirmation. Subsequent connections pin the key from the confirmed
  snapshot. No X.509, no hostname verification, no RSA chain building.
- **Fallback if rustls raw public keys are not usable:** a self-signed X.509 certificate
  containing the Ed25519 key, verified by a custom `ServerCertVerifier`/`ClientCertVerifier`
  that checks the key against the membership snapshot. This keeps the same trust model with
  more bytes.
- **No certificate files:** bindings are derived from the epoch snapshot and cached in RAM;
  a reboot re-joins and re-fetches. NVRAM stays small.

### 4.8 Failure, retry and close

| Event | Behaviour |
|---|---|
| Handshake timeout | 5 s; retry with backoff 250 ms → 10 s, ±25 % jitter |
| Idle timeout | 30 s; heartbeat every 5 s |
| PTO | RFC 9002 §6.2; max 3 probes; then close |
| Peer close | `CONNECTION_CLOSE` with an IRIX error code; all pending tags get `Broken` |
| Path validation failure | close `PATH_UNVALIDATABLE`; do not silently fall back to another path |
| Epoch change | control stream `EPOCH_CHANGED`; streams below the epoch floor are reset; handles invalidated |
| Migration | QUIC path migration; NodeId and TLS session unchanged; no re-join |
| Relay loss | CID mapping TTL expires; the node re-registers on the control connection |

The transport must never discard a pending operation without a terminal completion. This is
the gap the wire study found in the TCP path (`disconnect` clears the queue and pending
table without a `Broken` status); QUIC's `CONNECTION_CLOSE` is the natural place to fix it.

### 4.9 0-RTT policy

**Disabled in v1.** The reasons are concrete:

- RFC 9001 §9.2: 0-RTT is replayable; "disabling 0-RTT entirely is the most effective
  defense".
- The native flows include mutations: capability invoke with `WRITE`/`EXECUTE`, home move,
  migration `COMMIT`, module activation. A replayed 0-RTT frame could repeat a mutation.
- QUIC's own state is replay-safe; the application's is not. The mesh's `request_id` guard
  is not a full replay window at the transport.
- On a LAN the handshake is a few milliseconds; 0-RTT buys little.

If it is enabled later, it must be limited to idempotent reads, carry an `early=1` flag, be
gated by the epoch floor and a per-node request-id window, and never carry `MIGRATE`,
`HOME_MOVE`, `COMMIT` or a capability grant.

### 4.10 Observability

- **qlog-lite** (design reference: `draft-ietf-quic-qlog-main-schema-14`, 2026-07-06):
  `connection_started`, `packet_sent`, `packet_received`, `packet_lost`,
  `metrics_updated`, `stream_started`, `stream_finished`, `connection_migration`,
  `key_updated`, `connection_closed`.
- Compact binary ring in the module; export over `/mesh/quic` or the serial console; a host
  tool renders qlog JSON.
- Counters: handshake ms, min/smoothed RTT, cwnd, bytes/packets, loss, PTO, streams,
  flow-control blocks, 0-RTT accepted/rejected, migrations, CID rotations.
- **Test-only key log** (equivalent to `SSLKEYLOGFILE`) behind an NVRAM flag, so Wireshark
  can decrypt a lab capture. Never enabled in a release image.
- The relay records CID→endpoint mappings and packet counts, not payloads.

### 4.11 Boot trust chain

Today:

```text
ROM (assembly, no network)
  → kernel: DHCP, DNS, NTP
  → HTTPS/TLS 1.3 client (embedded-tls, CA roots, SHA-256)
  → boot/config.conf (mcp.url, mcp.sha256)
  → fetch module over HTTPS, verify SHA-256, load slot A
  → module runs
```

Proposed:

```text
ROM → kernel → HTTPS/TCP+TLS → boot config → verified MCP+QUIC image (SHA-256)
  → QUIC for every subsequent native flow (mesh, MCP, manager, RPC)
```

The first fetch stays TCP+TLS for three reasons:

1. No QUIC stack fits in 1,084 B of PROM headroom, and the ROM has no network at all.
2. TLS is the boot trust anchor; replacing it with plain HTTP would require a signed config
   and a new anti-rollback story (a separate ADR).
3. The product spec names HTTPS URLs and SHA-256 as the distribution contract
   (`docs/spec/system.md` §6); changing that is a product decision, not a transport detail.

**Slim-PROM option (separate ADR).** The kernel TLS client dominates the ~1 MiB kernel
text (the ROM README says the TLS client and verifier dominate); removing it is expected to
free a large part of the compressed payload, though the exact figure is unmeasured **[H]**.
The replacement is a signed plaintext bootstrap: fetch a config and module
over HTTP, verify an Ed25519 signature from a root key in ROM, and check the SHA-256 digest.
Integrity would come from the signature, not TLS; confidentiality of public images is not
required. This would:

- free enough PROM for the UDP/timer/IRQ prerequisites with real headroom;
- leave **one** TLS implementation (in the QUIC module), avoiding the current split;
- change an external contract (HTTPS URLs) and lose transport confidentiality at boot;
- require a signed config, a key-rotation/anti-rollback policy and an ADR.

It is attractive, but it is not part of the first QUIC cut. Do not smuggle it in.

**Double encryption.** There is none today: the mesh is plaintext, and the kernel TLS is
only on the module-fetch path. The risk is adding QUIC *and* keeping the kernel TLS,
producing two TLS stacks (code size and attack surface). The target is one TLS stack
(rustls or the minimal TLS) used by QUIC; the kernel's embedded-tls is retired when the
slim-PROM ADR lands. Do not run TLS inside QUIC streams, QUIC inside TLS, or QUIC-in-QUIC.

### 4.12 External interfaces

| Interface | Stays | Why |
|---|---|---|
| POSIX `socket(AF_INET, SOCK_STREAM/DGRAM)` | TCP/UDP | POSIX.1-2024, IRIX 6.5 apps, relibc |
| External MCP clients | Streamable HTTP at a gateway; HTTP/1.1 or HTTP/3 | standard clients; no guest HTTP/3 |
| Module URLs | HTTPS | product distribution contract |
| Telnet console | TCP | management |
| DHCP/DNS/NTP | UDP | bootstrap |
| Host rig | TCP/UDP forwards | test only; iris supports UDP forwards (`ForwardProto::Udp`, `nat_udp.rs`) |

If the maintainers want external MCP clients to speak QUIC directly, the cost is HTTP/3 +
QPACK at the gateway or on the guest; that is a separate product decision (Option I/G).

---

## 5. Inventive design details

### 5.1 Capabilities over QUIC streams

The existing capability is a 320-byte Ed25519-signed bearer token. QUIC changes the channel,
not the authorisation model. The design:

- **Channel identity:** TLS authenticates the *node* (NodeId + Ed25519 key). It does not
  authenticate the *subject* of a capability. A capability whose `subject` is an actor is
  still a bearer token; the node must present a delegation proof or be the subject.
- **Capability handles:** the first `INVOKE` on a connection carries the inline 320-byte
  token; the receiver validates issuer, subject, object, rights, epochs and signature, then
  installs a 4-byte handle in a per-connection table. Later invokes carry the handle.
- **Migration preserves handles:** QUIC connection migration keeps the connection state, so
  handles survive an address change. This is a real advantage over TCP, where a reconnect
  loses the handle and re-sends 320 bytes.
- **Reconnect re-installs:** on a new connection, the first invoke carries the inline token
  again, or a short resumption token derived from the old connection and the epoch. No
  handle table survives a connection close.
- **Revocation:** an epoch change invalidates handles; the control stream sends
  `EPOCH_CHANGED`; the receiver resets streams below the epoch floor and returns
  `Status::Broken` for pending tags. A capability whose `home_epoch` is below the current
  epoch is rejected at the stream preface, before the body is read.
- **No double crypto:** the capability signature is for authorisation, not channel
  integrity. TLS covers the bytes; do not add a per-record MAC or a second signature.
- **Replay:** `request_id` plus the epoch floor is the application replay guard. 0-RTT is
  off; a replayed 1-RTT stream is impossible within a connection (QUIC packet numbers and
  TLS), and a replayed new connection fails the TLS handshake.

### 5.2 Epoch fencing on streams

Every RPC stream preface carries `home_epoch` and `object_epoch`. The receiver:

1. rejects a stream whose `home_epoch` is below the confirmed epoch (stale home);
2. rejects a stream whose `object_epoch` is below the object's current epoch (stale object);
3. records the stream in the pending table with its epoch;
4. on epoch advance, resets streams that have not committed and completes their tags with
   `Broken`/`Moved`.

This is the transport-level expression of the lease/fencing rules in M2-ADR-0008. It makes
the "stale home cannot commit" invariant independent of TCP reconnect timing.

### 5.3 Join, homes, route and migrate over QUIC

| Protocol | Mapping | What changes |
|---|---|---|
| Join | one bidirectional stream; `JOIN_REQUEST` → `EPOCH_CONFIRM` → `JOIN_ACK` | TLS authenticates the root and proves member key possession; the join body binds NodeId to key; the epoch confirmation signs the member list including keys |
| Homes | no transport change; `GET /mesh/homes` becomes an RPC stream | response is length-prefixed JSON or a compact table |
| Route / `MOVED` | one RPC stream; `MOVED` returns a retry hint; exactly one retry | stream cancellation is `RESET_STREAM`; the retry opens a new stream |
| Migrate | one migration stream; `AUTHORISE/PREPARE/READY/STATE/COMMIT/ACK` | `STATE` can be large; QUIC flow control handles it; `ABORT` is `RESET_STREAM` |
| Failure | control stream heartbeat + QUIC idle/PTO | no TCP half-open; a restarted peer gets a new CID and re-registers |

**Join ordering:** the member dials the root's configured address (the root's raw public key
is pinned). TLS proves the root; the join request proves the member's key possession. The
root admits the member and signs the epoch; the member is then a confirmed peer. A member
that has not joined can open only the join ALPN; after confirmation, it can open the mesh
ALPN. This is stricter than today's unsigned join and uses the same authority root.

**Homes/routes:** the transport does not resolve homes; `mesh-core` does. QUIC only carries
the message. Do not put routing state in the CID beyond the node index; a CID is a hint, not
an authority.

**Migration:** distinguish two things:

- **QUIC path migration** — the 4-tuple changes, the connection and NodeId stay. Used for
  DHCP renewal, NAT rebinding, interface change.
- **IRIX actor/home migration** — the actor moves to another node; the old home drains, the
  new home takes over. Open QUIC streams are node-local resources and do **not** migrate
  with the actor (`docs/spec/system.md` §8). The migration stream transfers state; the old
  streams are reset; the new home opens new streams.

Conflating these would be a design error. Say so in the ADR.

### 5.4 Avoiding double encryption and duplicate TLS

- **One TLS layer end-to-end:** QUIC's TLS protects every stream. Do not add a mesh MAC,
  a per-record signature, or a second TLS inside a stream.
- **One TLS implementation:** choose rustls (no_std) or the minimal TLS, not both. Retire
  the kernel's embedded-tls when the slim-PROM ADR lands. Until then, the two stacks are on
  disjoint paths (boot vs runtime), not nested.
- **Boot is sequential, not nested:** the HTTPS module fetch completes before QUIC starts.
  The module payload is not re-encrypted by QUIC.
- **Relay is blind:** a CID-routed UDP relay forwards ciphertext; it does not terminate TLS.
  A TLS-terminating gateway is a declared trust boundary, and the inner link should be on a
  trusted physical segment (or use a single encryption layer, not two).
- **No QUIC-in-QUIC:** MASQUE CONNECT-UDP (RFC 9298) is for external HTTP/3 access, not for
  the mesh. QUIC-in-QUIC doubles the AEAD cost, which this CPU cannot afford.
- **Capability tokens** are signed for authorisation; TLS is the channel. Do not encrypt
  them again.

### 5.5 Congestion and fairness on 10 Mbit

The bottleneck is one Seeq queue shared by all flows. The design:

- **Pacing at 1 ms:** at 10 Mbit, one 1,200-byte packet per millisecond. The 1 ms timer is
  the pacing clock. Sending a 10-packet burst every 10 ms fills the 8-descriptor RX ring and
  the TX spin; it also adds 10 ms of queueing.
- **Initial cwnd 4 packets (4.8 KB), not 10 (12 KB).** On a 1 ms RTT, 10 packets is 12 ms
  of queue; on a 10 ms tick it is a burst the driver cannot absorb.
- **Max cwnd = BDP cap.** At 10 Mbit and a 1 ms RTT, BDP is 1.25 KB; a 16 KB cwnd is 13×
  the path. Cap it to avoid bufferbloat in the Seeq/HPC3 queue.
- **NewReno, not CUBIC.** The path is short and slow; CUBIC's aggressive growth buys
  nothing and adds code. CUBIC (RFC 9438) is a later option.
- **ACK every 2 packets or 5 ms.** Fewer ACKs, less overhead; 5 ms is below the 10 ms tick,
  so the timer must be 1 ms.
- **Control priority is sender-side.** QUIC streams share one congestion window; the sender
  schedules control/join/heartbeat before bulk. This does not guarantee priority under
  loss, but it prevents a bulk stream from filling the TX ring ahead of control. For strict
  isolation, use a separate connection for bulk (cost: another handshake) — not v1.
- **TCP coexistence.** If POSIX TCP bulk runs at the same time, QUIC and TCP compete in the
  same FIFO. Either cap QUIC to a share or move the bulk to QUIC. Measure; do not assume
  fairness.
- **No ECN.** The Seeq/HPC3 path does not expose ECN; disable it.
- **No GRO/GSO.** smoltcp and the Seeq driver do not support it; each packet is a copy and a
  descriptor.

The 1 ms pacing clock and the non-blocking TX ring are what make this work. With a 10 ms
tick and a spin-per-frame driver, QUIC's congestion controller cannot regulate the queue.

### 5.6 A hub that does not head-of-line block

The M3 hub is a host TCP process that multiplexes every member onto the root's single TCP
connection. It adds 2 ms per 128 B and one member's bulk record delays every other member.

Replace it with a **CID-routed UDP relay**:

- Each node registers its current endpoint and CID prefix with the relay over a control
  connection (or the relay learns from the first packet and validates with the node's
  signature/token).
- The relay forwards each datagram to the endpoint named by the destination CID; it does not
  parse QUIC, decrypt, or terminate TLS.
- The root sees one QUIC connection per member (or one connection with per-member streams),
  and the transport's stream multiplexing plus the sender's control priority removes the
  byte-stream HOL.
- The relay is stateless per packet, so a node restart is just a new CID registration.

This preserves end-to-end encryption and removes the host process from the plaintext path.
It is also the natural way to scale to 8 nodes without eight TCP connections at the root.

At 128 nodes, the root still has a fan-in problem; QUIC streams and a bounded per-member
queue help, but a full mesh of 8,128 connections is not viable. The M4 design should use a
routing overlay (hub/relay hierarchy), not per-peer connections. QUIC migration and CIDs
make the overlay cheaper, not unnecessary.

### 5.7 A smaller MCP

MCP over QUIC streams removes:

- the HTTP/1.0 parser, request line, headers and `Connection: close`;
- the four-phase `Arm → WaitRequest → WaitClose → Closing → Arm` machine (40 ms per tool
  call, wire study §4.3);
- the 16 KiB response buffer truncation risk (the stream is flow-controlled, and the JSON
  writer can stream);
- the two-TCP-slot constraint (one UDP socket for QUIC).

It adds the MCP stream binding: 4-byte length + JSON-RPC per message, one bidirectional
stream per request, server notifications on a unidirectional stream. The JSON-RPC semantics
are unchanged; the MCP 2026-07-28 spec explicitly permits custom transports provided the
message format and patterns are preserved. External clients still use Streamable HTTP at a
gateway.

### 5.8 What would make this fail

Be explicit: if the measured ChaCha throughput is below ~1.2 MB/s, the Indy cannot AEAD a
10 Mbit line; QUIC can still be used for control at 0.3–0.5 MB/s, but bulk must stay on TCP
or move to a gateway. If the linked QUIC+TLS code exceeds the repartitioned slot, the
native option is dead and the gateway is the only path. If UDP NAT/forwarding cannot be
made reliable in the rig, the two-kernel evidence cannot be produced. These are the three
kill criteria; the spike is designed to test them first.

---

## 6. Phased implementation plan

Estimates are engineering weeks for one implementer who knows the tree; they are not
commitments. Each phase leaves the tree green and has a rollback point.

### 6.1 Phase 0 — measurement spike (2 weeks)

**Goal:** decide whether a native QUIC stack is physically possible on the Indy.

| Step | Work | Output |
|---|---|---|
| 0.1 | Guest microbenchmark module (temporary, not merged): ChaCha20-Poly1305, AES-128-GCM, SHA-256, X25519, Ed25519 sign/verify, memcpy, at 64/512/1200/8192 B | cycles/byte and MB/s on the emulated R4400; real-Indy run if available |
| 0.2 | UDP echo prototype: ABI 13 UDP service + a test module; iris UDP forward between two kernels; ring 8 vs 32, 1 ms vs 10 ms tick | p50/p99 RTT, pps, loss, idle CPU |
| 0.3 | Seeq TX/RX prototype: non-blocking TX ring + word RX copy; measure frames/s and CPU | proof that the driver can sustain 1,000 pps without spinning |
| 0.4 | Code-size spike: build rustls no_std + RustCrypto provider for n32; link a minimal QUIC frame codec; measure final `.text`/`.bss` | linked size, not rlib size |
| 0.5 | Host interop: quinn client/server on the host ↔ a host UDP bridge to the guest echo; verify packet sizes, CIDs, handshake bytes | harness and wire-format evidence |

**Gate G0 (all must pass):**

- ChaCha20-Poly1305 ≥ **2.0 MB/s** at 1,200 B on the guest (so 1.18 MB/s costs ≤ 60 % CPU).
- UDP echo p50 ≤ **1 ms**, p99 ≤ **5 ms** through the iris NAT/forward at 1,000 pps, < 1 % loss.
- Non-blocking TX sustains **1,000 pps** with < 50 % CPU at 1,200 B.
- Linked QUIC+TLS ≤ **180 KiB** `.text` + **64 KiB** `.bss` (or a repartition is approved).
- Handshake (X25519 + Ed25519) ≤ **50 ms** on the guest.

If G0 fails, stop native QUIC and choose Option G (gateway) or Option J (Noise + streams).

### 6.2 Phase 1 — kernel prerequisites (4–6 weeks)

| Step | Work | Gate |
|---|---|---|
| 1.1 | Seeq TX ring + completion; RX word copy; RX ring 32 | 1,000 pps, no spin, < 50 % CPU |
| 1.2 | Event-driven RX (#248): IRQ pending bit, bounded drain, readiness mask; fix the trap vector | M1/M2 boot markers; no missed frames |
| 1.3 | 1 ms timer + `next_deadline_ms`; `clock_us` | idle CPU ≤ 3 %; deadlines honoured |
| 1.4 | UDP service (ABI 13): 2 slots, 32 KiB buffers, metadata, events | UDP echo through two kernels |
| 1.5 | CSPRNG service (`random`) seeded from NVRAM/timer/device noise | statistical smoke test; no xorshift |
| 1.6 | PROM size reclamation if needed; hard gate | PROM ≤ 524,288 with **≥256 B** headroom |
| 1.7 | `module_load_image` (ABI 14) for manager-fetched images | manager loads a verified image without a kernel URL fetch |

**Gate G1:** PROM ≤ 524,288 with ≥256 B headroom; M1 boot, M1 module load, M2-1 markers
green; UDP echo p50 ≤ 1 ms; 1,000 pps; idle CPU ≤ 3 %.

### 6.3 Phase 2 — QUIC core (10–14 weeks)

Implement the profile in §4.4, in-tree, behind a development feature flag (not a runtime
fallback):

- packet encode/decode, varints, Initial/Handshake/1-RTT, header protection;
- TLS 1.3 via rustls QUIC API + RustCrypto provider (or the minimal TLS if the size gate
  fails); raw public keys; Ed25519/X25519;
- ACK, loss detection, PTO, NewReno, pacing;
- stream state, flow control, `RESET_STREAM`/`STOP_SENDING`;
- connection IDs, path validation, close, stateless reset;
- fixed connection pool, no per-connection heap;
- qlog-lite and counters.

**Host model first:** a deterministic host harness (no iris) with packet loss/reorder,
handshake fuzz, stream fuzz, and interop against quinn. Then the guest.

**Gate G2:**

- Handshake p50 ≤ **30 ms**, p99 ≤ **100 ms** on the two-kernel rig.
- Request p50 ≤ **5 ms**, p99 ≤ **20 ms** (event-driven path).
- Bulk ≥ **0.4 MB/s** (1 MiB transfer) with ≤ **70 %** CPU at 0.4 MB/s.
- RAM ≤ **48 KiB** per connection; 8 connections in ≤ 384 KiB.
- Linked `.text` ≤ **180 KiB**; `.bss` ≤ **96 KiB**; slot A fits with the approved repartition.
- Interop: 100 % of handshakes and streams against quinn on the host (test vectors, not
  production interop).

### 6.4 Phase 3 — mesh and MCP cutover (6–8 weeks)

- Replace the TCP mesh link with QUIC streams; keep the codec until the compact record lands.
- Join/route/migrate over streams; capability handles; epoch fencing; terminal `Broken`.
- MCP JSON-RPC over streams; remove the HTTP phase machine from the Indy.
- External MCP gateway (host) for Streamable HTTP; no guest HTTP.
- Update M2/M3 rig: UDP forwards, QUIC mode, relay CID router; `--fault` modes.

**Gate G3:**

- M2-1/2/3/4/5/6 markers; M3 shard markers on the committed multi-node hub (three kernels committed; eight-node policy is host-tested — do not claim eight real kernels unless the rig supports it).
- Packet capture: **zero TCP bytes** on the mesh after boot.
- Partition (SIGSTOP) → suspect 6 s, failed 12 s, fence, no split brain; SIGCONT rejoins.
- Restart → new CID, rejoin; no stale tag leaks.
- Wrong key/NodeId → handshake reject; no membership change.

### 6.5 Phase 4 — module manager and service RPC (4–6 weeks)

- Manager uses the transport export for module fetch; `module_load_image` loads verified
  bytes.
- Module fetch over QUIC; boot fetch stays HTTPS.
- Service RPC (fabric invoke) over QUIC; CAS bulk over a bulk stream.
- Observability and evidence.

**Gate G4:** a 256 KiB arena module and a 1 MiB slot-B image fetch at ≥ 0.4 MB/s, digest
verified; manager loads a dependency graph over QUIC; boot markers unchanged; rollback works.

### 6.6 Phase 5 — optional extensions (6–8 weeks)

- Connection migration 20/20 through the relay (address/port change).
- 0-RTT only if a product need is proven; idempotent reads only; replay test.
- QUIC datagrams for heartbeats/hints.
- 128-node policy model with the CID relay; no full mesh.
- HTTP/3 gateway for external MCP if required.

Each extension has its own ADR and gate; none is required for "primary".

### 6.7 Benchmarks and acceptance metrics

| Metric | Method | Baseline (TCP) | Target (QUIC) | Gate |
|---|---|---|---|---|
| Handshake p50/p99 | 200 connects, two kernels | TCP 1 RTT + join; not separately measured | ≤ 30 / ≤ 100 ms | G2 |
| Request p50/p99 | 200 pings/invokes | 10.0 / 20.0 ms (wire study) | ≤ 5 / ≤ 20 ms | G2 |
| Bulk | 1 MiB / 2 MiB | 0.147 MB/s tick-bound; up to 1.25 MB/s fixed | ≥ 0.4 MB/s | G2/G4 |
| CPU at 0.4 MB/s | `clock_us` + host profile | TX spin dominates | ≤ 70 % | G2 |
| Memory | static + high-water | 429 KiB module footprint | ≤ +180 KiB code, ≤ 384 KiB for 8 conns | G2 |
| Code size | `llvm-size` after link | 429 KiB | ≤ 180 KiB QUIC+TLS | G2 |
| PROM | `stat` | 523,204 B | ≤ 524,288, ≥ 256 B headroom | G1 |
| Packet loss | relay counters, qlog | — | < 1 % at 1,000 pps | G0/G1 |
| Idle CPU | 60 s no traffic | 10 ms tick | ≤ 3 % | G1 |
| NAT keepalive | 5 min idle | heartbeat 5 s | connection alive | G0 |

Latency percentiles are reported, not gated in CI, because the host is noisy. Structural
assertions (zero TCP bytes, markers, digests, CIDs, no tag leaks) are gated.

### 6.8 Host and two-kernel iris tests

| Layer | What it proves |
|---|---|
| Unit | packet codec, varints, header protection, ACK ranges, flow control, CID encode/decode |
| Host model | loss/reorder, PTO, handshake fuzz, stream fuzz, 0-RTT replay, migration |
| Host interop | quinn ↔ in-tree stack; handshake, streams, close, stateless reset |
| Two-kernel iris | UDP forward, QUIC handshake, join, route, migrate, partition, restart |
| Relay | CID routing, no TLS termination, restart re-registration |
| Capture | zero TCP on the mesh; qlog/key log decode |

The rig already supports UDP forwards in iris (`ForwardProto::Udp`, `src/net/nat_udp.rs`,
`src/net/fwd.rs`); `tools/m2-rig` currently writes only `proto = "tcp"` and must add a UDP
forward and a QUIC mode. This is a host-tool change, not production code.

### 6.9 ABI, kernel and ROM changes

| Change | Where | Why |
|---|---|---|
| ABI 13: `udp_*`, `random`, `clock_us`, `next_deadline_ms` | `os-sdk/abi`, `kernel/src/module.rs`, `kernel/src/net.rs` | QUIC I/O, timers, entropy |
| ABI 14: `module_load_image` | `os-sdk/abi`, `kernel/src/module.rs` | manager/QUIC module fetch |
| Seeq TX/RX rewrite, RX ring 32 | `kernel/src/net.rs` | 1,000 pps without spinning |
| 1 ms timer, IRQ path | `kernel/src/idle.rs`, `kernel/src/traps.rs`, `kernel/src/main.rs` | pacing/PTO |
| Module-area repartition | `kernel/src/module.rs`, linker scripts, `docs/spec/modules.md` | fit QUIC |
| NVRAM keys (`mesh.key`, `mesh.pub`, QUIC port/relay) | `kernel/src/nvram.rs`, rig | identity and discovery; fit 298 B |
| Boot config (optional QUIC endpoints) | `boot/config.conf`, release workflow | no secrets; public endpoints only |
| ROM | **none** | no QUIC in PROM; size gate only |

### 6.10 Rollback points

- **After Phase 1:** no native flow changes; the UDP/timer/driver work is independently
  useful. Keep or revert without user impact.
- **After Phase 2:** host-only; no cutover. Revert the branch if G2 fails.
- **Phase 3 cutover:** one PR removes the TCP mesh and MCP HTTP from the Indy. Revert the
  PR and reboot; there are no deployed peers (pre-release policy). No runtime fallback.
- **Phase 4:** keep the kernel HTTPS fetch until QUIC module fetch passes G4; then remove it
  in one PR. The boot fetch is the last TCP flow and stays.
- **Any PROM change:** revert if the size gate fails; never merge over budget.

---

## 7. Risks

| # | Risk | Likelihood | Impact | Mitigation |
|---|---|---|---|---|
| 1 | **No production no_std QUIC stack** | certain | high | In-tree profile; do not port; budget 6–9 months |
| 2 | **Rustls MIPS code size** (482 KiB rlib) | high | high | Repartition; minimal TLS; measure linked size in Phase 0 |
| 3 | **MIPS crypto cost** | high | high | ChaCha-only profile; measure; cap at 0.4–0.6 MB/s; gateway fallback |
| 4 | **Seeq TX spin / RX per-byte copy** | certain | high | Non-blocking TX ring, word copies, RX ring 32; Phase 1 |
| 5 | **PROM headroom 1,084 B** | high | high | No QUIC in PROM; UDP/timer size gate; slim-PROM ADR |
| 6 | **Slot A free 95 KiB** | certain | high | Module-area repartition ADR; fixed pool; trim MCP bss |
| 7 | **No CSPRNG** | certain | high | `random` ABI; seeded ChaCha20 DRBG; lab caveat |
| 8 | **Member key provisioning** | medium | high | NVRAM seed per node; root-signed binding; no secure element |
| 9 | **UDP NAT/firewall** | high | high | LAN direct; CID relay; external gateway; keepalive 15 s |
| 10 | **Amplification/DoS** | medium | high | Retry token, 3× limit, per-source rate limit, half-open cap |
| 11 | **0-RTT replay** | high if enabled | high | Disable in v1; idempotent reads only if later |
| 12 | **Custom TLS/provider correctness** | high | high | Use rustls; RustCrypto constant-time; test vectors; external audit |
| 13 | **Debugging encrypted traffic** | high | medium | qlog-lite, test key log, relay CID trace, host interop |
| 14 | **Performance regression vs TCP** | medium | high | Measure; keep QUIC for control; TCP/gateway for bulk if needed |
| 15 | **External TCP compatibility** | product | high | POSIX sockets stay; external MCP HTTP at gateway; explicit ADR |
| 16 | **Timer/event dependency (#248)** | high | high | Phase 1 first; no QUIC on a 10 ms poll |
| 17 | **ABI churn** | medium | medium | Append-only ABI 13/14; pre-release; rebuild all modules |
| 18 | **Root key custody** | known | high | Lab fixture now; threshold root later; do not claim production security |
| 19 | **128-node scaling** | medium | medium | CID relay/hub; no full mesh; policy model |
| 20 | **Duplicate TLS stacks** | medium | medium | Slim-PROM ADR; one TLS implementation; no nested TLS |

The three risks that should stop the programme are 1–3: if the guest cannot run ChaCha at
the target rate, or the linked code cannot fit an approved slot, native QUIC is not viable.
Everything else is engineering.

---

## 8. Decision request

### 8.1 Recommendation

**GO — conditional.**

- Approve **Phase 0** (two-week spike) and **Phase 1** (kernel prerequisites) now.
- Commit to **Phase 2** only if G0 and G1 pass.
- **NO-GO** for porting quinn/s2n-quic/quiche/neqo, for QUIC in the PROM, for removing
  POSIX TCP, and for a runtime TCP/QUIC dual stack.
- If G0 fails, adopt **Option G** (QUIC at the gateway) and keep the Indy's native transport
  lean; do not ship a half-native QUIC.

### 8.2 The smallest spike that settles it

**Ten working days, four measurements, five gates.**

1. **Guest crypto** (3 days): ChaCha20-Poly1305, AES-128-GCM, SHA-256, X25519, Ed25519 at
   1,200 B; report MB/s and cycles/byte. *Gate: ChaCha ≥ 2.0 MB/s; handshake ≤ 50 ms.*
2. **Guest UDP/timer/driver** (4 days): UDP echo over two iris kernels, ring 8 vs 32,
   1 ms vs 10 ms, non-blocking TX prototype. *Gate: p50 ≤ 1 ms, p99 ≤ 5 ms, 1,000 pps,
   < 1 % loss, < 50 % CPU.*
3. **Code size** (2 days): link rustls no_std + RustCrypto provider + a QUIC frame codec for
   n32; measure `.text`/`.bss`; test the 768 KiB slot layout. *Gate: ≤ 180 KiB text,
   ≤ 96 KiB bss, fits with MCP.*
4. **Host interop** (1 day): quinn ↔ host bridge ↔ guest echo; capture handshake and stream
   bytes. *Gate: 100 % handshake/stream success; no unexpected packet sizes.*

If all five pass, proceed. If any fails, the failure mode names the fallback: crypto →
cap/gateway; UDP/timer → fix the kernel or stop; code size → minimal TLS or gateway; interop
→ protocol bug, not a hardware limit.

### 8.3 Decisions requested from the maintainers

1. **Native vs gateway.** Is the Indy required to terminate QUIC itself, or may a LAN
   gateway terminate it (Option G) with the Indy speaking a lean local protocol? The answer
   decides whether Phase 2 is worth starting.
2. **Module-area repartition.** Approve an ADR for A = 768 KiB / B = 1280 KiB (four Wasm
   arenas) if the spike shows a native stack needs it. Without this, native QUIC cannot
   coexist with MCP.
3. **POSIX/IRIX TCP.** Confirm that `socket(AF_INET, SOCK_STREAM)` stays TCP and that
   "primary" applies to native flows only. If the product intent is to remove TCP entirely,
   say so now: it breaks IRIX application compatibility and needs a separate product ADR.
4. **External MCP.** Are external MCP clients served by a gateway (Streamable HTTP or
   HTTP/3), or must the Indy itself speak HTTP/3? The latter adds QPACK and a much larger
   code budget.
5. **0-RTT.** Confirm disabled in v1. If a product need exists, define the idempotent
   operations and the replay window before any code.
6. **Boot chain.** Keep HTTPS/TCP for the first module (recommended), or fund the slim-PROM
   ADR (signed plaintext bootstrap) that removes the kernel TLS stack and frees PROM.

### 8.4 What would change this verdict to a plain GO

- A measured ChaCha throughput ≥ 3 MB/s on the guest.
- A linked QUIC+TLS stack ≤ 150 KiB with a 768 KiB slot.
- A non-blocking Seeq driver sustaining 1,000 pps at < 40 % CPU.
- A working two-kernel UDP/QUIC handshake through iris.
- Maintainer approval of the module-area repartition.

### 8.5 What would make it a NO-GO

- ChaCha < 1.2 MB/s on the guest (the link is faster than the cipher).
- Linked QUIC+TLS > 300 KiB and no approval to repartition.
- PROM cannot fit the UDP/timer prerequisites even after reclamation.
- UDP forwarding/NAT cannot be made reliable in the only reproducible rig.
- No member-key provisioning story (then mutual QUIC auth is impossible).

---

## Sources

All URLs accessed 2026-10-27 unless stated. Primary specifications first; crate versions
and dates from crates.io/GitHub on the same date.

### Specifications

1. RFC 9000, *QUIC: A UDP-Based Multiplexed and Secure Transport*, May 2021. §2.1 stream
   IDs, §5.1 connection IDs, §7 handshake, §8.1 address validation (3× anti-amplification),
   §9 migration, §10.1 idle timeout, §14 datagram size (1,200 B minimum).
   <https://www.rfc-editor.org/rfc/rfc9000.html>
2. RFC 9001, *Using TLS to Secure QUIC*, May 2021. §4.6 0-RTT, §5.3 AEAD, §5.4 header
   protection, §9.2 replay. <https://www.rfc-editor.org/rfc/rfc9001.html>
3. RFC 9002, *QUIC Loss Detection and Congestion Control*, May 2021. §6 loss detection,
   §7 congestion control, §7.2 initial window, §7.7 pacing.
   <https://www.rfc-editor.org/rfc/rfc9002.html>
4. RFC 9221, *An Unreliable Datagram Extension to QUIC*, March 2022.
   <https://www.rfc-editor.org/rfc/rfc9221.html>
5. RFC 9308, *Applicability of the QUIC Transport Protocol*, September 2022.
   <https://www.rfc-editor.org/rfc/rfc9308.html>
6. RFC 9312, *Manageability of the QUIC Transport Protocol*, September 2022.
   <https://www.rfc-editor.org/rfc/rfc9312.html>
7. RFC 9369, *QUIC Version 2*, May 2023. <https://www.rfc-editor.org/rfc/rfc9369.html>
8. RFC 9438, *CUBIC for Fast and Long-Distance Networks*, August 2023.
   <https://www.rfc-editor.org/rfc/rfc9438.html>
9. RFC 9114, *HTTP/3*, June 2022; RFC 9204, *QPACK*, June 2022.
   <https://www.rfc-editor.org/rfc/rfc9114.html>
10. RFC 9298, *Proxying UDP in HTTP*, August 2022; RFC 9484, *Proxying IP in HTTP*,
    October 2023. <https://www.rfc-editor.org/rfc/rfc9298.html>
11. RFC 7250, *Using Raw Public Keys in TLS and DTLS*, April 2014; RFC 8410, *Algorithm
    Identifiers for X25519, X448, Ed25519 and Ed448*, August 2018.
    <https://www.rfc-editor.org/rfc/rfc7250.html>
12. RFC 8446, *The Transport Layer Security (TLS) Protocol Version 1.3*, August 2018.
    <https://www.rfc-editor.org/rfc/rfc8446.html>
13. RFC 4787, *Network Address Translation (NAT) Behavioral Requirements for Unicast UDP*,
    January 2007, REQ-5 (mapping timer ≥ 2 minutes).
    <https://www.rfc-editor.org/rfc/rfc4787.html>
14. RFC 5382, *NAT Behavioral Requirements for TCP*, October 2008.
    <https://www.rfc-editor.org/rfc/rfc5382.html>

### IETF drafts (not standards; do not depend on the wire format)

15. `draft-ietf-quic-load-balancers-22`, *QUIC-LB: Generating Routable QUIC Connection
    IDs*, 2026-10-23. <https://datatracker.ietf.org/doc/draft-ietf-quic-load-balancers/>
16. `draft-ietf-quic-multipath-21`, *Managing multiple paths for a QUIC connection*,
    2026-03-17. <https://datatracker.ietf.org/doc/draft-ietf-quic-multipath/>
17. `draft-ietf-quic-ack-frequency-14`, *QUIC Acknowledgment Frequency*, 2026-08-09.
    <https://datatracker.ietf.org/doc/draft-ietf-quic-ack-frequency/>
18. `draft-ietf-quic-qlog-main-schema-14`, *qlog: Structured Logging for Network
    Protocols*, 2026-07-06. <https://datatracker.ietf.org/doc/draft-ietf-quic-qlog-main-schema/>
19. `draft-ietf-quic-reliable-stream-reset-11`, *QUIC Stream Resets with Partial
    Delivery*, 2026-10-12. <https://datatracker.ietf.org/doc/draft-ietf-quic-reliable-stream-reset/>
20. `draft-ietf-quic-address-discovery-01`, *QUIC Address Discovery*, 2026-08-15.
    <https://datatracker.ietf.org/doc/draft-ietf-quic-address-discovery/>
21. `draft-ietf-quic-extended-key-update-03`, *Extended Key Update for QUIC*, 2026-07-06.
    <https://datatracker.ietf.org/doc/draft-ietf-quic-extended-key-update/>

### Project sources

22. `docs/spec/pre-release.md`, `docs/spec/system.md`, `docs/spec/modules.md`,
    `docs/spec/mcp.md`, `docs/spec/native-abi.md`, `docs/spec/boot-protocol.md` at
    `41f3f24`.
23. `docs/research/wire-efficiency.md` (the wire study), `docs/adr/m2-0002-transport.md`,
    `m2-0003-authority.md`, `m2-0008-failure-fence.md`, `m2-0009-actor-migration.md`,
    `m3-0001-shard-slice.md`, `m3-0002-multi-node-hub.md`.
24. `kernel/src/net.rs`, `kernel/src/module.rs`, `kernel/src/idle.rs`, `kernel/src/main.rs`,
    `kernel/src/tls.rs`, `kernel/src/http.rs`, `kernel/src/autoload.rs`,
    `kernel/src/heap.rs`.
25. `modules/mcp/src/mesh.rs`, `modules/mcp/src/main.rs`, `crates/mesh-transport`,
    `crates/sys-primitives`, `crates/mesh-join`, `crates/mesh-route`,
    `crates/mesh-migrate`.
26. `tools/m2-rig`, `tools/protocol-bench`.
27. `/mnt/europa/iris` (the emulator): `src/config.rs` (`ForwardProto::Tcp|Udp`),
    `src/net/nat_udp.rs`, `src/net/fwd.rs`. Not part of this repository.

### Implementations and measurements

28. quinn 0.11.12 / quinn-proto 0.11.19, crates.io, 2026-09-14 / 2026-09-30; no_std issue
    #579 open since 2019-12-31. <https://github.com/quinn-rs/quinn/issues/579>
29. s2n-quic 1.91.0, 2026-10-14; `s2n-quic-core` 0.91.0 (no_std+alloc), transport/platform
    std. <https://github.com/aws/s2n-quic>
30. quiche 0.30.0, 2026-09-17; BoringSSL C dependency.
    <https://github.com/cloudflare/quiche>
31. neqo v0.34.0, 2026-10-22; NSS backend, server experimental.
    <https://github.com/mozilla/neqo>
32. quic-go, <https://github.com/quic-go/quic-go> (design/interop reference; Go, not
    embeddable).
33. rustls 0.23.46, 2026-10-14; `#![no_std]` + `alloc`, custom crypto provider, QUIC API
    (`pub mod quic`); webpki mandatory in 0.23.46.
    <https://github.com/rustls/rustls>
34. ring 0.17.14 (2025-03-11); aws-lc-rs 1.18.2 (2026-10-12) platform tables (no MIPS).
    <https://github.com/briansmith/ring>, <https://github.com/aws/aws-lc>
35. graviola 0.4.2 / rustls-graviola 0.4.0 (2026-10); aarch64/x86_64 only.
    <https://github.com/ctz/graviola>
36. RustCrypto: `chacha20poly1305` 0.11.0 (2026-08-05), `aes-gcm` 0.11.1 (2026-08-21),
    `sha2` 0.11.1 (2026-10-12), `x25519-dalek` 3.0.0 (2026-07-06), `ed25519-dalek` 3.0.0
    (2026-07-06). <https://github.com/RustCrypto>
37. embedded-tls 0.19.0 (2026-06-01); client-only, no QUIC handshake API.
    <https://github.com/drogue-iot/embedded-tls>
38. wolfSSL benchmark tables (PIC32MZ @200 MHz, Cortex-M4 @120 MHz, Cortex-M7 @216 MHz),
    accessed 2026-10-27. <https://www.wolfssl.com/docs/benchmarks/>
39. BearSSL speed page (AES-CTR/GHASH, Cortex-M0+), accessed 2026-10-27.
    <https://www.bearssl.org/speed.html>
40. Eggert, L., *Towards Securing the Internet of Things with QUIC*, 2020.
    <https://www.semanticscholar.org/paper/Towards-Securing-the-Internet-of-Things-with-QUIC-Eggert/e173512867c573262eefe16050beb41b38457488>
41. Saif, D., Lung, C.-H., Matrawy, A., *A Pure HTTP/3 Alternative to MQTT-over-QUIC in
    Resource-Constrained IoT*, 2021. <https://arxiv.org/abs/2106.12684>
42. Model Context Protocol, *Transports*, specification 2026-07-28 (custom transports are
    permitted; Streamable HTTP is the standard binding).
    <https://modelcontextprotocol.io/specification/2026-07-28/basic/transports>
43. Jennings, C., et al., *Model Context Protocol and Agent Skills over Media over QUIC
    Transport*, `draft-jennings-ai-mcp-over-moq-00`, 2026-03-02.
    <https://datatracker.ietf.org/doc/draft-jennings-ai-mcp-over-moq/>
44. `milli-http`, `minip2p`, `purecrypto` (no_std QUIC/TLS prototypes, October 2026;
    single-digit stars, no production use). <https://github.com/computer-whisperer/milli-http>,
    <https://github.com/deepso7/minip2p>, <https://github.com/KarpelesLab/purecrypto>

---

## Appendix A — measurement method and host

- Host: Intel Core i5-8500T @ 2.10 GHz, 6 vCPU, Linux 6.18.55, NixOS, x86_64.
- Toolchain: `nightly-2026-10-03`, `rustc 1.99.0 (b940084d7 2026-09-28)`,
  `cargo 1.99.0 (5f94df478 2026-08-27)`.
- PROM: `scripts/release/build.sh` recipe with `CARGO_PROFILE_RELEASE_OPT_LEVEL=z`,
  `-Z build-std=core,alloc`, target `targets/mips3-sgi-irix7-n32.json`; `rom/tools/make_prom.py`.
  Result: **523,204 B**.
- MCP: same target, `modules/mcp/linker.ld`, `opt-level=z`, `llvm-objcopy -O binary`.
  Result: **359,528 B**; `llvm-size -A`: text 337,712, rodata 16,356, data 5,448, bss 69,449.
- Rustls MIPS: `rustls 0.23.46`, `default-features = false`,
  `features = ["custom-provider"]`, `opt-level=z`, `-Z build-std=core,alloc`.
  `size -t` on the rlib: **482,113 B** text. This is an rlib, before link-time dead-code
  elimination; the final linked subset will be smaller, but the order of magnitude is the
  point.
- quinn-proto: `quinn-proto 0.11.19` default features (rustls-ring), host release; rlib
  `.text` **678,549 B**. Full host binary with rustls: **2,136,938 B** text vs a 267,053 B
  baseline.
- No guest measurement is claimed in this slice. The Guest numbers in §3.4 are published
  vendor benchmarks on comparable 32-bit cores, not this Indy.

## Appendix B — byte and slot budgets

```text
Slot A (512 KiB = 524,288 B), current MCP:
  .text      337,712
  .rodata     16,356
  .data        5,448
  .bss        69,449
  ------------------
  total      428,965
  free        95,323   (18.2 %)

Repartition (recommended first cut):
  A  768 KiB = 786,432 B   MCP 428,965 + QUIC 250,000 + spare 107,467
  B 1280 KiB = 1,310,720 B manager 262,144 + 4 × 262,144 arenas = 1,310,720

PROM:
  image      523,204 / 524,288
  headroom     1,084   (0.21 %)
```

## Appendix C — QUIC profile checklist

- [ ] QUIC v1, 1,200 B minimum, DF, no IP fragmentation.
- [ ] TLS 1.3, ChaCha20-Poly1305-SHA256 mandatory; AES-128-GCM optional.
- [ ] X25519; Ed25519; raw public keys; no X.509/RSA in the native profile.
- [ ] 8-byte routable CIDs; epoch-scoped; HMAC; rotation on epoch/migration.
- [ ] One UDP socket; 8–16 connections per node in v1; fixed pool.
- [ ] Control stream; RPC streams; migration streams; bulk streams; datagrams optional.
- [ ] NewReno, pacing 1 ms, initial cwnd 4–10 packets, BDP cap, no ECN.
- [ ] ACK every 2 packets or 5 ms; idle 30 s; heartbeat 5 s; PTO max 3.
- [ ] 0-RTT disabled; path migration enabled after handshake.
- [ ] `CONNECTION_CLOSE` maps to terminal `Broken` for every pending tag.
- [ ] Epoch fencing in every stream preface; `RESET_STREAM` on stale epochs.
- [ ] Capability handles per connection; inline token on first use; re-install on reconnect.
- [ ] qlog-lite, counters, test key log, relay CID trace.
- [ ] No TLS inside QUIC streams; no QUIC-in-QUIC; one TLS implementation.
- [ ] PROM unchanged; boot fetch remains HTTPS/TCP; module digest verified.
