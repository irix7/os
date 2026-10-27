# arch-ADR-0001 — x86_64 architecture boundary and second machine type

**Status:** proposed
**Date:** 2026-10-28
**Issue:** #266 — IRIX 7 x86_64 as the second supported machine type
**Related:** [`docs/arch/`](.), [`kernel/src/arch/`](../../kernel/src/arch), [`targets/x86_64-irix7.json`](../../targets/x86_64-irix7.json)

## Context

IRIX 7 currently supports a single machine: the SGI Indy (MIPS III, n32, big-endian,
kseg0 direct map, CP0 TLB, Z85C30 serial, CP0 Count/Compare timer).  All boot, trap,
TLB, timer, serial and cache-maintenance code is MIPS-specific and is referenced
directly by the shared kernel core (`main.rs`, `heap.rs`, `module.rs`, ...).

Issue #266 asks for x86_64 (QEMU first) as a second supported machine type, booting the
shared kernel core natively to a serial console.  This is not a hosted Linux personality
and not a dual-arch wire format: the module-abi v15 Services contract is identical on
every machine.

## Decision

Introduce an explicit architecture boundary, `kernel/src/arch`, behind a small `Arch`
trait (`kernel/src/arch/interface.rs`).  The shared core (frames, heap, module loader,
service ABI, scheduler, net stack, console) never touches a CPU register, page table,
IDT entry or device port directly; it calls the machine through `arch::arch()`.

Each supported machine provides one `Arch` implementation:

- MIPS (`kernel/src/arch/mips`): a thin delegation layer over the existing MIPS
  bring-up code (`traps`, `tlb`, `idle`, `cache`, `platform`).  The MIPS machine keeps
  its CP0/TLB/exception-vector model unchanged.  `direct_map_offset` returns
  `0x8000_0000` (kseg0).
- x86_64 (`kernel/src/arch/x86_64`): new bring-up code — Multiboot2 entry (`entry.S`),
  GDT (`gdt.rs`), 256-entry IDT with per-vector assembly stubs (`idt.rs`), 4-level
  paging (`paging.rs`), 16550 serial on COM1 (`mod.rs`), CMOS RTC.  Identity-mapped,
  so `direct_map_offset` returns `0`.  Cache maintenance is a no-op (coherent).

The concrete `ArchImpl` type is selected by `#[cfg(target_arch = "mips64")]` /
`#[cfg(target_arch = "x86_64")]`.  The MIPS target's `arch` field is `mips64`, so
`target_arch = "mips64"`.

`heap.rs` now derives the kernel direct-map virtual address from
`arch().direct_map_offset()` instead of hardcoding the MIPS kseg0 offset, so the
frame-backed bump allocator is arch-neutral.

## Scope of this slice

- Architecture boundary trait + MIPS implementation (builds; MIPS release kernel links
  with `kernel/linker.ld` as before).
- x86_64 target spec (`targets/x86_64-irix7.json`) mirroring the builtin
  `x86_64-unknown-none` freestanding configuration (soft-float, `code-model: kernel`,
  `max-atomic-width: 64`).
- x86_64 paging, GDT, IDT, entry.S, 16550 serial, CMOS RTC, `Arch` impl, linker
  script (`kernel/linker-x86_64.ld`).
- `.cargo/config.toml` entry-symbol configuration for x86_64.

## Known limitation (not resolved in this slice)

The x86_64 kernel binary does not yet fully link on the pinned nightly
(`nightly-2026-10-03`).  The TLS/crypto dependency chain pulled in by `embedded-tls`
(`x509-cert` → `sha2`/`polyval` → `cpufeatures`) hits an LLVM codegen error
("Do not know how to split the result of this operator!") on this custom x86_64
target.  This is a dependency/toolchain issue, not an arch-boundary issue.  Until it is
resolved, the x86_64 cross-build cannot produce a bootable image, and the virtio-net /
virtio-blk drivers and module loader on x86_64 are not yet implemented.

The MIPS Indy boot, M2 two-node and module-transport gates are unaffected and remain
green.

## Remaining x86_64 work

- Resolve the `embedded-tls`/crypto dependency chain so the x86_64 kernel links.
- virtio-net and virtio-blk drivers wired through the `PlatformShim` contract.
- x86_64 module loader: fixed slots, SHA-256 digest, flat load at linked slot,
  Services ABI v15 entry.
- QEMU boot CI gate (`-machine q35 -serial stdio -kernel ...`) asserting `BOOT-READY`.
- SMP is a later slice; this slice is uniprocessor.
