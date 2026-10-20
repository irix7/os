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

Cross-compile the kernel and ROM for the MIPS/IRIX targets:

```sh
cargo build -Z build-std=core,alloc -Zjson-target-spec \
  --target targets/mips3-sgi-irix7-n32.json
cargo build -Z build-std=core,alloc -Zjson-target-spec \
  --target targets/mips2-sgi-irix7-o32.json
```

The ROM bin is gated behind the `cross` feature (it is a `no_main` MIPS binary that is
never host-tested):

```sh
cargo build -Z build-std=core,alloc -Zjson-target-spec --features cross \
  --target targets/mips2-sgi-irix7-o32.json --bin rom
```

## Running in the iris emulator

The `irix7/iris` emulator fork is the primary development and test machine. It models the
SGI Indy (IP24) with an R4400 CPU, Newport XL graphics, Seeq 8003 Ethernet, WD33C93 SCSI,
HAL2 audio and DS1386 RTC/watchdog.

The rig scripts live in `irix7/project/scripts/rig/`. Start a private rig instance:

```sh
scripts/rig/task-rig.sh start <item>
```

The ROM currently prints its bring-up marker (`hello from ROM`) on the serial console from
the first instruction, initialises the Seeq Ethernet controller, reads its MAC address from
the DS1386 NVRAM, and runs DHCPv4. The remaining M1 work — HTTP GET of the kernel and boot
bundle, SHA-256 verification, BootInfo construction, cache flush and jump to the kernel —
requires iris hardware validation of the Seeq DMA descriptor flag values before the DMA
programming can be finalised.

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
  --target targets/mips3-sgi-irix7-n32.json \
  --target targets/mips2-sgi-irix7-o32.json
```

## Current status

| Milestone | Status |
|-----------|--------|
| M0 — Toolchain unblocked | Complete (PRs #31, #32) |
| M1 — ROM netboot | In progress (PRs #36, #37, #39, #41) |

M1 completed:

- Serial console works from the first instruction
- Seeq 8003 initialises and reads its MAC address from NVRAM
- smoltcp TCP/IP stack builds and cross-compiles
- DHCPv4 client obtains an IP address
- HTTP/1.1 GET client with Range support builds

M1 remaining (blocked on iris hardware validation of Seeq DMA descriptor flag values):

- HTTP GET execution over smoltcp TCP
- SHA-256 verification of the fetched kernel and boot bundle
- BootInfo construction and cache flush
- Jump to the kernel entry point

## Contributing

See [`CONTRIBUTING.md`](CONTRIBUTING.md) for the development setup, clean-room rules, DCO
sign-off, pull request process and code style. Every commit must carry a DCO sign-off
(`git commit -s`).

## Licence

IRIX 7 is GPL-3.0. See [`LICENCE`](LICENCE) for details.
