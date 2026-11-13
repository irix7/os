# x86 module load, fifth slice

Part of [`x86.md`](x86.md) and issue [#266](https://github.com/irix7/os/issues/266).

## What runs

The machine loads the MCP control-plane module from the boot disk, checks its
SHA-256 digest, and calls its entry through the shared module ABI. The module
returns status 0. The gate checks the digest marker and the status.

- The module is linked at `0x0200_0000` (`modules/mcp/linker-x86.ld`). The window
  is reserved in the machine's memory map, so the frame allocator never hands it
  out.
- Disk layout: sector 8 holds the canonical header from
  `crates/kernel-core/src/image.rs` (`IRIX7MOD`, the image length as a
  big-endian u32, and the SHA-256); the image starts at sector 16. The PowerPC
  machine reads the same header.
- The service table is the one `module_abi::Services` table in
  `crates/kernel-core/src/service.rs`, shared with the Indy and the PowerPC
  machine. The x86 platform installs `write`, `clock_ms`, `alloc`, `poll`,
  `config_get`, `info` and the block store. Every other entry keeps the shared
  refusal, so a module that calls one sees the same error on every machine.

## Serving (sixth slice)

The module now serves. The x86 kernel keeps its network stack alive for the
whole boot, implements the module TCP slots (`kernel-x86/src/tcp.rs`) with the
Indy kernel's semantics, and runs a serve loop after the module's init. The
loop polls the stack and calls the module's tick for 60 s.

The gate forwards a free host port to guest port 80 with QEMU's user networking,
waits for `mcp: listening on port 80`, and sends a real HTTP request from the
host. It got `HTTP/1.0 200 OK` from the module running on the x86 machine.

## Autoload fetch (seventh slice)

The kernel fetches the module over HTTP before it loads it, as the Indy kernel's
autoload does. The URL is on the boot disk (`a.b.c.d:port/path`, sector 9). The
kernel makes an HTTP/1.0 GET through module TCP slot 7, which is free at boot,
and checks the 676,384-byte body against the digest in the disk header. The gate
serves the module from a host fixture and points sector 9 at the gateway,
10.0.2.2.

This proves `tcp_connect`, `tcp_send`, `tcp_recv` and the close path on the
machine. It does not prove the M1 probe: the M1 probe loads the wasm module
through the Indy's autoload and checks the wasm runtime, and that is not done
here.

## The wasm policy module runs (eighth slice)

The machine now runs the wasm policy module, which logs `wasm: run=42` and
`m1-wasm: ... cap=ok`, the same result the Indy's M1 autoload check expects.

- The wasm module links at `0x0400_0000` in its own 2 MiB window
  (`modules/wasm/linker-x86.ld`). The image is 1,257,992 bytes, at disk sector
  2048, with its header at sector 10.
- Its runtime is deep, so the machine runs its load on a 2 MiB stack window
  (`kernel-x86/src/stack.rs`). The 32 KiB boot stack is too small.
- The heap grows to 16 MiB. The wasm runtime allocates through `services.alloc`.
- `services.info(1)` returns `mac: aa:bb:cc:dd:ee:ff`, which the module reads to
  derive its node identity. This is the same query the Indy kernel answers.

The gate runs the MCP module's fetch and HTTP serve first, then loads the wasm
module from the disk.

## Limits

- **Trust.** The digest comes from the disk header, which the machine itself
  trusts. A real boot takes the digest from the signed boot configuration. The
  x86 machine has no signed configuration yet.
- **Services.** TCP slots are real. UDP, console input, block-through-module
  and screen services still refuse. The mesh listener depends on UDP and is
  not exercised.
- **M1 fetch.** The wasm module comes from the disk, not from an HTTP fetch as
  on the Indy. Only the MCP module is fetched over HTTP.
- **Shared loader.** The header, digest, vtable and entry rules are shared in
  `kernel-core`, and so is the service table. The fetch transports and module
  windows remain machine-specific: the Indy has two fixed slots and a SCSI or
  HTTPS fetch, the x86 machine has two 2 MiB windows and an HTTP fetch, and the
  PowerPC machine streams from ATA.

## Next

1. The M1 probe: autoload of the wasm module (the fetch path now works), then the
   wasm runtime running it.
2. UDP slots over smoltcp, for the M3 transport gate.
3. ~~Move the arch-neutral loader and service logic into a shared module.~~
   Done: `kernel-core` is used by the Indy, x86_64 and PowerPC kernels.
4. Add an x86 userland/native syscall gate. Until then the machine has no
   POSIX/IRIX contract level, and the docs must not claim one.
