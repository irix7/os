# IRIX 7 — Loadable Kernel Modules

**Status:** draft
**Version:** 1.1
**Owner:** kernel
**Related:** [`boot-protocol.md`](boot-protocol.md), [`../plan/milestones.md`](../plan/milestones.md) M1.

## 1. Scope

The compressed PROM image is a fixed bootstrap. Anything that is not needed to boot is a
**module**: a flat image fetched into RAM, verified, and called in place. Modules run in
kernel mode; they are kernel extensions, not user processes (user objects and the
capability ABI are separate, later work).

TLS stays in the core. It is the transport that makes module downloads trustworthy, so the
module command uses HTTPS by default.

### 1.1 PROM freeze

Once M1 is re-closed, the compressed PROM/kernel image is **frozen for new
features**. Only correctness, security and hardware-enablement fixes may change
it. New policy, runtime, transport, server, device and management features ship
as downloadable modules through the package index, using the versioned service
table. The M1 live-policy bridge is a transitional exception; future module
work must not grow the kernel object graph. The stable module ABI and the
package index replace ad-hoc PROM changes.

## 2. Transport and integrity

1. The console command is
   `module [https://]<host>:<port><path> <sha256-hex>`.
   Without a scheme it uses HTTPS through the core TLS client with certificate-chain
   verification. `http://` exists only for the local iris rig.
2. The response body is streamed straight into the selected module slot while being hashed.
3. The entry is called only if the whole body fits and its SHA-256 matches the argument.
   A mismatch, a short capture, a TLS failure or a DNS failure leaves the area untouched
   and executes nothing.

The body is downloaded, not buffered elsewhere, so a module is limited by its slot and
the network.

For an offline or recovery boot, the DS1386 environment may instead carry
`mcp.local=<target>:<lba>:<sectors>:<sha256>`.  The kernel checks it before
`mcp.url`, reads the flat image directly from SCSI targets 0–6 in 255-sector
chunks, verifies the digest over the whole padded image and runs it in place.
A present but invalid `mcp.local` fails closed; it does not fall back to the
network.  The record is a local hardware override, not a release channel.

## 2.1 Boot autoload

A machine that reboots must bring the control plane back without a human at the console.
The primary configuration is `boot/config.conf` in the repository, fetched from:

```text
https://raw.githubusercontent.com/irix7/os/main/boot/config.conf
```

The release workflow proposes an update with immutable raw module URLs (a pull
request, because `main` is protected). The file supplies:

| Name | Value |
|---|---|
| `mcp.url` | `https://host[:port]/path` |
| `mcp.sha256` | 64 hex digits, the digest of the module image |
| `wasm.url` | optional NVRAM-only slot-B probe URL |
| `wasm.sha256` | optional slot-B probe digest |

The DS1386 environment may override the same two names for a local test rig; it is a
hardware chip, not a release distribution channel. After DHCP and DNS are up, and before
the boot handoff, the kernel reads the configuration, zeroes module slot A,
fetches the image, verifies the digest and calls the entry. A failure prints
`autoload: ...` and the boot continues with a `BOOT-NOT-READY` marker; it never runs
an unverified image. An optional NVRAM `wasm.url`/`wasm.sha256` pair loads the
slot-B Wasm probe after MCP; it does not affect the MCP readiness marker.

At startup the kernel also validates the DS1386 environment and MAC. A blank or corrupt
environment is repaired with a minimal default and a locally administered fallback MAC,
so a dead battery does not leave the machine with an unusable network identity. The
environment lives at offset `0x10` (the RTC registers occupy `0x00`–`0x0F`), one byte per
32-bit word in the low lane.

## 3. Address space

| Property | Value |
|---|---|
| Load virtual address (kseg0) | `0x8840_0000` (physical `0x0840_0000`) |
| Slot A | 512 KiB at `0x8840_0000`; MCP/control slot |
| Slot B | 1.5 MiB at `0x8848_0000`; first 256 KiB for the manager |
| Runtime arena | 1.25 MiB at `0x884c_0000`; five 256 KiB regions after the manager |
| Total area | 2 MiB (`0x0020_0000`) |
| Placement | inside the reserved low window, above the kernel stack top (`0x8830_0000`), below the frame allocator's first frame (`0x08B0_0000`) |
| Image format | flat binary, entry at offset 0 |
| Cache | kseg0 is cacheable; the loader must write back the data cache and invalidate the instruction cache before the call |

