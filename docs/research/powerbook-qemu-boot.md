# Booting a PowerPC kernel on QEMU's g3beige

**Status:** the sixth boot slice of #287, tracked as #313, makes the console
work on both ESCC channels. `kernel-ppc` is loaded by OpenBIOS, reads the
machine description from the device tree, installs its own BAT entries, points
SDR1 at an empty table, prints to both ESCC serial ports, takes ten decrementer
ticks and prints `BOOT-OK`. It then proves the DSI, ISI, alignment, program and
system-call vectors in order, prints `TRAPS-OK`, takes ten external interrupts
from the CUDA timer through the Heathrow interrupt controller at the `0x500`
vector, prints `IRQ-OK count=10`, identifies the ATA disk on the first Mac I/O
channel, reads the signature sector at LBA 4 with polled PIO, prints `ATA-OK`
and halts. Observed with QEMU 11.1.2 and its OpenBIOS 1.1 build.

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
`0x01F00000` while the entry remains `0x01000000`. The first slice passes
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
| `/pci/mac-io:compatible` | `"heathrow"`; selects the interrupt controller |
| `/pci/mac-io/escc:reg` | the ESCC window (`0x13000`, size `0x1000`) |
| `/pci/mac-io/escc/ch-a:reg` | channel A's control offset (`0x13020`) |
| `/pci/mac-io/escc/ch-b:reg` | channel B's control offset (`0x13000`) |
| `/pci/mac-io/escc/ch-a:AAPL,address` | the absolute channel A address, when the firmware publishes it |
| `/options:output-device` | the firmware's console selection (`screen`, `scca`, ...) |
| `/chosen:stdout-path` | the standard console path, when present |
| `/pci/mac-io/via-cuda:reg` | the CUDA window offset inside the Mac I/O BAR (`0x16000`) |
| `/pci/mac-io/via-cuda:AAPL,interrupts` | the CUDA's interrupt controller line (`0x12`) |
| `/pci/mac-io/ata-3@20000:reg` | the ATA task-file window (`0x20000`, size `0x1000`) and the DBDMA window (`0x8b00`, size `0x200`) |
| `/pci/mac-io/ata-3@20000:AAPL,interrupts` | the ATA channel's interrupt controller line (`0x0d`) |
| `/cpus/PowerPC,750@0:timebase-frequency` | `16600000` Hz on g3beige |

The Mac I/O BAR is assigned, not fixed. With no extra PCI devices it is
`0x81080000`; adding an `e1000` moves it to `0x81110000`. The ESCC address
must be derived from the tree, not compiled in.

Channel A of the ESCC occupies control `+0x20` and data `+0x30` at `it_shift =
4`. QEMU connects the first `-serial` to channel A (`chrA`) and the second to
channel B (`chrB`). Both channels are independent register banks in the same
Mac I/O window.

## Console channels

The ESCC has two independent channels. In the Mac I/O window, channel A's
control port is at `+0x20` and channel B's at `+0x00`; each data port is 16
bytes above its control port. The kernel resets and initialises both, clocks
the baud-rate generator from RTxC, routes both transmit and receive clocks
through it (WR11 = `0x50`), programs 38400 8N1 from the 3.6864 MHz input clock
(WR12 = 1, WR13 = 0) and mirrors every byte to both. It polls each channel's
`TXEMPTY` status before writing, exactly as the single-channel driver did.

The device tree describes the channels as children of the ESCC node:

| Node and property | Use |
|---|---|
| `ch-a:reg` | channel A's control offset (`0x13020`) |
| `ch-b:reg` | channel B's control offset (`0x13000`) |
| `ch-a:AAPL,address` | the absolute channel address, when present |
| `ch-a:compatible` | `ch-a`, `CHRP,es2` |

The child `reg` offsets are offsets in the Mac I/O window, not from the parent
ESCC node; the kernel also accepts a relative value. OpenBIOS publishes the
same children and adds the `scca`/`sccb` and `ttya`/`ttyb` aliases. Apple's Old
World firmware uses the same CHRP shape, so the lookup is not QEMU-specific.

