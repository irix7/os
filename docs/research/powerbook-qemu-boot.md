# Booting a PowerPC kernel on QEMU's g3beige

**Status:** the eighth boot slice of #287, tracked as #321, #323 and #324, adds
serial input, a minimal boot monitor, a framebuffer console and the ADB
keyboard. The seventh slice (#317) selects the Mac I/O node that carries the
ESCC console and the ATA disk. `kernel-ppc` is loaded by OpenBIOS, reads the
machine description from the device tree, installs its own BAT entries, points
SDR1 at an empty table, prints to both ESCC serial ports, takes ten decrementer
ticks and prints `BOOT-OK`. It maps the display aperture cache-inhibited, draws
an 8x16 text console, mirrors the boot log into it, reads a known block and
glyph back from the framebuffer and prints `GFX-OK`. It then proves the DSI,
ISI, alignment, program and system-call vectors in order, prints `TRAPS-OK`,
takes ten external interrupts from the CUDA timer through the Heathrow
interrupt controller at the `0x500` vector, prints `IRQ-OK count=10`,
identifies the ATA disk on the selected Mac I/O channel, reads the signature
sector at LBA 4 with polled PIO, prints `ATA-OK`, loads and enters the flat
module, and prints `MODULE-OK`. It then polls the selected ESCC channel for one
second: a key enters a `ppc>` monitor with `help`, `info` and `halt`, and
silence prints `ppc: monitor timeout`. Finally it synchronises the CUDA's ADB
state machine, waits up to three seconds for a key at the address the device
tree gives the keyboard, prints the scancode and US ASCII character, prints
`KEY-OK` and halts. QEMU's g3beige has a CUDA; the gate injects `a` through the
QEMU monitor once the kernel prints `ppc: keyboard waiting`. A boot with no
input prints `ppc: keyboard timeout`, `KEY-SKIP reason=no-key` and still
reaches `ppc: halted`. Observed with QEMU 11.1.2 and its OpenBIOS 1.1 build. On
a PMU machine such as the M4753 there is no `via-cuda`; the timer lookup then
selects `via-pmu`, programs the same 6522 timer and prints `IRQ-OK`. QEMU has
no PMU model, so that path is pending real M4753 verification under #16. The
keyboard path uses the PMU transport, which is written but also pending
hardware verification.

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

The paths below are QEMU's. Every Mac I/O lookup is relative to the node
selected in [Mac I/O selection](#mac-io-selection); on the PowerBook the unit
address is `@10` or `@D`, depending on which controller carries the console
and disk.

| Node and property | Use |
|---|---|
| `/memory:reg` | `[address, size]`; the RAM QEMU handed over |
| selected Mac I/O node `assigned-addresses` | the Mac I/O PCI BAR assigned by OpenBIOS |
| selected Mac I/O node `compatible` | `"heathrow"` or `"gatwick"`; selects the interrupt controller |
| `escc:reg` | the ESCC window (`0x13000`, size `0x1000`) |
| `escc/ch-a:reg` | channel A's control offset (`0x13020`) |
| `escc/ch-b:reg` | channel B's control offset (`0x13000`) |
| `escc/ch-a:AAPL,address` | the absolute channel A address, when the firmware publishes it |
| `/options:output-device` | the firmware's console selection (`screen`, `scca`, ...) |
| `/chosen:stdout-path` | the standard console path, when present |
| `via-cuda:reg` | the CUDA window offset inside the Mac I/O BAR (`0x16000`) |
| `via-cuda:AAPL,interrupts` | the CUDA's interrupt controller line (`0x12`) |
| `via-cuda/adb/keyboard:reg` | the keyboard's ADB address (`8` on QEMU; the firmware assigns it) |
| `via-pmu:reg` | the PMU's 6522 window on a machine with no CUDA (`0x16000` on the M4753) |
| `ata-3@20000:reg` | the ATA task-file window (`0x20000`, size `0x1000`) and the DBDMA window (`0x8b00`, size `0x200`) |
| `ata-3@20000:AAPL,interrupts` | the ATA channel's interrupt controller line (`0x0d`) |
| `/cpus/PowerPC,750@0:timebase-frequency` | `16600000` Hz on g3beige |
| `/pci@80000000/QEMU,VGA@1:device_type` | `display`; selects the framebuffer node |
| `QEMU,VGA@1:width`, `height`, `depth`, `linebytes` | the mode Open Firmware left active (`800`, `600`, `32`, `3200` on g3beige) |
| `QEMU,VGA@1:assigned-addresses` | the framebuffer BAR; first 32-bit memory entry `0x80000000`, size `0x01000000` |
| `RageLT:assigned-addresses` | the Rage LT framebuffer BAR on the PowerBook, in the same 20-byte shape |

The Mac I/O BAR is assigned, not fixed. With no extra PCI devices it is
`0x81080000`; adding an `e1000` moves it to `0x81110000`. The ESCC address
must be derived from the tree, not compiled in. Channel A of the ESCC occupies
control `+0x20` and data `+0x30` at `it_shift = 4`. QEMU connects the first
`-serial` to channel A (`chrA`) and the second to channel B (`chrB`). Both
channels are independent register banks in the same Mac I/O window.

## Mac I/O selection

`g3beige` has one Mac I/O node. The PowerBook G3 M4753 has two under
`/pci@80000000`:

| Path | `compatible` | Children |
|---|---|---|
| `mac-io@10` | `heathrow` | `mesh`, `escc`, `bmac`, `media-bay` (`floppy`, `ata1`), `ata0`, `via-pmu` |
| `mac-io@D` | `gatwick` | `escc`, `media-bay` (`floppy`, `ata4`) |

Both carry an ESCC, and either can have an ATA disk, so `/pci/mac-io` — which
the firmware resolves to the first match — is not a stable selector. The
kernel walks the children of `/pci`, accepts every node whose `device_type` or
`name` is `mac-io`, and scores each candidate from its descendants:

- an ESCC child is worth 100;
- an ATA channel with a `disk` child is worth 50;
- every ATA channel is worth 10.

The highest score wins; a tie stays with the first node in tree order. A
candidate with no ESCC is refused, because without it there is no console on
which to report a failure. The scan reaches an ATA channel below `media-bay`
as well as a direct child, so the media-bay channel is eligible when it holds
the boot disk. On the M4753 the primary controller scores higher because it
has `ata0` and the media-bay `ata1`. QEMU's `g3beige` has one built-in Mac I/O
node, so the gate adds a second `macio-oldworld` device at PCI device `0x03`.
That node appears first in tree order with an ESCC and empty ATA channels,
while the built-in `mac-io@10` has the attached disk. The gate requires the
kernel to keep selecting `mac-io@10`. This exercises the score order, not just
the single-candidate path.

The selected node's `assigned-addresses` gives the Mac I/O BAR. The ESCC, ATA,
VIA and interrupt-controller lookups all start at that node, so a tree that
orders the communication-slot controller first still boots from the primary
controller's console and disk. The boot log records the choice:

```
ppc: macio base=0x81080000 size=524288
ppc: macio path=/pci@80000000/mac-io@10
ppc: macio children escc=1 ata=2 disks=1
```

`package-to-path` supplies the full path. The fallback walk over `name`
properties omits unit addresses, so the base is logged as well. The gate
requires a `/pci` path containing `mac-io`, at least one ESCC child and at
least one ATA child, and checks that the ESCC and ATA windows lie inside the
selected Mac I/O window.

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
ppc: macio base=0x81080000 size=524288
ppc: macio path=/pci@80000000/mac-io@10
ppc: macio children escc=1 ata=2 disks=1
ppc: escc=0x81093000 ch-a=0x81093020 ch-b=0x81093000
ppc: console output-device=/pci@80000000/mac-io@10/escc/ch-a stdout-path=none selected=a mirror=a+b
```

The selection is informational for output: output is always mirrored to both
channels. The PowerBook G3 M4753 developer note wires the internal infrared
link to serial port B; the external GeoPort is the channel A port. The boot
firmware and the cable can still disagree with that expectation, and QEMU's
`-serial` mapping is its own. Mirroring makes the boot visible on either
wiring without a machine-specific default. On QEMU, `-nographic` makes
OpenBIOS name the serial path; `-display none` leaves `output-device=screen`,
in which case the kernel reports `selected=none` and still mirrors.

Input is not mirrored. Both receivers are enabled (WR3 = `0xc1`), but the
monitor polls exactly one channel: the one the firmware selected, or channel A
when the firmware names no ESCC channel. Channel A is the external Geoport on
the M4753, and reading both channels into one line buffer would interleave two
terminals. `serial::read_byte` selects RR0, tests bit 0 (Rx character
available) and drains every waiting byte into a 64-byte receive ring before
returning one; `serial::init` drains any byte latched before the monitor
existed. The ring is the paste path: while a command executes, the next bytes
wait there instead of overwriting the ESCC's single receive register. A full
ring drops the newest byte and prints `monitor: input overrun` once, so loss
is visible rather than silent. The receiver is polled rather than
interrupt-driven, and RR1 is not inspected.

The monitor accepts a line of printable ASCII up to 32 characters. Backspace
erases the previous character; a longer line is consumed without echo and
refused at CR with `monitor: command too long` rather than being matched in
truncated form. An unrecognised line prints `monitor: unknown command:` and
the retained line. The gate pastes several commands, a backspace edit, an
overlong line and an unknown command in one write.

The Geoport is a mini-DIN-9 serial connector, backwards-compatible with the
8-pin Mac serial port, on SCC channel A. A modem card in the communication slot
is a separate device on the second Heathrow controller, not the SCC console.

## Framebuffer console

QEMU's g3beige has a `QEMU,VGA@1` node under `/pci@80000000` with
`device_type = "display"`. OpenBIOS's VGA FCode sets a Bochs VBE mode from the
`FW_CFG_PPC_WIDTH`/`HEIGHT`/`DEPTH`/`LINEBYTES` values and publishes the
result as the node's `width`, `height`, `depth` and `linebytes` properties:

```
ppc: display path=/pci@80000000/QEMU,VGA@1 base=0x80000000 size=16777216 width=800 height=600 depth=32 linebytes=3200
```

The kernel finds the node by `device_type`, not by path or name, so the same
code accepts the PowerBook's Rage LT node. It reads the mode and then the
aperture from `assigned-addresses`, in the PCI 20-byte
`[phys_hi, addr_hi, addr_lo, size_hi, size_lo]` shape. Only a 32-bit memory
BAR below 4 GiB is accepted; a 64-bit BAR is left to a later slice. `reg` in
address/size pairs is the fallback for a non-PCI node, and the OpenBIOS VGA
driver's `address` property is the last fallback. The aperture must be at
least `linebytes * height`, so a node that describes a mode larger than its
BAR is refused.

The aperture is mapped by `DBAT3`, identity-mapped, with `WIMG = 0x28`:
guarded and cache-inhibited. It is installed in the same switch as the image,
Mac I/O and module BATs, and `mmu::activate` invalidates its pages with the
rest. A machine with no display leaves `DBAT3` cleared and prints
`ppc: display absent`; a display whose depth is not 8, 16 or 32 prints
`ppc: display unsupported depth=...` and `GFX-SKIP reason=mode`.

The console draws an 8x16 text cell using the classic IBM VGA font, white on
black. It supports the three formats the firmware can publish without a driver:

| Depth | Pixel format | White | Black |
|---|---|---|---|
| 8 | VGA palette index | `0x0f` | `0x00` |
| 16 | little-endian 5:6:5 | `0xffff` | `0x0000` |
| 32 | little-endian `XRGB` (`B G R X` in memory) | `0xff_ff_ff_00` as a big-endian word | `0` |

A 15, 20 or 24 bpp mode is refused rather than guessed at. The 8 bpp path uses
palette entry 15, which is the standard VGA white; the firmware's own palette
is assumed and not reprogrammed. The 32 bpp word is byte-swapped before the
store so a big-endian `stw` leaves `B G R X` in memory, which is what QEMU's
VGA and the Rage LT present on the PCI bus.

The text console has `width / 8` columns and `height / 16` rows, 100x37 at
800x600. It handles carriage return, line feed and tab, wraps at the right
edge, and scrolls by copying every visible row one text line up and clearing
the last line. There is no shadow buffer, cursor, hardware scroll or
acceleration. Every glyph cell is redrawn completely, so old text is erased.

`console::write` sends each byte to the ESCC first and then to the framebuffer.
The serial transcript is therefore unchanged. The display is initialised after
the BAT switch, so bytes printed before then are held in a 4 KiB buffer and
replayed; the LCD starts at `ppc: mmu enabled sdr1=...` rather than losing the
mapping report. The replay also covers the display report.

The self-test draws a solid 8x16 block in the second-last cell of the bottom
row and `G` in the last cell, then reads both back through the same
cache-inhibited mapping. The block proves the pixel format and the aperture;
the glyph is compared with the font table, so a wrong stride or byte order
fails. `GFX-OK` is printed only when every pixel matches. It runs after the ATA
read, so the block and glyph are still on the final screen after the boot log
has scrolled.

### The mode assumption

The kernel does not set a mode, so the mode and the aperture are exactly what
Open Firmware left. That is the VGA FCode's `800x600x32` on g3beige and
whatever the Rage LT's NDRV left on the PowerBook. If the firmware changes the
mode after the kernel has read the properties, or the node's `linebytes` does
not match the active stride, the console writes into the wrong pixels. The
kernel checks the stride against the depth and width, but it cannot ask the
device which mode is actually scanning out.

On QEMU, `-nographic` sets `FW_CFG_NOGRAPHIC` and leaves the VGA surface at
640x480 text mode even though the device tree still reports 800x600x32. The
gate therefore uses the guest readback for its pixel assertion. A visual check
needs a real display backend:

```sh
qemu-system-ppc -M g3beige -m 64M -kernel kernel-ppc.bin \
  -drive file=ata.img,format=raw,if=ide,index=0,media=disk \
  -vnc 127.0.0.1:1 -serial file:serial.log -monitor none -no-reboot
```

`screendump` from the monitor then writes an 800x600 PPM, and the boot log
and the self-test cells are visible. The real-hardware check is the same
sequence with `output-device=screen` at the Open Firmware prompt: the log must
name the Rage LT node and the mode it left, the boot log must appear on the
LCD from `ppc: mmu enabled` onward, and the bottom-right block and `G` must be
legible. A depth outside 8/16/32 is expected to print `GFX-SKIP reason=mode`
and leave the serial console working.

## Decrementer

The exception vector at `0x900` branches to `decrementer_handler`. The handler
saves the volatile registers and the special registers, increments the tick
count, reloads DEC with `timebase_frequency / 100`, and returns with `rfi`.
`timer::prepare` writes the reload value and programs DEC before the switch;
`timer::enable` sets `MSR[EE]` once the kernel's own BAT entries are live. Ten ticks
are one tenth of a second at the g3beige timebase.

## The kernel's mapping

`kernel-ppc/src/mmu.rs` installs the BAT entries that own every translation
the kernel uses after the switch:

| Register | Range | WIMG | Purpose |
|---|---|---|---|
| `IBAT0`/`DBAT0` | `0x01000000` + 256 KiB | `0x0` | image, `.bss` and the 64 KiB boot stack |
| `IBAT1` | `0x00000000` + 128 KiB | `0x0` | exception vectors |
| `IBAT2`/`DBAT2` | `0x01800000` + 2 MiB | `0x0` | module slot |
| `DBAT1` | Mac I/O BAR + 512 KiB | `0x28` | ESCC and the rest of the register window |
| `DBAT3` | display aperture (`0x80000000` + 16 MiB on g3beige) | `0x28` | framebuffer console |

The image block is sized from `__kernel_start` and `__kernel_end`, so a larger
image widens the block rather than falling off it. The Mac I/O block is sized
from the BAR and its `assigned-addresses` size, so an extra PCI device that
moves the BAR also moves the BAT. The framebuffer block covers both the
display's assigned aperture and the visible frame. `WIMG=0x28` sets guarded
and cache-inhibited for the device windows; the image, vector and module blocks
stay cacheable. A machine without a display leaves `DBAT3` cleared.

BAT block sizes on the 750 are `128 KiB << BL`, with an 11-bit BL field in
`BATU` bits 2..12. `BEPI` and `BRPN` hold address bits 0..14, so every block is
identity-mapped. The unused IBAT slots are cleared, and `DBAT3` is cleared when
there is no display, rather than left to whatever the firmware installed.

After the BAT entries are in place, `mmu::activate` writes a 64 KiB zeroed table to
SDR1 and invalidates the pages in the mapped ranges with `tlbie`, then
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

The timer is timer 1 in the MOS 6522 VIA. The device tree gives its window
(`reg = <0x16000 0x2000>`) and line (`AAPL,interrupts = <0x12>`) for the CUDA,
or the corresponding `via-pmu` window and line on the M4753; the VIA registers
are 0x200 bytes apart inside that window. `platform::via_timer` selects
`via-cuda` first and `via-pmu` second, matching Linux's
`via_calibrate_decr`; both nodes program the same timer. Timer 1 runs in
continuous mode at 4.7 MHz / 6: a latch of 781 gives one interrupt about every
millisecond. The handler clears the VIA's T1 flag, writes the event bit to the
controller's acknowledge port, and checks that the next event read has the
bit clear. That write is both the acknowledge and the end of interrupt. A
timer underflow can set the flag again between clearing it and acknowledging
the controller, so the handler repeats the clear/acknowledge pair a bounded
number of times instead of reporting a false failure. The stop path disables
the interrupt, switches T1 to one-shot mode, loads the longest count and then
clears the flag, so its final IFR read cannot race the next underflow. Before
T1 is enabled the VIA's other interrupt sources are masked: all seven share
one output, and a source the firmware left enabled must not reach the PIC
during the proof.

On the M4753 there is no `via-cuda` node. The same probe now selects
`via-pmu`, logs `ppc: timer transport=pmu via=... line=...`, programs timer 1
through the same `Via` abstraction and expects `IRQ-OK count=10`. QEMU's
`g3beige` has no PMU model, so the gate cannot run this path and does not
pretend to; it is compiled in and the real M4753 check is pending under #16.
The QEMU boot continues to use the CUDA and must keep printing
`ppc: timer transport=cuda` and `IRQ-OK`.

The `0x500` stub shares `trap_entry` with the fault vectors. An external
interrupt is taken at an instruction boundary, so SRR0 already holds the
instruction to resume at and the dispatcher writes it back unchanged. SRR1
holds the interrupted MSR with `EE` set. The boot leaves the controller masked
and the timer stopped while the fault probes run, then starts the timer,
unmasks the tree's line (line `0x12` on `g3beige`) and collects ten interrupts:

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

The handler disables the line and stops the timer as soon as the count
reaches `TARGET`. Drawing the report on the framebuffer makes the handler
slower than the one-millisecond timer, so leaving the source running would
latch a fresh event before the main context could observe the count and the
probe would overshoot ten. `irq::stop` repeats the disable and acknowledge, so
its final pending-event and IFR checks still hold.

## ADB keyboard

The keyboard is the `adb` child of the VIA node. QEMU's tree is
`/pci@80000000/mac-io@10/via-cuda@16000/adb/keyboard@8`; the M4753 tree has
`via-pmu/adb` instead. `platform::adb` looks for `via-cuda`, then `via-pmu`,
reads the VIA window from `reg`, walks the `adb` children for a `keyboard`
node and takes the address from that node's `reg`. The ADB command encoding
and the scancode map are shared; only the outer framing differs.

The CUDA path drives the 6522 shift register and port B. TREQ (PB3, input,
active low), TACK (PB4, output, active low) and TIP (PB5, output, active low)
are the handshake; the shift register is clocked externally (`ACR = 0x0c`)
and shifted out by setting `SR_OUT` (`ACR = 0x10`). `CudaAdb::reset` performs
the Linux `via-cuda` sync: negate TIP/TACK, drain any byte in flight, assert
TACK without TIP, wait for TREQ and the shift-register flag, release TACK and
wait for TREQ to drop. A packet is `[ADB_PACKET, command]`: the first byte is
shifted in, each following byte is written to SR and TACK is toggled, and
negating TIP/TACK closes the packet. The reply is
`[ADB_PACKET, status, data..]`; status zero carries the device data, and a
nonzero status is the CUDA's no-response indication, which is how an empty
keyboard queue appears. A real CUDA may echo the command byte before the
data; a three-byte reply is treated that way, while QEMU's two-byte reply is
not.

The driver deliberately does not issue an ADB bus reset. A reset returns
every device to its default address, and the address the driver uses comes
from the firmware tree; preserving it keeps the two in agreement without an
enumeration pass. It also leaves the VIA's shift-register interrupt disabled,
so the timer interrupt proof is unaffected.

The keyboard is polled with `talk(address, 0)`.  `Modifiers` tracks left and
right shift and caps lock from the transitions, because the direct talk reply
does not carry register 2.  `adb::characters` maps the Apple Standard Keyboard
scancodes to the unshifted and shifted US ASCII characters; letters use shift
XOR caps, the digit and punctuation keys use the shifted symbol. A release has
bit 7 set and no ASCII character. `keyboard::poll` decodes every non-`0xff`
byte in the reply into a `Key { code, down, ascii }` and pushes it into a
32-event ring; `keyboard::pop` is the consumer API for the input path in
#321. `keyboard::wait_for_key` is the boot proof: it polls every 5 ms,
discards events until the first printable press, and is bounded by three
seconds of timebase.

The PMU path is the M4753's hardware route. The same 6522 has TREQ as an
output and TACK as an input, so a byte is sent by writing SR with `SR_OUT` set
and pulsing TREQ; each byte waits for TACK low and then high. An ADB talk is
`PMU_ADB_CMD` (`0x20`) with the arguments `<adb byte>, <flags>, <following
count>`, followed by `PMU_INT_ACK` (`0x78`), whose reply starts with a length
byte and carries the interrupt bits. A packet with `PMU_INT_ADB` and without
`PMU_INT_ADB_AUTO` is the direct reply; `PMU_ADB_POLL_OFF` (`0x21`) is sent
at init so the talk owns the bus. QEMU's g3beige has no PMU model, so this
path cannot run in the gate and is marked pending hardware verification.

Observed on the gate:

```
ppc: adb transport=cuda via=0x81096000 keyboard=8
...
ppc: keyboard transport=cuda address=8
ppc: keyboard reset=ok
ppc: keyboard waiting
ppc: adb key code=0x00 down=1 ascii=0x61 char=a
KEY-OK
ppc: halted
```

## ATA disk

The disk path is the Mac I/O ATA controller, not an ISA IDE port. QEMU's
`hw/ide/macio.c` places each channel in the Mac I/O BAR at a firmware-assigned
offset and gives each task-file register a 16-byte stride. The `ata-3` node
publishes two `reg` ranges — the task-file window and the DBDMA channel — plus
`AAPL,interrupts = <line, dma-line>`. `platform::ata` walks the selected Mac
I/O node's children, accepts `device_type = "ata"` or an `ata*` node name,
prefers the channel with a `disk` child, and takes the first `reg` range. The
walk follows `media-bay`, where the PowerBook puts its `ata1`/`ata4` channel,
so the media-bay disk is eligible as well as the direct `ata0`. The channel
base is the selected Mac I/O BAR plus that offset; no ATA address is compiled
in.

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

## Boot monitor

After the module check the kernel prints `ppc: monitor wait input=a` and polls
the input channel for 1000 ms of timebase. A received byte enters the monitor;
silence prints `ppc: monitor timeout` and the unchanged `ppc: halted` line, so
a headless boot and every gate marker stay the same. The wait is measured on
the timebase and is independent of the decrementer.

The monitor reads one line of at most 32 printable ASCII characters. It echoes
each character, erases with `\b \b` on backspace or delete, ends the line on CR
or LF, and matches whole command names:

| Command | Output |
|---|---|
| `help` | the three command names |
| `info` | Mac I/O base, size and path, console and input channel, memory size, timebase, ATA model and sectors, module status |
| `halt` | `monitor: halt`, `ppc: halted`, then the spin loop |

The prompt is `ppc> ` and is printed before the first line and after every
command. There is no line editor, history, argument parsing, boot command or
module loader. `info` reports copies of the boot values; the module line is
the loader's report, or `skipped=not-gate-disk` when the disk was not the gate
disk. A line whose name matches nothing prints `monitor: unknown command: `
and the line, then returns to the prompt.

The gate types at channel A through a pipe chardev:

```
ppc: monitor wait input=a
ppc: monitor ready
ppc> info
monitor: macio base=0x81080000 size=524288 path=/pci@80000000/mac-io@10 console=a input=a
monitor: memory base=0x0 size=64 MiB
monitor: timebase=16600000 Hz
monitor: ata model=QEMU HARDDISK sectors=8192
monitor: module status=0 tick=1 length=708
ppc> help
monitor: help    list commands
monitor: info    machine selection and status
monitor: halt    stop the machine
ppc> halt
monitor: halt
ppc: halted
```

The first key is consumed by the one-second wait and then processed by the
monitor, so a command can begin in the same write that wakes the machine. The
gate sends each byte separately: QEMU's ESCC model holds a single received
character, and a burst written in one call loses every byte after the first
until the monitor drains the register.

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
  -monitor unix:monitor.sock,server=on,wait=off -no-reboot
```

That boot has no input path. A second boot replaces channel A's file with a
pipe chardev, so the gate can type at the console:

```sh
qemu-system-ppc -M g3beige -m 64M -kernel kernel-ppc.bin \
  -drive file=ata.img,format=raw,if=ide,index=0,media=disk \
  -nographic -chardev pipe,id=mon0,path=serial-monitor \
  -serial chardev:mon0 -serial file:serial-monitor-b.log \
  -monitor none -no-reboot
```

The gate creates `serial-monitor.in` and `serial-monitor.out` and copies
`.out` to `serial-monitor.log`. It waits for `ppc: monitor wait` before
typing. The first commands are written as whole lines: `info`, then `help`.
A single paste then carries another `help`, a backspace-edited `help`
(`helx`, backspace, `p`), a 40-character line and `bogus`. The gate requires
the two help outputs, `monitor: command too long` and
`monitor: unknown command: bogus`, then sends `halt` and requires
`ppc: halted`. This exercises the receive ring, echo, erase, overlong and
unknown paths in one burst.

A further boot proves the B-selected channel. OpenBIOS under `-nographic`
always names channel A, so the gate seeds NVRAM with `auto-boot?=false`
(`-prom-env`), waits at the OpenBIOS prompt, types
`setenv output-device sccb`, then `boot`:

```sh
qemu-system-ppc -M g3beige -m 64M -kernel kernel-ppc.bin \
  -drive file=ata.img,format=raw,if=ide,index=0,media=disk \
  -nographic -chardev pipe,id=b-a,path=serial-b-a \
  -serial chardev:b-a -chardev pipe,id=b-b,path=serial-b-b \
  -serial chardev:b-b -prom-env 'auto-boot?=false' \
  -monitor none -no-reboot
```

The kernel must log `output-device=sccb ... selected=b`, print
`ppc: monitor wait input=b`, and run a pasted `info`/`help`/`halt` on channel
B. `info` must report `console=b input=b`. Channel A remains the OpenBIOS
console; both kernel outputs are still mirrored.

The gate creates `ata.img` as 8192 zero sectors and writes
`IRIX7-ATA-SIG001` at LBA 4. It attaches that image to the first Mac I/O ATA
channel, so the selected Mac I/O node has the disk the kernel identifies.
When the serial log shows `ppc: keyboard waiting`, the gate opens the monitor
socket and sends `sendkey a`; the decoded scancode, ASCII character and
`KEY-OK` are then required in the log.

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
- the primary boot injects `a`, reports `ppc: adb key code=0x00 down=1
  ascii=0x61 char=a`, prints `KEY-OK` and does not print the timeout marker;
- the selected Mac I/O path is a `/pci` node containing `mac-io` and reports at
  least one ESCC child and one ATA child; the ESCC and ATA windows lie inside
  that node's window;
- a second boot with an extra `macio-oldworld` device first in tree order
  still selects the built-in `mac-io@10`, completes the ESCC, ATA, IRQ and
  module path on both channels, receives no input, and reaches
  `ppc: keyboard timeout` plus `KEY-SKIP reason=no-key` before `ppc: halted`;
- the ESCC report names channel B at the window base and channel A at
  `window + 0x20`, and the firmware's `output-device` names the same channel
  the kernel reports as selected;
- the display node is a `/pci` node with `device_type = "display"`, its depth
  is 8, 16 or 32, its `linebytes` equals `width * depth / 8`, and the
  framebuffer BAT covers the whole `linebytes * height` frame;
- the guest self-test read a solid block and a known glyph back from the
  framebuffer and printed `GFX-OK`, on the main boot, the two-controller boot
  and the corrupted-module boot;
- channel B's kernel log is byte-for-byte identical to channel A's from
  `ppc: mmu enabled` onward, so the banner and every marker reached both
  ports;
- the no-input boot prints `ppc: monitor wait input=a`, `ppc: monitor timeout`
  and `ppc: halted`, and never a `ppc>` prompt;
- a pipe-chardev boot enters the monitor, echoes `info`, `help` and `halt`,
  prints three prompts, and repeats the boot's Mac I/O base, size, path,
  console and input channel, memory size, timebase and ATA sector count in the
  `info` output; the module line names the loaded image; the whole monitor
  session is mirrored to channel B;
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
ppc: mmu module base=0x1800000 size=2097152
ppc: mmu fb base=0x80000000 size=16777216
ppc: mmu fb wimg=0x28
ppc: display path=/pci@80000000/QEMU,VGA@1 base=0x80000000 size=16777216 width=800 height=600 depth=32 linebytes=3200
IRIX 7 PowerPC G3 bring-up
ppc: booted
ppc: of client=0xfff025d0
ppc: macio base=0x81080000 size=524288
ppc: macio path=/pci@80000000/mac-io@10
ppc: macio children escc=1 ata=2 disks=1
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
ppc: ata model=QEMU HARDDISK sectors=8192 lba48=1
ppc: ata lba=4 sector=49524958372d4154412d534947303031
ATA-OK
ppc: adb transport=cuda via=0x81096000 keyboard=8
...
ppc: framebuffer bytes=2308
GFX-OK
ppc: module length=708 digest=44d8a8c78bd75eec6a05fcaab7049e83e6c371a3ab187c38fb903bd1d1a3c64e
PPC-MODULE-RUN
module info: ppc: g3beige
module tick
ppc: module status=0 tick=1 length=708
MODULE-OK
ppc: monitor wait input=a
ppc: monitor timeout
ppc: keyboard transport=cuda address=8
ppc: keyboard reset=ok
ppc: keyboard waiting
ppc: adb key code=0x00 down=1 ascii=0x61 char=a
KEY-OK
ppc: halted
```

## Limits

- The kernel maps only its image, the vector page, the Mac I/O window, the
  module slot and the display aperture. RAM beyond the image is not mapped,
  and there is no frame allocator.
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
  not a general interrupt layer. There is no block service, network device or
  PMU housekeeping, and no handler is shared between devices. The ADB keyboard
  is polled directly; its shift-register interrupt stays masked so the timer
  proof owns the controller line.
- The ATA driver is polled PIO only. It reads one 512-byte sector per command,
  addresses LBA28, and performs no DMA, writes, ATAPI commands, partition
  parsing or filesystem access. The block layer and the interrupt-driven path
  come with the disk service slice.
- The PowerBook G3 M4753 has no OpenPIC; a New World machine with the KeyLargo
  OpenPIC needs a separate controller driver and a different machine tree.
  The Gatwick Mac I/O controller's interrupt block is accepted as the same
  Heathrow register layout; no other Gatwick-specific device is driven.
- The PowerBook's timer is the PMU's VIA, not a CUDA. `platform::via_timer`
  looks for `via-cuda` and then `via-pmu` below the selected Mac I/O node and
  programs either through the same 6522 registers, so the M4753 path is
  expected to run the `0x500` proof and print `IRQ-OK`. QEMU has no PMU model:
  the PMU path is compiled and selected from the device tree but its evidence
  is pending a real M4753 run under #16. The keyboard lookup also falls back
  to `via-pmu` and uses the PMU ADB command path; that path is written from
  the Linux `via-pmu` sequence and is likewise pending hardware verification.
  PMU housekeeping (RTC, NVRAM, power events) remains outside this slice.
- The media-bay ATA scan is device-tree support only; no PowerBook media-bay
  hardware has been tested. The gate's second controller is a
  `macio-oldworld` instance with an ESCC and empty ATA channels, so it proves
  tree-order independence and ATA selection, not the Gatwick media-bay
  wiring.
- The console receives on one ESCC channel at a fixed 38400 8N1. It programs
  WR11, WR12, WR13 and WR14 for the 3.6864 MHz ESCC clock and enables both
  receivers, but input is polled on the selected channel only. A 64-byte ring
  between the receiver and the monitor retains a paste; a full ring reports
  `monitor: input overrun`. There is no interrupt-driven input, flow control,
  termios or baud negotiation. RR1 is not read, so an ESCC overrun is only
  visible as a ring overrun.
- The monitor is fixed at three commands with no line editor, history,
  cursor movement or argument parsing. A line longer than 32 characters is
  refused as too long. There is no command to boot, load a module or dump
  memory. It reads the serial channel only; the decoded ADB queue and the
  framebuffer are not input consoles yet.
- The ADB driver has no autopoll, no device enumeration or address assignment,
  no mouse or trackpad support and no keyboard LEDs. It preserves the address
  the firmware assigned instead of resetting the bus, so a keyboard that the
  firmware left at a non-standard address still works as long as the tree
  names it.
- The framebuffer has no keyboard, cursor, mode setting, acceleration or
  multiple-display selection.
- The pre-display log buffer is 4 KiB. A boot that prints more than that
  before the BAT switch keeps the full serial transcript, but the LCD replay
  holds only the first 4 KiB and drops the rest of the mirror.
- The framebuffer format is fixed by the Open Firmware mode. It supports 8 bpp
  indexed, 16 bpp 5:6:5 and 32 bpp `XRGB`; a 15, 20 or 24 bpp mode is refused
  and the serial console carries on. The 8 bpp path assumes the firmware's DAC
  has the standard VGA white at palette entry 15; it does not program the DAC.
  The 32 bpp path assumes the little-endian `B G R X` layout QEMU and the Rage
  LT present on the PCI bus. A device whose framebuffer is big-endian in memory
  would need its own format flag.
- The gate runs `-nographic`, where QEMU leaves the VGA surface at 640x480 text
  mode while the device tree still reports 800x600x32. The gate's pixel proof
  is therefore the guest readback of the block and glyph, not a host
  screenshot. The VNC run in [The mode assumption](#the-mode-assumption)
  checks the visible 800x600 screen.
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
