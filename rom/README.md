# IRIX 7 ROM

The ROM is a **MIPS III assembly stub** that replaces the factory ARCS PROM on the SGI
Indy. It is linked for the PROM socket and executed directly at `0xBFC00000`; it is not an
ECOFF image loaded by ARCS.

There is no Rust in the ROM. `src/main.rs` is a stub that pulls the assembly in and
provides the `no_std` panic handler; everything the ROM does is assembly.

## Boot flow

1. `src/boot/reset.S` starts at `0xBFC00000` with no stack and no writable data.
2. It sets up CP0 (interrupts off) and invalidates the TLB.
3. It maps a probe window (VA `0x2000_0000`..`0x4000_0000`, uncached) so the DRAM
   banks can be sized, then calls `memsize.S` to program the memory controller and map
   them at `0x0800_0000`.
4. It calls `memtest.S` to test every byte of DRAM. Still stackless, still with
   nothing of its own in RAM.
5. It initialises the SCC console (the Z85C30 in the IOC) and prints the boot banner.
6. The network stage (`rom/src/netstage.rs`, `rom/src/fetch.rs`) downloads
   `kernel-indy-latest.bin` over HTTPS to `0x0804_0000` (kseg1 `0xA804_0000`,
   uncached).  It follows redirects from `releases/latest/download`.  It returns 0
   once the kernel is in RAM, or 1.
7. On 0, the stub flushes the caches and jumps to `0x8804_0000`.  On 1, it prints
   `BOOT-NOT-READY` and halts.

The kernel clears its own `.bss` on entry — the RAM test scribbled over all of DRAM
before the kernel image was copied in.

## The kernel download

The kernel is not in the PROM.  The ROM fetches
`https://github.com/irix7/os/releases/latest/download/kernel-indy-latest.bin` at
every boot (M1-ADR-0002).  The release workflow publishes that asset under a fixed
name.  GitHub answers with a 302 to `release-assets.githubusercontent.com`, which
the stage follows.

- TLS 1.3 uses the embedded roots in `rom/src/ca/`: SSL.com ECC, USERTrust ECC and
  ISRG Root X1.  The root is chosen from the issuer of the server chain's last
  certificate.
- The ROM has no wall clock, so certificate validity dates are not checked.  The
  chain signatures and the host name are.
- The body is not hashed or signed.  The owner accepted this in M1-ADR-0002.
- A download larger than 3 MiB fails with `download larger than the kernel window`.
- The heap for TLS records and sockets is 768 KiB at `0x08340000`, with its cursor
  in the heap's last word.

The PROM image is about 470 KiB at `opt-level=z` with one codegen unit.  At
`opt-level=2` the ROM text alone is about 650 KiB and does not fit.

## Memory layout

| Range | Use |
|---|---|
| `0xBFC00000` | ROM text/rodata, executed in place |
| `0x08040000` | kernel image, downloaded (`0x88040000` kseg0); 3 MiB window |
| `0x08340000` | ROM heap, 768 KiB (TLS records, sockets) |
| `0x08300000` | kernel stack top (`0x88300000` kseg0) |
| `0x08400000` | module load area, 2 MiB (`0x88400000` kseg0) |
| `0x08600000` | Seeq device DMA descriptors and buffers (`0xA8600000` kseg1, uncached) |
| `0x08700000` | framebuffer shadow, 1280×1024 BGR (`0x88700000` kseg0, CPU-only) |
| `0x08B00000` | first frame allocator page |
| `0x08000000` | DRAM base (`LOMEM_BASE`); physical `0x00000000` is an alias for it |

The low 512 KiB of physical address space is a special alias window: physical
`0x0000_0000` forwards to `LOMEM_BASE + 0`, so kseg1 `0xA000_0000` is the first
usable DRAM.

## Build

The ROM does not depend on the kernel.  Build the ROM at `opt-level=z` with one
codegen unit, or the image is too large for the PROM window:

```bash
# 1. the ROM
CARGO_PROFILE_RELEASE_OPT_LEVEL=z CARGO_PROFILE_RELEASE_CODEGEN_UNITS=1 \
  CARGO_TARGET_MIPS3_SGI_IRIX7_N32_RUSTFLAGS="-Clink-arg=-Trom/linker.ld" \
  cargo build --release -Z build-std=core,alloc -Zjson-target-spec \
  --features cross --target targets/mips3-sgi-irix7-n32.json -p rom

# 2. the raw PROM image
python3 rom/tools/make_prom.py \
  target/mips3-sgi-irix7-n32/release/rom rom.bin
```