The firmware's console choice is read from `/options:output-device`, with
`/chosen:stdout-path` as the standard fallback. A value that names `ch-a`,
`scca` or `ttya` selects channel A; `ch-b`, `sccb` or `ttyb` selects channel B.
Anything else, such as `screen`, selects neither. The kernel logs both
properties and the resolved channel:

```
ppc: escc=0x81093000 ch-a=0x81093020 ch-b=0x81093000
ppc: console output-device=/pci@80000000/mac-io@10/escc/ch-a stdout-path=none selected=a mirror=a+b
```

The selection is informational: output is always mirrored to both channels.
The PowerBook G3 M4753 developer note wires the internal infrared link to
serial port B; the external GeoPort is the channel A port. The boot firmware
and the cable can still disagree with that expectation, and QEMU's `-serial`
mapping is its own. Mirroring makes the boot visible on either wiring without a
machine-specific default. On QEMU, `-nographic` makes OpenBIOS name the serial
path; `-display none` leaves `output-device=screen`, in which case the kernel
reports `selected=none` and still mirrors.

The Geoport is a mini-DIN-9 serial connector, backwards-compatible with the
8-pin Mac serial port, on SCC channel A. A modem card in the communication slot
is a separate device on the second Heathrow controller, not the SCC console.

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

## Exception vectors

`trap::install` writes a branch to every vector before the switch:

| Vector | Exception | Handler |
|---|---|---|
| `0x300` | data storage | `dsi_handler` |
| `0x400` | instruction storage | `isi_handler` |
| `0x500` | external interrupt | `external_handler` |
| `0x600` | alignment | `alignment_handler` |
| `0x700` | program | `program_handler` |
| `0x900` | decrementer | `decrementer_handler` |
| `0xC00` | system call | `system_call_handler` |

The five fault vectors, the system call and the external interrupt share
`trap_entry` in `boot.S`. Each stub parks the
interrupted r0 in SPRG0, puts its vector number in r0 and branches to the
common entry. The entry saves r0 and r2-r12 plus CR, LR, CTR and XER in a
128-byte frame, calls `trap_dispatch`, stores the returned r3 and restores the
context with `rfi`. r13-r31 are callee-saved by the Rust handler. SPRG0 is
safe as scratch because the hardware clears `MSR[EE]` on exception entry, so a
stub cannot be interrupted by another exception.

`trap_dispatch` prints one report for every exception. DSISR and DAR are
printed for the DSI and alignment vectors, the two memory faults:

```
ppc: trap vector=0x300 srr0=0x1001d08 srr1=0xa030 dsisr=0x40000000 dar=0x20000000
ppc: trap vector=0x400 srr0=0x20000000 srr1=0x4000a030
ppc: trap vector=0x600 srr0=0x1001d4c srr1=0xa030 dsisr=0x40000060 dar=0x1020121
ppc: trap vector=0x700 srr0=0x1001d70 srr1=0x8a030
ppc: trap vector=0xc00 srr0=0x1001d80 srr1=0xa030
ppc: syscall call=0x2a result=0x2b
```

### Resume policy

For a data fault, alignment fault or program exception, SRR0 holds the
faulting instruction; the handler writes `SRR0 + 4` and `rfi` resumes at the
next instruction. The system call exception already saves the instruction
after `sc`, so its SRR0 is written back unchanged; the handler reads the call
number from r3 and returns call + 1 in r3.

An instruction fetch has no instruction at SRR0 to step over. The probe
reaches `0x20000000` with `bctrl`, so the handler resumes at the interrupted
LR. It refuses an LR outside the image or not word aligned, prints
`ppc: isi without a resume address` and halts instead of returning to an
arbitrary address.

An external interrupt is taken between instructions, so SRR0 is the
instruction to resume at and is written back unchanged. There is no faulting
instruction to step over and no LR fixup.

The probes run after `BOOT-OK`, each marker after the faulting instruction has
returned:

| Probe | Fault | Marker |
|---|---|---|
| `0x20000000` load | DSI | `FAULT-OK` |
| `bctrl` to `0x20000000` | ISI | `ISI-OK` |
| `lwarx` at an odd address | alignment | `ALIGN-OK` |
| the all-zero word | program | `PROGRAM-OK` |
| `li 3, 0x2a; sc` | system call | `SYSCALL-OK` |

