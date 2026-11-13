# PowerBook G3 development shell

**Status:** toolchain, target spec and build check work. There is no PowerBook
machine layer yet; this document covers the cross shell only. The machine
requirements are in [`../research/powerbook-g3.md`](../research/powerbook-g3.md).

## Scope

The PowerBook G3 Series (M4753) is a 32-bit big-endian PowerPC 750 machine with
32 MiB of RAM. This change adds:

- `targets/powerpc-unknown-irix7-ppc.json`, a bare-metal PowerPC target spec;
- `devShells.x86_64-linux.powerbook` in `flake.nix`;
- `tools/powerpc-check.sh`, which proves the shell builds 32-bit big-endian
  PowerPC objects.

This slice does not change the target spec or the PowerPC checks. The default dev shell
was later repinned to the same nightly through `rust-overlay`; see
[M0-ADR-0001](../adr/m0-0001-llvm-only.md).

## Toolchain

| Item | Value |
|---|---|
| Pinned channel | `nightly-2026-10-03`, rustc 1.101.0-nightly (`0abfedbc7`, 2026-10-02), LLVM 23.1.1 |
| Source | stock rustup, `pkgs.rustup` in the shell; `rust-toolchain.toml` selects the channel |
| `rust-src` | installed by `rust-toolchain.toml`; required by `-Zbuild-std` |
| `llvm-tools-preview` | installed by `rust-toolchain.toml`; provides `rust-lld` and `llvm-readobj` |
| Host C compiler | `pkgs.gcc`, needed by the compiler-builtins build script |
| Python | `pkgs.python3`, used by the image tools |
| ELF tools | `pkgs.llvmPackages.bintools` (`llvm-readobj`, `llvm-objcopy`) |
| Emulator | `pkgs.qemu` 11.1.2, which includes `qemu-system-ppc` |

The shell uses rustup rather than the pinned-toolchain derivation because its first
entry installs the channel from `rust-toolchain.toml` into `RUSTUP_HOME`, including
`rust-src` and `llvm-tools-preview`. `RUSTUP_TOOLCHAIN` is set to the pinned channel so
the shell works from outside the repository root.

The irix7 organisation has no Rust or LLVM fork. `irix7/gcc` is a fork of
`rust-lang/gcc` carrying the libgccjit changes needed for the compatibility container's
system toolchain, default branch `master`; `irix7/binutils-gdb` is a read-only mirror of
sourceware's binutils-gdb, default branch `master`. `flake.nix` has two inputs (nixpkgs
and rust-overlay) and `rust-toolchain.toml` names the stock channel, so the pinned
toolchain is stock. Rust code generation uses rustc's LLVM backend only; see
[M0-ADR-0001](../adr/m0-0001-llvm-only.md).

## Target spec

`targets/powerpc-unknown-irix7-ppc.json` follows the shape of the n32 spec:

| Field | Value | Why |
|---|---|---|
| `arch` | `powerpc` | PowerPC 32-bit |
| `cpu` | `750` | The G3 core has no AltiVec and no PowerPC 2.03 `isel` |
| `data-layout` | `E-m:e-p:32:32-Fn32-i64:64-n32` | Matches rustc's built-in `powerpc-unknown-linux-gnu` |
| `llvm-target` | `powerpc-unknown-none-elf` | Accepted by LLVM 23.1.1; there is no built-in Rust target |
| `target-endian` | `big` | The 750 in this machine is big-endian |
| `target-pointer-width` | `32` | 32-bit addresses |
| `max-atomic-width` | `32` | `lwarx`/`stwcx.` cover 32 bits; 64-bit atomics are not lock-free |
| `panic-strategy` | `abort` | No unwinder |
| `relocation-model` | `static` | No PIE, no dynamic loader |
| `position-independent-executables` | `false` | Same |
| `linker` / `linker-flavor` | `rust-lld` / `gnu-lld` | Same linker as the MIPS images |
| `eh-frame-header` | `false` | No unwinding tables |
| `crt-objects-fallback` | `"false"` | No compiler-provided crt objects |
| `os` / `env` / `vendor` | `none` / `""` / `unknown` | Bare metal |

`rustc --print cfg` on the spec reports `target_arch="powerpc"`,
`target_endian="big"`, `target_pointer_width="32"`, `target_os="none"` and
8/16/32/pointer-width atomics.

## Soft-float is not expressible

The first version of the spec disabled the `hard-float` feature so kernel code
would need no FPU state. The pinned nightly rejects that:

```text
error: error loading target specification: target feature `hard-float` is required by the ABI but gets disabled in target spec
```

Adding `"abi": "softfloat"` and `"rustc-abi": "softfloat"` gives:

```text
error: error loading target specification: invalid PowerPC Rust-specific ABI and `cfg(target_abi)` combination:
       Rust-specific ABI: Some(Softfloat)
       cfg(target_abi): softfloat
```

rustc's `RustcAbi::Softfloat` is limited to x86, aarch64 and s390x; the only
PowerPC Rust ABI variant is `powerpc-spe`. No fork changes this: the libgccjit
fork does not alter rustc's target validation, and there is no rustc or LLVM
fork in the organisation. The spec therefore keeps the hard-float ABI.

The 750 has an FPU, so the hardware executes the code. A probe compiled for the
target turns `f64` addition into `stfd`/`fadd`, which confirms that floating
point goes through FPRs. Kernel code must avoid `f32`/`f64` until the machine
layer saves and restores FPRs across context switches; integer-only code never
touches the FPU. A soft-float PowerPC target needs a rustc change (a PPC32
soft-float `RustcAbi` plus LLVM soft-float codegen) and is not available from
the pinned nightly.

