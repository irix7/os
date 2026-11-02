# PowerBook G3 (M4753) as the third machine type

**Status:** target identified, no code. This note records the machine, what IRIX 7
can reuse, what has to be written, and the risks.

## Machine

| Item | Value |
|---|---|
| Model | PowerBook G3 Series 14.1", M4753 |
| CPU | PowerPC 750 (G3), 233 MHz, 512 KiB backside L2 |
| RAM | 32 MiB |
| Storage | 2 GiB IDE |
| Display | 14.1" TFT, 4 MiB video |
| Removable | CD-ROM, 1.44 MB floppy module (optional on some configurations) |
| Modem | built-in |
| Ethernet | built-in 10Base-T |
| Firmware | Old World ROM, Open Firmware 2.0.1 |

The CPU is 32-bit and big-endian, with a PowerPC MMU (segment registers, BATs
and hashed page tables) and a decrementer timer.

## What this changes

IRIX 7 currently has two machine layers: the Indy (MIPS, n32) and x86_64 (QEMU,
PVH). The PowerBook is the second **real** machine and the first 32-bit
big-endian target that is not MIPS. It is not a variant of either existing
machine.

## What is reusable

- `crates/mesh-*`: the mesh protocol, join, routes and transport are
  endian-neutral and already shared.
- `crates/module-tcp`, `irix7-netif`, `frame-alloc`, `irix7-fmt`,
  `irix7-sha256`: no architecture assumptions.
- The module ABI's layout: 32-bit pointers and big-endian fields. The
  `target_pointer_width = "32"` layout assertions are about size and order, and
  a 32-bit big-endian target matches them.
- `modules/mcp` and `modules/wasm` sources, if the heap fits (see risks).

## What has to be written

A new machine layer, in the same shape as the x86 one:

1. Open Firmware entry. The firmware loads an image from CD or disk and hands
   over with a client interface. This replaces the PVH note and the Indy PROM
   handoff.
2. Exception vectors, the MMU (BATs for early identity mapping, then the hashed
   page table), and the decrementer as the tick source.
3. The platform controller (Heathrow-class I/O): the Heathrow interrupt
   controller, ATA for the disk and CD, ADB for keyboard and trackpad, and the
   PMU for NVRAM and RTC. The M4753 is an Old World Heathrow machine; it has
   no OpenPIC, which is the New World controller in KeyLargo.
4. A console: Open Firmware's own console first, then either the serial port or
   the ATI framebuffer.
5. A network device. The listed configuration has a modem; mesh needs Ethernet,
   so the built-in Ethernet (if present on this model) or a PCMCIA card has to
   be confirmed before networking is planned.

## Risks and constraints

- **32 MiB of RAM.** The x86 image uses a 16 MiB Wasm heap and a 2 MiB stack.
  On this machine the frame table must track at most 64 MiB of frames, and the
  Wasm heap must shrink or become dynamic. Start without the Wasm policy module.
- **No built-in Rust target.** There is no `powerpc-unknown-none`; a custom
  target JSON like the MIPS one is needed, built with `-Zbuild-std`. LLVM emits
  ppc32 big-endian code.
- **ABI.** n32 and its syscall numbers are MIPS-specific. The PowerPC machine
  needs the SysV 32-bit ABI for its own code, and a PPC ELF loader for userland.
- **Boot media.** Old World firmware boots from CD or a blessed System Folder.
  The first image should be CD-bootable through Open Firmware, not a disk
  install.
- **No userland yet.** As on x86, userland follows the machine layer.

## Netboot and the floppy

The M4753 has built-in 10Base-T Ethernet, and Open Firmware 2.0.1 can netboot
over it with BOOTP and TFTP.  Open Firmware versions before 3 expect an XCOFF
second-stage loader, so the netboot path is:

```text
0 > boot enet:,IRIX7.XCF
0 > boot enet:192.168.1.5,IRIX7.XCF      (if BOOTP does not supply the server)
```

The built-in NIC is the only netboot path; PCI, CardBus and Airport interfaces
have no Open Firmware drivers.

