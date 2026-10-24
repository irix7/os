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
2. The response body is streamed straight into the module area while being hashed.
3. The entry is called only if the whole body fits and its SHA-256 matches the argument.
   A mismatch, a short capture, a TLS failure or a DNS failure leaves the area untouched
   and executes nothing.

The body is downloaded, not buffered elsewhere, so a module is limited only by the module
area (2 MiB) and the network.

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

The DS1386 environment may override the same two names for a local test rig; it is a
hardware chip, not a release distribution channel. After DHCP and DNS are up, and before
the boot menu is drawn, the kernel reads the configuration, zeroes the module area,
fetches the image, verifies the digest and calls the entry. A failure prints
`autoload: ...` and leaves the machine at the menu; it never runs an unverified image.

At startup the kernel also validates the DS1386 environment and MAC. A blank or corrupt
environment is repaired with a minimal default and a locally administered fallback MAC,
so a dead battery does not leave the machine with an unusable network identity. The
environment lives at offset `0x10` (the RTC registers occupy `0x00`–`0x0F`), one byte per
32-bit word in the low lane.

## 3. Address space

| Property | Value |
|---|---|
| Load virtual address (kseg0) | `0x8840_0000` (physical `0x0840_0000`) |
| Maximum size | 2 MiB (`0x0020_0000`) |
| Placement | inside the reserved low window, above the kernel stack top (`0x8830_0000`), below the frame allocator's first frame (`0x08B0_0000`) |
| Image format | flat binary, entry at offset 0 |
| Cache | kseg0 is cacheable; the loader must write back the data cache and invalidate the instruction cache before the call |

The module is linked for that exact address by its own linker script; there is no
relocation at load time. The Seeq device's uncached DMA structures start at physical
`0x0860_0000`, immediately after the module area, so a full 2 MiB module is the limit.

## 4. ABI

The entry point is a normal C call:

```c
const struct ModuleVtable *_start(const struct Services *services);
```

`Services` is `#[repr(C)]` and versioned. Version 10:

| Offset | Field | Meaning |
|---|---|---|
| 0x00 | `version` | 10 |
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

The struct lives in the SDK's `abi/` crate (crate `module-abi`), pinned at tag `v0.3.0`.  The kernel and every module
link that one crate, and compile-time assertions pin these offsets for the n32 target;
the table is 0x50 bytes.

New services are appended and `version` is bumped; existing offsets never move. A module
that needs a newer table can reject an older core by checking `version`.

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

- One module slot. Loading a second image overwrites the first, clears its `.bss`, and
  drops any previous tick callback; there is no unload and no destructor.
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
`v0.3.0` tag, so the contract has one source of truth.

A module built with the SDK is still a flat image linked at `0x88400000`, fetched over
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

The CI `modules` job builds every reference module (`hello`, `ticker`, `block`, `mcp` and
`erofs`) in release with the pinned nightly, each with its own linker script, flattens the
ELF with `llvm-objcopy -O binary` and fails if any image exceeds the 2 MiB module area. It
uses `build-std=core` for `hello`, `ticker` and `block`, and `build-std=core,alloc` for
`mcp` and `erofs`; that job is the authoritative list and prints the size of each image.

Serve a module and load it:

```text
module http://10.1.1.3:18080/ticker.module <sha256 of ticker.module>
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
- Multiple concurrent modules or dependency resolution.
- Threads inside a module; a module gets the tick callback and the idle loop's context.
- Arbitrary relocatable ELF objects; modules are linked at their final address.
