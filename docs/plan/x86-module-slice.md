# x86 module load, fifth slice

Part of [`x86.md`](x86.md) and issue [#266](https://github.com/irix7/os/issues/266).

## What runs

The machine loads the MCP control-plane module from the boot disk, checks its
SHA-256 digest, and calls its entry through the shared module ABI. The module
returns status 0. The gate checks the digest marker and the status.

- The module is linked at `0x0200_0000` (`modules/mcp/linker-x86.ld`). The window
  is reserved in the machine's memory map, so the frame allocator never hands it
  out.
- Disk layout: sector 8 holds the header (`IRIX7MOD`, the image length as a
  little-endian u64, and the SHA-256); the image starts at sector 16.
- The services table (`kernel-x86/src/services.rs`) is the SDK's `Services`
  struct at the pinned tag. It implements `write`, `clock_ms`, `alloc`, `poll` and
  `config_get`. Every other entry returns an error, so a module that calls one
  sees a refusal, not a null pointer.

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

## Limits, stated plainly

- **Trust.** The digest comes from the disk header, which the machine itself
  trusts. A real boot takes the digest from the signed boot configuration. The
  x86 machine has no signed configuration yet.
- **Services.** TCP slots are real. UDP, console input, block-through-module
  and screen services still refuse. The mesh listener depends on UDP and is
  not exercised.
- **Shared loader.** The loader is x86-only. The Indy kernel's loader in
  `kernel/src/module.rs` is still the only one with the full service table. The
  shared home is the next step.

## Next

1. The M1 probe: autoload of the wasm module (the fetch path now works), then the
   wasm runtime running it.
2. UDP slots over smoltcp, for the M3 transport gate.
3. Move the arch-neutral loader and service logic into a shared module, with the
   Indy as the first user, so the two kernels stop duplicating it.