A slot-A module is linked at `0x8840_0000`; a slot-B module at `0x8848_0000` by its own linker script. The manager is limited to the first 256 KiB of slot B; the five runtime regions follow it. There is no load-time relocation: every image is linked for its final region. The Seeq device's uncached DMA structures start at physical `0x0860_0000`, immediately after the module area, so a full 2 MiB module is the limit.

## 4. ABI

The entry point is a normal C call:

```c
const struct ModuleVtable *_start(const struct Services *services);
```

`Services` is `#[repr(C)]` and versioned. The version 11 rows are unchanged in version 12; §8.5 lists the appended loader fields:

| Offset | Field | Meaning |
|---|---|---|
| 0x00 | `version` | 16 |
| 0x04 | `write` | `void (*)(const uint8_t *ptr, size_t len)` — serial and telnet |
| 0x08 | `clock_ms` | `uint64_t (*)(void)` — monotonic milliseconds since boot |
| 0x0C | `alloc` | `void *(*)(size_t size, size_t align)` — kernel heap, never freed |
| 0x10 | `poll` | `void (*)(void)` — drive the shared network stack once |
| 0x14 | `tcp_listen` | `int32_t (*)(uint32_t slot, uint16_t port)` — listen; 0 or -1 |
| 0x18 | `tcp_state` | `uint32_t (*)(uint32_t slot)` — 0 closed, 1 listening, 2 established, 3 closing |
| 0x1C | `tcp_recv` | `intptr_t (*)(uint32_t slot, uint8_t *ptr, size_t len)` — bytes, 0 for none, -1 for no socket |
| 0x20 | `tcp_send` | `intptr_t (*)(uint32_t slot, const uint8_t *ptr, size_t len)` — bytes accepted, -1 for no socket |
| 0x24 | `tcp_close` | `void (*)(uint32_t slot)` — send FIN |
| 0x28 | `tcp_abort` | `void (*)(uint32_t slot)` — send RST and release the slot |
| 0x2C | `console_read` | `intptr_t (*)(uint64_t offset, uint8_t *ptr, size_t len)` — serial ring, oldest byte clamped |
| 0x30 | `console_total` | `uint64_t (*)(void)` — bytes ever written to the serial console |
| 0x34 | `reset` | `void (*)(void)` — restart from the PROM entry; does not return |
| 0x38 | `poweroff` | `void (*)(void)` — halt the machine; does not return |
| 0x3C | `status` | `intptr_t (*)(uint8_t *ptr, size_t len)` — the shared machine status block |
| 0x40 | `config_get` | `intptr_t (*)(const uint8_t *name, size_t name_len, uint8_t *out, size_t out_len)` — PROM environment value, or -1 |
| 0x44 | `info` | `intptr_t (*)(uint32_t kind, uint8_t *ptr, size_t len)` — 0 status, 1 net, 2 mem, 3 time |
| 0x48 | `screen_read` | `intptr_t (*)(uint32_t offset, uint8_t *ptr, size_t len)` — BGR shadow bytes |
| 0x4C | `block_read` | `int32_t (*)(uint8_t target, uint32_t lba, uint8_t *ptr)` — one 512-byte SCSI sector; 0 or -1 |
| 0x50 | `block_write` | `int32_t (*)(uint8_t target, uint32_t lba, const uint8_t *ptr)` — one 512-byte SCSI sector; 0 or -1 |
| 0x54..0x6C | loader/arena | ABI v12 services, see §8.5 |
| 0x70 | `tcp_connect` | `int32_t (*)(uint32_t slot, uint32_t address, uint16_t port)` — IPv4 in network byte order; 0 or -1 |
| 0x74 | `tcp_events` | `uint32_t (*)(uint32_t slot)` — level-triggered RX/TX/CLOSED/LISTEN/ESTAB bits; bit 31 = no socket |
| 0x78 | `tcp_slots` | `uint32_t (*)(void)` — number of module TCP slots the kernel provides |
| 0x7C | `udp_open` | `int32_t (*)(uint32_t slot, uint16_t port)` — bind one socket to a nonzero local port; 0 or -1 |
| 0x80 | `udp_send` | `intptr_t (*)(uint32_t slot, uint32_t address, uint16_t port, const uint8_t *ptr, size_t len)` — one datagram; bytes queued, 0 when the ring is full, -1 for no socket |
| 0x84 | `udp_recv` | `intptr_t (*)(uint32_t slot, uint8_t *ptr, size_t len, uint32_t *src_address, uint16_t *src_port)` — one datagram; bytes, 0 for none, -1 for no socket, -2 when too large |
| 0x88 | `udp_close` | `void (*)(uint32_t slot)` — close and release the slot |
| 0x8C | `udp_slots` | `uint32_t (*)(void)` — number of module UDP slots the kernel provides |
| 0x90 | `udp_state` | `int32_t (*)(uint32_t slot, uint32_t *address, uint16_t *port)` — bound local endpoint; 0 or -1 |

