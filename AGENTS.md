# IRIX 7 agent notes

## One system, two link addresses

The ROM and the kernel are **one program**, split across two load addresses:

- ROM: `0xBFC00000` (PROM window).
- Kernel: `0x88040000` (kseg0 DRAM).

There is no BIOS/firmware boundary between them. The kernel must use the objects and state
the ROM initialised, in place, as native parts of the same system. It must not re-create
the DRAM geometry, memory-controller mapping or serial console the ROM set up. The kernel
initialises the network stack, timers, graphical console and telnet console once; those
were not ROM state. Calls between the two address ranges are normal intra-program calls.

Consequences:

- The ROM is a **pure MIPS3 assembly stub**. It has no Rust in it at all. Everything it
  does is: size and map the DRAM banks, test every byte of RAM, bring up the serial
  console, LZ4-decompress the kernel image from PROM to `0x88040000`, and jump there.
- The kernel image is embedded in the PROM image by `rom/build.rs`, which extracts the
  kernel's loadable segments, compresses them as an LZ4 block, and emits them as
  `rom/src/boot/payload.S`. The payload layout is
  `[4 bytes magic 0x184D2204][4 bytes size, big-endian][LZ4 block]`.
- **The assembler renumbers `$t0`-`$t3`** in the ROM's assembly. Two source registers
  that differ only in that range can end up as the same machine register, so the
  decompressor's temporaries live in `$a0`-`$a3`, `$v0` and `$s5`-`$s7` instead.
- The ROM hands over the RAM geometry it discovered in registers. The kernel uses it to
  size its frame allocator; there is no shared `BootInfo` structure yet, so the kernel
  falls back to the register handoff when `BootInfo` is absent.
- The telnet console lives in the **kernel**. The ROM's only console is the serial
  line, and it stops being used once the kernel has taken over.

## Userland and POSIX

- The production C/POSIX layer is the clean-room [relibc fork](https://github.com/irix7/relibc)
  on `irix7-n32`: n32, big-endian, 64-bit `time_t`/`off_t`, and POSIX.1-2024 as the
  userland contract.  Its Linux/QEMU gate passes TLS, `setjmp` and pthreads; the Linux
  syscall backend still has to become the IRIX PAL.
- The current running user image is a static Rust `std` probe.  It uses the 42-symbol
  `crates/irix7-libc` shim and provisional kernel syscalls, a small console/ramfs fd
  table, a 1 MiB 256 KiB-page heap, and a four-page stack.  It is single-threaded and
  has no process table, threads/TLS, `posix_spawn`, signals, termios or sockets.
- Do not present the bring-up shim as the final libc, or the provisional syscalls as
  the native capability ABI.  `docs/spec/native-abi.md` and `docs/spec/native-libc.md`
  are the contracts; `docs/research/std-hello-bringup.md` records the bring-up.

## Other standing requirements

- **Rust-first components.** Prefer a Rust implementation for every new
  component, tool and dependency. If a non-Rust option is proposed, find a
  maintained Rust equivalent first; if none exists, develop one in Rust.
  Porting an existing Rust project is preferred over binding a non-Rust one.
- **Dependencies are work items, not disqualifiers.** Do not reject a component
  merely because it has a large dependency tree. For every dependency, record
  its licence, target/platform assumptions, build requirements and port or
  replacement strategy. Prefer implementing the missing OS service or a
  Rust-native replacement over dropping the feature. Only defer a dependency
  when it genuinely belongs to a later layer, and say which layer.
- Rust, n32, MIPS III minimum; the kernel is n32. The ROM is assembly.
- The ROM executes from PROM space, not RAM.
- RAM tests run before any RAM structures exist.
- Use Australian English in prose.

## Machine specifics worth remembering

- **DRAM is not mapped at reset.** The memory controller's `MEMCFG0`/`MEMCFG1` are zero,
  so every DRAM access faults until the ROM programs them. The ROM sizes the banks by
  probing them through a TLB window at physical `0x2000_0000`, then maps them at
  `0x0800_0000` (the `LOMEM_BASE` the Indy's memory controller uses).
- **`0x0800_0000` is the alias.** Physical `0x0000_0000` is aliased to
  `LOMEM_BASE + 0`; writing there lands in DRAM.
- **The SCC is in the IOC**, not the HPC3. The serial console is the Z85C30 at kseg1
  `0xBFBD_9830` (channel B), reached through the IOC's PIO window. Register access is
  indirect: write the register number to the control port, then the data through the
  same port.
- **The PS/2 controller is also in the IOC** (offsets `0x40` data, `0x44` command).
  The *keyboard's* commands go to the data port; the command port is the controller's.
- **The REX3 needs its `rgbmode` bit** (DRAWMODE1 bit 15) set as well as RGB planes, or
  the engine stays in colour-index mode and only the red channel of a fill colour
  reaches the framebuffer.
- **The REX3 command FIFO is shallow.** A driver has to drain it between primitives or
  the writes are dropped; `gfx.rs` polls `STATUS` the way the SGI diagnostics'
  `REX3WAIT(REX)` does.

## Modules and the PROM budget

- The compressed PROM image is the fixed bootstrap. TLS stays in it: it is the transport
  that makes module downloads trustworthy. New features go into modules, not into the
  compressed image.
- A module is a flat image linked at `0x88400000`, fetched over HTTPS into RAM, SHA-256
  checked, and called through the versioned C service table.  The table's layout lives in
  the `abi/` crate of `irix7/os-sdk`, pinned by the `v0.3.0` tag; the kernel and every module
  link it, and compile-time assertions pin the n32 offsets.  Never hand-maintain a second
  copy of the struct.
  A module may return a tick callback and use the table's TCP slots; long-running
  features (servers, the MCP control plane) belong in modules, not in the core.
- The module area is inside the reserved low window. Never hand it to the frame allocator.
- Boot autoload is configured by `boot/config.conf` in the repository, fetched at boot from
  `https://raw.githubusercontent.com/irix7/os/main/boot/config.conf`. A release workflow
  updates that file with immutable raw module URLs and SHA-256 digests. A local test rig
  may override the same keys through the DS1386 environment (`mcp.url`, `mcp.sha256`) at
  register offset `0x10` (one byte per 32-bit word in the low lane). A failed fetch or
  digest records `autoload: ...`, the boot continues with a `BOOT-NOT-READY` marker,
  and no unverified module runs. The module reads the optional
  `mcp.token` through the ABI v7 `config_get` service and requires a bearer token on
  `POST /mcp` when it is set. NVRAM is a hardware chip, not a distribution channel: the
  kernel validates it at startup and rewrites a blank or corrupt environment and MAC.

## Cache coherency

- `CP0 Config.K0 = 3`: kseg0 is **cacheable**. DMA-visible memory must be accessed through
  the kseg1 alias (uncached) or maintained explicitly. The Seeq device and its descriptors
  and frame buffers now live at kseg1 `0xA860_0000` (physical `0x0860_0000`), immediately
  after the 2 MiB module area.
- Code written into RAM and executed (modules) needs a data-cache writeback and an
  instruction-cache invalidate before the call. A compiler fence is not enough on real
  hardware.
- The CPU framebuffer shadow lives at kseg0 `0x8870_0000` (physical `0x0870_0000`,
  3.75 MiB). It is CPU-only and part of the reserved low window; never hand it to the
  frame allocator.
