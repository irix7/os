# Changelog

All notable changes to IRIX 7 are recorded here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the project uses
[Semantic Versioning](https://semver.org/).

## [Unreleased]

### Added

- Raw PROM boot path: a **MIPS III assembly** ROM executes directly from `0xBFC00000`,
  sizes and maps the DRAM banks through the memory controller, tests every byte of RAM
  stacklessly, brings up the SCC console, copies the kernel image out of PROM to
  `0x0804_0000` and jumps to it. The ROM has no Rust in it at all.
- The kernel image is embedded in the PROM image: `rom/build.rs` extracts the kernel
  ELF's loadable segments and emits them as `.word` directives in a generated
  `rom/src/boot/payload.S`.
- Graphical boot menu in the kernel: the REX3 (Newport) display is programmed for
  24-bit RGB and the menu is drawn with the PROM's own IRIS 10 raster font, converted
  from the IRIX source by `kernel/tools/convert_font.py`.
- PS/2 keyboard input in the kernel, with the scancode decoder unit-tested on the host
  (`kernel/tests/kbd.rs`).
- One canonical `targets/mips3-sgi-irix7-n32.json` target for both the ROM and the kernel.
- `rom/linker.ld` and the `rom/src/boot/` reset, memory-sizing and memory-test assembly.
- Research note on Meta CRAM and compressed RAM (`docs/research/compressed-ram.md`);
  compressed RAM is rejected for the Indy in favour of the read-only EROFS root.
- Kernel handoff: the real n32 kernel is linked at `0x88040000`, receives a physical
  `BootInfo` in `$a0`, uses its memory map for the frame allocator, and runs with caches
  enabled.
- Telnet console on guest port 23, now in the kernel. The ROM is a stub and has no
  network stack of its own.
- Styled telnet UI: a `no_std` Lip Gloss subset with truecolor gradients, rounded boxes,
  tables and 256/16/plain profiles; NAWS sizing, command history, tab completion and
  system/network/memory/CP0 dashboards.
- Advanced architecture specification (`docs/spec/architecture.md`), including the D1–D31
  decision register, hybrid SAS/MMU isolation, the native capability ABI, the IRIX
  compatibility container, R2-first boot, EROFS root, X11, the native display server and the
  in-OS MCP control plane.
- Project plan (`docs/plan/`): milestones, GitHub operating model, risk register, interface
  index, hardware lab, testing, security, release, legal, community and cost.
- Interface specifications: boot protocol, MCP control plane, driver `ctl`/`data`/`status`.
- Research reports (`docs/research/`): Indy hardware, Rust X11, native window systems, Rust
  filesystems, Plan 9 in Rust, big-endian EROFS and the broader Rust OS design survey.
- GitHub configuration: issue forms, PR template, CODEOWNERS, Dependabot, release notes,
  labeler and workflows for CI, security, licence, docs, release, project automation, stale
  items, hardware-in-the-loop and nightly builds.
- Community and governance files: contributing, code of conduct, security policy, support,
  governance and maintainers.
- Root Cargo workspace (`Cargo.toml`, `Cargo.lock`) so CI runs the Rust jobs; `kernel-test`
  is the freestanding `no_std` tracer bullet.
- `-Z build-std=core,alloc` cross-builds for the n32 `mips3-sgi-irix7-n32` target
  (compiles `core`/`alloc` from source).
- Nix devshell (`flake.nix`) pinning rustc, cargo, rustfmt, clippy, gcc and binutils.

### Changed

- The ROM is now a **pure MIPS III assembly stub**. Everything Rust that used to live
  in it — the smoltcp stack, DHCP, NTP, the HTTP fetch, the telnet console and its
  Lip Gloss UI — has moved to the kernel, which now runs from RAM at `0x8804_0000`. The
  ROM only sizes and maps DRAM, tests it, brings up the serial console, copies the
  kernel image across and jumps.
- The kernel payload is embedded in the PROM image rather than fetched over HTTP, so
  the boot path no longer depends on the network.
- The ROM hands the RAM geometry it discovered over in registers (`$a1` bytes, `$a2`
  base) instead of a `BootInfo` structure; the kernel falls back to that when
  `BootInfo` is absent.
- The kernel clears its own `.bss` on entry: the ROM's RAM test scribbles over all of
  DRAM before the kernel image is copied in.
- Consolidated the ROM and kernel on n32/MIPS III and removed the misnamed
  `mips2-sgi-irix7-o32` target and its duplicate spec.
- The ROM now links text at the PROM window (`0xBFC00000`) and writable data at 7 MiB
  (`0xA8700000`), with an 8 MiB stack top, matching the original IP22 PROM layout.
- CI and release scripts build the kernel and ROM from the same target; the ROM build
  passes `rom/linker.ld`.
- README links the architecture spec and the plan.
- `docs/map.md` records the advanced decisions and the research index.
- CI cross-build job (`.github/workflows/ci.yml`) is now the M0 gate: `continue-on-error`
  removed, pinned nightly + `rust-src`, host `cc`, `rust-lld`, `-Zjson-target-spec`, and
  `Swatinem/rust-cache` (`m0-cross`).

### Fixed

- The PS/2 keyboard's commands go to the data port, not the controller command port;
  sending them to the command port left the keyboard's scanning disabled.
- The keyboard's letter case is now right: shift gives the lower case letter and caps
  lock flips the case independently.
- A stray `0xE0` prefix no longer swallows the next key.
- The REX3 needs its `rgbmode` bit set as well as RGB planes, or the engine stays in
  colour-index mode and only the red channel of a fill colour reaches the
  framebuffer.
- The REX3 command FIFO is shallow, so the driver now drains it between primitives
  instead of dropping writes.

- Seeq 8003 receive/transmit over HPC3 PDMA, including descriptor self-references that
  must be taken after the device is in its final location.
- DHCP, NTP and HTTP now share one `Interface`, `SocketSet` and `SeeqDevice`; NTP queries
  the DHCP router instead of the guest's own address.
- HTTP response parsing across packet boundaries and connection-close handling.
- The fetched kernel image is copied to the hand-off address before jumping.
- `kernel-test` `panic_handler` is now `#[cfg(not(test))]` so the host test harness (which
  links std) no longer conflicts with the freestanding `no_std` crate.

## [0.0.0] — 2026-10-20

- Initial repository state: toolchain targets, tracer-bullet scratch code, licence and
  project map.