`rom/tools/make_prom.py` lays out the `.text` and `.rodata` sections by *virtual*
address.  It fails if they do not fit the 524,288-byte window.

The raw `rom.bin` is what the PROM socket and iris `--prom` expect.

## Run

```bash
cd /path/to/iris
./target/launcher/bin/iris --prom /path/to/rom.bin \
  --bank0 32 --bank1 0 \
  --monitor-port 9998 --serial-log serial.log
```

Use iris `v0.1.13+irix7` or later (CI pins that release). The launcher carries the flake's runtime
library path, so it needs no `LD_LIBRARY_PATH`; build it once with
`nix develop --command cargo build --release --features lightning,gpujit`, which installs
`target/launcher/bin/iris` alongside the raw `target/release/iris`.

The serial console is SCC channel B and is exposed on TCP port **8881** (channel A is
8880). `--serial-log` is the simplest way to capture it.

The kernel paints the boot log full-screen on the REX3 (Newport) display using
the PROM's own IRIS 10 raster font.  There is no boot menu: after the readiness
marker the kernel enters the native n32 image on SCSI target 2.  The PS/2 keyboard
is initialised for the local console, while diagnostics moved to the telnet
console.

The kernel programs the 1280x1024 VC2 timing table, XMAP9 RGB24 mode and an
identity Bt445 gamma table itself.  The host preset still keeps the emulator
surface and the earliest ROM output at a known size.  `rom/iris-rig.toml` has
the settings:

```toml
[graphics]
board = "newport"
heads = 1
resolution = "1280x1024"
```

and run iris **without** `--headless`: headless mode turns the Newport off
entirely, and there is then no graphics device for the kernel to draw on. In `--ci`
mode the window is hidden but the REX3 keeps rendering to an offscreen buffer, which
`rex fbdump` can then write out as a PNG.

## Scripted smoke test

`rom/tools/smoke.py` starts iris, releases the CPU, waits for the kernel's
`BOOT-READY` marker and drives the
telnet console:

```bash
python3 rom/tools/smoke.py \
  --iris /path/to/iris/target/launcher/bin/iris --prom /tmp/rom.bin \
  --cwd /path/to/iris --commands "status,mem,ping 10.1.1.3"
```

It is not part of CI (it needs iris and the disk images); it is the repeatable
version of the manual checks in this document.  Each run gets a fresh temporary
work directory and log; pass `--work-dir` or `--keep-artifacts` to retain them.
`rom/tools/mcp-smoke.py` does the same with `--run-dir` / `--keep-artifacts`.

## Kernel console

The kernel owns the Seeq 8003 (HPC3 PDMA) and the smoltcp stack, so the console is
available over the network once DHCP completes. `rom/iris-rig.toml` forwards host
`127.0.0.1:2323` to guest port 23:

```bash
telnet 127.0.0.1 2323
```

Commands:

| Command | Purpose |
|---|---|
| `help` | list commands |
| `status` | stage, IP, MAC, RAM, uptime, wall clock |
| `net` | interface addresses and link |
| `mem` | total and free RAM |
| `time` | uptime and UTC date |
| `echo <text>` | echo the argument |
| `ntp <host>[:port]` | set the wall clock from NTP |
| `ping <host>` | ICMP echo (three requests) |
| `module [https://]<host>:<port><path> <sha256>` | fetch, verify and run a RAM module |
| `http <host>:<port><path> [sha256] [range=start-end]` | fetch over HTTP, optionally verify or range |
| `https <host>:<port><path> [sha256]` | fetch over TLS 1.3, optionally verify (no range) |
| `boot` | retry the target-2 userland image |
| `reboot` | restart from the PROM |

`http` and `https` print the status, body length and a streaming SHA-256 of the body,
so a fetched image can be checked against the build host.  The clock is advanced from
CP0 Count; the console and idle loops execute `WAIT`, which lets iris park the vCPU.

## Loadable modules

