# Rust `std` build spike for native IRIX 7

**Status:** spike report (evidence; not a design decision)
**Date:** 2026-10-24
**Scope:** build the `std` **rlib** — not link a program — for
`targets/mips3-sgi-irix7-native-n32.json` from a local patched `rust-src` and a
local patched `libc`, and leave a reproducible patch set.
**Toolchain:** `nightly-2026-10-03`; `rustc 1.101.0-nightly (0abfedbc7
2026-10-02)`; `cargo 1.101.0-nightly (f3865b2a4 2026-09-29)`; LLVM 23.1.1.
**Companions:** [`rust-std-irix7.md`](rust-std-irix7.md) (the product decision and
the API contract analysis) and [`../spec/native-libc.md`](../spec/native-libc.md)
(the native ABI specification).

---

## 1. Result

**The `std` rlib builds.** From a clean target directory, the command

```sh
cargo build -Zjson-target-spec -Zbuild-std=std \
  --target targets/mips3-sgi-irix7-native-n32.json
```

exits 0 in 70–80 s on the build host and produces:

| Item | Value |
|---|---|
| rlib | `libstd-1d5ef169874e6597.rlib` |
| size | 18,761,614 bytes (17.9 MiB) |
| md5 | `38f7159a8146eaf813a06b780584a7ce` (this patch revision) |
| object format | ELF32, **big-endian**, `EM_MIPS`, `e_flags = 0x2000_0021` |
| ISA / ABI | `EF_MIPS_ARCH_3` (MIPS III) + `EF_MIPS_ABI2` (n32) |
| `restricted_std` | **absent** (the port has a `std/build.rs` entry) |
| std features | build-std defaults: `backtrace`, `panic-unwind` |

This is a **compile-only** result. There is no native libc, cell loader,
unwinder, TLS block or pthread implementation, and nothing was linked or run.
The rlib still has 167 distinct undefined C symbols (including 11 `_Unwind_*`),
listed in [`../../tools/std-spike/std-undefined-c-symbols.txt`](../../tools/std-spike/std-undefined-c-symbols.txt).
Every libc function in the patch is an `extern "C"` declaration, not an
implementation.

Two independent checks confirm the build really used the patched sources:

- the verbose build log passes `--extern libc=.../libc-std-spike/...` (not the
  registry copy) and compiles `std` from
  `/mnt/europa/rust-src-std-spike/library/std`;
- mangled names for the new module appear in the rlib metadata:
  `libc::unix::irix7::stat`, `...::sockaddr_storage`, `...::addrinfo`.

## 2. Exact method

### 2.1 Pointing `-Zbuild-std` at the patched source

`-Zbuild-std` normally builds from `$(rustc --print sysroot)/lib/rustlib/src/rust`.
Cargo also honours an undocumented environment variable used by the Rust
project's own test suite:

```sh
export __CARGO_TESTS_ONLY_SRC_ROOT=/mnt/europa/rust-src-std-spike/library
```

It replaces the `library` directory that `build-std` resolves from, and needs no
sysroot copy and no change to the installed toolchain. The spike used exactly
this; the build log shows every `rustc` invocation reading
`/mnt/europa/rust-src-std-spike/library/...`.

The installed toolchain was not modified. `rust-src` was copied once (31 MiB)
to the tank, and the libc copy lives beside it.

### 2.2 The libc dependency

`std` declares `libc = "0.2.189"`. Before patching, the build resolved the
**registry** copy (`$CARGO_HOME/registry/src/.../libc-0.2.189`) even though
`rust-src` ships `library/vendor/libc-0.2.189`, reproducing the note in
[`rust-std-irix7.md`](rust-std-irix7.md) §12.1. `-Zbuild-std` resolves `std`'s
dependencies from the **library workspace**, so the redirect must live there:

```toml
# library/Cargo.toml (patched copy only)
[patch.crates-io]
libc = { path = "/mnt/europa/libc-std-spike" }
```

The vendored crates in `library/vendor/` are left untouched. `cargo` updates
`library/Cargo.lock` for the path patch; that lock file is part of the scratch
tree, not the repository.

### 2.3 Reproduce it

```sh
export PATH=/mnt/europa/rust-toolchain/rustup/toolchains/nightly-2026-10-03-x86_64-unknown-linux-gnu/bin:$PATH
export CARGO_HOME=/mnt/europa/rust-toolchain/cargo
export CC=/nix/store/79mr0jw3qccq7hhf1hh62knxd88dwazc-gcc-wrapper-15.3.0/bin/cc
tools/std-spike/build-std.sh          # --no-clean to reuse the scratch trees
```

The script copies `rust-src`, copies libc 0.2.189, applies
[`01-std-irix7.patch`](../../tools/std-spike/patches/01-std-irix7.patch) and
[`02-libc-irix7.patch`](../../tools/std-spike/patches/02-libc-irix7.patch),
wires the `[patch.crates-io]` line, builds the rlib through
`tools/std-spike/probe/`, and fails if `restricted_std` appears in the log.

A host C compiler is needed only for build scripts (`compiler_builtins`,
`libc`, `object`, `std`). It is not used to link the target.