The deliberate DSI load is from `0x20000000`, below RAM at every memory size
the gate uses and outside the PCI hole. DSISR bit 1 (`0x40000000`) is the
translation-not-found load case. The alignment probe uses `lwarx` because
QEMU's 750 model does not check a plain `lwz` for word alignment. The program
probe's illegal-instruction cause appears in SRR1 bit 12 (`0x00080000`).

## External interrupts

The machine has no OpenPIC. QEMU's `g3beige` Mac I/O node is
`compatible = "heathrow"`, and the same is true of the PowerBook G3 M4753: the
interrupt controller is the Heathrow block at the start of the Mac I/O window,
not the OpenPIC used by New World machines such as `mac99`. The kernel reads
the Mac I/O BAR and the `compatible` property, refuses a controller it does
not know, and treats Mac I/O offset zero as the Heathrow register block. That
offset is part of the chip, not a firmware assignment.

The controller has two banks. Bank 0 at `+0x10` carries sources 32..63 and
bank 1 at `+0x20` carries 0..31; each bank has a pending-event register, a
mask, an acknowledge port and a raw-level register. A source sets its event
bit on the rising edge and stays pending until software writes that bit back
to the acknowledge port. The mask has one bit per source, with one meaning
enabled. The registers are little-endian on the Mac I/O bus, so the driver
byte-reverses every 32-bit access.

The timer is timer 1 in the CUDA's MOS 6522 VIA. The device tree gives its
window (`reg = <0x16000 0x2000>`) and line (`AAPL,interrupts = <0x12>`); the
VIA registers are 0x200 bytes apart inside that window. Timer 1 runs in
continuous mode at 4.7 MHz / 6: a latch of 781 gives one interrupt about every
millisecond. The handler clears the VIA's T1 flag, writes the event bit to the
controller's acknowledge port, and checks that the next event read has the
bit clear. That write is both the acknowledge and the end of interrupt. A
timer underflow can set the flag again between clearing it and acknowledging
the controller, so the handler repeats the clear/acknowledge pair a bounded
number of times instead of reporting a false failure. The stop path disables
the interrupt, switches T1 to one-shot mode, loads the longest count and then
clears the flag, so its final IFR read cannot race the next underflow.

The `0x500` stub shares `trap_entry` with the fault vectors. An external
interrupt is taken at an instruction boundary, so SRR0 already holds the
instruction to resume at and the dispatcher writes it back unchanged. SRR1
holds the interrupted MSR with `EE` set. The boot leaves the controller masked
and the timer stopped while the fault probes run, then starts the timer,
unmasks line `0x12` and collects ten interrupts:

```
ppc: irq controller=0x81080000 timer=0x81096000 line=0x12 period=781
...
ppc: irq vector=0x500 line=0x12 count=1 srr0=0x1002b20 srr1=0xa030
...
ppc: irq pending=0x0 ifr=0x0
IRQ-OK count=10
```

The wait is bounded by two seconds of timebase, so a timer that never fires
prints `ppc: irq stalled count=N` and halts instead of hanging the gate.

## ATA disk

The disk path is the Mac I/O ATA controller, not an ISA IDE port. QEMU's
`hw/ide/macio.c` places each channel in the Mac I/O BAR at a firmware-assigned
offset and gives each task-file register a 16-byte stride. The `ata-3` node
publishes two `reg` ranges — the task-file window and the DBDMA channel — plus
`AAPL,interrupts = <line, dma-line>`. `platform::ata` walks the children of
`/pci/mac-io`, accepts `device_type = "ata"`, prefers the channel with a `disk`
child, and takes the first `reg` range. The channel base is the Mac I/O BAR plus
that offset; no ATA address is compiled in.

`ata::Channel` performs two operations, both polled:

- `IDENTIFY DEVICE` (`0xec`) after selecting device 0, reading 256 data words;
- one `READ SECTORS` (`0x20`) for a 28-bit LBA.