The compressed PROM image is a bootstrap, not the place for every feature. Code
that is not needed to boot can be a **module**: a flat image linked at
`0x88400000` (in the reserved low window, above the kernel stack), downloaded
into RAM, SHA-256 checked, and called through a versioned C service table.
The table's layout is the shared `module-abi` crate in `irix7/os-sdk`'s
`abi/` directory, pinned by the `v0.2.1` tag and linked by the kernel and every
module.

TLS stays in the core: it is the transport that makes module downloads
trustworthy. `module` uses HTTPS unless the argument starts with `http://`, and
the image is only executed after its digest matches. Any new feature should be a
module, not a larger PROM image.

Modules use ABI version 3: the entry returns a `ModuleVtable` with a status and
an optional `tick` callback, which the kernel calls from the idle loop at most
every 10 ms. The service table adds `alloc` (the kernel heap, never freed),
`poll` (drive the network stack), and two TCP slots with listen/state/recv/send/
close/abort. `modules/hello` is the one-shot reference; `modules/ticker` is the
periodic reference; `modules/mcp` is the TCP reference: it listens on guest
port 80, answers one request per connection with its uptime, and alternates its
two slots so back-to-back clients do not hit the close handshake.

Build a module and serve the flat image (swap `hello` for `ticker` for the
periodic example):

```bash
CARGO_TARGET_MIPS3_SGI_IRIX7_N32_RUSTFLAGS="-Clink-arg=-Tmodules/hello/linker.ld" \
  cargo build --release -Z build-std=core -Zjson-target-spec \
  --target targets/mips3-sgi-irix7-n32.json -p hello-module

$(rustc --print sysroot)/lib/rustlib/$(rustc --print host)/bin/llvm-objcopy \
  -O binary target/mips3-sgi-irix7-n32/release/hello-module hello.module
```

Then in the console (with `hello.module` on the HTTP server; the explicit
`http://` is only for the local test rig):

```text
module http://10.1.1.3:18080/hello.module <sha256 of hello.module>
hello from a downloaded module
module: returned 0
```

Without the scheme the command uses HTTPS, which is the normal module path.

### Boot autoload

The kernel reads `mcp.url` and `mcp.sha256` from the DS1386 PROM environment after DHCP
and DNS, before the boot handoff. The environment starts at register offset `0x10` (the RTC
registers occupy `0x00`–`0x0F`), one byte per 32-bit word in the low lane. For the iris
rig, stop the emulator and write both strings into `nvram.bin` at offset `0x10`:

```text
mcp.url=http://10.1.1.3:18080/mcp.module
mcp.sha256=<64 hex digits of mcp.module>
mcp.token=<shared secret>          (optional; requires Authorization: Bearer)
```

On the next boot the serial log shows `autoload: fetching module`, the module prints
`mcp: listening on port 80`, and the MCP endpoint is live without telnet. A missing or
wrong digest prints `autoload: ...` and the boot continues with a
`BOOT-NOT-READY` marker.

### Screen capture

`GET /screen.bmp` streams the 1280×1024 24-bit BMP from the kernel's CPU shadow of the
REX3 output. It is about 3.75 MiB and takes roughly 25 s at 10 Mbit. The MCP
`screen.capture` tool returns a 320×200 1-bit PNG thumbnail inline (via `image/png`) and
the full BMP URL.

### Testing the MCP endpoint

