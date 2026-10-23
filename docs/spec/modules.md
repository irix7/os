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
relocation at load time.

## 4. ABI

The entry point is a normal C call:

```c
int32_t _start(const struct Services *services);
```

`Services` is `#[repr(C)]` and versioned. Version 1:

| Offset | Field | Meaning |
|---|---|---|
| 0x00 | `version` | 1 |
| 0x04 | `write` | `void (*)(const uint8_t *ptr, size_t len)` — serial and telnet |
| 0x08 | `clock_ms` | `uint64_t (*)(void)` — milliseconds since boot |

New services are appended and `version` is bumped; existing offsets never move. A module
that needs a newer table can reject an older core by checking `version`.

The entry returns an `i32` status, which the console prints as `module: returned N`.

## 5. Lifecycle

- One module slot. Loading a second image overwrites the first; there is no unload and no
  destructor.
- The loader is synchronous: the console is blocked for the duration of the fetch and the
  module call.
- Exceptions inside a module use the kernel's trap path. The current trap handler prints
  the cause and halts, which is the expected failure mode for a bad module.

## 6. Reference module

`modules/hello` is the reference: a 152-byte flat image that prints one line through the
service table and returns 0. Build it with its own linker script and flatten the ELF:

```bash
export CARGO_TARGET_MIPS3_SGI_IRIX7_N32_RUSTFLAGS="-Clink-arg=-Tmodules/hello/linker.ld"
cargo build --release -Z build-std=core -Zjson-target-spec \
  --target targets/mips3-sgi-irix7-n32.json -p hello-module

$(rustc --print sysroot)/lib/rustlib/$(rustc --print host)/bin/llvm-objcopy \
  -O binary target/mips3-sgi-irix7-n32/release/hello-module hello.module
```

Serve `hello.module` and load it:

```text
module http://10.1.1.3:18080/hello.module <sha256 of hello.module>
hello from a downloaded module
module: returned 0
```

## 7. Planned services

- `alloc(size, align)` for modules that need dynamic buffers (TLS-style workloads).
- TCP connect/send/recv/state/remove over the shared interface, for modules that speak
  protocols not implemented in the core.
- The NTP wall clock.

These are additive to the version 1 table.

## 8. Non-goals

- User-mode code, capabilities or process isolation (see the architecture specification).
- Multiple concurrent modules or dependency resolution.
- Arbitrary relocatable ELF objects; modules are linked at their final address.