Every wait is bounded by two seconds of timebase and checks `BSY`, `DRQ`,
`DRDY`, `ERR` and `DF`. The drive's `nIEN` bit is set, so the channel never
raises its interrupt line; line `0x0d` is read from the tree and logged but
stays masked at the Heathrow controller.

The data port is a 16-bit little-endian register on a big-endian bus. A
halfword load returns the two wire bytes in memory order, which is the order a
sector buffer needs. Identify word fields are little-endian on the wire, so
each word is byte-swapped before the feature word and the LBA capacities are
read; the model string then comes out of the swapped word in character order.

This slice also corrects the BAT layer. The 750's BL field is the block size in
128 KiB units minus one, not a power-of-two exponent. The 512 KiB Mac I/O block
had been installed as BL=2 (384 KiB) instead of BL=3 (512 KiB), so the
translation did not cover the ATA window at `+0x20080` and the first ATA
register access raised a DSI. `mmu::Bat::upper` now encodes
`size / 128 KiB - 1`.

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
  -drive file=ata.img,format=raw,if=ide,index=0,media=disk \
  -nographic -serial file:serial.log -serial file:serial-b.log \
  -monitor none -no-reboot
```

The gate creates `ata.img` as 2048 zero sectors and writes
`IRIX7-ATA-SIG001` at LBA 4. It attaches that image to the first Mac I/O ATA
channel, so the device tree's `ata-3@20000` node has a `disk` child.

The g3beige machine has no `isa-debug-exit` device and no RTAS shutdown call,
so the kernel halts in a spin loop after `ppc: halted` and the gate stops QEMU
once that marker reaches both serial files. The gate checks every marker in the
order it must appear on channel A, then parses the exception reports:

- the banner, memory map, tick count and `BOOT-OK` appear after
  `ppc: mmu enabled`;
- `FAULT-OK`, `ISI-OK`, `ALIGN-OK`, `PROGRAM-OK`, `SYSCALL-OK`, `TRAPS-OK`,
  the external interrupt reports, `IRQ-OK` and `ppc: halted` follow in that
  order;
- every vector report matches `ppc: trap vector=0x... srr0=0x... srr1=0x...`,
  with `dsisr` and `dar` for the DSI and alignment vectors;
- DSISR is a translation-not-found load, DAR is `0x20000000`, and the DSI
  SRR0 falls inside the image BAT range printed in the log;
- the ISI SRR0 is `0x20000000`, the alignment DAR is not word aligned, the
  program SRR1 carries the illegal-instruction cause, and the system call
  returns `0x2b` for call `0x2a`;
- every external interrupt reports vector `0x500`, line `0x12` and a count
  that runs from one without a gap; the saved SRR1 has `EE` set and the saved
  SRR0 is inside the image BAT range;
- at least four external interrupts were counted, `IRQ-OK` repeats the same
  count, and the controller's pending-event and the timer's flag register are
  both zero after the last acknowledge;
- the ESCC report names channel B at the window base and channel A at
  `window + 0x20`, and the firmware's `output-device` names the same channel
  the kernel reports as selected;
- channel B's kernel log is byte-for-byte identical to channel A's from
  `ppc: mmu enabled` onward, so the banner and every marker reached both
  ports;
- the ATA window from the `ata-3` node lies inside the Mac I/O BAT, the identify
  model is `QEMU HARDDISK`, the sector count equals the attached image's size in
  512-byte sectors, and the bytes read from LBA 4 match the image;
- the image contains no FP instruction.

Observed serial output:

```