## 3. The `libc` `irix7` module

`src/unix/irix7/mod.rs` (1,147 lines) is a **compile-only** platform module:

| Kind | Count |
|---|---:|
| `pub type` aliases | 29 |
| structs | 33 |
| constants | 304 |
| `extern "C"` declarations | 201 |

It follows [`native-libc.md`](../spec/native-libc.md) §4.1 for the type widths
(`size_t`/`ssize_t` 32-bit, `off_t`/`ino_t`/`dev_t`/`time_t` 64-bit, `long`
32-bit) and the public struct list `std` touches. Notable choices:

- `struct stat` is 128 bytes with `st_dev`, `st_ino`, `st_mode`, `st_nlink`,
  `st_uid`, `st_gid`, `st_rdev`, `st_size`, `st_blksize`, `st_blocks`,
  `st_atim`, `st_mtim`, `st_ctim` and 16 reserved bytes. The POSIX field names
  are used; the Linux `st_mtime_nsec` spelling is **not** added.
- `struct dirent` exposes `d_type` as the documented extension, at the same
  offset `std`'s minimal `dirent64_min` expects (`d_ino` 0, `d_type` 18).
- `errno` uses the Linux/asm-generic numbers recommended by §4.3, with
  `EWOULDBLOCK == EAGAIN` and `ENOTSUP == EOPNOTSUPP`. Signal numbers and
  `O_*`/`F_*`/`SO_*` values are self-consistent placeholders; the
  specification leaves them open.
- `pthread_*_t` are 32-byte opaque, 8-byte-aligned storage with
  `PTHREAD_*_INITIALIZER` constants.
- POSIX-2024 members used by the recommended next patches are declared:
  `getentropy`, `pipe2`, `accept4`, `dup3`, `posix_close`,
  `pthread_cond_clockwait`, `pthread_mutex_clocklock`,
  `posix_spawn`/`posix_spawnp`.
- Documented extensions from §3.4 are declared: `getexecname`,
  `pthread_getattr_np`, `pthread_setname_np`, `ioctl`, `strerror_r`.
- Types `std` never reaches are kept minimal or omitted rather than guessed at
  (`ucontext_t` and the spawn attribute types are omitted; `sem_t` is an
  opaque pointer and `Dl_info` a placeholder struct because the shared Unix
  declarations mention them). The module header lists the omissions.

All structs are generated with `libc`'s `s!` macro, so they are
`Clone`/`Copy`/`Debug` as the Unix PAL expects.

## 4. The `std` patch

Twelve files, in [`01-std-irix7.patch`](../../tools/std-spike/patches/01-std-irix7.patch):

| File | Change | Why |
|---|---|---|
| `std/build.rs` | add `target_os == "irix7"` to the known-platform list | without it every downstream crate needs `#![feature(restricted_std)]` |
| `std/src/os/mod.rs` | declare `pub mod irix7` for `target_family = "unix"` | `std::os::unix` resolves `platform` through `os::irix7` |
| `std/src/os/unix/mod.rs` | `pub use crate::os::irix7::*` in `platform` | same |
| `std/src/os/irix7/mod.rs` | new | module root |
| `std/src/os/irix7/fs.rs` | new `MetadataExt` | `std::os::unix::fs` imports `platform::fs::MetadataExt`; the accessors use `st_atim`/`st_mtim`/`st_ctim` |
| `std/src/os/irix7/raw.rs` | new type aliases | `std::os::unix::raw` imports `platform::raw` |
| `std/src/sys/args/unix.rs` | add `irix7` to the `.init_array` argc/argv implementation | the generic Unix arm is an allow-list |
| `std/src/sys/fs/unix.rs` | add `irix7` to the POSIX `st_mtim`/`st_atim` arms and exclude it from the Linux spelling | the native `struct stat` has no `st_mtime_nsec` |
| `std/src/sys/io/error/unix.rs` | `link_name = "__errno_location"` for `irix7` | the unspecified default would look for a symbol called `errno_location` |
| `std/src/sys/paths/unix.rs` | `current_exe` via `getexecname` | `/proc/self/exe` is not a native requirement |
| `std/src/sys/random/mod.rs` | select the `getentropy` backend for `irix7` | without an arm `fill_bytes` does not exist; POSIX-2024 has `getentropy` |
| `std/src/sys/thread/unix.rs` | add `irix7` to the `pthread_setname_np` arm | there is no generic `set_name` for unlisted Unix targets |

No kernel, ROM, module, target JSON or repository `Cargo.toml` change is needed.
The two committed target files are untouched.

## 5. Verification

### 5.1 The rlib is the right architecture

```text
class 1 data 2 (1=LE,2=BE)     -> ELF32, big-endian
e_machine 0x8 (MIPS)
e_flags   0x20000021           -> EF_MIPS_ARCH_3 | EF_MIPS_ABI2 (n32)
```

Checked on `std-...cgu.00.rcgu.o` extracted from the rlib.

### 5.2 The kernel target still does not get `std`

The same command against `targets/mips3-sgi-irix7-n32.json` (the kernel target,
`target_os = "none"`) still fails at compile time with **14 errors**, exactly
the class documented in [`rust-std-irix7.md`](rust-std-irix7.md) §3.1:

