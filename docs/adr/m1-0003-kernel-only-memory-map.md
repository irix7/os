# M1-ADR-0003: One kernel image, and its memory map

**Status:** Proposed
**Supersedes:** the module-slot layout in `AGENTS.md` (slots A and B, the 2 MiB module area)
**Date:** 2026-11-04

## Decision

1. The MCP control plane and the Wasm host are linked into the kernel. They are no longer
   loaded as modules. The module loader, the module service table, the slot layout, the
   module SHA-256 and signing checks, and the module artefacts are deleted in the same
   change series. Pre-release policy applies: no compatibility path is kept.
2. The kernel image is the unit the ROM downloads (M1-ADR-0002). Its size is the size of the
   linked kernel, not a slot.
3. The memory map changes as listed below.

## Memory map

Physical addresses. kseg0 is `0x8000_0000 + physical`, kseg1 is `0xA000_0000 + physical`.

| Range | Use | Change |
|---|---|---|
| `0x0804_0000`..`0x0860_0000` | Kernel image, from the ROM download, and kernel stack | Window grows from 3 MiB to 5.75 MiB (ROM, M1-ADR-0002 follow-up) |
| `0x0840_0000`..`0x0860_0000` | Former module area (2 MiB) | Free for the kernel image and its growth |
| `0x0860_0000` | Seeq DMA descriptors and buffers | Unchanged. The kernel image must end below it. |
| `0x0870_0000` | Framebuffer shadow (CPU-only, 3.75 MiB) | Unchanged |
| `0x08B0_0000` | First frame allocator page | Unchanged |
| `0x0800_0000` | DRAM base (`LOMEM_BASE`) | Unchanged |

The kernel stack top moves from `0x883f_0000` to a value the linker script sets below
`0x8860_0000`. Its size stays at 1 MiB unless a measurement shows otherwise.

The ROM heap (TLS records, sockets) lives at kseg1 `0xA8A0_0000`..`0xA8B0_0000`. It sits
above the ROM stack top (`0xA8A0_0000`) and is reused by the kernel after the handoff.

## Consequences

- The kernel image must fit in `0x0804_0000`..`0x0860_0000` less the stack. That is about
  4.75 MiB. The MCP module (431,064 bytes) and the Wasm module (1,346,280 bytes) plus the
  current kernel (about 1.2 MB) come to about 3 MB, so there is room.
- Kernel code and data live in one link. The `ASSERT(__bss_end < __stack_top)` guard in
  `kernel/linker.ld` still applies, and its stack-depth bound is measured as before (#272).
- Gates that booted a separate `mcp.module` or `wasm.module` (`tools/m2-rig/build.sh`,
  the M1 Wasm-autoload job, the relibc probe rig) need their build steps changed. Their
  pass criteria do not change.
- The release pipeline no longer signs modules. The unsigned release decision applies to the
  kernel as well; the signing key is not part of this ADR.

## Not decided here

- The stack size and whether the stack should move below the Seeq area.
- Whether the kernel image should be split again later, for a runtime plug-in mechanism.
- The order of the deletions and the PR breakdown. That is a plan item.

## Validation required before code lands

- The linked kernel fits the window and the stack bound, with a measured stack high-water
  mark.
- The iris boot reaches `BOOT-READY` with the MCP and Wasm code linked into the kernel.
- The M1 and M2 gates pass with no module artefact.

## Migration plan

The change lands as a series of PRs. Each step keeps the M1 and M2 gates green. The
layout change (guest stack after `.bss`, boot stack top above the old module area) lands
in the step that removes the module area, not before.

Module inventory on `main` (non-test Rust lines, excluding `guest/`):

| Module | Lines | Gate that depends on it |
|---|---|---|
| `mcp` | 8001 | M1 boot (`BOOT-READY ... mcp=listening`), M2 two-kernel gate |
| `wasm` | 632 | M1 Wasm autoload (`wasm: run=42`) |
| `manager` | 590 | boot autoload |
| `logical` | 370 | logical namespace |
| `erofs` | 423 | EROFS read path |
| `m2-mesh`, `m2-root` | 262, 294 | M2 two-kernel gate |
| `cache` | 228 | block cache |
| `ports` | 291 | module TCP transport gate (8 TCP slots) |
| `demo-dep`, `demo-root` | 65, 112 | `demo-dep: entered by runtime manager` gate |
| `hello`, `ticker`, `block` | 82, 75, 95 | smoke tests |

Sequence:

1. Port the test-only modules (`hello`, `ticker`, `block`, `demo-dep`, `demo-root`) into the
   kernel and replace their gate markers with kernel markers. Their gates keep their
   text.
2. Port `erofs`, `cache` and `logical`, the storage and namespace code with no network
   dependency.
3. Port `manager` and `ports`. `ports` owns the eight TCP slots, which the transport gate
   measures.
4. Port `wasm` (the probe), with its guest stack and the linker layout change in the same
   PR as the removal of the module area.
5. Port `mcp` and the two mesh modules (`m2-mesh`, `m2-root`). This is the largest step;
   measure the kernel image after each sub-step.
6. Delete the module loader, the module ABI table, the slot layout, the module artefacts
   and the module signing checks. Re-run the M1, M2, module-transport and relibc gates.

Each step records its kernel image size and the stack high-water mark.