`rom/tools/mcp-smoke.py` drives the JSON-RPC surface, including the module's
`block.read` and EROFS `fs.*` tools; see [MCP smoke test](#mcp-smoke-test) below
for the command and the checks.
The loader zeroes the module area before the download, so a module's `.bss`
starts at zero; it flushes the data cache and invalidates the instruction cache
before the entry call.

The HTTP module is reachable through the rig's host port 8088:

```bash
curl -s http://127.0.0.1:8088/
curl -s -X POST -H 'Content-Type: application/json' \
  -d '{"jsonrpc":"2.0","id":1,"method":"tools/list"}' \
  http://127.0.0.1:8088/mcp
```

`POST /mcp` speaks JSON-RPC 2.0: `initialize`, `ping`, `tools/list` and
`tools/call` (`status`, `echo`, `console`, `serial.read`, `system.reset`, `system.poweroff`, `shell.exec`;
the shell accepts `net`, `mem` and `time`; `screen.capture` returns the BMP endpoint).
`shell.exec` accepts `echo`, `uname`, `uptime`, `status`, `net`, `mem`, `time`, `help`,
`reboot` and `poweroff`.
It is a prototype: no bearer token or SSE yet. `serial.read` returns the kernel's
8 KiB serial ring.

## MCP smoke test

`rom/tools/mcp-smoke.py` checks the live control plane: `initialize`, `tools/list`,
`status`, `shell.exec uname`, `serial.read`, `echo`, `block.read` on target 1, and
the EROFS `fs.read`, `fs.list`, `fs.stat`, `fs.readlink`, `fs.write` and
`fs.remove` tools.  It reads `/etc/motd`, lists `/` (which must include `etc` and
`link`), and checks the fixture's metadata: `/` is a directory from `erofs`,
`/link` is a symlink to `hello.txt` (and `fs.readlink /link` returns that
target), `/etc/motd` is an 18-byte file with mode `100644`, and `fs.readlink /etc/motd`
fails as not a symlink.  The overlay round trip writes
`/tmp/mcp-smoke`, stats it as `overlay`, reads it back, removes it, confirms the
read now fails and that `/etc/motd` still comes from the read-only lower image.

Against an already-running guest whose SCSI target 2 holds the fixture:

```bash
rom/tools/mcp-smoke.py --no-launch --token secret-token
```

To start and stop its own guest, give it `--iris`, `--prom` and either
`--erofs-image` (the fixture to attach at SCSI ID 2) or `--scsi2` (a raw image):

```bash
rom/tools/mcp-smoke.py --iris /path/to/iris/target/launcher/bin/iris \
  --prom /tmp/rom.bin --cwd /path/to/iris \
  --erofs-image crates/erofs/tests/fixtures/root.erofs \
  --token secret-token
```

`--erofs-image` and `--scsi2` are resolved against the caller's working
directory, not `--cwd`.  The guest also needs SCSI target 1 (`scsi1.raw`) for
`block.read`; the rig configuration supplies it.  When the script launches iris
without either option it skips the `fs.*` checks; `--skip-fs` skips them for an
already-running guest that has no target 2.  `--skip-block` skips the
`block.read` checks (before and after a reset) for a diskless guest with no
target 1; when the script launches iris, give `--config` a rig configuration
that does not attach `scsi1.raw`.

A diskless guest therefore needs only the module and its network path:

```bash
rom/tools/mcp-smoke.py --no-launch --skip-block --skip-fs --token secret-token
```

With `--reset` it also calls `system.reset`, waits for the reboot and autoload,
and checks `status`, `block.read` (unless `--skip-block`) and the filesystem
tools on the new instance; that path needs NVRAM autoload.  To load the module
through the telnet console instead, pass `--module-url` and `--module-sha256`.
The token must match `mcp.token`.

The script's own logic runs without a guest against a fake MCP endpoint:

```bash
python3 -m unittest discover -s rom/tools -p 'test_*.py'
```

## Status

- Boots from the raw PROM image, sizes and maps DRAM, and tests every byte of it
  before creating any RAM structures.
- The real n32 kernel is linked at `0x88040000`, is copied there by the ROM, and
  prints `kernel: up`.
- The kernel brings up the REX3 and paints the boot log full-screen.  A
  telnet-visible `tlb`, `gfx`, `trap` and user smoke path exercise the TLB refill,
  REX3 RGB24/FIFO contract and user exit; the automatic handoff runs the std
  bring-up image from SCSI target 2.
- After RTC, network and MCP autoload, the kernel emits one
  `BOOT-READY v=1 boot=<id> kernel=up network=up mcp=listening` line (or a
  distinct `BOOT-NOT-READY` line with a reason).  Test tools poll this marker.
- The ROM downloads the kernel at boot; see "The kernel download".
- The ROM brings up the Seeq and takes a DHCP lease, for the download only.  The
  kernel owns DHCP, DNS, NTP, HTTP and HTTPS (TLS 1.3), the telnet console, and
  the Seeq/HPC3 driver after the handoff.
- HTTPS verifies server chains against the embedded SSL.com TLS ECC Root CA 2022
  (`kernel/src/ca/`); chains that end elsewhere are rejected.
- A CP0 Compare tick drives the network and console; idle loops park in `WAIT`.
