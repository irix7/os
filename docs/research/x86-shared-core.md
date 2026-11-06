# Shared kernel core: module loader and service ABI

Date: 2026-11-03
Issues: [#266](https://github.com/irix7/os/issues/266),
[#330](https://github.com/irix7/os/issues/330)
Branch: `audit-x86`

## Why this slice

The audit in [`docs/audit/closed-platform.md`](../audit/closed-platform.md)
found #266 divergent: `kernel-x86` was a separate kernel with its own module
loader, service table, net, TCP, heap and memory. The issue requires one
architecture boundary around a shared core, with no x86 fork of kernel logic.

A full userland extraction is blocked on the process model in
[#326](https://github.com/irix7/os/issues/326): `kernel/src/fd.rs`,
`ramfs.rs`, `thread.rs` and `native_sys.rs` own global statics, a single
address space and a cooperative thread table. Moving them before per-process
address spaces and fd tables exist would produce a shared module that only one
machine can use. This slice therefore extracts the part that is genuinely
architecture-neutral today and has three consumers: the module loader protocol
and the module service ABI.

## What is now shared

The new crate `crates/kernel-core` has two modules:

- `image` owns the on-disk header (`IRIX7MOD`, big-endian `u32` length,
  SHA-256), the size check, the digest comparison, the vtable bounds and
  alignment check, and the entry call. `Host` supplies only the window and its
  cache maintenance.
- `service` owns the one `module_abi::Services` table, every argument guard,
  the refusal policy for absent resources, and the `Platform` trait. A machine
  implements `Platform` for the resources it owns and installs it once during
  boot.

The three kernels now link it:

| Kernel | Platform implementation | Module windows and transport |
|---|---|---|
| `kernel` (Indy, MIPS) | `kernel/src/services.rs`: console, CP0 clock, heap, module TCP/UDP, NVRAM, status blocks, REX3 shadow, SCSI, arena | two fixed slots, HTTPS/TLS or SCSI fetch |
| `kernel-x86` | `kernel-x86/src/services.rs`: serial, PIT clock, heap, module TCP slots, disk config, MAC info, block store | two 2 MiB windows, HTTP fetch |
| `kernel-ppc` | `kernel-ppc/src/services.rs`: ESCC console, decrementer clock, fixed info blocks | one 2 MiB slot, ATA stream |

Removed duplicated paths:

- The service tables in `kernel/src/module.rs`, `kernel-x86/src/services.rs` and
  `kernel-ppc/src/services.rs` are gone. There is one table, one set of pointer
  and length guards, and one answer for a service a machine does not provide.
- `vtable_in_slot` in the Indy kernel and the inline copy in the PowerPC loader
  are now `kernel_core::image::validate_vtable`. The x86 loader had no such
  check at all: it trusted the vtable pointer returned by module code. Sharing
  the rule fixed that out-of-bounds read.
- The Indy and x86 loaders now use the same two-argument `ModuleEntry` and
  pass a null manager context; the previous one-argument transmute was an
  ABI mismatch waiting for a module that reads the second argument.
- The x86 disk header was a little-endian `u64` at offset 8 with the digest at
  16, and the kernel did not check the magic. It is now the canonical header,
  validated by `parse_header`. The gate and the `m2-rig` disk writer changed
  in the same commit.
- `block-store` and `tcp-slots` gained safe slice entry points, so the
  machine adapters no longer pass raw module pointers through the kernel.

The module TCP slot table was already shared (`tcp-slots`); the mesh
transport was already shared (`mesh-transport`, `mesh-core`, `mesh-service` and
the M2 modules). Neither is forked on x86. The per-machine `tcp_slots::Stack`
adapter remains, because it is the stack accessor, not duplicated policy.

## What remains architecture-specific

- PVH entry, long mode, the identity map, IDT, PIT, virtio-net, virtio-blk and
  the x86 serial port stay in `kernel-x86`. That is the machine layer the issue
  asked to keep.
- MIPS boot, traps, TLB, cache maintenance, SCE/SEEQ, SCSI, REX3, DS1386 and
  the Indy heap stay in `kernel`. PowerPC Open Firmware, BAT entries, traps, ESCC,
  Heathrow ATA and the CUDA VIA stay in `kernel-ppc`.
- Module fetch transports and slot layouts remain per machine. `image::Host`
  separates the copy and cache flush from the transport; the MIPS TLS/SCSI
  path, the x86 HTTP path and the PPC ATA path do not share a transport.
- The runtime arena registry (the `module_arena`, `module_load`,
  `module_execute`, `module_find`, `module_find_export`, `module_query` and
  `module_unload` service bodies) is still in `kernel/src/module.rs`, because
  only the Indy has a fetch hook and a uniform arena today. It is
  architecture-neutral policy and the next candidate to move behind a
  `kernel_core::module` registry with `fetch`, `enter` and `region` hooks.
  The x86 and PowerPC machines answer those services with the shared refusal.
- The heap remains two bump allocators: the Indy allocates frames per request;
  x86 takes one contiguous run at boot. A shared bump allocator over a
  `FrameSource` is possible but has no second consumer with different needs
  yet.
- The userland is still MIPS-only. There is no x86 `user.rs`, `fd.rs`,
  `ramfs.rs`, `thread.rs` or `native_sys.rs` and no x86 syscall gate. The
  machine must not be described as a second POSIX/IRIX target until that gate
  exists. This is the remaining half of #266 and depends on #326.

## Gates

Run on the branch:

- `cargo test -p kernel-core -p tcp-slots -p block-store -p kernel`: pass,
  including the six `kernel-core` image tests and the host service-table test.
- `cargo clippy -p kernel-core --all-targets -- -D warnings`: clean.
- `cargo fmt --all -- --check`: clean.
- `tools/x86-qemu.sh /tmp/opencode/x86-shared`: exit 0. The gate printed
  `x86-qemu: ok (255 MiB usable, exit 1)`, `x86: block service read/write ok`,
  both `sha256 ok` markers, `mcp: entry status=0`, `mcp: listening on port 80`,
  `wasm: run=42` and `x86: exception vector=3 (breakpoint)`; the host HTTP
  probe returned `HTTP/1.0 200 OK`.
- `tools/powerpc-qemu.sh /tmp/opencode/ppc-shared`: exit 0. The gate printed
  `powerpc-qemu: ok (...)`, `MODULE-OK`, `MODULE-DIGEST-BAD` on the corrupted
  image and the monitor transcript.
- MIPS release build with the kernel linker script: succeeds. The debug build
  still fails the linker's `.bss overlaps the boot stack` assertion; that is
  pre-existing on `origin/main` and is not caused by this slice.

The x86 gate is QEMU only. There is no x86 hardware run and no x86 userland
gate, so neither is claimed.

## Verdict for #266

The machine layer, module loader protocol, service ABI, M2 mesh and the shared
crate now satisfy the architecture-boundary part of the issue. The "same
POSIX/IRIX contract level" part does not: there is no x86 userland, fd table,
ramfs, thread table or native syscall gate. #266 should stay open for that
half, or be re-scoped to split "machine and shared module core" (this slice)
from "x86 userland and native gate" (blocked on #326).
