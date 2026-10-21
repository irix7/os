# IRIX 7 agent notes

## One system, two link addresses

The ROM and the kernel are **one program**, split across two load addresses:

- ROM: `0xBFC00000` (PROM window).
- Kernel: `0x88040000` (kseg0 DRAM).

There is no BIOS/firmware boundary between them. The kernel must use the objects and state
the ROM initialised, in place, as native parts of the same system. It must not re-create
the network stack, console, timers or other shared state after the handoff. Calls between
the two address ranges are normal intra-program calls.

Consequences:

- The Seeq device, `Interface`, `SocketSet`, telnet console and clock are initialised once
  by the ROM and remain live after the kernel starts.
- The telnet ROM console is always available while the machine is running, including after
  the kernel boots. The kernel polls the resident console service from its idle path.
- Shared state lives in reserved RAM and is described in `BootInfo`. The kernel's memory
  map must reserve it.
- The ROM is not a disposable bootloader. Do not replace shared state with a re-init, a
  proxy, or a separate console implementation in the kernel.

## Other standing requirements

- Rust, n32, MIPS III minimum; the ROM and kernel are both n32.
- The ROM executes from PROM space, not RAM.
- RAM tests run before any RAM structures exist.
- Use Australian English in prose.
