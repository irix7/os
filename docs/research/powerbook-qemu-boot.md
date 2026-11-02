# Booting a PowerPC kernel on QEMU's g3beige

**Status:** the second boot slice of #287, tracked as #291, works. `kernel-ppc`
is loaded by OpenBIOS, reads the machine description from the device tree,
installs its own BAT entries, points SDR1 at an empty table, prints to the ESCC
serial port, takes ten decrementer ticks, halts with `BOOT-OK`, then takes a
deliberate unmapped load through the 0x300 vector and reports `DSISR`, `DAR`,
`EPC` and `FAULT-OK`. Observed with QEMU 11.1.1 and its OpenBIOS 1.1 build.

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
reachable at the addresses the linker used. The second slice replaces that
mapping; see [The kernel's mapping](#the-kernels-mapping).

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
`timer::prepare` writes the reload value and programs DEC before the switch;
`timer::enable` sets `MSR[EE]` once the kernel's own BAT entries are live. Ten ticks
are one tenth of a second at the g3beige timebase.

## The kernel's mapping

`kernel-ppc/src/mmu.rs` installs four BAT entries and owns every translation
the kernel uses after the switch:

| Register | Range | WIMG | Purpose |
|---|---|---|---|
| `IBAT0`/`DBAT0` | `0x01000000` + 256 KiB | `0x0` | image, `.bss` and the 64 KiB boot stack |
| `IBAT1` | `0x00000000` + 128 KiB | `0x0` | exception vectors |
| `DBAT1` | Mac I/O BAR + 512 KiB | `0x28` | ESCC and the rest of the register window |

The image block is sized from `__kernel_start` and `__kernel_end`, so a larger
image widens the block rather than falling off it. The Mac I/O block is sized
from the BAR and its `assigned-addresses` size, so an extra PCI device that
moves the BAR also moves the BAT. `WIMG=0x28` sets guarded and cache-inhibited
for the device window; the image and vector blocks stay cacheable.

BAT block sizes on the 750 are `128 KiB << BL`, with an 11-bit BL field in
`BATU` bits 2..12. `BEPI` and `BRPN` hold address bits 0..14, so every block is
identity-mapped. The unused IBAT/DBAT slots are cleared rather than left to
whatever the firmware installed.

After the BAT entries are in place, `mmu::activate` writes a 64 KiB zeroed table to
SDR1 and invalidates the pages in the three mapped ranges with `tlbie`, then
executes `sync` and `isync`. With SDR1 pointing at zeros, a missing BAT cannot
fall back to an OpenBIOS PTE: it raises a DSI.

QEMU's 750 model advertises `PPC_MEM_TLBIE` and `PPC_MEM_TLBSYNC` but not
`PPC_MEM_TLBIA`, so `tlbia` is an illegal instruction there. The page sweep is
the equivalent for the ranges the kernel relies on, and is correct on the 750
where `tlbie` invalidates the addressed entry.

## Data storage interrupt

`trap::install` writes branches to the 0x300 and 0x900 vectors before the
switch. `dsi_handler` in `boot.S` reads DSISR (SPR 18), DAR (SPR 19) and SRR0
(SPR 26), resets r1 to `__stack_top` and calls `dsi_report`, which prints:

```
ppc: dsi dsisr=0x40000000 dar=0x20000000 epc=0x010009dc
FAULT-OK
ppc: halted
```

The deliberate load is from `0x20000000`, below RAM at every memory size the
gate uses and outside the PCI hole. DSISR bit 1 (`0x40000000`) is the
translation-not-found load case. The image never returns from the handler.

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
so the kernel halts in a spin loop after `ppc: halted` and the gate stops QEMU
once that marker reaches the serial file. The gate then checks:

- the banner, memory map, tick count and `BOOT-OK` appear after
  `ppc: mmu enabled`;
- `FAULT-OK` appears after `BOOT-OK`;
- the DSI line matches
  `ppc: dsi dsisr=0x... dar=0x... epc=0x...`;
- DSISR is a translation-not-found load and DAR is `0x20000000`;
- EPC falls inside the image BAT range printed in the log;
- the image contains no FP instruction.

Observed serial output:

```

>> [ppc] Kernel already loaded (0x01000000 + 0x000203b4) (initrd 0x00000000 + 0x00000000)
>> [ppc] Kernel command line:
>> switching to new context:
ppc: mmu enabled sdr1=0x1010000
ppc: mmu image base=0x1000000 size=262144
ppc: mmu io base=0x81080000 size=524288
ppc: mmu io wimg=0x28
IRIX 7 PowerPC G3 bring-up
ppc: booted
ppc: of client=0xfff025d0
ppc: escc=0x81093000
ppc: memory base=0x0 size=64 MiB
ppc: timebase=16600000 Hz
ppc: decrementer ticks=10
BOOT-OK
ppc: deliberate unmapped load
ppc: dsi dsisr=0x40000000 dar=0x20000000 epc=0x10009dc
FAULT-OK
ppc: halted
```

## Limits

- The kernel maps only its image, the vector page and the Mac I/O window. RAM
  beyond the image is not mapped, and there is no frame allocator.
- BAT entries only: there are no page tables, no user address space and no demand
  paging. SDR1 points at a zeroed table solely to make a missing BAT fault.
- The only interrupt is the decrementer. There is no console input, block
  device, network device, ADB or PMU.
- The ESCC baud generator is not programmed; QEMU's model ignores it. Real
  hardware needs WR12/WR13/WR14 and the correct clock source.
- The target is the 750 with the hard-float ABI. Kernel code avoids `f32` and
  `f64`; the gate disassembles the image and fails if it contains an FP
  instruction.
- The gate depends on OpenBIOS's `-kernel` preload path, not on a bootable CD
  or disk image. The PowerBook's Old World firmware path remains a separate
  slice.
