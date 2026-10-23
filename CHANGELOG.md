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
- The kernel image is embedded in the PROM image, LZ4-compressed: `rom/build.rs`
  extracts the kernel ELF's loadable segments, compresses them as an LZ4 block,
  and emits them as `.word` directives in a generated `rom/src/boot/payload.S`.
  The ROM stub decompresses them into RAM before jumping. The image is now about
  476 KiB, inside the 512 KiB PROM window.
- Graphical boot menu in the kernel: the REX3 (Newport) display is programmed for
  24-bit RGB and the menu is drawn with the PROM's own IRIS 10 raster font, converted
  from the IRIX source by `kernel/tools/convert_font.py`.
- PS/2 keyboard input in the kernel, with the scancode decoder unit-tested on the host
  (`kernel/tests/kbd.rs`).
- One canonical `targets/mips3-sgi-irix7-n32.json` target for both the ROM and the kernel.
- `rom/linker.ld` and the `rom/src/boot/` reset, memory-sizing and memory-test assembly.
- Research note on Meta CRAM and compressed RAM (`docs/research/compressed-ram.md`);
  compressed RAM is rejected for the Indy in favour of the read-only EROFS root.
- Kernel handoff: the real n32 kernel is linked at `0x88040000` and runs with caches
  enabled. The assembly ROM passes the RAM geometry in `$a1`/`$a2`; there is no `BootInfo`
  structure, and the kernel falls back to the register handoff.
- Telnet console on guest port 23, in the kernel. The ROM is a stub and has no network
  stack of its own.
- Kernel networking: Seeq 8003 over HPC3 PDMA, DHCPv4 with renewals, a DNS resolver,
  NTP wall clock, and ICMP echo (`ping`).
- HTTP/1.1 and HTTPS/TLS 1.3 clients with DNS names, streaming SHA-256 and an optional
  expected digest. TLS verifies chains against an embedded SSL.com TLS ECC Root CA 2022
  and trims a trailing cross-signed root.
- Plain-text telnet console commands: `status`, `net`, `mem`, `time`, `echo`, `ntp`,
  `ping`, `http`, `https`, `module`, `boot`, `reboot`.
- CP0 Compare tick and a `WAIT` idle path; the vCPU parks when the guest is idle.
- Frame-backed bump allocator, with the frame allocator clamped above `0x08B00000` so it
  cannot hand out the exception vector, ROM state, kernel image or stack.
- Loadable modules: flat images linked at `0x88400000`, fetched over HTTPS into RAM,
  SHA-256 checked and called through a versioned C service table. `modules/hello` is the
  one-shot reference module.
- Module ABI version 3: an entry returns a vtable with a status and an optional `tick`
  callback, which the kernel calls from the idle loop at most every 10 ms. The service
  table adds a leak-only heap allocator, a network poll, and two TCP slots with
  listen/state/recv/send/close/abort. The loader zeroes the module area before the
  download so `.bss` starts defined. `modules/ticker` demonstrates a periodic module and
  the allocator; `modules/httpd` serves a status page on guest port 80.
- The ROM flushes the data cache and invalidates the instruction cache before jumping to
  the kernel, so the RAM test cannot leave stale lines in the kernel's text or data.
- The kernel inherits the serial console the ROM configured instead of resetting the SCC;
  the reset raced with the ROM's final characters and intermittently stuck the transmitter.
- The httpd module serves a prototype MCP control plane at `POST /mcp`: JSON-RPC
  `initialize` (version negotiation), `ping`, `tools/list` and `tools/call` with `status`,
  `echo`, `console`, `serial.read`, `system.reset` and `system.poweroff`. `GET /` remains a
  plain status page. The kernel keeps an 8 KiB serial ring (ABI version 4 services
  `console_read`/`console_total`); version 5 adds deferred `reset` and `poweroff`.
- The DS1386 real-time clock is read at boot and seeds the wall clock (the two BCD year
  digits count from 1940), so `status` reports the correct date before any NTP query.
