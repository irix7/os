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
| Removable | CD-ROM |
| Modem | built-in |
| Firmware | Old World ROM with Open Firmware |

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
3. The platform controller (Heathrow-class I/O): OpenPIC interrupts, ATA for the
   disk and CD, ADB for keyboard and trackpad, and the PMU for NVRAM and RTC.
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

## Repository layout

With three machines, the per-machine trees belong under one parent, for example
`machines/indy/`, `machines/x86_64/` and `machines/powerbook-g3/`, with the
shared crates unchanged. The restructure is tracked separately; the PowerBook
work should not start by adding another top-level directory.