## Entering the shell

```sh
nix develop .#powerbook
```

The shell prints the pinned rustc, the target spec path, the check script and
the QEMU version. `nix develop` with no attribute still enters the default shell.

## The check

```sh
tools/powerpc-check.sh
```

The script builds `irix7-fmt`, a dependency-free `no_std` crate, with:

```sh
cargo build -Zbuild-std=core -Zjson-target-spec \
  --target targets/powerpc-unknown-irix7-ppc.json -p irix7-fmt
```

`-Zjson-target-spec` is required by this cargo; without it the build stops with
`` `.json` target specs require -Zjson-target-spec to be added to the cargo
invocation ``. The build directory defaults to `target/powerpc-check`, which is
inside the ignored `/target` tree.

The script then finds `llvm-readobj` (from `llvm-tools-preview`), or falls back
to `readelf`, and checks that every ELF member of the resulting rlib is
`elf32-powerpc`, big-endian, `EM_PPC` and 32-bit. `POWERPC_CHECK_READER` forces
a specific reader, which is how both branches were tested.

Observed output in the shell:

```text
powerpc-check: rustc 1.101.0-nightly (0abfedbc7 2026-10-02)
powerpc-check: target /path/to/os/targets/powerpc-unknown-irix7-ppc.json
powerpc-check: build irix7-fmt
   Compiling compiler_builtins v0.1.160 (...)
   Compiling core v0.0.0 (...)
   Compiling irix7-fmt v0.1.0 (/path/to/os/crates/irix7-fmt)
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 15.26s
powerpc-check: /path/to/os/target/powerpc-check/powerpc-unknown-irix7-ppc/debug/libirix7_fmt.rlib (38482 bytes, 8 ELF members, llvm-readobj)
powerpc-check: OK: elf32-powerpc, big-endian
```

The same archive checked with GNU `readelf` through `POWERPC_CHECK_READER`
reports the same result.

## Results

Commands run on 2026-11-01, in the `powerbook-devshell` worktree:

| Command | Result |
|---|---|
| `nix eval .#devShells.x86_64-linux.powerbook.name` | `"irix7-powerbook-dev"` |
| `nix eval .#devShells.x86_64-linux.default.name` | `"irix7-os-dev"` (unchanged) |
| `nix develop .#powerbook --command rustc --version` | `rustc 1.101.0-nightly (0abfedbc7 2026-10-02)` |
| `nix develop .#powerbook --command qemu-system-ppc --version` | `QEMU emulator version 11.1.2` |
| `nix develop .#powerbook --command tools/powerpc-check.sh` | `OK: elf32-powerpc, big-endian` |
| `rustc --target targets/powerpc-unknown-irix7-ppc.json --print cfg` | `powerpc`, big-endian, 32-bit, `target_os="none"` |

Linker check: `rust-lld -flavor gnu -m elf32ppc` accepts and resolves the produced PPC objects.
With `-Zbuild-std-features=compiler-builtins-mem` the only unresolved symbol is
`__rustc::rust_begin_unwind`, the panic handler the machine layer must supply;
without that feature the link also reports `memcpy`. A real image must enable
the feature and define the panic handler.

## QEMU machine choice

`qemu-system-ppc -M g3beige` models a beige Power Mac G3: Grackle memory
controller, Heathrow I/O and OpenBIOS as Open Firmware. The PowerBook G3 M4753
is a Heathrow-class design, so this is the closest emulated machine for the
first slices and the one a boot gate should use.

`mac99` models a Power Mac G4 with UniNorth and a different interrupt and memory
layout. Neither machine emulates the PowerBook's PMU, ADB trackpad or media
bays; those need real hardware.

## What is missing

- **Boot stub.** There is no `_start`, no Open Firmware client interface call,
  no exception vectors, no BAT setup, no decrementer handler and no panic path.
- **Boot image.** There is no linker script, load address or image format for
  Open Firmware. Old World firmware boots a blessed System Folder or a CD; the
  first image should be CD-bootable through Open Firmware rather than a disk
  install.
- **QEMU gate.** No script boots the image under `qemu-system-ppc -M g3beige`
  and checks serial output.
- **Machine layer.** The Heathrow interrupt controller, ATA, ADB,
  PMU/NVRAM/RTC, a console and a network device are unwritten.
- **Memory sizing.** The frame allocator must be sized for 32 MiB, and the Wasm
  policy module does not fit that budget yet.
- **Userland.** There is no PPC ELF loader and no SysV 32-bit userland ABI.

## Next step

Add the PowerBook machine crate with an Open Firmware entry point:

1. A `#![no_std]` binary built with `-Zbuild-std=core` and
   `-Zbuild-std-features=compiler-builtins-mem`, linked by `rust-lld` with a
   linker script that places `.text` at the address Open Firmware loads to.
2. A `_start` that reads the client interface pointer handed over at entry,
   calls `write` to print a banner, calls `quiesce` for the firmware console,
   and reads the memory node and `/chosen` properties.
3. A `#[panic_handler]` and a deliberate fault to prove the exception vectors
   and the decrementer work under `g3beige`.

The register convention for the client interface pointer is part of the Open
Firmware client interface definition and must be confirmed against the firmware
before the stub is written.
