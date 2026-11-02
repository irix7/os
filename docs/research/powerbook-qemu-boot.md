# Booting a PowerPC kernel on QEMU's g3beige

**Status:** the first boot slice of #287 works. `kernel-ppc` is loaded by
OpenBIOS, prints to the ESCC serial port, reads the machine description from
the device tree, takes ten decrementer ticks and halts with `BOOT-OK`.
Observed with QEMU 11.1.1 and its OpenBIOS 1.1 build.

## The `-kernel` path

QEMU's `hw/ppc/mac_oldworld.c` makes two decisions when `-kernel` is present:

- the load address is `KERNEL_LOAD_ADDR` (`0x01000000`);
- the fw_cfg boot device is set to `'m'`, which tells OpenBIOS that a kernel
  image is already in RAM.

QEMU then records `FW_CFG_KERNEL_ADDR` and `FW_CFG_KERNEL_SIZE`. OpenBIOS's
`arch/ppc/qemu/main.c:boot` sees `'m'`, prints `[ppc] Kernel already loaded`,
calls `arch_init_program`, overwrites `r3`/`r4` with the initrd address and
size, sets `pc` to `FW_CFG_KERNEL_ADDR` and enters the client context.

A raw image is loaded with `load_image_targphys` at `0x01000000`. An ELF image
goes through `translate_kernel_address`, which maps `(address & 0x0fffffff) +
0x01000000`; a segment linked at `0x01000000` would therefore be loaded at
`0x02000000` while the entry remains `0x01000000`. The first slice passes
`llvm-objcopy -O binary` output, not the ELF.

## Register state at entry

| Register | Value |
|---|---|
| `MSR` | `FP | ME | DR | IR`; the firmware MMU is on and `EE` is off |
| `r1` | the client stack, 32 KiB plus a 32-byte link area below OpenBIOS's hash table |
| `r3` | initrd address, zero when no initrd was supplied |
| `r4` | initrd size, zero |
| `r5` | the Open Firmware client interface handler |
| `r6`, `r7` | zero (unused client argument pointer and length) |
| `r2`, `r13`-`r31` | zero from the zeroed client context |

There is no direct device tree pointer. The tree is reached by calling the
handler in `r5` with `r3` pointing at a `prom_args` block:

```c
struct prom_args {
    u32 service;    /* pointer to the service name */
    u32 nargs;
    u32 nret;
    u32 args[10];   /* returns are written at args[nargs..] */
};
```

`of_call` in `kernel-ppc/src/boot.S` performs that call; `kernel-ppc/src/of.rs`
wraps `finddevice`, `child` and `getprop`. The handler returns 0 for success
and -1 for an unimplemented service.

The firmware mapping is one-to-one for RAM, and the client must keep the MMU
enabled. The kernel is linked at `0x01000000`, so its code and data are
reachable at the addresses the linker used. This slice does not install BATs
or a page table of its own.

## Devices read from the tree

| Node and property | Use |
|---|---|
| `/memory:reg` | `[address, size]`; the RAM QEMU handed over |
| `/pci/mac-io:assigned-addresses` | the Mac I/O PCI BAR assigned by OpenBIOS |
| `/pci/mac-io/escc:reg` | the ESCC offset inside the Mac I/O BAR (`0x13000`) |
| `/cpus/PowerPC,750@0:timebase-frequency` | `16600000` Hz on g3beige |

The Mac I/O BAR is assigned, not fixed. With no extra PCI devices it is
`0x81080000`; adding an `e1000` moves it to `0x81110000`. The ESCC address
must be derived from the tree, not compiled in.

Channel A of the ESCC occupies control `+0x20` and data `+0x30` at `it_shift =
4`. QEMU connects `-serial` to channel A (`chrA`). The driver resets the
channel, selects 8 bits with one stop bit and transmit enable, then polls the
status register's `TXEMPTY` bit before each byte.

## Decrementer

The exception vector at `0x900` branches to `decrementer_handler`. The handler
saves the volatile registers and the special registers, increments the tick
count, reloads DEC with `timebase_frequency / 100`, and returns with `rfi`.
`timer::start` writes the reload value, installs the branch, flushes the
instruction cache and sets `MSR[EE]`. Ten ticks are one tenth of a second at
the g3beige timebase.

## Gate

The build needs `cargo`, a host C compiler for the compiler-builtins build
script, and `qemu-system-ppc` on `PATH`.

```sh
tools/powerpc-qemu.sh
```

The image is built with `-Zbuild-std=core`, the raw binary is extracted with
`llvm-objcopy`, and QEMU runs with:

```sh
qemu-system-ppc -M g3beige -m 64M -kernel kernel-ppc.bin \
  -display none -serial file:serial.log -monitor none -no-reboot
```

The g3beige machine has no `isa-debug-exit` device and no RTAS shutdown call,
so the kernel halts in a spin loop after `BOOT-OK` and the gate stops QEMU once
the marker reaches the serial file.

Observed serial output:

```
IRIX 7 PowerPC G3 bring-up
ppc: booted
ppc: of client=0xfff025d0
ppc: escc=0x81093000
ppc: memory base=0x0 size=64 MiB
ppc: timebase=16600000 Hz
ppc: decrementer ticks=10
ppc: halted
BOOT-OK
```

## Limits

- The firmware's one-to-one mapping is the only mapping; BATs and a page table
  are not installed yet.
- The only interrupt is the decrementer. There is no console input, block
  device, network device, ADB or PMU.
- The ESCC baud generator is not programmed; QEMU's model ignores it. Real
  hardware needs WR12/WR13/WR14 and the correct clock source.
- The target is the 750 with the hard-float ABI. Kernel code avoids `f32` and
  `f64`; no FP instruction is emitted by the linked image.
- The gate depends on OpenBIOS's `-kernel` preload path, not on a bootable CD
  or disk image. The PowerBook's Old World firmware path remains a separate
  slice.
