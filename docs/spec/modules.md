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

## 3. Address space

| Property | Value |
|---|---|
| Load virtual address (kseg0) | `0x8840_0000` (physical `0x0840_0000`) |
| Maximum size | 2 MiB (`0x0020_0000`) |
| Placement | inside the reserved low window, above the kernel stack top (`0x8820_0000`), below the frame allocator's first frame (`0x08B0_0000`) |
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

`Services` is `#[repr(C)]` and versioned. Version 2:

| Offset | Field | Meaning |
|---|---|---|
| 0x00 | `version` | 2 |
| 0x04 | `write` | `void (*)(const uint8_t *ptr, size_t len)` — serial and telnet |
| 0x08 | `clock_ms` | `uint64_t (*)(void)` — monotonic milliseconds since boot |
| 0x10 | `alloc` | `void *(*)(size_t size, size_t align)` — kernel heap, never freed |
| 0x14 | `poll` | `void (*)(void)` — drive the shared network stack once |

New services are appended and `version` is bumped; existing offsets never move. A module
that needs a newer table can reject an older core by checking `version`.

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

## 6. Reference modules

`modules/hello` is the one-shot reference: a small flat image that prints one line through
the service table and returns a vtable with status 0. Build it with its own linker script
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

## 7. Planned services

- TCP listen/accept/send/recv/state/close over the shared interface, so a module can serve
  a protocol without linking smoltcp.
- The NTP wall clock (distinct from monotonic `clock_ms`).
- A module manifest with a name and required ABI version.

These are additive to the version 2 table.

## 8. Non-goals

- User-mode code, capabilities or process isolation (see the architecture specification).
- Multiple concurrent modules or dependency resolution.
- Threads inside a module; a module gets the tick callback and the idle loop's context.
- Arbitrary relocatable ELF objects; modules are linked at their final address.