>> [ppc] Kernel already loaded (0x01000000 + 0x00020792) (initrd 0x00000000 + 0x00000000)
>> [ppc] Kernel command line:
>> switching to new context:
ppc: mmu enabled sdr1=0x1010000
ppc: mmu image base=0x1000000 size=262144
ppc: mmu io base=0x81080000 size=524288
ppc: mmu io wimg=0x28
IRIX 7 PowerPC G3 bring-up
ppc: booted
ppc: of client=0xfff025d0
ppc: escc=0x81093000 ch-a=0x81093020 ch-b=0x81093000
ppc: console output-device=/pci@80000000/mac-io@10/escc/ch-a stdout-path=none selected=a mirror=a+b
ppc: memory base=0x0 size=64 MiB
ppc: timebase=16600000 Hz
ppc: ata reg base=0x810a0000 size=4096
ppc: ata irq=0xd
ppc: irq controller=0x81080000 timer=0x81096000 line=0x12 period=781
ppc: decrementer ticks=10
BOOT-OK
ppc: deliberate unmapped load
ppc: trap vector=0x300 srr0=0x1002be4 srr1=0xa030 dsisr=0x40000000 dar=0x20000000
FAULT-OK
ppc: deliberate unmapped fetch
ppc: trap vector=0x400 srr0=0x20000000 srr1=0x4000a030
ISI-OK
ppc: deliberate misaligned load
ppc: trap vector=0x600 srr0=0x1002c28 srr1=0xa030 dsisr=0x40000060 dar=0x1020261
ALIGN-OK
ppc: deliberate illegal instruction
ppc: trap vector=0x700 srr0=0x1002c4c srr1=0x8a030
PROGRAM-OK
ppc: trap vector=0xc00 srr0=0x1002c5c srr1=0xa030
ppc: syscall call=0x2a result=0x2b
SYSCALL-OK
TRAPS-OK
ppc: irq vector=0x500 line=0x12 count=1 srr0=0x1004abc srr1=0xa030
ppc: irq vector=0x500 line=0x12 count=2 srr0=0x1004abc srr1=0xa030
...
ppc: irq vector=0x500 line=0x12 count=10 srr0=0x1004abc srr1=0xa030
ppc: irq pending=0x0 ifr=0x0
IRQ-OK count=10
ppc: ata model=QEMU HARDDISK sectors=2048 lba48=1
ppc: ata lba=4 sector=49524958372d4154412d534947303031
ATA-OK
ppc: halted
```

## Limits

- The kernel maps only its image, the vector page and the Mac I/O window. RAM
  beyond the image is not mapped, and there is no frame allocator.
- BAT entries only: there are no page tables, no user address space and no demand
  paging. SDR1 points at a zeroed table solely to make a missing BAT fault.
- The exception layer is a bring-up proof, not a production fault handler. A
  data, alignment or program fault is always stepped over, and an ISI resumes
  only through the LR fixup, so a real unexpected fault in arbitrary code would
  resume at a meaningless address rather than panic.
- There is one kernel stack and no nested-exception frame. The handlers run on
  the interrupted stack with `MSR[EE]` clear; a fault inside a handler would
  recurse.
- The external interrupt path is the Heathrow controller and one timer source,
  not a general interrupt layer. There is no console input, block device,
  network device, ADB or PMU, and no handler is shared between devices.
- The ATA driver is polled PIO only. It reads one 512-byte sector per command,
  addresses LBA28, and performs no DMA, writes, ATAPI commands, partition
  parsing or filesystem access. The block layer and the interrupt-driven path
  come with the disk service slice.
- The PowerBook G3 M4753 has no OpenPIC; a New World machine with the KeyLargo
  OpenPIC needs a separate controller driver and a different machine tree.
- The console is transmit-only at a fixed 38400 8N1. It programs WR11, WR12,
  WR13 and WR14 for the 3.6864 MHz ESCC clock, but there is no receive path,
  flow control, termios, or baud negotiation, and no keyboard or framebuffer
  console.
- Mirroring proves that both ESCC register banks accept the boot log. It does
  not prove that both are electrically wired on a particular PowerBook: QEMU's
  channel B is a chardev, and the Geoport/IR wiring still needs the hardware
  test described in [powerbook-g3.md](powerbook-g3.md). The hardware check is
  the Geoport at 38400 8N1 on channel A; channel B is the internal IR link, so
  proving it needs the IR transceiver or a logic probe rather than a serial
  cable.
- The target is the 750 with the hard-float ABI. Kernel code avoids `f32` and
  `f64`; the gate disassembles the image and fails if it contains an FP
  instruction.
- The gate depends on OpenBIOS's `-kernel` preload path, not on a bootable CD
  or disk image. The PowerBook's Old World firmware path remains a separate
  slice.
