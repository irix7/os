# Software development kit

The SDK provides the tools and interfaces for developing IRIX 7 native applications and
kernel services. It is divided into the following sections:

## Native development

The native SDK target is `targets/mips3-sgi-irix7-native-n32.json` (`target_os = "irix7"`,
Unix family). It uses the n32 ABI (64-bit registers, 32-bit pointers, big-endian) and
the relibc fork as its C/POSIX layer. The native SDK covers:

- Compiling and linking native executables and shared libraries
- The Unix PAL (platform abstraction layer) for kernel interaction
- The 42-symbol `crates/irix7-libc` bring-up libc shim
- TLS, event loops, thread management and networking via the native ABI
- Building and running Rust `std` on the native target

## Compatibility development

The compatibility SDK target is `targets/mips3-sgi-irix7-n32.json` (`target_os = "none"`).
It runs IRIX 6.5 o32 and n32 binaries unchanged through the legacy ABI layer. It covers:

- ELF32 parsing and load planning ( [`elf.md`](../programming/elf.md) )
- The IRIX 6.5 syscall ABI and `sproc` semantics
- Compatibility container interfaces and NVRAM environment handling
- Running IRIX 6.5 binaries on IRIX 7 without modification

## Wasm and mesh development

The Wasm core is the architecture-independent WebAssembly image that every replica runs.
It owns scheduling policy, the global namespace, the capability engine, the actor directory
and the placement view. Development involves:

- Building and testing the Wasm core crate
- Policy probe configuration and policy-as-code
- Mesh envelope framing and transport between nodes
- Capability and actor directory management

## MCP control plane

The MCP (Model Context Protocol) control plane allows developers to inspect and drive a
running Indy over the network. It is implemented in `crates/mcp`, linked into the kernel
image. The API specification is in [`using/mcp.md`](../using/mcp.md) and
[`using/mcp-apps.md`](../using/mcp-apps.md). Developers can:

- Call `POST /mcp` with JSON-RPC 2.0 methods
- Use the `tools/list` and `tools/call` endpoints
- Extend the MCP with custom tools and methods

## Build and deployment

- The toolchain uses a pinned Rust nightly with custom MIPS/IRIX target specs
- All builds use `-Z build-std` because there is no prebuilt standard library for
  freestanding targets
- The kernel, ROM and native userland are built separately and combined at boot
- See the [milestone plan](../project/milestones.md) for gate-dependent features

Project terms are defined in the [glossary](../introduction/glossary.md).