The struct lives in the SDK's `abi/` crate (crate `module-abi`), pinned at tag `v0.11.0`.  The kernel and every module
link that one crate, and compile-time assertions pin these offsets for the n32 target;
the v11 prefix is 0x54 bytes, the ABI v12 table was 0x70 bytes, ABI v13 was 0x74 bytes, ABI v14 was 0x78 bytes, the ABI v15 table was 0x7c bytes and the ABI v16 table is 0x94 bytes.

New services are appended and `version` is bumped; existing offsets never move. A module
that needs a newer table can reject an older core by checking `version`. ABI v12 appends
the loader/arena services described in §8.5. The kernel multiplexes the two boot slots and
the five arena regions; a bounded download queue remains future work.

### TCP slots

`tcp_slots()` returns eight. Slots are `0..8`; each owns one socket, a 16 KiB receive
ring and a 16 KiB transmit ring in the kernel. `tcp_listen` and `tcp_connect` close and
replace any socket already in the slot. `tcp_recv` returns 0 when the connection is open
but no data is queued, and -1 when the slot has no socket. `tcp_send` may accept fewer
bytes than requested; loop until the whole response is sent. `tcp_close` sends a FIN; once
the socket reaches the closed state the module can call `tcp_listen` again.

Every `tcp_connect` slot gets its own ephemeral local port (`49152 + slot`), so a module
can hold several peer connections at once; a fixed local port would collide on the second
connect. Listening slots can use different ports; smoltcp permits two listening sockets on
the same port, but the kernel does not require that trick.

A server can arm one slot while another connection is closing. MCP still uses slot 0 for
HTTP and slot 1 for the mesh listener. The remaining slots are free for a multi-peer
transport: eight slots are enough for a star of seven peers plus the control plane, which
is the Version 1 transport prerequisite for the four-to-eight-node membership work tracked
in issue #249. This is a fixed slot pool, not a module slot manager; general runtime module
dependencies remain issue #217. The kernel's socket set has 28 entries, so eight module
TCP sockets and eight module UDP sockets coexist with DHCP, telnet, the MCP listener and
the userland client.

### UDP slots

`udp_slots()` returns eight. Slots are `0..8`; each owns one socket, a 2 KiB receive ring,
a 2 KiB transmit ring and eight packet-metadata entries each way. `udp_open` binds a
nonzero local port and replaces any socket already in the slot; there is no ephemeral bind
because the in-tree stack has none, so a module chooses its ports.

`udp_send` queues exactly one datagram and never splits it. It returns the number of bytes
queued, `0` when the socket ring is full (retry after the next `poll`) and `-1` for no
socket or an invalid argument. `udp_recv` dequeues one datagram and returns its length, `0`
when nothing is queued, `-1` for no socket and `-2` when the datagram did not fit the
caller's buffer; that datagram is dropped, not truncated. When the out-parameters are
non-null they receive the sender's IPv4 address (network byte order) and port, so an echo
or reply path needs no second lookup. `udp_state` reports the bound local endpoint.

The kernel owns the socket and the interface queue. Framing, retransmission, congestion
control, connection identity and demultiplexing are the module's responsibility, which is
what a datagram transport such as QUIC would replace. The ABI is not a QUIC stack: the
[`M3-ADR-0006`](../adr/m3-0006-udp-transport.md) slice only makes datagrams possible
between two real kernels.

The entry returns a pointer to a vtable, or null for "initialised, no callback":

