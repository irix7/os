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

- The ROM is a **Rust downloader** (M1-ADR-0002). It sizes and maps the DRAM banks,
  tests RAM, brings up the network, fetches `kernel-indy-latest` from GitHub over TLS and
  jumps to it at `0x88040000`. The kernel is not embedded in the PROM.
- Boot needs the network. There is no offline fallback. The kernel is run without an
  integrity check; this is an accepted risk recorded in M1-ADR-0002.
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
  userland contract.  Its Linux/QEMU gate passes TLS, `setjmp` and pthreads.  The
  `irix7-pal` branch is the native port; the kernel now provides the negative-errno
  channel, the 136-byte n32 `stat`, the auxiliary vector and the `$28` TCB, and the
  branch is being brought up against them.
- `irix7-pal` is the source of truth for the native ABI.  A change to the syscall
  gate, `stat`/`dirent` layout, TLS/thread-pointer convention or auxiliary vector
  updates that branch, records its commit in this repository, and lands as one
  change.  This repository holds the build scripts and pinned commits, not a
  divergent copy of the fork.
- The current running user image is a static Rust `std` probe.  It uses the 42-symbol
  `crates/irix7-libc` shim and provisional kernel syscalls, a small console/ramfs fd
  table, a 1 MiB 256 KiB-page heap, and a four-page stack.  It is single-threaded and
  has no process table, threads/TLS, `posix_spawn`, signals, termios or sockets.
- Do not present the bring-up shim as the final libc, or the provisional syscalls as
  the native capability ABI.  `docs/programming/native-abi.md` and `docs/programming/native-libc.md`
  are the contracts; `docs/research/std-hello-bringup.md` records the bring-up.
- The Stage 0 CPU/MMU gate is in place: user mode uses the correct `Status.KSU`
  and `UX` bits, the XTLB vector is installed, user pages are cacheable and
  LL/SC-safe, PageMask uses the R4000 encodings, the trap frame is 64-bit and
  LL is cleared on traps.  Do not regress these in thread or process work.

## Pre-release compatibility policy

There are no deployed users or stable base. Treat every internal interface as **Version 1**
and replace it outright when a better design exists. Do not add compatibility with an
earlier in-tree version, old-peer/old-client fallback, dual-stack paths, format or schema
migration, deprecation aliases, or version negotiation between two ends built from this
repository. Update every in-tree producer/consumer atomically and delete the old path,
tests and docs.

This covers the ROM/kernel handoff and ABIs, syscalls, module/service table, manifests,
package/index and signing formats, mesh transport/join/homes/routes/membership/migration,
MCP wire/API, capability/namespace/CAS/actor/process/IPC interfaces, filesystems and
installer formats, libc/POSIX shims, display/device interfaces, configuration and tooling.
Only safety/security invariants, explicit external standards/hardware requirements, and
the M1/M2/M3 marker evidence are preserved. External IRIX/POSIX compatibility is a product
goal, not a reason to retain our own old versions. See
[`docs/administration/pre-release.md`](docs/administration/pre-release.md).

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
- Rust, n32, MIPS III minimum; the kernel is n32. The ROM is a Rust downloader (M1-ADR-0002).
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

## Kernel image and the PROM budget

- The compressed PROM image is the fixed bootstrap. It holds the TLS stack the ROM uses to
  fetch the kernel from GitHub.
- The kernel image holds every service: MCP, the mesh, the Wasm policy probe and the
  demos. There are no loadable modules and no module loader (ADR M1-0003 step 6). New
  features link into the kernel image.
- Services that run long, such as the MCP control plane, start from the kernel's boot
  path and tick from the idle loop. They use the kernel's TCP slots through the service
  table in `kernel-core::service`.
- The NVRAM environment selects features: `wasm.policy` runs the Wasm probe, `mesh.peer`
  enables the mesh listener, and `mcp.token` sets the bearer token that `POST /mcp`
  requires. NVRAM is a hardware chip, not a distribution channel: the kernel validates it
  at startup and rewrites a blank or corrupt environment and MAC.

## Cache coherency

- `CP0 Config.K0 = 3`: kseg0 is **cacheable**. DMA-visible memory must be accessed through
  the kseg1 alias (uncached) or maintained explicitly. The Seeq device and its descriptors
  and frame buffers now live at kseg1 `0xA860_0000` (physical `0x0860_0000`).
- The CPU framebuffer shadow lives at kseg0 `0x8870_0000` (physical `0x0870_0000`,
  3.75 MiB). It is CPU-only and part of the reserved low window; never hand it to the
  frame allocator.
