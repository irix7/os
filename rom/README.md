# IRIX 7 ROM

The ROM is a freestanding n32/MIPS III firmware image that replaces the factory ARCS PROM
on the SGI Indy. It is linked for the PROM socket and executed directly at `0xBFC00000`;
it is not an ECOFF image loaded by ARCS.

## Boot flow

1. `src/boot/reset.S` starts at `0xBFC00000` with no stack and no writable data.
2. It invalidates the TLB, maps the bank-probe window, initialises the SCC console, then
   calls `memsize.S` to size and map DRAM and `memtest.S` to test every byte of it.
3. Only after the RAM test does it set `$sp` to `0xA8A00000`, copy `.data` from its PROM
   load image to `0xA8700000`, clear `.bss`, and enter Rust.
4. Rust initialises the Seeq 8003 (MAC from the Dallas NVRAM `eaddr` slot), runs DHCP, and
   opens a telnet ROM console on guest port 23. NTP and an HTTP GET then run on the same
   smoltcp interface; the image is copied to `0x88040000` and the kernel is entered.
5. The network stack, clock and telnet session stay live. The kernel calls the resident
   `rom_console_poll` service from its idle path, so the console remains available after
   `kernel: up`.

## Memory layout

| Range | Use |
|---|---|
| `0xBFC00000` | ROM text/rodata, executed in place |
| `0xA8700000` | `.data`/`.bss` + 2 MiB heap + persistent network/console state |
| `0xA8A00000` | initial ROM stack top (grows down) |
| `0x88040000` | kernel load/entry (`0x08040000` physical) |
| `0x88200000` | kernel stack top |
| `0x88300000` | `BootInfo` + memory map (`0x08300000` physical) |
| `0xBFBD9830` | SCC console (`DUART1B`, `tty1`) |
| `0xBFBD4000` | Seeq 8003 registers |
| `0xBFB94000` / `0xBFB96000` | HPC3 ENET RX / TX DMA |

The low 512 KiB of DRAM is a special alias window and is deliberately not used. The
`BootInfo` memory map reserves the first 11 MiB (kernel image, kernel stack, `BootInfo`,
ROM data/heap/stack and the persistent console state); the frame allocator only sees the
remainder.

## Build

```bash
CARGO_TARGET_MIPS3_SGI_IRIX7_N32_RUSTFLAGS="-Clink-arg=-Trom/linker.ld" \
  cargo build -Z build-std=core,alloc -Zjson-target-spec \
  --features cross --target targets/mips3-sgi-irix7-n32.json -p rom

mips-sgi-irix6.5-objcopy -O binary \
  target/mips3-sgi-irix7-n32/release/rom rom.bin
```

The raw `rom.bin` is what the PROM socket and iris `--prom` expect.

## Run

```bash
cd /mnt/europa/iris
./target/release/iris --prom /path/to/rom.bin \
  --bank0 32 --bank1 0 --headless \
  --monitor-port 9998 --serial-log serial.log
```

The serial console is SCC channel B and is exposed on TCP port **8881** (channel A is
8880). `--serial-log` is the simplest way to capture it.

The ROM also opens a telnet console on guest TCP port **23**. iris forwards host
`127.0.0.1:2323` to guest 23 by default:

```bash
telnet 127.0.0.1 2323
```

It opens a styled dashboard with the IRIX 7 logo, a live status card and a `rom ❯` prompt.
The initial window waits for `boot` (or about five seconds with no client); after that the
same session stays live across the kernel hand-off.

Useful commands:

| Command | Purpose |
|---|---|
| `help` | command reference, lipgloss-style table |
| `status` | stage, board, CPU, firmware, IP, MAC, RAM, NTP, uptime |
| `net` | lease, gateway, MAC, DHCP, driver |
| `mem` | tested/reserved/usable RAM |
| `time`, `date` | NTP time in UTC |
| `uptime` | ROM clock |
| `sysinfo` | CP0 PRId, Status, Config and Count |
| `log` | replay the boot log |
| `history` | recent commands |
| `banner`, `clear` | redraw or clear |
| `color true/256/16/off` | colour profile |
| `boot`, `exit` | continue boot or close the session |

The UI is a `no_std` subset of Lip Gloss in `src/gloss.rs`: truecolor gradients, rounded
boxes, styled tables and a colour-profile fallback. The existing Rust ports
(`rusty-lipgloss`, `lipgloss`, `charming-lipgloss`) depend on `std`/libc, so the ROM keeps
the useful subset rather than linking them. It also handles telnet NAWS resizing, command
history (↑/↓), tab completion and Ctrl-C/Ctrl-L/Ctrl-U editing.

## Kernel fetch

`src/main.rs` currently points at a host HTTP server for bring-up:

```rust
const HTTP_SERVER_IP: [u8; 4] = [10, 1, 1, 3];
const HTTP_SERVER_PORT: u16 = 18080;
```

Run any HTTP server serving `/kernel` at that address, e.g.:

```bash
python3 -m http.server 18080 --bind 0.0.0.0
```

The seeq MAC is read from the NVRAM `eaddr` slot that iris seeds (`08:00:69:12:34:56` by
default).

## Status

- Boots from the raw PROM image and tests RAM before creating any RAM structures.
- DHCP, NTP and HTTP work on one network stack; SHA-256 matches the served image.
- The real n32 kernel is linked at `0x88040000`, receives `BootInfo` in `$a0`, uses the
  memory map for its frame allocator, runs with caches enabled and prints `kernel: up`.
- The telnet ROM console is created immediately after DHCP and remains live after the
  kernel boots; the kernel polls the resident network/console service. It has a styled
  dashboard, truecolor/256/16-colour profiles, NAWS sizing, history, tab completion and
  system/network/memory/CP0 views.
- HTTPS/TLS is not implemented yet; the boot specification currently requires only HTTP.