| Offset | Field | Meaning |
|---|---|---|
| 0x00 | `status` | `int32_t`; 0 is success, anything else is printed and no callback is registered |
| 0x04 | `tick` | `int32_t (*)(const struct Services *)`, or null |

`tick` is called from the kernel's idle loop at most every 10 ms. Return 0 to keep running;
return non-zero to unregister the callback. The kernel only accepts a vtable that lies
inside the verified module image. The console prints the status as `module: returned N`.

A one-shot module can simply return null (status 0) or a vtable with `tick = null`.

## 5. Lifecycle and multiplexer

- The kernel multiplexes seven executable regions: slot A (512 KiB), slot B (1.5 MiB,
  with the manager confined to its first 256 KiB) and five 256 KiB arena regions. Each
  region has at most one module, one verified entry and one tick callback.
- A boot-slot load (`module`) and an arena load (`module_load`) clear that region's old
  callback before writing. `module_execute` validates the returned vtable against the
  verified image and installs its tick; `module_unload` clears the callback and the
  region. A failed fetch or a non-zero entry leaves no callback behind.
- `module::poll` runs every registered callback at most once per 10 ms, in region order.
  A callback returning non-zero unregisters itself. MCP in A, the manager in B and an
  independent arena tick can therefore run concurrently without overwriting each other.
- Fixed regions are not relocation: the manager chooses a region and each image is linked
  for its final base. There is no dynamic region allocator, destructor or shared-library
  model; the five-region arena is the current scaling limit.
- The loader is synchronous: the console is blocked for the duration of the fetch and the
  entry call. The tick callback runs on the idle loop, so it must be quick.
- The flat image contains `.text`, `.rodata` and `.data`, not `.bss`. The loader zeroes the
  whole region before the download, which gives `.bss` a defined initial value.
- Exceptions inside a module use the kernel's trap path. The current trap handler prints
  the cause and halts, which is the expected failure mode for a bad module.

## 6. Developer SDK

