# Installing and running IRIX 7

IRIX 7 is a clean-room, GPL-3.0, Rust-native operating system for the SGI Indy. This
guide walks you through building from source, running in the iris emulator, and running
the test suite.

## Prerequisites

- **Nix** (with flakes enabled) — the canonical development environment
- **Rust** nightly (pinned in `rust-toolchain.toml`) — for host tooling and cross-builds
- **GitHub CLI** (`gh`) — authenticated with `repo`, `workflow` and `project` scopes
- **git**

## Build host setup

The supported development environment is the Nix devshell defined in `flake.nix`:

```sh
# Enter the development environment
nix develop

# Verify the toolchain
rustc --version
cargo --version
```

If you cannot use Nix, install a recent Rust nightly manually and pin it with
`rust-toolchain.toml`. The repository pins `nightly-2026-10-03` with the `rust-src`
component (required for `-Z build-std`):

```sh
rustup toolchain install nightly-2026-10-03 --component rust-src
rustup default nightly-2026-10-03
```

## Building from source

Clone the repository:

```sh
git clone https://github.com/irix7/os.git
cd os
```

Build the host tools and run the test suite:

```sh
cargo build
cargo test
```

Cross-compile the kernel and ROM for the n32/MIPS III target:

```sh
cargo build -Z build-std=core,alloc -Zjson-target-spec \
  --features cross --target targets/mips3-sgi-irix7-n32.json -p kernel

CARGO_TARGET_MIPS3_SGI_IRIX7_N32_RUSTFLAGS="-Clink-arg=-Trom/linker.ld" \
  cargo build -Z build-std=core,alloc -Zjson-target-spec \
  --features cross --target targets/mips3-sgi-irix7-n32.json -p rom
```

## Running in the iris emulator

The `irix7/iris` emulator fork is the primary development and test machine. It models the
SGI Indy (IP24) with an R4400 CPU, Newport XL graphics, Seeq 8003 Ethernet, WD33C93 SCSI,
HAL2 audio and DS1386 RTC/watchdog.

The rig scripts live in `irix7/project/scripts/rig/`. Start a private rig instance:

```sh
scripts/rig/task-rig.sh start <item>
```

The ROM boots from the PROM window (`0xBFC00000`), sizes and tests DRAM stacklessly, brings
up the serial console, LZ4-decompresses the embedded kernel to `0x8804_0000` and jumps to
it. The ROM passes the RAM geometry in registers; there is no `BootInfo` structure. The
kernel then owns the machine: Seeq/HPC3 networking, DHCP with renewals, DNS, NTP, ICMP,
HTTP/HTTPS (TLS 1.3 with certificate verification), the telnet console, the REX3 menu and
diagnostics, and the module loader.

## Running tests

Host unit and integration tests:

```sh
cargo test
```

Format and lint checks (these are the CI gates):

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
```

Cross-build check (M0 gate):

```sh
cargo build -Z build-std=core,alloc -Zjson-target-spec \
  --features cross --target targets/mips3-sgi-irix7-n32.json -p kernel
```

## Current status

| Milestone | Status |
|-----------|--------|
| M0 — Toolchain unblocked | Complete |
| M1 — ROM netboot | Complete (superseded: the ROM is an assembly stub and the kernel owns networking) |
| M2 — Kernel core | In progress |

Working on iris:

- The assembly ROM sizes and tests all DRAM stacklessly, then decompresses and jumps to the
  n32 kernel at `0x8804_0000`.
- The kernel prints `kernel: up`; the REX3 shows the boot menu and the PS/2 keyboard drives
  it; "Run diagnostics" prints CPU, RAM and network state.
- Seeq 8003 over HPC3 PDMA, DHCP (with renewal), DNS, NTP wall clock, ICMP ping.
- HTTP/1.1 and HTTPS/TLS 1.3 with a verified chain and SHA-256 digests.
- Telnet console on guest port 23 with `status`, `net`, `mem`, `time`, `echo`, `ntp`, `ping`,
  `http`, `https`, `module`, `boot` and `reboot`.
- CP0 Compare timer and `WAIT` idle loop; the vCPU parks when idle.
- Modules are flat images fetched over HTTPS into RAM, SHA-256 checked, and called through
  a versioned C service table.

M2 remaining:

- calibrate CP0 Count against the 8254 reference;
- page-fault handling and a real panic screen with a register dump;
- grow the scheduler/capability framework into the user-object model.

## Contributing

See [`CONTRIBUTING.md`](CONTRIBUTING.md) for the development setup, clean-room rules, DCO
sign-off, pull request process and code style. Every commit must carry a DCO sign-off
(`git commit -s`).

## Licence

IRIX 7 is GPL-3.0. See [`LICENCE`](LICENCE) for details.
