# Changelog

All notable changes to IRIX 7 are recorded here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the project uses
[Semantic Versioning](https://semver.org/).

## [Unreleased]

### Added

- `rom/tools/mcp-smoke.py` now exercises the module tools and the autoload
  loop: `block.read` target 1 LBA 0 (the `4d436f6d`/`MComprHD` volume header),
  `fs.read /etc/motd`, `fs.list /` (which must include `etc` and `link`), a
  `fs.write`/`fs.read`/`fs.remove` round trip on `/tmp/mcp-smoke`, and a
  lower-image read after the overlay change.  `--erofs-image`/`--scsi2` attach
  the fixture at SCSI ID 2 when the script launches iris, `--skip-fs` covers
  guests without one, and HTTP/JSON-RPC failures now name the failing tool with
  the server's code and message.  `--reset` re-checks `block.read` and
  `fs.read` through the autoloaded module.  Eleven standard-library tests
  drive the script against a fake MCP server.

- SCSI READ(6) data phases now use the HPC3 descriptor DMA engine.  The kernel
  allocates a physically contiguous bounce buffer and a descriptor frame,
  builds a bounded chain of page-sized descriptors with a zero-length `EOX`
  terminator, programs `ndptr`/`bcd`/`ctrl` for a big-endian device-to-memory
  transfer, sets the WD33C93 to burst DMA, and cleans or invalidates the cache
  around the buffer.  Bounded waits, a residual-count check and an explicit
  stop (`FLUSH`, then `ctrl = 0`) keep the channel from being left active;
  setup and transfer failures abort the command and retry the read through the
  existing polled PIO path, which remains for oversized reads and machines
  without working SCSI DMA.  `crates/hpc3` carries the host-tested descriptor
  layout and chain builder (seven tests).  The direction bit convention is
  documented in `docs/research/indy-hardware.md` §5.4.1; the DMA path is not
  guest-verified.
- HTTPS byte ranges on the telnet console: `https <host>:<port><path> range=start-end`
  sends one `Range: bytes=start-end` request through the kernel TLS 1.3 client,
  requires a `206` response whose body is exactly the requested length (a
  `Content-Length` that disagrees is a protocol error), and hashes the bytes as
  they arrive.  The console prints `https: 206 range=start-end N bytes` and the
  body digest, which a host-side `curl | head -c` download can reproduce.
  Certificate verification is unchanged: the same embedded SSL.com TLS ECC Root
  CA 2022 chain, hostname check and signature check run before the request is
  written.  The `erofs` command now accepts an `https://` URL and mounts the
  image through the TLS range device, so a SHA-256-gated image can be read from
  an HTTPS host; each range there is a fresh handshake, and the TLS record
  buffers are not yet reused between fetches.

- A CI `modules` job builds all five loadable modules (`hello`, `ticker`, `block`,
  `httpd`, `erofs`) in release with the pinned nightly, each with its own linker script
  and `build-std` set, flattens the images with `llvm-objcopy` and fails if any exceeds
  the 2 MiB module area. It prints a stable size table and the job summary records it.

- MCP `fs.stat` and `fs.readlink` tools in the `httpd` module.  `fs.stat` reports the
  merged namespace's node type (`file`/`dir`/`symlink`), size, octal mode, ownership and a
  symlink target from the EROFS inode, or the stored length of a RAM-upper entry, without
  following a final symlink; `fs.readlink` returns a link target and rejects a non-symlink.
  The target 2 fixture's `/link` reports as a 9-byte symlink with target `hello.txt`, and
  `/etc/motd` as an 18-byte regular file with mode `100644`.  `overlay::Stat` gains
  `upper`, so a caller can tell a RAM node from a lower node without reading it.  Both
  responses are bounded: a link target is capped at 512 bytes, well inside the module's
  16 KiB response buffer.

- `crates/erofs`: LZ4 compressed-file support for the M4 root image.
  Compressed-full (layout 1) and compressed-compact (layout 3) inodes are
  decoded, including the 2-byte compact index packs, big pclusters with
  `CBLKCNT`, and inline tail pclusters from `ztailpacking`; pclusters handle
  `lz4_0padding` and the plain ("shifted") layout.  The LZ4 block decoder is a
  small `no_std` implementation in the crate with no unsafe code and no new
  dependency.  Unsupported algorithms, fragments, encoded extents and
  interlaced pclusters return `UnsupportedCompression`; malformed headers,
  indexes and streams return `InvalidCompressedData`, so compressed bytes are
  never returned as file data.  Twenty-two new tests (11 integration, 11 LZ4
  and zero-padding unit tests) run against three new reproducible
  `mkfs.erofs -zlz4` fixtures (compact indexes, full/legacy indexes and 16 KiB
  big pclusters); the three uncompressed fixtures are unchanged.  The `lz4`
  feature is on by default; the kernel's HTTP EROFS command disables it because
  the compressed image is already at the PROM budget, while the loadable
  `erofs` and `httpd` modules enable it.  Compressed inodes reached through the
  kernel HTTP command return `UnsupportedLayout` rather than being misread.
- MCP `fs.read` and `fs.list` tools in the `httpd` module: mount the target 2
  EROFS fixture through ABI 10 `block_read`, the `block-cache` LRU and a global
  allocator over `alloc`, then read `/etc/motd` and list `/`.  Verified with
  JSON-RPC against the guest: the motd content and the eight root entries come
  back through the module; no kernel filesystem command is added.  `fs.write`
  and `fs.remove` exercise the RAM overlay (`/tmp` is created at mount), so MCP
  can write and whiteout overlay files while the EROFS lower stays read-only.
- `modules/erofs`: a loadable module that mounts the EROFS fixture on SCSI
  target 2 at LBA 0, reads `/etc/motd` and prints it through the console
  service.  It links `crates/erofs` and `crates/block-cache` by path, reads
  512-byte sectors through the ABI version 10 `block_read` service, and
  supplies its own global allocator over the `alloc` service, and exercises the
  RAM overlay over the read-only lower (`/tmp/motd` write, read and whiteout).
  Moving the filesystem feature into a module keeps it out of the compressed
  PROM image; the kernel's debug `erofsdisk` command is removed in favour of
  this module.
- MCP `block.read` tool in the `httpd` module: reads one 512-byte SCSI sector
  through the ABI 10 `block_read` service and returns 1024 hex characters.
  Verified against target 1 block 0: the response starts `4d436f6d` (`MComprHD`).
  The tool lives entirely in the module; the kernel only supplies the sector
  service.
- Module ABI version 10 adds `block_read(target, lba, buffer)` for one 512-byte
  SCSI sector, exposed by `modules/block` via `Context::block_read`.  The OS
  consumes SDK `v0.3.0`; verified on iris with target 1: the module prints
  `block: magic 0x4d436f6d`.  The block read stays a kernel service, so feature
  modules do not grow the PROM.
- `crates/overlay`: a `no_std` + `alloc` read-only-lower, RAM-upper overlay for
  `/tmp`, `/etc` and `/home` on the EROFS root.  It merges upper files and
  directories over a caller-supplied `Lower`, hides lower entries with whiteouts,
  normalises absolute paths and rejects `..` above the root.  There is no
  copy-up: a write stores a complete upper file.  Nineteen host tests cover lower
  reads, shadowing, whiteouts, directory merge and removal, path normalisation,
  missing and empty files.
- `crates/block-cache`: a `no_std` + `alloc` LRU block cache over a
  byte-addressed `ReadAt` device, with hit/miss counters and no unsafe.  Seven
  host tests cover hits, eviction, cross-block reads, zero-length reads,
  configuration errors and device-error propagation; the block cell will use it
  with the SCSI driver or a larger HTTP block source.
- `crates/erofs`: a `no_std` read-only EROFS reader for the M4 root image.  It
  decodes the standard little-endian format through an explicit byte-order codec:
  the superblock, compact and extended inodes, flat plain and flat inline data,
  directories, symlinks and 48-bit flat block addresses.  Compressed, chunk-based
  and metabox inodes are recognised and rejected with typed errors.  Nineteen host
  tests (sixteen filesystem, three codec) run against reproducible `mkfs.erofs`
  1.9.4 golden images (compact, extended and 48-bit) checked with `fsck.erofs`.
- `crates/erofs`: symlink following and full path traversal.  `lookup_path_follow`
  resolves `.`, `..`, relative and absolute link targets with a bounded follow
  depth of eight (`MAX_SYMLINK_DEPTH`), returning `Error::SymlinkLoop` on a longer
  chain or a self-reference; `lookup_path` remains the no-follow resolver and
  `read_file_path` now follows.  The telnet `erofs` command uses the following
  resolver, so `cat /link` prints the target file and `ls` works on a link to a
  directory.  Four new host tests cover relative, absolute and loop-bounded
  targets and `..`, without changing the golden images: the absolute and loop
  cases rewrite the `/link` target in an in-memory copy of the compact image.
- Telnet `erofs <host>:<port><path> [info|ls|cat] [path]` mounts an EROFS image
  over HTTP Range and prints the superblock, a directory or a file.  It probes
  the superblock to learn the image size, then reads with a 64 KiB read-ahead
  window clamped to the final block, so metadata and inline data stay cached and
  adjacent file blocks arrive together.  Verified on the iris guest with
  `rom/tools/range-server.py`, including an eight-entry `ls /` and the 9000-byte
  flat-inline `big.bin`.  `sha256=<hex>` performs a full GET and aborts before
  mount on a length or digest mismatch; verified with the correct fixture digest
  and an all-zero digest.  SCSI, the shared LRU and 256 KiB read-ahead, and the
  overlay are still to come.
- `scsi` console command: resets the HPC3 SCSI channel and the WD33C93 through
  the external register window, selects target 1, sends IDENTIFY and INQUIRY,
  then reads block 0 with READ(6).  Verified on iris: `csr 0x01`, own ID `0x2f`,
  vendor `SGI`, product `IRIS EMUL DISK`, revision `1.0`, and a volume-header
  magic of `0x4d436f6d` (`MComprHD`).  The indirect data port must be reselected
  as register 0x19 before every data byte; otherwise the identify byte `0x80`
  is written to COMMAND and resets the chip.
- HTTP/1.1 Range requests: `http ... range=start-end` sends `Range`, accepts a 206,
  verifies the body length and hashes the range; the full-GET path is unchanged. TLS
  remains full-body only.
- The module ABI now lives in the separate [IRIX 7 module SDK](https://github.com/irix7/os-sdk)
  (`abi/`); the OS consumes it at tag `v0.2.1`, and the in-tree `modules/abi` copy is gone.
- Link to the separate [IRIX 7 module SDK](https://github.com/irix7/os-sdk), which pins
  the `module-abi` crate and packages the module template, target/linker and build tool.

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

- The overlay now skips the `.` and `..` entries that EROFS returns from a
  directory read, so `fs.list` lists the real children instead of failing with
  `invalid directory entry name`.
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