The separate [`irix7/os-sdk`](https://github.com/irix7/os-sdk) repository packages the
developer-facing pieces: a `Context` over the service table, `entry!`/`panic_handler!`
macros, the n32 target and flat linker, a copyable template, a hello example and
`tools/irix7-module.py` (build, flatten, hash). The SDK owns the `module-abi` crate in `abi/`; this repository consumes it at the pinned
`v0.7.0` tag, so the contract has one source of truth.

A module built with the SDK is still a flat image linked at its slot base (`0x88400000`
for A, `0x88480000` for B), fetched over
HTTPS, digest-checked and called through this table. The SDK changes none of the contract
below.

## 7. Reference modules

`modules/hello` is the one-shot reference: a small flat image that prints one line through
the service table and returns a vtable with status 0. `modules/block` reads block 0 of SCSI
target 1 through `block_read` and prints the volume-header magic. Build them with their own linker script
and flatten the ELF:

```bash
export CARGO_TARGET_MIPS3_SGI_IRIX7_N32_RUSTFLAGS="-Clink-arg=-Tmodules/hello/linker.ld"
cargo build --release -Z build-std=core -Zjson-target-spec \
  --target targets/mips3-sgi-irix7-n32.json -p hello-module

$(rustc --print sysroot)/lib/rustlib/$(rustc --print host)/bin/llvm-objcopy \
  -O binary target/mips3-sgi-irix7-n32/release/hello-module hello.module
```

`modules/ticker` is the long-running reference: it allocates 32 bytes through the service
table, prints `ticker: alloc ok`, then prints `ticker: tick` once a second for five seconds
and returns non-zero to unregister. Swap `hello` for `ticker` in the two commands above.

`modules/cache` proves the ABI 11 write path: it reads SCSI target 1 LBA 0 into RAM,
hashes it with BLAKE3, and during idle ticks writes an `I7CACHE1` header and the sector
to target 3, then verifies the readback. It is a probe, not the final package cache.

`modules/wasm` is the slot-B Wasm core for M1. It is linked for slot B, embeds a
small Rust guest, instantiates it with the no_std interpreter and runs its `run`
export. The guest drives the local policy: it chooses the `/etc` namespace
layout, stores `/etc/motd`, resolves the root home, derives the local actor,
chooses the capability rights and validates the allow and deny cases. It prints
`wasm: host=1`, reproducible `m1-wasm: root=... actor=... cap=ok rights=35 home=...`
evidence matching the native `m1:` root, actor, rights and home, and `wasm: run=42`.
The guest imports only the `i7host` surface through its own `extern "C"` block;
the host side uses the no_std `wasm-host` crate and supplies the staged
CAS/namespace, actor table, capability table, clock and console through
`HostHooks::policy`. `commit` publishes the stage as one value
and the module aborts on a guest trap or a non-42 result, so a failed policy
leaves no partial object, actor or capability. Load it with `module b ...`;
slot A (MCP) remains live. The kernel's native `m1:` policy remains the
fallback. The image must stay within the 1,572,864-byte slot-B limit.

`modules/logical` is the M1 single-node policy probe. It links the architecture
crates into a module (so the compressed PROM does not grow), builds a local
membership view from the `info` MAC, stores an `/etc/motd` object in the CAS
namespace, spawns and queues an actor, and signs and verifies a 1-of-1
capability over the root directory. It prints `logical: root=... actor=...
cap=ok` and ticks three times. Build it with `-Z
build-std-features=compiler-builtins-mem`; the B-tree and compare operations
need the compiler-builtins memory routines on MIPS.

`modules/mcp` is the TCP reference. It listens on guest port 80, answers one request per
connection, and alternates its two TCP slots so a back-to-back client does not hit the
close handshake. `GET /` returns a status page; `POST /mcp` speaks the prototype MCP
JSON-RPC methods (see [`mcp.md`](mcp.md)), including resources for the serial ring, status,
network and memory. Its tools include `serial.read` (console ring
services), `serial.write`, `system.reset`, `system.poweroff`, `status` (the same
IP/MAC/RAM/uptime/date text as the telnet console), `shell.exec` with `net`, `mem` and
`time`, and `screen.capture` (a 320×200 1-bit PNG thumbnail and the full BMP endpoint).  The system tools defer
the action by five idle ticks so the JSON-RPC response is queued first. The iris rig forwards host `8088` to guest `80`; after
loading the module:

```bash
curl -s http://127.0.0.1:8088/
```

prints the module's uptime. Build it by swapping `hello` for `mcp` in the commands above;
it links the same flat image.

The CI `modules` job builds every reference module (`hello`, `ticker`, `block`, `mcp`,
`erofs`) in release with the pinned nightly, each with its own linker script, flattens the
ELF with `llvm-objcopy -O binary` and fails if any image exceeds the 2 MiB module area. It
uses `build-std=core` for `hello`, `ticker` and `block`, and `build-std=core,alloc` for
`mcp` and `erofs`; that job is the authoritative list and prints the size of each image.

Serve a module and load it:

```text
module http://10.1.1.3:18080/ticker.module <sha256 of ticker.module>
module b http://10.1.1.3:18080/wasm.module <sha256 of wasm.module>
ticker: alloc ok
module: returned 0
ticker: tick
ticker: tick
ticker: tick
ticker: tick
ticker: tick
```

## 8. Planned services

- TCP listen/accept/send/recv/state/close over the shared interface, so a module can serve
  a protocol without linking smoltcp.
- The NTP wall clock (distinct from monotonic `clock_ms`).
- A module manifest with a name, version, required ABI and exports.
- Runtime module dependencies: a module declares exact `name`, `version` and
  SHA-256 records; a downloadable module manager resolves the graph, verifies
  every image, loads dependencies before dependents and publishes versioned C
  export tables. This is required after M1 and is tracked in #217. The first
  host slice, `crates/module-resolver`, implements the manifest records,
  deterministic DAG order and fail-closed cycle/missing/digest/ABI tests; it
  does not load or execute an image. The fixed-slot loader below remains
  standalone-only.

These are additive to the version 2 table.

### 8.1 Manifest record v2

`crates/module-resolver` encodes a manifest record as little-endian bytes:

```text
I7MM | u16 format=2 | u16 abi major | u16 abi minor
     | u32 entry_offset
     | u16 name length | UTF-8 name
     | u16 version length | UTF-8 version
     | u16 dependency count
     | (u16 name length, name, u16 version length, version, 32-byte SHA-256)*
     | u16 export count
     | (u16 interface length, interface, u16 export version, u32 table offset)*
```

`entry_offset` and export table offsets are image-relative and 4-byte aligned;
zero entry means legacy `_start`, zero export offset means a declaration with no
table. The record is limited to 64 KiB. Counts are bounded by remaining bytes
before a vector is reserved. Strings must be UTF-8. There is no CRC in the
record: the enclosing image digest is the integrity boundary. Version 1 records
remain decode-only with zero entry/export offsets; the encoder emits v2.
Dependency versions are exact; ranges and a solver are not part of this format.

### 8.2 Manifest footer v1 (implemented host slice)

A module image may append its manifest record and a 16-byte footer:

```text
image bytes | manifest record | I7MF | u16 version=1 | u16 reserved=0
            | u32 manifest offset | u32 manifest length
```

The manifest range must end exactly at the footer, and the record must decode
as version 1 or version 2. A standalone image has no footer. A present but
malformed footer fails closed; it is not treated as an older standalone image.
The image digest covers the image, manifest and footer; the footer has no
separate checksum.

The host implementation is `module_resolver::footer::{wrap, parse}`.

### 8.3 Host manager model and runtime transaction

`module_resolver::manager::ModuleManager` is the reference-counting host model:
it consumes a `Registry` and an `ImageSource`, resolves the graph, verifies
every SHA-256 and publishes the set atomically. It does not touch the arena.

`module_resolver::runtime` is the host-testable transaction the downloadable
manager runs. It consumes a parsed package index, resolves the exact graph
(dependencies first, root last), verifies every release signature against a
trust policy, loads every image into its declared arena region before entering
any of them, parses each manifest v2 footer, checks the manifest against the
index record, verifies the entry offset and registered export tables, and only
then executes in dependency order. Any failure unloads every image from the
transaction in reverse order. Stale handles, digest mismatches, oversized
images, region exhaustion, ABI/cycle/missing-dependency failures and export
mismatches are host tests in `crates/module-resolver/src/runtime.rs`.

### 8.4 Signatures, trust policy and key epochs

`crates/module-trust` verifies Ed25519 signatures over a domain-separated image
digest. A `SignatureRecord` names the key epoch. A `TrustStore` keeps live and
revoked epochs; unknown, revoked or invalid signatures fail closed. A
`Rotation` is signed by the current epoch key, must advance the epoch, must
install a live key at the new epoch and may revoke the old key. Key rotation
does not rewrite old module signatures; the manager verifies them against the
recorded epoch.

The release trust source is a small line-oriented text policy:

```text
version=1
current-epoch=2
require-signatures=1
key 1 <64 hex Ed25519 public key>
key 2 <64 hex Ed25519 public key>
revoked 1
```

`modules/manager/build.rs` embeds `packages/release-trust.conf` unless
`IRIX7_TRUST_POLICY` names another policy at build time. `manager.trust=<url>,<sha256>`
can load a digest-pinned policy at runtime. Missing signatures are accepted
only when the policy explicitly contains `require-signatures=0`; the
checked-in release policy requires signatures. The signed record text is
`epoch:128-hex-digits`, used for both `manager.index.sig` and index `sig=`
fields.

### 8.5 Loader/arena ABI v12

ABI v12 appends seven module-loader services to `Services` (offsets
`0x54..0x6c`): `module_arena`, `module_load`, `module_execute`,
`module_find`, `module_find_export`, `module_query` and `module_unload`.

The kernel reserves a bounded arena after the runtime-manager image:

- base `0x884c_0000`;
- five regions of 256 KiB (`0x0014_0000` total);
- `module_load` fetches an immutable URL, checks SHA-256, copies atomically
  into the chosen region, and registers the exact identity and export table;
- `module_execute` performs cache maintenance and calls the entry with
  `Services *` and an opaque `ModuleContext *`;
- handles are `generation << 32 | region`; stale handles fail closed;
- `module_find_export` returns the verified table pointer and owner handle;
- `module_unload` clears the region and invalidates the handle.

The kernel only loads/executes verified images. Resolution, manifests,
signatures, package metadata, rollback and unload policy stay in the
downloadable manager. This is the explicit ROM-freeze exception; the M1 image
is 523,816 bytes of 524,288 in the release build.

### 8.6 MCP runtime graph status

The manager writes a `ModuleStatus` record at the fixed `0x883f_e000` status
block. MCP's `GET /modules` reads it and returns the resolved graph/load result;
before a manager has published, it returns `runtime manager: unavailable`. The
text is bounded to 1 KiB. The status block is informational; it is not an
authority path.

### 8.7 Downloadable manager and dependency graph

`modules/manager` is a flat slot-B image that fits within the first 256 KiB of
slot B. It reads:

| Name | Meaning |
|---|---|
| `manager.index` | package-index URL and SHA-256, `url,sha256` |
| `manager.index.sig` | detached index signature, `epoch:128-hex` |
| `manager.root` | exact root identity, `name@version` |
| `manager.trust` | optional trust-policy URL and digest |
| `manager.dep` | legacy single-image fallback, `url,sha256` |

The normal path loads the trust policy and index image into the reserved
metadata region through `module_load`, verifies the index digest and detached
signature, parses the v2 index, resolves the exact graph and verifies every
record signature before a module image is fetched. It then loads each module
into the region named by the index, parses the manifest v2 footer, checks the
manifest identity/ABI/dependencies/exports against the index, verifies the
entry offset and export registration, and executes dependencies before the
root. A failure unloads every image from this transaction and publishes a failed
status; no dependent is entered.

`modules/demo-dep` is linked at arena region 0 (`0x884c_0000`) and exports
`irix7.demo.service@1`. `modules/demo-root` is linked at region 1
(`0x8850_0000`), depends on `demo-dep` by exact digest, resolves the export
through `module_find_export`, checks the owner handle and calls the table. It
prints `demo-root: dep=0x217 owner=0x...`.

The M2 runtime graph is a second pair in the same signed index:

- `modules/m2-mesh` is linked at region 2 (`0x8854_0000`), links
  `mesh-transport` and `sys-primitives`, and exports
  `irix7.mesh.transport@1` at `MESH_API`. Its deterministic probe encodes and
  decodes one fixed M2 Ping envelope and checks the 64 KiB framing bounds.
- `modules/m2-root` is linked at region 3 (`0x8858_0000`), declares the exact
  `m2-mesh@1.0.0` digest, and does not link `mesh-transport`. It resolves the
  export through `module_find_export`, checks the table version and owner
  handle/region, calls the probe, and prints
  `m2-root: dep=m2-mesh@1.0.0 ... owner=0x... result=ok magic=... len=340 max=65536`.

Selecting `m2-root@1.0.0` as `manager.root` proves that the mesh library can be
a real runtime dependency. The current M2-1 MCP mesh listener still links
`mesh-transport` at build time; moving that listener onto this runtime table is
a later refactor, not part of this graph. The two graphs share the signed index
and trust policy but execute separately; a boot selects one `manager.root`.

`tools/module-release` wraps flat images with their manifest v2 footer, resolves
entry/export symbol offsets from the ELF, signs image digests and emits the v2
package index, the detached index signature and the release trust policy.
`packages/runtime-modules.toml` is the exact graph spec; `--root` selects a
different root from the same spec for the M2 self-check.
The manager publishes the resolved graph in the fixed `ModuleStatus` block; MCP
`GET /modules` returns it.

The CI modules job builds and signs the graph with a throwaway key and checks
the manager's 256 KiB limit. The iris boot job builds manager, `demo-dep`,
`demo-root`, `m2-mesh`, `m2-root` and a current MCP, signs them with one
throwaway key, loads each graph over HTTP and asserts the serial order,
`GET /modules` text and the unchanged no-`mesh.peer` M1/M2 markers. The
release-built manager image is about 121 KiB and the clean ROM is 523,816 of
524,288 bytes. The kernel multiplexer costs the final ~0.9 KiB of that budget.

### 8.8 Module identity lookup

`module_find` matches the exact `(name, version)` stored by `module_load`.
`module_find_export` remains the ownership path for interface binding because
an export also proves the owner handle. The general multiplexer adds no second
identity table: `module_query` reports the same region/handle and `module_unload`
invalidates it.

## 9. Non-goals

- User-mode code, capabilities or process isolation (see the architecture specification).
- More than seven fixed regions or load-time relocation. The arena manager
  resolves exact versions only; there is no range solver, relocation or
  shared-library model. Modules are linked for one region, and release metadata
  must carry that region.
- Threads inside a module; a module gets the tick callback and the idle loop's context.
- Arbitrary relocatable ELF objects; modules are linked at their final address.
