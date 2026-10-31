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

## Limits, stated plainly

- **Trust.** The digest comes from the disk header, which the machine itself
  trusts. A real boot takes the digest from the signed boot configuration. The
  x86 machine has no signed configuration yet.
- **Services.** No TCP, UDP, console-input, block-through-module or screen
  services. The module's HTTP server and mesh listener cannot run. The module
  initialises and returns; it does not serve.
- **Shared loader.** The loader is x86-only. The Indy kernel's loader in
  `kernel/src/module.rs` is still the only one with the full service table. The
  shared home is the next step.
- **Module tick.** The vtable's tick is not called yet. The idle loop that calls
  it does not exist on x86.

## Next

1. Implement TCP and UDP services over the x86 smoltcp stack, so the module's
   listener can run. This is the bulk of the Indy's `module.rs` socket glue.
2. Move the arch-neutral loader and service logic into a shared module, with the
   Indy as the first user, so the two kernels stop duplicating it.
3. Call the module's tick from an x86 idle loop.
4. Run the M1 probe on x86 once the HTTP path works.
