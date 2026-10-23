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
6. It LZ4-decompresses the kernel image out of PROM to `0x0804_0000` (kseg1, uncached,
   so the bytes are in memory before the kernel runs from the cached kseg0 alias).
7. It sets the boot flag and jumps to `0x8804_0000`.

The kernel clears its own `.bss` on entry — the RAM test scribbled over all of DRAM
before the kernel image was copied in.

## The kernel image

The kernel is compressed into the PROM image. `rom/build.rs`:

1. reads the linked kernel ELF;
2. extracts its `PT_LOAD` segments into a flat binary laid out at their physical
   addresses;
3. compresses it as an LZ4 block (RFC 7857);
4. emits it as `.word` directives in `src/boot/payload.S` (generated; not in
   git), which the linker places in a `.payload` section after the boot code.

The payload is `[4 bytes magic 0x184D2204][4 bytes size, big-endian][LZ4 block]`.
The stub's `lz4_decompress` reads the header, then walks the block's sequences:
a token byte (high nibble literal length, low nibble match length minus 4), the
literals, a two-byte little-endian distance, and the match.  Lengths of 15 or
more continue in extra bytes worth 255 each until one is smaller.  The block ends
when the uncompressed size has been produced; the last sequence is literals only.

`ROM_PAYLOAD_LEN` is computed at link time from the section symbols, so the ROM always
copies exactly the right number of bytes.

The kernel is now about 1 MiB of text and data (the TLS client and its verifier
dominate), which compresses to roughly 460 KiB. The PROM image is about 473 KiB,
inside the 512 KiB PROM window but with little headroom.

## Memory layout

| Range | Use |
|---|---|
| `0xBFC00000` | ROM text/rodata, executed in place |
| `0xBFC00000`+ | kernel payload (compressed or not), copied to RAM at boot |
| `0x08040000` | kernel image (`0x88040000` kseg0) |
| `0x08200000` | kernel stack top (`0x88200000` kseg0) |
| `0x08400000` | module load area, 2 MiB (`0x88400000` kseg0) |
| `0x08600000` | Seeq device DMA descriptors and buffers (`0xA8600000` kseg1, uncached) |
| `0x08B00000` | first frame allocator page |
| `0x08000000` | DRAM base (`LOMEM_BASE`); physical `0x00000000` is an alias for it |

The low 512 KiB of physical address space is a special alias window: physical
`0x0000_0000` forwards to `LOMEM_BASE + 0`, so kseg1 `0xA000_0000` is the first
usable DRAM.

## Build

The kernel must be built first — the ROM embeds it:

```bash
# 1. the kernel
CARGO_TARGET_MIPS3_SGI_IRIX7_N32_RUSTFLAGS="-Clink-arg=-Tkernel/linker.ld" \
  cargo build --release -Z build-std=core,alloc -Zjson-target-spec \
  --features cross --target targets/mips3-sgi-irix7-n32.json -p kernel

# 2. the ROM, with the kernel embedded
CARGO_TARGET_MIPS3_SGI_IRIX7_N32_RUSTFLAGS="-Clink-arg=-Trom/linker.ld" \
  cargo build --release -Z build-std=core,alloc -Zjson-target-spec \
  --features cross --target targets/mips3-sgi-irix7-n32.json -p rom

# 3. the raw PROM image
python3 rom/tools/make_prom.py \
  target/mips3-sgi-irix7-n32/release/rom rom.bin
```

`rom/tools/make_prom.py` lays the sections out by *virtual* address, not by
concatenating them: the linker leaves an alignment gap between the boot code
and the payload, and shifting the payload by that gap would corrupt it.

The raw `rom.bin` is what the PROM socket and iris `--prom` expect.

## Run

```bash
cd /mnt/europa/iris
./target/release/iris --prom /path/to/rom.bin \
  --bank0 32 --bank1 0 \
  --monitor-port 9998 --serial-log serial.log
```

The serial console is SCC channel B and is exposed on TCP port **8881** (channel A is
8880). `--serial-log` is the simplest way to capture it.

The kernel puts a **graphical boot menu** on the REX3 (Newport) display: a framed
panel with a title, a list of boot options and a status line, drawn with the PROM's
own IRIS 10 raster font. The PS/2 keyboard drives it — arrow keys move the
selection, a number selects that option, and Enter boots the highlighted one.

iris has to program the video mode, because the ROM stub does not: the replacement
kernel draws through the real REX3 pipeline but the VC2 timing tables come from the
host. `rom/iris-rig.toml` has the settings:

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

`rom/tools/smoke.py` starts iris, releases the CPU, waits for DHCP and drives the
telnet console:

```bash
export LD_LIBRARY_PATH=/nix/store/...-libglvnd/lib:/nix/store/...-alsa-lib/lib:/nix/store/...-gcc-lib/lib
python3 rom/tools/smoke.py \
  --iris /path/to/iris --prom /tmp/rom.bin \
  --cwd /mnt/europa/iris --commands "status,mem,ping 10.1.1.3"
```

It is not part of CI (it needs iris and the disk images); it is the repeatable
version of the manual checks in this document.

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
| `http <host>:<port><path> [sha256]` | fetch over HTTP, optionally verify |
| `https <host>:<port><path> [sha256]` | fetch over TLS 1.3, optionally verify |
| `boot` | leave the menu and continue booting |
| `reboot` | restart from the PROM |

`http` and `https` print the status, body length and a streaming SHA-256 of the body,
so a fetched image can be checked against the build host.  The clock is advanced from
CP0 Count; the menu and idle loops execute `WAIT`, which lets iris park the vCPU.

## Loadable modules

The compressed PROM image is a bootstrap, not the place for every feature. Code
that is not needed to boot can be a **module**: a flat image linked at
`0x88400000` (in the reserved low window, above the kernel stack), downloaded
into RAM, SHA-256 checked, and called through a versioned C service table
(`Services` in `kernel/src/module.rs`).

TLS stays in the core: it is the transport that makes module downloads
trustworthy. `module` uses HTTPS unless the argument starts with `http://`, and
the image is only executed after its digest matches. Any new feature should be a
module, not a larger PROM image.

Modules use ABI version 2: the entry returns a `ModuleVtable` with a status and
an optional `tick` callback, which the kernel calls from the idle loop at most
every 10 ms. The service table adds `alloc` (the kernel heap, never freed) and
`poll` (drive the network stack). `modules/hello` is the one-shot reference;
`modules/ticker` is the long-running reference: it allocates 32 bytes, then
prints `ticker: tick` once a second for five seconds before unregistering.

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
The loader zeroes the module area before the download, so a module's `.bss`
starts at zero; it flushes the data cache and invalidates the instruction cache
before the entry call.

## Status

- Boots from the raw PROM image, sizes and maps DRAM, and tests every byte of it
  before creating any RAM structures.
- The real n32 kernel is linked at `0x88040000`, is copied there by the ROM, and
  prints `kernel: up`.
- The kernel brings up the REX3 and shows a graphical boot menu, and reads the
  PS/2 keyboard to drive it.
- The kernel payload is LZ4-compressed and decompressed by the ROM stub.
- The ROM has no network stack; the kernel owns DHCP, DNS, NTP, HTTP and HTTPS
  (TLS 1.3), the telnet console, and the Seeq/HPC3 driver.
- HTTPS verifies server chains against the embedded SSL.com TLS ECC Root CA 2022
  (`kernel/src/ca/`); chains that end elsewhere are rejected.
- A CP0 Compare tick drives the network and console; idle loops park in `WAIT`.