A 1.44 MB floppy is ample for a netbooter.  The floppy module must be in the
left MediaBay, and Open Firmware loads a file from it directly:

```text
0 > boot fd2:,\IRIX7.XCF
```

NetBSD's `boot.fs` is a full 1.44 MB boot-and-install image, so a loader plus a
kernel fits with room to spare.  A custom loader that fetches the kernel and
the module images over TFTP and jumps to the kernel entry with the Open
Firmware client handler in `r5` is tens of kilobytes.  The work item is the
file format, not the size: Open Firmware 2.0.1 loads XCOFF, while the kernel
build is ELF, so the loader needs an ELF-to-XCOFF wrapper or an XCOFF link
step.  The floppy image is HFS with the loader at the root.

The boot-time network is unauthenticated TFTP.  A loader may check SHA-256 for
the kernel and modules, but that is integrity, not transport security; the
module-trust rule still wants TLS or local media for a production boot.  For
the development loop a trusted LAN is acceptable, and fetching the kernel and
modules in one stage avoids needing a PowerPC NIC driver in the kernel for the
first console tests.

### Release floppy image

The release workflow builds `irix7-powerpc-floppy.img` with
`tools/powerpc-floppy.sh`: a 1.44 MB HFS floppy containing `IRIX7.XCF` and
`README.TXT`.  `IRIX7.XCF` is the kernel wrapped by `tools/ppc-xcoff`; the
wrapper is needed because Open Firmware 2.0.1 loads XCOFF and the kernel links
as ELF.  This image boots the kernel directly; it is not the netbooter.

Write it with a USB floppy drive on macOS:

```sh
hdiutil convert irix7-powerpc-floppy.img -format UDRW -o irix7-powerpc-floppy.dmg
```

Then in Disk Utility: File > Open Disk Image, select the `.dmg`, select the
floppy drive, and Restore.  `sudo dd if=irix7-powerpc-floppy.img of=/dev/rdiskN
bs=512` is the direct alternative; check the device with `diskutil list` before
writing.  The boot command is `boot fd2:,\IRIX7.XCF` with the module in the
left MediaBay.

The image has not been booted on real hardware yet.  The kernel initialises
both ESCC channels, programs both for 38400 8N1 and mirrors the boot log to
both, so the Geoport and the internal IR link both show it.  It also reports
the firmware's `/options:output-device` (or `/chosen:stdout-path`) selection at
boot.  The M4753 developer note wires the IR link to serial port B and the
external Geoport to port A; the mirror makes the wiring choice irrelevant to
whether the banner appears.  The hardware check is the Geoport at 38400 8N1;
channel B is the IR link, so proving it needs the IR transceiver or a logic
probe rather than a serial cable.

## Bring-up slices

1. Open Firmware loads an image and prints to the firmware console; read the
   memory map and the device tree.
2. Exception vectors, decrementer, and BAT-based identity mapping; a deliberate
   fault is reported.
3. Hashed page table and a frame allocator sized for 32 MiB.
4. ATA read, then load a flat module from disk and call its entry.
5. A console the kernel owns (serial or framebuffer).
6. Ethernet and the two-node gate, if the machine has a usable NIC.
7. Userland.

Slice 4's first part is in place: #305 reads the `ata-3` node and its register
window from the device tree, runs `IDENTIFY DEVICE` and one PIO `READ SECTORS`
through the Mac I/O ATA controller, and checks the model, capacity and a
signature sector in the QEMU gate. It is polled, LBA28 and read-only; DMA, the
interrupt path, ATAPI and the module loader follow.

Slice 5's serial half is in place: #313 reads the ESCC `ch-a`/`ch-b` child
nodes and the firmware console property, programs both channels for 38400 8N1
and mirrors the boot log to both. The framebuffer console remains.

## Repository layout

With three machines, the per-machine trees belong under one parent, for example
`machines/indy/`, `machines/x86_64/` and `machines/powerbook-g3/`, with the
shared crates unchanged. The restructure is tracked separately; the PowerBook
work should not start by adding another top-level directory.
