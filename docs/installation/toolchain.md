# IRIX 7 Toolchain Setup

This document describes the development environment and cross-build commands for IRIX 7.

## Toolchain overview

The project uses a **pinned Rust nightly** with custom MIPS/IRIX target specs. All target
builds use `-Z build-std` because there is no prebuilt standard library for the
freestanding targets. rustc's LLVM backend is the only Rust code generator; see
[M0-ADR-0001](../adr/m0-0001-llvm-only.md), which amends ADR-0018 in the project series.

## Pinned toolchain

**Channel:** `nightly-2026-10-03` — rustc 1.101.0-nightly (`0abfedbc7`, 2026-10-02),
LLVM 23.1.1.

Configuration in `rust-toolchain.toml`:

```toml
[toolchain]
channel = "nightly-2026-10-03"
components = ["rust-src", "rustfmt", "clippy", "llvm-tools-preview"]
profile = "minimal"
```

**Why nightly?** `-Z build-std` compiles `core`/`alloc` from source for a freestanding
target, which requires nightly rustc. The `asm_experimental_arch` feature is also used for
MIPS inline assembly in the kernel.

The file is the only pin. The Nix devshell reads it through `rust-overlay`, a rustup user
reads it automatically inside the repository, and CI installs the same channel. If the
channel changes, change this file and CI together.

`llvm-tools-preview` provides `llvm-objcopy` for flat module images; the devshell also
puts the host rustlib `bin/` on `PATH`, where `rust-lld` lives.

## Custom target specs

### `targets/mips3-sgi-irix7-n32.json` — n32 / MIPS III

Both the ROM and the kernel use this one target.

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

There is deliberately no separate o32 target: the ROM and kernel are both n32/MIPS III,
and o32 belongs to the IRIX 6.5 compatibility container, which loads existing binaries.
`tools/build-o32-hello.sh` builds an n32 object and relabels its `e_flags` as
`EF_MIPS_ABI_O32`; it is a loader/personality probe, not a second codegen target.

### `targets/mips3-sgi-irix7-native-n32.json` — hosted native

The same n32/MIPS III ABI with `target_os = "irix7"` and the Unix family. It is for the
POSIX.1-2024 libc and Rust `std` work; the kernel and loadable modules never link it.

## Build host setup

### Nix (canonical)

```sh
git clone https://github.com/irix7/os.git
cd os
nix develop
rustc -Vv    # 1.101.0-nightly (0abfedbc7 2026-10-02), LLVM 23.1.1
```

The `#default` shell supplies the pinned nightly with `rust-src`, `rustfmt`, `clippy` and
`llvm-tools-preview`, plus `gcc`, `binutils`, `python3`, `pkg-config` and the LLVM
binutils. `nix develop -c <command>` runs one command in the same environment.

### rustup (without Nix)

```sh
rustup toolchain install nightly-2026-10-03 \
  --component rust-src --component llvm-tools-preview \
  --component rustfmt --component clippy
```

Inside the repository `rust-toolchain.toml` selects that channel automatically, so do not
set `RUSTUP_TOOLCHAIN` or edit the default toolchain. You also need a host `cc`,
`pkg-config` and `python3` for `build-std`'s build scripts and the image tools.

## Build commands

### Host build for tests

```sh
cargo build
cargo test
```

### Cross-build the kernel

The kernel links at `0x8804_0000` through `kernel/linker.ld`, which also defines
`__bss_end` and `__stack_top`. Release with `opt-level=z` is the supported build: the
debug image does not fit the boot stack.

```sh
CARGO_PROFILE_RELEASE_OPT_LEVEL=z \
CARGO_TARGET_MIPS3_SGI_IRIX7_N32_RUSTFLAGS="-Clink-arg=-Tkernel/linker.ld" \
  cargo build --release -Z build-std=core,alloc -Zjson-target-spec \
    --features cross --target targets/mips3-sgi-irix7-n32.json -p kernel
```

The output ELF is `target/mips3-sgi-irix7-n32/release/kernel`.

### Cross-build the ROM

The ROM embeds the kernel ELF from the same target directory and profile, so build the
kernel first. The ROM links its text at the PROM reset window (`0xBFC00000`) and its
writable data in DRAM (`0xA8700000`), so it needs `rom/linker.ld`:

```sh
CARGO_TARGET_MIPS3_SGI_IRIX7_N32_RUSTFLAGS="-Clink-arg=-Trom/linker.ld" \
  cargo build --release -Z build-std=core,alloc -Zjson-target-spec \
    --features cross --target targets/mips3-sgi-irix7-n32.json -p rom
```

### Produce a raw PROM image

The PROM socket takes raw bytes at physical `0x1FC00000` (kseg1 `0xBFC00000`), not an ELF,
and the kernel payload is embedded in the ROM ELF. Use the repository's image tool:

```sh
python3 rom/tools/make_prom.py \
  target/mips3-sgi-irix7-n32/release/rom /tmp/irix7-rom.bin
```

The image must stay within the 512 KiB PROM window; the current build is about 490 KiB.

### Build everything the two-node rig needs

```sh
tools/m2-rig/build.sh
```

This cross-builds the release kernel and ROM, builds the MCP module as a flat image with
`llvm-objcopy`, writes the PROM, and records `SHA256SUMS` and
`BUILD-PROVENANCE.txt` under `target/m2-rig/`.

## Running under iris

The `irix7/iris` fork is the primary development and test machine. It models the SGI Indy
(IP24) with an R4400 CPU, Seeq 8003 Ethernet, WD33C93 SCSI, HAL2 audio and DS1386
RTC/watchdog.

