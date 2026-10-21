# IRIX 7 Toolchain Setup

This document describes the development environment setup for building IRIX 7.

## Toolchain Overview

The project uses a **Rust nightly toolchain** with custom MIPS/IRIX target specs for cross-compilation. All builds use `-Z build-std` since there is no standard library for these freestanding targets.

## Pinned Toolchain

**Channel:** `nightly-2026-10-03` (rustc 1.85.0-nightly)

Configuration in `rust-toolchain.toml`:

```toml
[toolchain]
channel = "nightly-2026-10-03"
components = ["rust-src", "rustfmt", "clippy", "llvm-tools-preview"]
profile = "minimal"
```

**Why nightly?** The project uses `-Z build-std` to compile `core`/`alloc` from source for freestanding targets, which requires nightly rustc. The `asm` feature is also used for inline assembly in the kernel.

## Custom Target Specs

Two custom MIPS/IRIX target specs live in `targets/`:

### `targets/mips2-sgi-irix7-o32.json` — ROM target (o32/MIPS II)

| Field | Value |
|-------|-------|
| arch | mips |
| cpu | mips2 |
| target-endian | big |
| target-pointer-width | 32 |
| max-atomic-width | 32 |
| features | `+mips2,+soft-float,+noabicalls` |
| linker | rust-lld |
| linker-flavor | gnu-lld |
| llvm-abiname | o32 |

### `targets/mips3-sgi-irix7-o32.json` — Kernel target (n32/MIPS III)

| Field | Value |
|-------|-------|
| arch | mips64 |
| cpu | mips3 |
| target-endian | big |
| target-pointer-width | 32 |
| max-atomic-width | 64 |
| features | `+mips3,+soft-float,+noabicalls` |
| linker | rust-lld |
| linker-flavor | gnu-lld |
| llvm-abiname | n32 |

## Build Commands

### Host build (for tests):

```bash
cargo build
cargo test
```

### Cross-build ROM for o32:

```bash
cargo build -Z build-std=core,alloc -Zjson-target-spec \
  --features cross --target targets/mips2-sgi-irix7-o32.json -p rom
```

### Cross-build kernel for o32:

```bash
cargo build -Z build-std=core,alloc -Zjson-target-spec --features cross --target targets/mips2-sgi-irix7-o32.json -p kernel
```

## Environment Setup (`/mnt/europa/env.sh`)

```bash
export CARGO_HOME=/mnt/europa/rust-toolchain/cargo
export RUSTUP_HOME=/mnt/europa/rust-toolchain/rustup
export PATH=/mnt/europa/rust-toolchain/bin:/mnt/europa/rust-toolchain/rustup/toolchains/nightly-2026-10-03-x86_64-unknown-linux-gnu/lib/rustlib/x86_64-unknown-linux-gnu/bin:/mnt/europa/rust-toolchain/cargo/bin:$PATH
rustup default nightly-2026-10-03
```

### Why /mnt/europa?

The root filesystem `/` is 100% full and `/nix/store` is read-only. The Rust toolchain (rustup + nightly-2026-10-03 + rust-src) is installed on `/mnt/europa/rust-toolchain/` which has 6.1T available.

## Development Environment

- **Rust nightly-2026-10-03** with `rust-src` component (required for `-Z build-std`)
- **rust-lld** as the linker for cross-compilation
- **Nix** is available but `/nix/store` is read-only, so the Nix devshell (`flake.nix`) cannot be evaluated in this environment — it is a spec for a working Nix system

## Key Constraints

1. **`/nix/store` is read-only** — cannot install Nix packages locally
2. **`/` is 100% full** — Rust toolchain lives on `/mnt/europa`
3. **smoltcp 0.13** is used but has exhaustiveness-check errors with this nightly unless both `proto-ipv4` and `proto-ipv6` features are enabled
4. **rustc_codegen_gcc** (GCC backend) is NOT available — blocked on GCC backend/libgccjit build in `irix7/gcc` repo

## Verification Commands

```bash
# Verify toolchain
rustc --version          # rustc 1.85.0-nightly
cargo --version          # cargo 1.85.0-nightly

# Build host tests
cargo test -p kernel-test -p boot-protocol-tests -p rom

# Cross-build ROM for o32
cargo build -Z build-std=core,alloc -Zjson-target-spec \
  --features cross --target targets/mips2-sgi-irix7-o32.json -p rom

# Cross-build kernel for o32
cargo build -Z build-std=core,alloc -Zjson-target-spec --features cross --target targets/mips2-sgi-irix7-o32.json -p kernel
```

### Emulator

The `irix7/iris` emulator is available at `/mnt/europa/iris` with a prebuilt binary at `/mnt/europa/iris/target/release/iris`. It supports `--load-elf` for loading freestanding ELF binaries directly (bypassing the PROM).

### Known Issue

The kernel cross-builds as valid MIPS-II ELF but crashes on iris at `0xc454f` (7 bytes past `.bss` end at `0xc4548`) because the kernel ELF uses low virtual addresses but needs to run at kseg0 addresses (`0x80000000+`) — this is the ROM's job (M1 milestone), not the kernel's.