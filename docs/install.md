# Installing and running IRIX 7

IRIX 7 is a clean-room, GPL-3.0, Rust-native operating system for the SGI Indy. This
guide builds it from source, boots the PROM image in the iris emulator, and runs the
single-node and two-node gates.

## Prerequisites

- A Linux `x86_64` host.
- **Nix** with flakes enabled — the canonical development environment. It supplies the
  pinned Rust nightly, `rust-src`, the host C compiler and the image tools.
- **iris `v0.1.13+irix7`** and its matching `iris-ci` — the emulator used for every boot
  and rig gate. The source is [`irix7/iris`](https://github.com/irix7/iris).
- **git**, and **GitHub CLI** (`gh`) if you are opening pull requests.

A rustup toolchain is a supported alternative to Nix; see
[`toolchain.md`](toolchain.md) for the exact channel and components. The rest of this
guide assumes the Nix shell.

## Build host setup

```sh
git clone https://github.com/irix7/os.git
cd os

# Enter the development environment
nix develop

# Verify the pinned toolchain
rustc -Vv    # 1.101.0-nightly (0abfedbc7 2026-10-02), LLVM 23.1.1
cargo --version
```

The `#default` shell reads `rust-toolchain.toml` through `rust-overlay`, so it always
matches CI: `nightly-2026-10-03` with `rust-src`, `rustfmt`, `clippy` and
`llvm-tools-preview`. It also provides `gcc`, `binutils`, `python3`, `pkg-config`,
`rust-lld` and `llvm-objcopy`. `nix develop -c <command>` runs one command in that
environment.

Without Nix, install the same channel with rustup:

```sh
rustup toolchain install nightly-2026-10-03 \
  --component rust-src --component llvm-tools-preview \
  --component rustfmt --component clippy
```

`rust-toolchain.toml` selects the channel inside the repository. You also need `cc`,
`pkg-config` and `python3` on the host.

## Building from source

Host tools and tests:

```sh
cargo build
cargo test
```

Cross-build the kernel. The release profile with `opt-level=z` and the kernel linker
script is the supported build; it fits the stack and the PROM-embedded image budget:

```sh
CARGO_PROFILE_RELEASE_OPT_LEVEL=z \
CARGO_TARGET_MIPS3_SGI_IRIX7_N32_RUSTFLAGS="-Clink-arg=-Tkernel/linker.ld" \
  cargo build --release -Z build-std=core,alloc -Zjson-target-spec \
    --features cross --target targets/mips3-sgi-irix7-n32.json -p kernel
```

Build the ROM and produce the raw PROM image. The ROM build embeds the kernel ELF from
the same target directory, so the kernel build must run first:

```sh
CARGO_TARGET_MIPS3_SGI_IRIX7_N32_RUSTFLAGS="-Clink-arg=-Trom/linker.ld" \
  cargo build --release -Z build-std=core,alloc -Zjson-target-spec \
    --features cross --target targets/mips3-sgi-irix7-n32.json -p rom

python3 rom/tools/make_prom.py \
  target/mips3-sgi-irix7-n32/release/rom /tmp/irix7-rom.bin
```

The image must be at most 512 KiB; the current build is about 490 KiB.

`tools/m2-rig/build.sh` does all of the above, builds the MCP module and writes
`SHA256SUMS` and `BUILD-PROVENANCE.txt` under `target/m2-rig/`.

## Running in the iris emulator

The ROM boots from the PROM window (`0xBFC00000`), sizes and tests DRAM stacklessly, brings
up the serial console, LZ4-decompresses the embedded kernel to `0x8804_0000` and jumps to
it. The ROM passes the RAM geometry in registers; there is no `BootInfo` structure. The
kernel then owns the machine: Seeq/HPC3 networking, DHCP with renewals, DNS, NTP, ICMP,
HTTP/HTTPS (TLS 1.3 with certificate verification), the telnet console, the REX3 boot
log, the native SCSI user image and the module loader.

### One diskless boot

This is the path the CI `Boot (iris)` job runs
([`.github/workflows/ci.yml`](../.github/workflows/ci.yml), `Boot (iris)`):

```sh
mkdir -p /tmp/irix7-boot && cd /tmp/irix7-boot

cat > machine.toml <<'EOF'
banks = [32, 0, 0, 0]
no_audio = true

[machine]
profile = "indy_ip24"

[scsi]
EOF

# A DS1386 NVRAM image with a fixed MAC (the kernel derives its NodeId from it).
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
iris-ci --socket ./iris.sock quit
```

`BOOT-READY v=1 boot=<rtc> kernel=up network=<up|off|down> mcp=<listening|local|disabled|failed> mesh=<single|not-ready>`
is the readiness marker. With no module and no disk the boot continues to the `sash>`
prompt, the native recovery shell.

For the telnet console, add a forward to `machine.toml` and connect to it:

```toml
[[port_forward]]
proto = "tcp"
host_port = 2324
guest_port = 23
bind = "localhost"
```

```sh
telnet localhost 2324
```

The shell has `help`, `ls`, `cat`, `pwd`, `cd`, `printenv`, `status`, `hinv`, `packages`,
`mount`, `boot` and `reboot`; the kernel diagnostics (`net`, `mem`, `scsi`, `time`,
`http`, `https`, `ntp`, `ping`, `module`) remain available from the same console.

### Single-node smoke test

`rom/tools/smoke.py` starts iris with the shipped rig configuration, waits for DHCP and drives
the telnet console. Run it from a directory that contains `scsi1.raw`, or pass `--cwd`:

```sh
rom/tools/smoke.py --iris /path/to/iris --prom /tmp/irix7-rom.bin --cwd /path/to/iris
```

### Two kernels and the M2 gate

The two-node rig starts two independent iris processes, each with its own NVRAM, MAC, NAT,
CI socket and serial log:

```sh
tools/m2-rig/build.sh

cargo run --release --manifest-path tools/m2-rig/Cargo.toml -- run \
  --iris /path/to/iris --iris-ci /path/to/iris-ci
```

The run waits for both kernels' `BOOT-READY`, then proves transport, join and epoch,
rendezvous homes, the remote capability route and `MOVED` retry, L0 actor migration in
both directions, and the failure/partition scenarios. Evidence is written under
`target/m2-rig/evidence/`, with `summary.md` as the entry point. The fault modes are
`--fault no-peer`, `wrong-authority`, `no-route`, `kill-peer` and `partition`. See
[`plan/m2-rig.md`](plan/m2-rig.md) for the full contract.

The one-prompt M2 object/actor proof runs from `tools/one-computer/run.sh`; the transcript
is written under `target/one-computer/demo/`.

### M3 first shard slice

The first M3 slice uses the same rig with the `shard` command:

```sh
cargo run --release --manifest-path tools/m2-rig/Cargo.toml -- shard \
  --iris /path/to/iris --iris-ci /path/to/iris-ci
```

Two real kernels converge on the same shard table; eight-node convergence is not claimed.
See [`plan/m3-rig.md`](plan/m3-rig.md).

## Running tests

Host unit and integration tests:

```sh
cargo test
python3 -m unittest discover -s tools -p 'test_*.py'
```

Format and lint checks (the CI gates):

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
```

M0 cross-build (kernel and ROM):

```sh
CARGO_PROFILE_RELEASE_OPT_LEVEL=z \
CARGO_TARGET_MIPS3_SGI_IRIX7_N32_RUSTFLAGS="-Clink-arg=-Tkernel/linker.ld" \
  cargo build --release -Z build-std=core,alloc -Zjson-target-spec \
    --features cross --target targets/mips3-sgi-irix7-n32.json -p kernel

CARGO_TARGET_MIPS3_SGI_IRIX7_N32_RUSTFLAGS="-Clink-arg=-Trom/linker.ld" \
  cargo build --release -Z build-std=core,alloc -Zjson-target-spec \
    --features cross --target targets/mips3-sgi-irix7-n32.json -p rom
```

## Current status

| Milestone | Status |
|-----------|--------|
| M0 — Toolchain unblocked | Complete: pinned nightly, `rust-src`, LLVM-only backend ([M0-ADR-0001](adr/m0-0001-llvm-only.md)) |
| M1 — Single-node logical computer | Complete on iris; the real-Indy gate remains open (#16) |
| M2 — Two-node join, route, migration and failure | Complete on iris (2026-10-27); one-prompt object/actor proof 2026-11-03 (#329) |
| M3 — Sharded homes and multi-member | First shard slice: two real kernels converge; eight-node convergence not claimed |

Working on iris:

- The assembly ROM sizes and tests all DRAM stacklessly, then decompresses and jumps to the
  n32 kernel at `0x8804_0000`.
- The kernel prints `kernel: up` and `BOOT-READY`; the REX3 paints the boot log, and the
  diagnostic inventory is available from the telnet console. The kernel then enters the
  native n32 image on SCSI target 2 when one is attached.
- Seeq 8003 over HPC3 PDMA, DHCP (with renewal), DNS, NTP wall clock, ICMP ping.
- HTTP/1.1 and HTTPS/TLS 1.3 with a verified chain and SHA-256 digests.
- The `sash` recovery shell over telnet, keyboard and display, plus the kernel diagnostic
  commands.
- CP0 Compare timer and `WAIT` idle loop; the vCPU parks when idle.
- Modules are flat images fetched over HTTPS into RAM, SHA-256 checked, and called through
  a versioned C service table.
- M2: two independent kernels join at a signed membership epoch, serve rendezvous homes,
  route a remote capability with one `MOVED` retry, migrate a stackless actor both ways,
  and survive a killed peer or a partition with lease fencing.
- M3: the first shard slice splits the fixture object set by `mesh-core::home_for` and
  serves only the local assignments.

M2 and M3 remaining:

- general page-table refill and page-fault handling;
- grow the scheduler/capability framework into the user-object and thread model;
- real threads/TLS, `posix_spawn`, signals, termios and sockets;
- gossip/epoch push, a replicated epoch log and linearisable write handover;
- the full M3 gate: CAS fetch, placement, two-node failure, churn and the GPU object path.

## Contributing

See [`CONTRIBUTING.md`](../CONTRIBUTING.md) for the development setup, clean-room rules, DCO
sign-off, pull request process and code style. Every commit must carry a DCO sign-off
(`git commit -s`).

## Licence

IRIX 7 is GPL-3.0. See [`LICENCE`](../LICENCE) for details.