```text
error: none of the predicates in this `cfg_select` evaluated to true
error: Using no_threads implementation on a target with threads
error: unresolved imports `crate::sys::io::decode_error_kind`, ...
error: unresolved imports `super::key::Key`, `super::key::LazyKey`, ...
error: cannot find function `fill_bytes` in module `sys`
error: could not compile `std` (lib) due to 14 previous errors
```

So adding the `irix7` libc module and the std arms does not accidentally give
the kernel target a Unix PAL. The kernel, ROM and modules remain `core` +
`alloc`.

### 5.3 What is still undefined

The rlib has 167 distinct non-Rust undefined symbols: the C ABI `std` expects.
They include `__errno_location` (proving the patched errno arm was used),
`getentropy` (the patched random arm), `getexecname` (the patched
`current_exe`), and 11 `_Unwind_*` functions.

The unwinder is the structural gap: the target is `panic = "abort"`, but `std`
always links the `unwind` crate for backtraces. `panic = "abort"` removes
`catch_unwind`, not the `_Unwind_*` dependency. A native runtime must provide
them, or the `unwinding` crate must gain MIPS support
([`rust-std-irix7.md`](rust-std-irix7.md) §2.4).

## 6. Known gaps and the next patches

The build succeeds because the **fallbacks** compile. It does not show that the
preferred POSIX-2024 paths are used. Each gap below is a deliberate,
small follow-up:

1. **`posix_spawn` is not selected.** `irix7` is not in `std`'s
   `posix_spawn` allow-list, so `std::process::Command` takes `fork` + `execvp`.
   Adding it (and the file-action calls) is the next one-line patch; the libc
   declarations already exist.
2. **Atomic creation forms are not selected.** `socket` + `fcntl(FD_CLOEXEC)`,
   `accept`, and `pipe` + `fcntl` are used instead of `accept4`/`pipe2`/
   `SOCK_CLOEXEC`. POSIX-2024 provides them and the libc module declares them;
   `std` needs `irix7` added to the relevant lists.
3. **Stack-overflow detection is `handler_none`.** The signal handler needs
   `pthread_getattr_np` (declared, but not implemented) and `sigaltstack`.
4. **`File::lock` returns `Unsupported`.** `flock` is not provided (correct);
   the native implementation should use `fcntl` record locks or `lockf` and
   `std` should take that arm.
5. **Thread names** only work if the native libc implements
   `pthread_setname_np`; the declaration exists but nothing backs it.
6. **The libc module is not an ABI.** No layout assertions, no C cross-check,
   no frozen constant values. It is a compile-time sketch that the M9
   `native-abi` crate must replace (research report §7.6).
7. **`restricted_std` removal is local.** The target is "known" only in the
   patched `std/build.rs`; upstreaming is post-v1.
8. **Version lock.** The rlib is tied to this exact nightly. Nothing about it
   is ABI-stable, and the rlib must not be shipped as a system library.

## 7. Cargo freshness on this host

Cargo did not always notice edits to the patched trees on the tank
(`/mnt/europa` is an NFS mount). The visible symptom was a **stale
`build_script_build` binary**: `std/build.rs` was patched, but cargo re-ran the
old binary, which re-emitted `restricted_std` and left it in the rlib. Deleting
the crate's build directory (or starting from a fresh `CARGO_TARGET_DIR`) fixed
it, and the recipe therefore defaults to a clean target directory. The script
also asserts that `--cfg restricted_std` never appears in the verbose log.

The same applies to `libc`: after editing it, force a rebuild rather than
trusting an incremental one.

## 8. What this proves, and what it does not

**Proves:**

- The native target's cfg identity (`os = "irix7"`, `family = "unix"`, n32) is
  sufficient to select the whole Unix PAL once a libc platform module exists.
- The required `std` changes are a target entry in `build.rs`, an
  `os::irix7` re-export module and six small cfg arms — not a 34-file patch.
- The POSIX-2024-shaped libc surface in
  [`native-libc.md`](../spec/native-libc.md) §5 is close enough to compile
  `std`: the only platform types the shared Unix declarations still needed were
  the ordinary C ones (`tm`, `lconv`, `Dl_info`, `wchar_t`, `sem_t`); the rest
  are defined once in `libc`'s `src/unix/mod.rs`.
- The kernel target is not affected.

**Does not prove:**

- That any libc function exists or behaves. Nothing links.
- That the struct layouts are right. They are the specification's recommended
  values, with no C oracle.
- That `std` runs. It cannot: there is no loader, TLS, unwinder, allocator
  backing, pthreads or cell runtime.
- That the constant values are final. `errno` is the recommended default; most
  other numeric sets are placeholders.

## 9. Recommendation

Keep the mechanism and the patch set; do not treat the rlib as a deliverable.
The next useful step is to freeze the libc ABI in the `native-abi` crate,
generate the `libc` `irix7` module from it, and add layout assertions — then
re-run this build as the ABI's first compile-time gate. The `std` patch itself
should be rebased on each toolchain bump and proposed upstream once the native
target and libc have a named maintainer.
