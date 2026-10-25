# IRIX 7 — Loadable Kernel Modules

**Status:** draft
**Version:** 1.0
**Owner:** kernel
**Related:** [`boot-protocol.md`](boot-protocol.md), [`../plan/milestones.md`](../plan/milestones.md) M1.

## 1. Scope

The compressed PROM image is a fixed bootstrap. Anything that is not needed to boot is a
**module**: a flat image fetched into RAM, verified, and called in place. Modules run in
kernel mode; they are kernel extensions, not user processes (user objects and the
capability ABI are separate, later work).

TLS stays in the core. It is the transport that makes module downloads trustworthy, so the
module command uses HTTPS by default.

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
| Slot A | 512 KiB at `0x8840_0000`; default control/probe slot |
| Slot B | 1.5 MiB at `0x8848_0000`; Wasm/large module slot |
| Total area | 2 MiB (`0x0020_0000`) |
| Placement | inside the reserved low window, above the kernel stack top (`0x8830_0000`), below the frame allocator's first frame (`0x08B0_0000`) |
| Image format | flat binary, entry at offset 0 |
| Cache | kseg0 is cacheable; the loader must write back the data cache and invalidate the instruction cache before the call |

A slot-A module is linked at `0x8840_0000`; a slot-B module at `0x8848_0000` by its own linker script. There is no general relocation;
relocation at load time. The Seeq device's uncached DMA structures start at physical
`0x0860_0000`, immediately after the module area, so a full 2 MiB module is the limit.

## 4. ABI

The entry point is a normal C call:

```c
const struct ModuleVtable *_start(const struct Services *services);
```

`Services` is `#[repr(C)]` and versioned. Version 11:

| Offset | Field | Meaning |
|---|---|---|
| 0x00 | `version` | 11 |
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

The struct lives in the SDK's `abi/` crate (crate `module-abi`), pinned at tag `v0.6.0`.  The kernel and every module
link that one crate, and compile-time assertions pin these offsets for the n32 target;
the table is 0x54 bytes.

New services are appended and `version` is bumped; existing offsets never move. A module
that needs a newer table can reject an older core by checking `version`.  ABI v11 appends
`block_write`; the optional cache probe uses it.  A bounded download queue and a module
multiplexer are still future work.

### TCP slots

There are two slots (0 and 1), each with a 2 KiB receive and transmit buffer in the core.
`tcp_listen` closes and replaces any socket already in the slot. `tcp_recv` returns 0 when
the connection is open but no data is queued, and -1 when the slot has no socket. `tcp_send`
may accept fewer bytes than requested; loop until the whole response is sent. `tcp_close`
sends a FIN; once the socket reaches the closed state the module can call `tcp_listen`
again.

A server can arm the second slot while the first connection is closing. smoltcp permits
two listening sockets on the same port, and the mcp module uses that to avoid a re-arm
gap between back-to-back clients.

The entry returns a pointer to a vtable, or null for "initialised, no callback":

| Offset | Field | Meaning |
|---|---|---|
| 0x00 | `status` | `int32_t`; 0 is success, anything else is printed and no callback is registered |
| 0x04 | `tick` | `int32_t (*)(const struct Services *)`, or null |

`tick` is called from the kernel's idle loop at most every 10 ms. Return 0 to keep running;
return non-zero to unregister the callback. The kernel only accepts a vtable that lies
inside the verified module image. The console prints the status as `module: returned N`.

A one-shot module can simply return null (status 0) or a vtable with `tick = null`.

## 5. Lifecycle

- Two fixed slots. Loading into a slot overwrites that slot, clears its `.bss`, and
  drops that slot's tick callback; the other slot keeps running. Slot A holds MCP and
  small probes; slot B holds the Wasm policy module. There is no general relocatable
  multiplexer, unload or destructor.
- The loader is synchronous: the console is blocked for the duration of the fetch and the
  entry call. The tick callback runs on the idle loop, so it must be quick.
- The flat image contains `.text`, `.rodata` and `.data`, not `.bss`. The loader zeroes the
  whole module area before the download, which gives `.bss` a defined initial value.
- Exceptions inside a module use the kernel's trap path. The current trap handler prints
  the cause and halts, which is the expected failure mode for a bad module.

## 6. Developer SDK

The separate [`irix7/os-sdk`](https://github.com/irix7/os-sdk) repository packages the
developer-facing pieces: a `Context` over the service table, `entry!`/`panic_handler!`
macros, the n32 target and flat linker, a copyable template, a hello example and
`tools/irix7-module.py` (build, flatten, hash). The SDK owns the `module-abi` crate in `abi/`; this repository consumes it at the pinned
`v0.6.0` tag, so the contract has one source of truth.

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
- A module manifest with a name and required ABI version.

These are additive to the version 2 table.

## 9. Non-goals

- User-mode code, capabilities or process isolation (see the architecture specification).
- More than two fixed slots or a relocatable multiplexer.
- Concurrent modules beyond slot A (MCP/probes) and slot B (Wasm), or dependency resolution.
- Threads inside a module; a module gets the tick callback and the idle loop's context.
- Arbitrary relocatable ELF objects; modules are linked at their final address.