Use **v0.1.13+irix7** or later; CI pins that release through
`irix7/iris/.github/actions/setup-iris`, which installs `iris` and `iris-ci` on `PATH`.
The launcher in a local checkout is `target/launcher/bin/iris`, built with:

```sh
cd /path/to/iris
nix develop --command cargo build --release --features lightning,gpujit
```

A diskless boot of the PROM image, matching the CI `Boot (iris)` job:

```sh
cd /tmp/irix7-boot
cat > machine.toml <<'EOF'
banks = [32, 0, 0, 0]
no_audio = true

[machine]
profile = "indy_ip24"

[scsi]
EOF
python3 - <<'PY'
image = bytearray(8192)
image[314:320] = bytes([0x02, 0x11, 0x22, 0x33, 0x44, 0x55])
open("nvram.bin", "wb").write(image)
PY

iris --config machine.toml --ci \
  --prom /tmp/irix7-rom.bin --nvram nvram.bin \
  --ci-socket ./iris.sock --monitor-port 18887 \
  --serial-log serial.log &

iris-ci --socket ./iris.sock status --wait-ready 30
iris-ci --socket ./iris.sock start
iris-ci --socket ./iris.sock serial-wait 'BOOT-READY' --timeout 120
```

`BOOT-READY v=1 boot=<rtc> kernel=up network=<state> mcp=<state> mesh=<state>` is the
readiness marker. `iris-ci --socket ./iris.sock quit` stops the instance cleanly.

To reach the telnet console, add a forward to `machine.toml`:

```toml
[[port_forward]]
proto = "tcp"
host_port = 2324
guest_port = 23
bind = "localhost"
```

then `telnet localhost 2324` while the guest is running.

Serial channels:

| iris TCP port | SCC channel | SGI name |
|---|---|---|
| 8881 | B | `tty1`, the console |
| 8880 | A | `tty2` |

The ROM console is channel B, so use port **8881** or `--serial-log`.

`--load-elf` is only useful for early bring-up with an ELF linked in kseg0. It does not
test the real PROM load path; use `--prom` with the raw image for firmware work.

## Environment notes

No environment file is required. The Nix shell sets the toolchain, `cc`, `pkg-config` and
the LLVM tools; a rustup install provides the same compiler when `rust-toolchain.toml`
selects it.

If the host `cc` wrapper is stale (a Nix store path that no longer exists), point `CC` or
`PATH` at a working compiler before building. Only host build scripts need the C compiler;
the kernel, ROM and modules link with `rust-lld`.

## Key constraints

1. The ROM cannot assume RAM exists: it sizes and tests DRAM in assembly before creating a
   stack, copying `.data` or clearing `.bss`.
2. The low 512 KiB of DRAM is a special alias window; the ROM's data, stack and DMA buffers
   live at 7 MiB and above (`.data` at `0xA8700000`, a 2 MiB heap, stack top at
   `0xA8A00000`), matching the original IP24 PROM's BSSADDR and staying clear of the alias.
3. The ROM runs uncached during bring-up. Cache setup and the cache-flushed handoff are part
   of the kernel entry work.
4. `smoltcp 0.13` requires both `proto-ipv4` and `proto-ipv6` with this nightly because its
   `Address` enum is non-exhaustive.
5. rustc's LLVM backend is the only code generator. There is no `rustc_codegen_gcc` build
   path; `irix7/gcc` remains the IRIX system toolchain for the compatibility container.
   See [M0-ADR-0001](../adr/m0-0001-llvm-only.md).
6. The kernel stack top is `0x883f_0000`; the module area starts at `0x8840_0000`.

## Verification

```sh
# Host tests
cargo test

# Cross-build the kernel (release, with the kernel linker script)
CARGO_PROFILE_RELEASE_OPT_LEVEL=z \
CARGO_TARGET_MIPS3_SGI_IRIX7_N32_RUSTFLAGS="-Clink-arg=-Tkernel/linker.ld" \
  cargo build --release -Z build-std=core,alloc -Zjson-target-spec \
    --features cross --target targets/mips3-sgi-irix7-n32.json -p kernel

# Cross-build the ROM
CARGO_TARGET_MIPS3_SGI_IRIX7_N32_RUSTFLAGS="-Clink-arg=-Trom/linker.ld" \
  cargo build --release -Z build-std=core,alloc -Zjson-target-spec \
    --features cross --target targets/mips3-sgi-irix7-n32.json -p rom

# Raw PROM image
python3 rom/tools/make_prom.py \
  target/mips3-sgi-irix7-n32/release/rom /tmp/irix7-rom.bin
```

## Current status

- ROM: a MIPS III assembly stub; boots from the PROM window, sizes and tests 32 MiB of DRAM
  stacklessly, brings up the serial console, LZ4-decompresses the kernel to `0x8804_0000`
  and jumps to it.
- Kernel: n32, linked at `0x8804_0000`, booting on iris. It owns the Seeq/HPC3 network
  stack, DHCP/DNS/NTP/ICMP, HTTP/HTTPS with certificate verification, the telnet console,
  the CP0 timer/WAIT idle path and the HTTPS module loader.
- M2: the two-kernel join, epoch, homes, route, L0 migration and failure/partition slice
  pass on iris (`tools/m2-rig`), and the one-prompt object/actor transcript is
  `tools/one-computer`.
- M3: the first shard slice converges two real kernels; eight-node convergence is not
  claimed. See [`plan/m3-rig.md`](../project/m3-rig.md).