- MCP resources: `resources/list`/`resources/read` expose `irix7://serial`, `status`,
  `net` and `mem` as text, so an agent can read the guest log without a tool call.
- MCP `screen.capture` returns an inline 320×200 1-bit PNG thumbnail (stored-deflate,
  base64) alongside the full BMP URL; the module transmit/response buffers grow to 16 KiB.
- A CPU shadow of the REX3 framebuffer (BGR, 1280×1024, kseg0 `0x88700000`) is kept in
  step by the drawing primitives. ABI version 9 exposes `screen_read`, and the httpd
  module streams the shadow as a 24-bit BMP at `GET /screen.bmp`; MCP `screen.capture`
  returns the URL.
- ABI version 8 adds one `info` service (status/net/mem/time), shared by the telnet
  console and the MCP `shell.exec` command.
- `rom/tools/mcp-smoke.py` checks the live MCP endpoint (`initialize`, `tools/list`,
  `status`, `shell.exec`, `serial.read`, `echo`) and can prove the reset/autoload loop.
- The MCP module gains `shell.exec` with `echo`, `uname`, `uptime`, `status`, `help`,
  `reboot` and `poweroff`; `ls`, `cat` and `ps` report that they need the future filesystem
  and process work.
- MCP bearer authentication: the module reads `mcp.token` from the NVRAM environment
  (ABI version 7 service `config_get`); `POST /mcp` then requires
  `Authorization: Bearer <token>` and returns 401 with `WWW-Authenticate` otherwise.
- `rom/tools/mcp-smoke.py`: starts the rig (or attaches with `--no-launch`), checks the
  MCP initialize/tools/status/shell/serial/echo surface, and with `--reset` verifies the
  reset → autoload → MCP loop.
- ABI version 8 adds the `info(kind)` service (status, net, mem, time); `shell.exec`
  exposes `net`, `mem` and `time` alongside the existing commands.
- The module ABI is a shared crate (`modules/abi`) linked by the kernel and every module,
  with compile-time offset assertions for the n32 target.  This corrected the documented
  layout: `clock_ms` is a 4-byte function pointer, so `alloc` starts at 0x0C and the table
  is 0x44 bytes.
- Boot-time module autoload: the kernel reads `mcp.url` and `mcp.sha256` from the DS1386
  PROM environment, fetches the module over HTTPS (explicit `http://` for the rig), verifies
  the digest and runs it before the boot menu. The control plane therefore survives a
  reset without a telnet step.
- A shared machine-status formatter now backs both the telnet `status` command and the MCP
  `status` tool (ABI version 6 service `status`): IP, MAC, RAM, uptime and date from one
  place.
- The SCC status read now has a settle delay after the register-select write and a bounded
  instruction spin.  The kernel's cached polling loop could read the previous register
  (RR8) through the IOC and spin forever; the ROM's uncached fetch had hidden it.  The
  spin fallback also stops depending on CP0 Count before the timer is initialised.
- Boot-menu status line with the live address, and a diagnostics action that prints CPU,
  RAM and network state.
- `rom/tools/smoke.py`: starts iris, releases the CPU, waits for DHCP and drives the
  telnet console.
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

- The LZ4 match-length extension was read before the match distance; the sequence
  is distance first.
- The big-endian size and magic fields in the compressed payload were assembled
  with the shifts the wrong way round.
- The ROM's PROM image is laid out by virtual address, not by concatenating the
  sections: the linker leaves an alignment gap that would otherwise shift the
  payload.
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
- DHCP, DNS, NTP, HTTP and TLS share one `Interface`, `SocketSet` and `SeeqDevice`.
- HTTP body counting across receive batches: the first batch is usually larger than the
  header buffer, and bytes beyond it still count as body.
- TLS treats a bare TCP close (no close_notify) as end of response.
- `kernel-test` `panic_handler` is now `#[cfg(not(test))]` so the host test harness (which
  links std) no longer conflicts with the freestanding `no_std` crate.

## [0.0.0] — 2026-10-20

- Initial repository state: toolchain targets, tracer-bullet scratch code, licence and
  project map.
