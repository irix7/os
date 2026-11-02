# PowerPC module load

Part of [`powerbook-devshell.md`](powerbook-devshell.md) and issue
[#309](https://github.com/irix7/os/issues/309).

## What runs

The G3 Beige machine reads a flat module from the ATA disk, verifies its
SHA-256, copies it into a reserved 2 MiB slot at `0x0300_0000`, and calls its
entry with the shared `module_abi::Services` table. The module writes
`PPC-MODULE-RUN`, reads `info(0)`, returns a tick callback, and the kernel runs
that tick once before reporting `MODULE-OK`.

- IBAT2/DBAT2 cover the slot as cacheable memory, so the loader reads and
  hashes the image in place. The boot log prints the BAT range as
  `ppc: mmu module base=0x3000000 size=2097152`.
- The slot needs at least 50 MiB of RAM. The gate uses 64 MiB; 32 MiB leaves
  `0x0300_0000` outside DRAM and the boot stops with
  `ppc: memory does not cover the module slot`.
- The module is `modules/hello` built for
  `targets/powerpc-unknown-irix7-ppc.json` with
  `modules/hello/linker-ppc.ld`. It links at `0x0300_0000` and is 708 bytes.

## Disk layout

| LBA | Contents |
|---|---|
| 4 | `IRIX7-ATA-SIG001` |
| 6 | `IRIX7MOD`, the image length as a big-endian `u32`, the 32-byte SHA-256 |
| 16 | the flat module image |

The gate disk is 4 MiB, enough for the signature, the header and a full 2 MiB
image. The header length must be nonzero and no larger than the slot. The
digest covers the image bytes only, not the header or the sector padding.

## Loader

`kernel-ppc/src/module.rs` reads the header, rejects a bad magic or length,
zeroes the whole slot, reads each image sector into the slot while feeding it
to `irix7-sha256`, and compares the digest in constant time. A mismatch zeroes
the slot again and returns without entering the image. On success the loader
writes the data cache back with `dcbst`, invalidates the instruction cache
with `icbi` and synchronises before the call.

The entry pointer is offset 0 of the slot. The returned vtable must lie inside
the verified image; the kernel checks its alignment and bounds, then reads the
status and the optional tick.

## Services

`kernel-ppc/src/services.rs` implements:

- `write`, bounded to 8 KiB per call and sent to the ESCC.
- `clock_ms`, derived from the 100 Hz decrementer.
- `info` for the four documented kinds; kind 0 is `ppc: g3beige`.

Allocation, network, block, screen, console-read, configuration and
module-loader services return a refusal. The machine has no heap, network
stack or block layer in this slice.

## Gate

`tools/powerpc-qemu.sh` builds the kernel and the module, writes the header and
image to the disk, boots at 64 MiB, and keeps every earlier BAT, trap, IRQ and
ATA marker. It asserts the module's digest line, `PPC-MODULE-RUN`,
`module info: ppc: g3beige`, `module tick`, the status line and `MODULE-OK`.

After the normal boot, the gate copies the disk, flips one byte in the module
image and boots again. The second boot must print `MODULE-DIGEST-BAD` and
`ppc: halted`, and must not print `MODULE-OK` or the module marker.

The kernel and module ELF files are both checked for FP instructions.
`tools/module-layout-check.sh` also builds the shared hello source for the G3
and asserts that its ELF entry is `0x0300_0000` and its flat image fits the
slot. The 1.44 MB HFS floppy image build is unchanged and still runs in the CI
job.

## Limits

- One fixed slot, one fixed image, no unload and no dependency loading.
- The digest comes from the disk header. A real boot must take it from the
  signed boot configuration, which this machine does not have yet.
- There is no idle loop after the module entry, so the kernel calls the tick
  once rather than at the 10 ms cadence the Indy kernel uses.
- No network, block, screen, allocation or module-to-module service.

## Next

1. A small idle loop and the 10 ms tick cadence.
2. A network device and the TCP service slots, so MCP can load on the G3.
3. Move the arch-neutral loader and service stubs into a shared crate once the
   Indy, x86 and PowerPC loaders agree on the shape.
