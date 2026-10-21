# IRIX 7 Toolchain Setup

This document describes the development environment and cross-build commands for IRIX 7.

## Toolchain Overview

The project uses a **Rust nightly toolchain** with one custom MIPS/IRIX target spec.
All target builds use `-Z build-std` because there is no standard library for the
freestanding target.

## Pinned Toolchain

**Channel:** `nightly-2026-10-03` (rustc 1.85.0-nightly)

Configuration in `rust-toolchain.toml`:

```toml
[toolchain]
channel = "nightly-2026-10-03"
components = ["rust-src", "rustfmt", "clippy", "llvm-tools-preview"]
profile = "minimal"
```

**Why nightly?** `-Z build-std` compiles `core`/`alloc` from source for a freestanding
target, which requires nightly rustc. The `asm_experimental_arch` feature is also used for
MIPS inline assembly in the ROM and kernel.

## Custom Target Spec

Both the ROM and the kernel use **one** target:

### `targets/mips3-sgi-irix7-n32.json` — n32 / MIPS III

| Field | Value |
|-------|-------|
| abi | n32 |
| arch | mips64 |
| cpu | mips3 |
| target-endian | big |
| target-pointer-width | 32 |
| max-atomic-width | 64 |
| data-layout | `E-m:e-p:32:32-…-n32:64-S128` |
| features | `+mips3,+soft-float,+noabicalls` |
| linker | rust-lld |
| linker-flavor | gnu-lld |
| llvm-abiname | n32 |

The resulting ELF is `ELF32`, big-endian, with `EF_MIPS_ABI2` set and `.mdebug.abiN32`.
It uses 64-bit GPRs and instructions (`daddiu`, `sd`, `ld`) with 32-bit pointers.

There is deliberately no separate o32 target: the ROM and kernel are both n32/MIPS III.
o32 support belongs to the future IRIX binary-compatibility layer, not to the firmware.

## Build Commands

### Host build (for tests):

```bash
cargo build
cargo test
```

### Cross-build the kernel:

```bash
cargo build -Z build-std=core,alloc -Zjson-target-spec \
  --features cross --target targets/mips3-sgi-irix7-n32.json -p kernel
```

### Cross-build the ROM:

The ROM links its text at the PROM reset window (`0xBFC00000`) and its writable data in
DRAM (`0xA8700000`), so it needs `rom/linker.ld`:

```bash
CARGO_TARGET_MIPS3_SGI_IRIX7_N32_RUSTFLAGS="-Clink-arg=-Trom/linker.ld" \
  cargo build -Z build-std=core,alloc -Zjson-target-spec \
  --features cross --target targets/mips3-sgi-irix7-n32.json -p rom
```

### Produce a raw PROM image:

The PROM socket takes raw bytes at physical `0x1FC00000` (kseg1 `0xBFC00000`), not an ELF:

```bash
mips-sgi-irix6.5-objcopy -O binary \
  target/mips3-sgi-irix7-n32/release/rom rom.bin
```

Any MIPS-aware `objcopy -O binary` works. The binary is currently around 140 KiB and is
padded by the loader, so it fits the 1 MiB PROM window.

## Running under iris

The `irix7/iris` fork is the primary development and test machine. It models the SGI Indy
(IP24) with an R4400 CPU, Seeq 8003 Ethernet, WD33C93 SCSI, HAL2 audio and DS1386
RTC/watchdog.

Use a current iris build that includes the bulk-DMA fix (`1492b44`; `nightly.2026.10.20` or
later). Run the raw ROM image:

```bash
cd /mnt/europa/iris
./target/release/iris --prom /path/to/rom.bin \
  --bank0 32 --bank1 0 --headless \
  --monitor-port 9998 --serial-log serial.log
```

Serial channels:

| iris TCP port | SCC channel | SGI name |
|---|---|---|
| 8881 | B | `tty1`, the console |
| 8880 | A | `tty2` |

The ROM console is `DUART1B` (`HPC3_SERIAL1_CMD/DATA`, kseg1 `0xBFBD9830/0xBFBD9834`),
which is channel B, so use port **8881** or `--serial-log`.

`--load-elf` is only useful for early bring-up with an ELF linked in kseg0. It does not
test the real PROM load path; use `--prom` with the raw image for firmware work.

## Environment Setup

```bash
export CARGO_HOME=/mnt/europa/rust-toolchain/cargo
export RUSTUP_HOME=/mnt/europa/rust-toolchain/rustup
export PATH=/mnt/europa/rust-toolchain/bin:/mnt/europa/rust-toolchain/rustup/toolchains/nightly-2026-10-03-x86_64-unknown-linux-gnu/bin:/mnt/europa/rust-toolchain/cargo/bin:$PATH
```

The build scripts also need a host C compiler as `cc` and `pkg-config`; the Nix devshell
(`nix develop`) provides them, as well as the native libraries required by iris itself.

## Key Constraints

1. The ROM cannot assume RAM exists: it sizes and tests DRAM in assembly before creating a
   stack, copying `.data` or clearing `.bss`.
2. The low 512 KiB of DRAM is a special alias window; the ROM's data, stack and DMA buffers
   live at 7 MiB and above, matching the original IP22 PROM (`BSSADDR 0xA8700000`,
   `PROM_STACK 0xA8800000`).
3. The ROM runs uncached during bring-up. Cache setup and the cache-flushed handoff are part
   of the kernel entry work.
4. `smoltcp 0.13` requires both `proto-ipv4` and `proto-ipv6` with this nightly because its
   `Address` enum is non-exhaustive.
5. `rustc_codegen_gcc` is not available; the LLVM backend is the only code generator.

## Verification

```bash
# Host tests
cargo test

# Cross-build both artefacts
cargo build -Z build-std=core,alloc -Zjson-target-spec \
  --features cross --target targets/mips3-sgi-irix7-n32.json -p kernel

CARGO_TARGET_MIPS3_SGI_IRIX7_N32_RUSTFLAGS="-Clink-arg=-Trom/linker.ld" \
  cargo build -Z build-std=core,alloc -Zjson-target-spec \
  --features cross --target targets/mips3-sgi-irix7-n32.json -p rom
```

## Current Status

- ROM: boots from the PROM window, sizes/tests 32 MiB of DRAM stacklessly, brings up the
  Seeq, gets DHCP, queries NTP, fetches a kernel over HTTP, verifies SHA-256 and jumps to
  the load address.
- Kernel: cross-builds n32, but is not yet linked at the handoff address or consuming
  `BootInfo`. That is the current M1/M2 boundary.
