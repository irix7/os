# Rust `std` for IRIX 7

> **Update 2026-10-25:** the compile-only target spike has become a running
> static `std` image (args, time, sleep, a tiny `std::fs` ramfs, `chdir`).
> See [`std-hello-bringup.md`](std-hello-bringup.md).  The production POSIX
> layer is the [relibc fork](https://github.com/irix7/relibc); its n32 Linux/QEMU
> gate passes, while the native PAL and kernel threads/process/termios remain.

**Status:** research report (decision-oriented)
**Date:** 2026-10-24
**Scope:** what “a Rust standard library for IRIX 7” can mean, which product is
load-bearing, the POSIX.1-2024 native API baseline and the binary ABI it
requires, and a concrete implementation plan.
**Pinned toolchain:** `nightly-2026-10-03`, `rustc 1.101.0-nightly (0abfedbc7
2026-10-02)`, `cargo 1.101.0-nightly (f3865b2a4 2026-09-29)`.
**Sources:** `rust-src` at
`/mnt/europa/rust-toolchain/rustup/toolchains/nightly-2026-10-03-x86_64-unknown-linux-gnu/lib/rustlib/src/rust`
(quoted below as `library/...`); the local IRIX 6.5 source capture at
`/mnt/europa/irix7-sources/irix-655-src`; the repository specifications and
ADRs; and the two public Rust-on-IRIX ports listed in §12.

---

## 0. Executive answer

“Rust `std` for IRIX 7” is three different products, and only one of them should
be load-bearing:

1. **A native IRIX 7 `std`**, for new Rust programs running on the native
   capability ABI. This is the product the project needs. It must sit on the
   **new native libc / syscall ABI defined by the M9 native work**, not on IRIX
   6.5 syscall numbers, trap conventions or `struct stat` layouts. (M5 starts
   the separate IRIX 6.5 compatibility ABI, o32 first and n32 later; it is not
   the native one.) It is the standard library that makes the native world’s
   shell, desktop, MCP cell, network/FS clients and user applications writable
   in Rust with the crates they already use.
2. **An IRIX 6.5 compatibility `std`**, hosted inside the legacy compatibility
   container. This is a separate target (`target_os = "irix"`) that emits IRIX
   6.5 binaries using the IRIX 6.5 ABI. It is useful as a **test harness** for
   the compat container and as a way to run Rust tools on a real IRIX 6.5
   system, but it is **not** the native OS ABI and must never define it.
3. **A `std` for kernel or module code.** This is not a product. The kernel and
   loadable modules are `no_std`, kernel-mode, single-address-space code with no
   POSIX process/thread model, no signals, no per-process file descriptors and
   no environment. `std` assumes all of those; linking it into the kernel would
   be a category error. A “`std`-compatible shim” over the module service table
   is also not `std`, and is rejected.

**Recommendation.** Make (1) load-bearing. Build it as a **small patched
`rust-src` + `-Zbuild-std` sysroot** for an `os = "irix7"` n32 target, with no
rustc fork. Design the native libc to the **POSIX.1-2024 base** that `std`
expects — the `*at` family, `O_CLOEXEC`/`O_CLOFORK`, `posix_spawn` (including
`posix_spawn_file_actions_addchdir`), `CLOCK_MONOTONIC`,
`futimens`/`utimensat`, `getentropy`, `getaddrinfo`,
`MAP_ANON`/`MAP_ANONYMOUS`, `pthread_cond_clockwait`, `posix_getdents`, `ppoll`,
`posix_devctl`, `tcgetwinsize`, `dladdr` — plus a short, documented extension
list, instead of porting `std` to IRIX 6.5’s gaps.
POSIX.1-2024 closes most of them; the remaining non-POSIX assumptions
(`pthread_getattr_np`, `pthread_setname_np`, `dl_iterate_phdr`, `flock`,
`madvise`, `getrandom`, `RTLD_DEFAULT`/`RTLD_NEXT`, `readdir`'s `d_type`,
`st_mtime_nsec`, `getexecname`) are documented extensions or `std` patches.
`accept4`, `pipe2`, `dup3`, `SOCK_CLOEXEC`/`SOCK_CLOFORK`, `SOCK_NONBLOCK` and
`MSG_NOSIGNAL` are POSIX-2024 (the last since POSIX-2008), not extensions.
Keep (2) as a separate, non-load-bearing compat/test track. Keep the kernel and
modules on `core`/`alloc`.

If the project instead chooses an IRIX-6.5-shaped native libc, the native `std`
becomes the 34-file patch plus the nine crate patches, `rustix`/`tempfile` stay
blocked, and the native product inherits 1990s POSIX gaps that POSIX.1-2024 and
a new OS have no reason to reproduce. That is the central architectural
decision in this report.

### 0.1 Can we move up to POSIX-2024?

**Yes, for the native world; no, not as a replacement for the IRIX 6.5
compatibility ABI.** POSIX.1-2024 (IEEE Std 1003.1-2024, The Open Group Base
Specifications Issue 8, the core of SUSv5) becomes the native libc’s normative
**source API** baseline, with the capability ABI underneath (D4). The
compatibility container remains the IRIX 6.5 ABI (ADR-0008/0011); POSIX-2024
does not change its syscall numbers, `struct stat`, `errno` values, `rld`
conventions or o32/n32 compatibility promise.

POSIX is an API/source contract, not a binary ABI. The standard itself says
“Interface, Not Implementation” and “Source, Not Object, Portability”
(<https://pubs.opengroup.org/onlinepubs/9799919799/frontmatter/preface.html>).
We still have to define the n32 symbol names, type widths and struct layouts,
syscall/capability invocation numbers, error numbers, thread-local storage,
process startup and linkage, and the dynamic loader. POSIX tells us what the C
source interface must look like; it does not tell us the n32 binary contract.

For Rust `std`, POSIX.1-2024 is a large improvement over IRIX 6.5: it includes
the `*at` family, `futimens`/`utimensat`, `O_CLOEXEC` and the new `O_CLOFORK`,
`MAP_ANON`/`MAP_ANONYMOUS` (new in Issue 8), `getentropy` (new),
`posix_spawn_file_actions_addchdir` (new), `pthread_cond_clockwait`/
`pthread_mutex_clocklock`/`pthread_rwlock_clock*`/`sem_clockwait` (new),
`posix_getdents` (new), `ppoll` (new), `posix_close` (new), `posix_devctl` (new),
`tcgetwinsize`/`tcsetwinsize` (new), `dladdr` (new), `accept4`, `pipe2`, `dup3`,
`SOCK_CLOEXEC`/`SOCK_CLOFORK`/`SOCK_NONBLOCK` and `MSG_NOSIGNAL`. It removes
`ioctl` and `gettimeofday`, so the native libc must use `posix_devctl` and
`clock_gettime` in the POSIX base and keep `ioctl`/`gettimeofday` only as
documented extensions for legacy code. The remaining `std` assumptions that are
**not** POSIX-2024 — `pthread_getattr_np`, `pthread_setname_np`,
`dl_iterate_phdr`, `flock`, `madvise`, `getrandom`, `RTLD_DEFAULT`/`RTLD_NEXT`,
`readdir`'s `d_type`, the Linux `st_mtime_nsec` spelling and `getexecname` — are either
provided as documented extensions or patched in `std` (§5.1.3).

**What must be decided or changed in the governing records.**

- The native target must be **distinct** from the current kernel target. The
  current `targets/mips3-sgi-irix7-n32.json` has `target_os = "none"` and no
  `target_family`; it is the kernel/ROM/module target and must stay that way.
  The proposed native file is `targets/mips3-sgi-irix7-native-n32.json` with
  `target_os = "irix7"` and `target_family = ["unix"]` (§3, §12.1).
- **POSIX.1-2024** becomes the native libc’s normative source API; it is not a
  binary ABI and does not replace the IRIX 6.5 compat ABI. The native libc
  profile plus extension list should become ADR-0021 (§0.1, §5.1, §11).
- **ADR-0018/0019**: `std` must be shown to build and run under
  `rustc_codegen_gcc` before the GCC backend can be called primary for
  userland, or ADR-0018 must be amended to make LLVM primary for userland and
  keep GCC as the parallel/portability backend. This is the highest-risk
  unresolved question (§10.1).
- **ADR-0016** is incorrectly summarised in `docs/spec/architecture.md` §2.1, and the
  repository’s current assembly ROM supersedes both readings. This is a
  documentation-hygiene issue, not a `std` dependency; `std` is not linked into
  the ROM or kernel (§11).
- **ADR-0001/0009/0014/0015**: an IRIX 6.5 `libc` binding module is generated
  from SGI headers and must be treated as private build input, not published in
  `irix7/os`; the native `irix7` libc module is clean-room and publishable
  (§10.4, §11).
- **D2–D4, D9, D11–D13** already support the native-vs-compat split; they do
  not need to change. D7/D8/D9/D17 forbid `std` in the kernel and drivers.

**Two-sentence recommendation.** Make a native IRIX 7 `std`, over the new
capability-backed native libc, the load-bearing product, and treat IRIX 6.5
`std` as a separate compat-container/test track. Ship it as a pinned,
patched-`rust-src` `-Zbuild-std` sysroot for `os = "irix7"` with a libc that
implements the POSIX.1-2024 base `std` expects plus a documented extension list,
while the kernel and modules remain `no_std`.

**Status update (2026-10-24):** the decision is no longer only a proposal.  D32 in
[`docs/spec/architecture.md`](../../docs/spec/architecture.md) adopts POSIX.1-2024 as the
native libc API baseline, and [`docs/spec/posix.md`](../../docs/spec/posix.md) records the
profile and ABI obligations.  This report remains the evidence base and the std-specific
implementation plan.

---

## 1. The three products called “std”

### 1.1 Native IRIX 7 `std` (load-bearing)

The native world is the part of IRIX 7 that is allowed to be new. Its ABI is
capability-based (D11), its process is one cell with a Unix personality (D3),
its libc is Unix-shaped on top of capability invocations (D4), and its cells
load from Rust object files (D13). A native Rust program is therefore a cell
running in the shared kernel address space, with a per-cell capability table,
one or more tasks, a Unix-shaped libc and a set of capability-backed objects.

For that program, `std` must provide `std::io`, `std::fs`, `std::net`,
`std::process`, `std::env`, `std::time`, `std::thread`, `std::sync` and the
`std::os::unix` extension traits, implemented over the **native libc** — which
is itself a new interface, not the IRIX 6.5 libc. The architecture is explicit
that “the native libc is a separate library from the IRIX libc” and that it
presents the familiar surface with capability invocations underneath
(`docs/spec/architecture.md` §5.5, lines 536–549). `std` sits on that libc.

This is the product that is load-bearing because the native desktop, the MCP
cell, the network/FS clients and ordinary user applications are Rust. Without
it, every native program is `no_std` and reimplements strings, vectors, files,
threads, sockets and time; the ecosystem cannot be used; the native world
cannot be a daily driver.

### 1.2 IRIX 6.5 compatibility `std` (separate, not load-bearing)

The compatibility container runs IRIX 6.5 o32/n32 binaries unchanged
(`docs/spec/architecture.md` §6.1). Those binaries use the IRIX 6.5 syscall
numbers, the IRIX 6.5 libc ABI and the IRIX 6.5 ELF/`rld` conventions. A Rust
target that emits those binaries is a **hosted IRIX 6.5 target**, with
`target_os = "irix"` and the IRIX 6.5 ABI. It can run `ripgrep` or `tokio` on a
real IRIX 6.5 machine, and it can exercise the compat container’s syscall and
libc implementation with `std`’s runtime test suite.

It cannot define the native OS ABI. Its `struct stat`, `dirent`, `sockaddr_in6`,
`pthread_*`, `errno` location, syscall numbers and trap convention are IRIX
6.5’s, not IRIX 7’s. Using it as the native target would freeze the native OS
into 1990s POSIX and defeat D2 (“Native ABI is new; IRIX lives only in the
compatibility container”, `docs/spec/architecture.md` line 172).

### 1.3 Kernel/module `std` (not a product)

The kernel is `#![no_std]`, `#![no_main]`, `panic = "abort"` and runs in kernel
mode (`kernel/src/main.rs` lines 14–16; `kernel/Cargo.toml` profiles). Loadable
modules are flat images linked at `0x8840_0000`, capped at 2 MiB, called through
a C service table, and their linker script discards `.eh_frame`
(`docs/spec/modules.md` §3–§4; `modules/hello/linker.ld`). They have no
process, no threads, no signals, no file descriptors and no environment. `std`
requires all of these; its runtime startup (`lang_start`), TLS, panic runtime,
`std::fs`, `std::net` and `std::process` assume a user process.

A “`std`-compatible shim” over the service table (option d in §6) could expose
`println!`, `Vec` and a few files, but it would not be `std`, ecosystem crates
would not compile against it, and it would put a second standard library in the
kernel TCB. The correct module SDK is `core` + `alloc` + `module-abi` +
`embedded-io`, which is what the repository already uses
(`modules/httpd/src/main.rs` lines 9–20; `docs/spec/modules.md` §6).

The kernel/module distinction also matters for size: `std` pulls in `libc`,
`unwind`, `backtrace`, `hashbrown`, `miniz_oxide` and more; a `std`-linked image
will not fit the 2 MiB module area, and the module loader zeroes `.bss` and
discards `.eh_frame` (`docs/spec/modules.md` §3, §5). Native cells are a
different loader (M9) and must allocate from the frame allocator, not the fixed
module area (§8.3).

---

## 2. How Rust `std` is structured today

This section is the source map that the rest of the report relies on. All paths
are relative to the pinned `rust-src`; line numbers are from the pinned tree.

### 2.1 The PAL and the `unix` family

`std` is not a single portable library with small platform hooks. It has a
**platform abstraction layer (PAL)** plus per-feature `sys` modules:

- `library/std/src/sys/pal/mod.rs` lines 6–10: the first `cfg_select!` arm is
  `unix => { mod unix; pub use self::unix::*; }`. The `unix` cfg is set when
  `target_family = "unix"` is set (Rust Reference, “`unix` and `windows`”:
  <https://doc.rust-lang.org/reference/conditional-compilation.html#unix-and-windows>).
- `library/std/src/sys/pal/unix/mod.rs` lines 22–49: the Unix runtime
  initialisation (standard-fd sanitisation, SIGPIPE handling, stack-overflow
  setup, argv capture).
- `library/std/src/sys/fs/mod.rs` lines 8–18: filesystem implementation is
  selected by `any(target_family = "unix", target_os = "wasi")`.
- `library/std/src/sys/thread/mod.rs` lines 53–100: threads are selected by
  `any(target_family = "unix", target_os = "wasi")`.
- `library/std/src/sys/sync/{mutex,condvar,rwlock,once}/mod.rs`: on
  `target_family = "unix"`, mutexes and condition variables use pthreads
  (`library/std/src/sys/sync/mutex/mod.rs` lines 21–24; `condvar/mod.rs` lines
  18–21), while `Once`/`RwLock` use the queue implementation
  (`once/mod.rs` lines 27–36; `rwlock/mod.rs` lines 18–27).
- `library/std/src/sys/thread_local/mod.rs` lines 44–58: if
  `target_thread_local` is set, `std` uses native `#[thread_local]`; otherwise
  it uses the `os` implementation, and on Unix that is the pthread-key
  implementation (`mod.rs` lines 151–168; `key/unix.rs` lines 1–60).

The practical consequence is: **a `target_family = "unix"` target gets the whole
Unix PAL**, including the `libc` dependency, pthreads, `fork`/`exec`, sockets
and the filesystem. There is no “minimal Unix std” switch. The `std` crate is
all-or-nothing at the module level; unsupported operations are either compiled
out by `cfg` or return `Unsupported` at runtime.

### 2.2 The `libc` dependency

`library/std/Cargo.toml` lines 36–39:

```toml
libc = { version = "0.2.189", default-features = false, features = [
    'rustc-dep-of-std',
], public = true }
```

Every Unix PAL operation ultimately calls a `libc::` symbol or constant. The
`libc` crate is therefore the **ABI boundary** between `std` and the OS. For a
new OS, `libc` must have a platform module for the target; without it, the
`unix` module compiles the “Unknown target_os” branch and provides no POSIX
definitions (`library/vendor/libc-0.2.189/src/unix/mod.rs`, final `cfg_if!`
block: “Unknown target_os”). The first `-Zbuild-std=std` attempt for a scratch
`os = "irix"`, `target-family = ["unix"]` target failed exactly there:

```text
error[E0432]: unresolved import `unistd`
   --> libc-0.2.189/src/new/mod.rs:255:17
    |
255 |         pub use unistd::*;
```

This is the concrete proof that adding `target_family = ["unix"]` to the target
JSON is not enough. A `libc` platform module is required; depending on how
modern the native libc is, a few `std` `cfg` arms are also required.

### 2.3 Custom targets and `restricted_std`

A custom JSON target is not in `std/build.rs`’s known-platform list, so
`build.rs` sets `restricted_std` (lines 20–76). `std` then carries an unstable
`restricted_std` attribute (`library/std/src/lib.rs` lines 216–226), and user
crates need `#![feature(restricted_std)]` unless the target is added to
`build.rs` or `RUSTC_BOOTSTRAP_SYNTHETIC_TARGET` is set (build.rs lines 62–63).
For a supported product, add the target to `build.rs`; do not make users write a
feature gate for the OS’s own standard library.

`build-std` itself is the Cargo unstable feature that compiles `core`, `std`,
`alloc` and `proc_macro` from `rust-src` (`Cargo Book`, “build-std”:
<https://doc.rust-lang.org/cargo/reference/unstable.html#build-std>). It
requires a nightly Cargo/rustc and the `rust-src` component, and it must be
passed to every Cargo invocation. `-Zbuild-std-features` selects std features
such as `backtrace` and `panic-unwind`; the default is `backtrace` and
`panic-unwind`.

### 2.4 Panic and unwinding

`std` needs a panic runtime. On Unix, `panic_unwind` selects `gcc.rs`
(`library/panic_unwind/src/lib.rs` lines 36–44), which declares the `_Unwind_*`
API and links the platform unwinder. Even with `panic = "abort"`, `std` still
links the `unwind` crate for backtraces (`library/std/src/lib.rs` line 480),
and on a `unix` target that crate builds the `libunwind` shim and declares
`_Unwind_*` (`library/unwind/src/lib.rs` lines 31–45;
`library/unwind/src/libunwind.rs` lines 24–39). So an abort build does **not**
avoid the unwinder dependency; it only avoids `catch_unwind` and thread-panic
recovery. The choices are:

- provide `_Unwind_*` in the native runtime (a libgcc_s/`unwinding`-style
  implementation), or
- the pure-Rust `unwinding` crate, which `std` already uses for `xous`
  (`library/unwind/Cargo.toml` lines 19–20; `library/unwind/src/libunwind.rs`
  lines 4–9). The crate is MIT OR Apache-2.0
  (`library/vendor/unwinding-0.2.10/Cargo.toml` line 26), but its MIPS support
  is not in the pinned version; the prior art added it out of tree (§12.2).

`std::backtrace` always needs an unwinder (`library/std/src/lib.rs` line 480:
“We always need an unwinder currently for backtraces”). Symbolisation is
optional (`backtrace`/`gimli`), but the unwinder is not.

### 2.5 TLS and threads

`std::thread` on Unix is pthreads: `pthread_attr_init`,
`pthread_attr_setstacksize`, `pthread_create`, `pthread_join`,
`pthread_detach`, `pthread_self` (`library/std/src/sys/thread/unix.rs` lines
55–151). It also uses `sysconf(_SC_NPROCESSORS_ONLN)` for
`available_parallelism` in the generic path (lines ~260), and pthread attributes
for stack size. `std::sync` uses pthread mutexes/condition variables and the
queue/parking implementation, not futexes, on Unix.

TLS has two modes. With `target_thread_local` (native ELF TLS), `std` uses
`#[thread_local]`; without it, it uses `pthread_key_create` /
`pthread_setspecific` / `pthread_getspecific` (`library/std/src/sys/thread_local/key/unix.rs`
lines 22–60). A new OS can choose either; the pthread-key path is the cheaper
first bring-up and is what the IRIX 6.5 port uses (`has-thread-local: false` in
its target JSON).

### 2.6 Filesystem, networking, process and time

- **Filesystem**: `library/std/src/sys/fs/unix.rs` lines 7–18 choose
  `dirent`/`stat`/`open` names for non-Linux Unix targets; lines 275–293 choose
  the `openat`-based `Dir` unless the target is in a path-based fallback list;
  lines 555–640 show the `st_mtim` (AIX) and `st_mtime`/`st_mtime_nsec`
  (generic) timestamp variants; lines 1534–1620 use `futimens`/`utimensat` for
  file times; line 1212 uses `O_CLOEXEC`.
- **Networking**: `library/std/src/sys/net/connection/socket/unix.rs` lines
  70–110 use `SOCK_CLOEXEC` or the `socket` + `fcntl(FD_CLOEXEC)` fallback;
  lines 252–280 use `accept4` or `accept` + `fcntl`; the `MSG_NOSIGNAL` choice
  is in `socket/mod.rs` lines 85–110. DNS goes through `getaddrinfo` (the
  `ToSocketAddrs` implementation in `library/std/src/net/addr.rs`), not through
  a platform-specific resolver.
- **Process**: `library/std/src/sys/process/unix/unix.rs` lines 426–442 use
  `posix_spawn` only for a fixed list of targets and otherwise fall back to
  `fork` + `exec` (the `fork`/`execvp` path is at lines 193 and 413). Pipes use
  `pipe2` where available, otherwise `pipe` + `fcntl` (`library/std/src/sys/pipe/unix.rs`
  lines 14–39).
- **Time**: `library/std/src/sys/pal/unix/time.rs` lines 106–109 call
  `clock_gettime`; the `Instant` clock is `CLOCK_MONOTONIC` by default. The
  condvar uses `CLOCK_MONOTONIC` and `pthread_condattr_setclock` by default
  (`library/std/src/sys/pal/unix/sync/condvar.rs` lines 138–179), with a
  `PRECISE_TIMEOUT = false` fallback for platforms that lack them.

This is why the native libc surface in §5 is not a wish list: each entry is a
symbol or constant that a specific `std` code path already calls.

### 2.7 The target JSON is not the ABI

A target JSON controls codegen (`arch`, `cpu`, `data-layout`, `features`,
`llvm-target`, `relocation-model`, `panic-strategy`, `eh-frame-header`,
`has-thread-local`, `linker-flavor`, `crt-objects-fallback`, and so on). It does
not define the OS ABI. The ABI is defined by the header/libc contract that
`libc` and `std` compile against. That is why this report spends more time on
the native libc than on the JSON.

The JSON schema is printed by the pinned rustc:
`rustc -Zunstable-options --print target-spec-json-schema`. The relevant fields
are `os`, `target-family` (an array of strings), `env`, `abi`, `panic-strategy`
(`"unwind"`, `"abort"`, `"immediate-abort"`), `crt-objects-fallback`
(`"false"`, `"true"`, `"wasm"`, `"musl"`, `"mingw"`), `has-thread-local`,
`eh-frame-header`, `dynamic-linking`, `linker`, `linker-flavor` and
`llvm-target`. The rustc book documents custom targets and the schema:
<https://doc.rust-lang.org/rustc/targets/custom.html>. The target tier policy
is at <https://doc.rust-lang.org/rustc/target-tier-policy.html>.

---

## 3. The target matrix: three products, three targets

The correction that prompted this revision is decisive: IRIX 7 is not IRIX 6.5.
The current target is a **kernel** target. A native `std` needs a **different**
target, and the compat `std` needs a **third** target. They must not share a
`target_os`.

| Product | Proposed target file | `target_os` | `target_family` | `std` | `panic` | ABI | Runs on | Status |
|---|---|---|---|---|---|---|---|---|
| Kernel, ROM, loadable modules | `targets/mips3-sgi-irix7-n32.json` (existing) | `none` | — | no | `abort` | n32/MIPS III | bare metal | current |
| Native cells/apps | `targets/mips3-sgi-irix7-native-n32.json` (proposed) | `irix7` | `["unix"]` | yes | `unwind` (phase 2) | native IRIX 7 n32 | native cell loader (M9) | M9+ |
| IRIX 6.5 compat, n32 | `targets/mips64-sgi-irix-n32.json` (proposed) | `irix` | `["unix"]` | yes | `unwind` or `abort` | IRIX 6.5 n32 | compat container | post-M5/M7, optional |
| IRIX 6.5 compat, o32 | `targets/mips-sgi-irix.json` (proposed) | `irix` | `["unix"]` | yes | `unwind` or `abort` | IRIX 6.5 o32/MIPS II | compat container | optional |

### 3.1 Why the kernel target must not gain `target_family = ["unix"]`

The current target prints:

```text
$ rustc -Zunstable-options --print cfg --target targets/mips3-sgi-irix7-n32.json
target_abi="n32"
target_arch="mips64"
target_endian="big"
target_env=""
target_os="none"
target_pointer_width="32"
target_vendor="sgi"
panic="abort"
```

There is no `target_family`, no `unix`, and `panic = "abort"`. A
`-Zbuild-std=std` build for this target fails at compile time with 14 errors,
including:

```text
error: none of the predicates in this `cfg_select` evaluated to true
  --> library/std/src/sys/alloc/mod.rs:66
error: none of the predicates in this `cfg_select` evaluated to true
  --> library/std/src/sys/io/error/mod.rs:1
error: Using no_threads implementation on a target with threads
  --> library/std/src/sys/sync/condvar/no_threads.rs:6
...
error[E0432]: unresolved imports `crate::sys::thread_local::key::...`
error[E0425]: cannot find function `fill_bytes` ...
error: could not compile `std` (lib) due to 14 previous errors
```

This is the correct failure: the target has `target_has_threads` but no
`target_family`, so `std` cannot choose a Unix PAL, an allocator, an error
backend, a thread-local backend or a random backend. Adding
`target_family = ["unix"]` to this target would be worse: it would select the
Unix PAL while the target still describes a kernel, and a partial libc could
compile a `std` that lies about files, threads and processes. Do not do it.

### 3.2 The native target: `target_os = "irix7"`

The native OS is a new OS. Its target should be a new `target_os`, not `irix`.
The name prevents accidental reuse of IRIX 6.5 `cfg` arms and makes the ABI
boundary visible in every `#[cfg]`. The pinned rustc accepts arbitrary
`target_os` values for custom targets; the schema defines `Os` as a string
(`target-spec-json-schema`, `$defs/Os`). We verified:

```text
$ rustc -Zunstable-options --print cfg --target irix7.json
target_env=""
target_family="unix"
target_os="irix7"
unix
```

An illustrative native target (not committed by this report) is:

```json
{
  "arch": "mips64",
  "cpu": "mips3",
  "abi": "n32",
  "llvm-abiname": "n32",
  "llvm-target": "mips64",
  "data-layout": "E-m:e-p:32:32-i8:8:32-i16:16:32-i64:64-i128:128-n32:64-S128",
  "features": "+mips3,+soft-float,+noabicalls",
  "os": "irix7",
  "target-family": ["unix"],
  "env": "",
  "vendor": "sgi",
  "target-endian": "big",
  "target-pointer-width": 32,
  "max-atomic-width": 64,
  "panic-strategy": "unwind",
  "default-uwtable": true,
  "eh-frame-header": true,
  "has-thread-local": false,
  "relocation-model": "static",
  "linker-flavor": "gnu-lld",
  "linker": "rust-lld",
  "llvm-args": ["-mno-check-zero-division"],
  "crt-objects-fallback": "false"
}
```

The proposed file is `targets/mips3-sgi-irix7-native-n32.json`. The existing
`targets/mips3-sgi-irix7-n32.json` remains the kernel/ROM/module target and is
not changed. The file name is a repository convention; the cfg identity is
`target_os = "irix7"`, `target_family = ["unix"]`, `abi = "n32"`.

Points to decide before committing it:

- **Panic**: `unwind` is the target; `abort` is a bring-up variant, not the
  product. Note that abort still needs the `unwind` crate’s `_Unwind_*` (§2.4);
  it only drops `catch_unwind` and thread-panic recovery. A `std` where a
  thread panic kills the process is a half-std (§8.2).
- **TLS**: `has-thread-local: false` first, using pthread keys. Move to `true`
  only when the native cell loader implements ELF TLS and `__tls_get_addr`.
- **Floating point**: the kernel target uses `+soft-float`. If native user code
  uses hardware FP, the kernel must save and restore FP state per task; the
  architecture currently says the FPU is disabled at boot
  (`docs/spec/boot-protocol.md` §2). Keep `+soft-float` for v1 and treat
  hardware FP as a separate kernel/ABI decision, not a `std` decision.
- **Dynamic linking**: `false` for the first static cell; `true` if the native
  libc becomes a shared cell/library. The architecture says the libc is a cell
  that can be updated independently (`docs/spec/architecture.md` §5.5), which
  points at a shared library eventually; start static (ADR-0007’s static-first
  principle) and make the loader support shared libc later.
- **CRT**: a cell is a `#![no_main]` object with an SDK entry macro that calls
  `std::rt::lang_start`; `crt-objects-fallback: "false"` and the cell linker
  script provide `_start`. A normal `cargo run`/`cargo test` executable needs a
  native CRT and loader, which is a later feature.

### 3.3 The compat target: `target_os = "irix"`

The compat target is a hosted IRIX 6.5 target. The prior art’s target JSON is a
good starting point (with the caveat that it targets 6.5.22, not 6.5.7m):

```json
{
  "arch": "mips64",
  "cpu": "mips3",
  "abi": "n32",
  "llvm-abiname": "n32",
  "llvm-target": "mips64-sgi-irix6.5",
  "llvm-args": ["-mno-check-zero-division"],
  "features": "+mips3,+xgot",
  "os": "irix",
  "target-family": ["unix"],
  "env": "",
  "vendor": "sgi",
  "target-endian": "big",
  "target-pointer-width": 32,
  "max-atomic-width": 64,
  "panic-strategy": "unwind",
  "default-uwtable": true,
  "eh-frame-header": true,
  "has-thread-local": false,
  "dynamic-linking": true,
  "linker-flavor": "unix-cc",
  "linker-is-gnu": false,
  "crt-objects-fallback": "false",
  "crt-static-respected": false,
  "position-independent-executables": false,
  "static-position-independent-executables": false,
  "relro-level": "off",
  "function-sections": false,
  "has-rpath": true,
  "target-mcount": "_mcount",
  "is-like-solaris": false
}
```

This is the IRIX 6.5 ABI, not the native ABI and not a POSIX-2024 target. It
needs the IRIX-patched LLD (`elf32btsmipn32_irix` emulation) and the captured
IRIX sysroot; it is a legacy process inside the compat container. It is not the
native cell ABI and must not share a `target_os` with it.

### 3.4 The `target_os` naming decision

Options:

1. **Distinct `target_os` values** (`irix7` native, `irix` compat). Recommended.
   It makes every `cfg` explicit, prevents the native build from accidentally
   taking an IRIX 6.5 arm, and lets `libc`/`std` patches target the right ABI.
2. **One `target_os = "irix"` with `target_env`** (`irix7` vs `irix65`).
   Workable; `target_env` is a free string in the schema. It keeps one OS name
   but makes the ABI distinction a second-class `env` value, and `libc`’s
   per-OS dispatch would need an `irix` module that branches on `target_env`.
3. **Reuse `target_os = "none"` with `target_family = ["unix"]`.** Rejected: it
   describes the kernel, not a user process, and conflates the two products.

The report recommends option 1. If the project prefers option 2, the same std
patch applies; only the cfg names change.

**Decision.** Use a **separate target file**,
`targets/mips3-sgi-irix7-native-n32.json`, with `target_os = "irix7"`,
`target-family = ["unix"]`, `env = ""` and `abi = "n32"`. Do **not** change the
existing `targets/mips3-sgi-irix7-n32.json` (kernel/ROM/modules, `os = "none"`)
and do not reuse the compat target file. The separate file gives the native PAL
a clean `cfg(target_os = "irix7")`, keeps `cfg(target_os = "irix")` for the
IRIX 6.5 compat target, and makes an accidental cross-link a visible error
rather than a silent ABI mismatch. The file name is a repository convention;
the cfg values are the contract.

---

## 4. What “std for IRIX 7” cannot be

This section is deliberately blunt, because the wrong product definition would
waste the most effort.

### 4.1 It cannot be the IRIX 6.5 std

The IRIX 6.5 ABI is the compatibility contract, not the native ABI. Native IRIX
7 must not inherit:

- IRIX 6.5 syscall numbers (`SYS_*` from `f/root/usr/include/sys.s`, with
  `SYSVoffset = 1000`);
- the IRIX 6.5 trap convention (`$v0` syscall number, `$a3` error flag:
  `f/root/usr/include/sys/syscall.h`, lines 31–40);
- IRIX 6.5 libc layouts (`struct stat` with `st_atim`/`st_mtim` and IRIX-
  specific padding: `f/root/usr/include/sys/stat.h`, lines 36–101);
- IRIX 6.5’s missing `*at` family, `O_CLOEXEC`, `MAP_ANON`, `futimens`,
  `utimensat`, `CLOCK_MONOTONIC`, `getaddrinfo`, `pthread_getattr_np`,
  `pthread_setname_np`, `accept4`, `pipe2`, `SOCK_CLOEXEC`, `F_DUPFD_CLOEXEC`
  (all verified in the 6.5.7m headers; see §5.2 and §12.2).

Those are facts about a 1990s system. A new OS should not reproduce them in its
native ABI. It should reproduce them only inside the compat container, where
they are required for binary compatibility (ADR-0008).

### 4.2 It cannot be a kernel/module library

The kernel is one program at `0x8804_0000`, in kernel mode, with no user
process, no MMU isolation for native cells, and a `panic = "abort"` policy. The
module ABI is a C service table, and module images are flat, 2 MiB-capped and
`.eh_frame`-free. `std` would need a process, threads, signals, files,
environment, a TLS block and an unwinder. None exist in the kernel. A
`std`-shaped API in the kernel would be a second std and a kernel TCB liability.

### 4.3 It cannot be a compiler-ABI-stable system library

Rust `std` has no stable ABI. A `std` rlib is tied to the exact rustc version
and metadata; a cell built with one nightly cannot link a `std` built with
another. The OS must ship a **pinned toolchain/SDK** and rebuild cells/apps
against it, or require `-Zbuild-std` with the pinned nightly. `abi_stable` is a
tool for Rust-to-Rust plugin interfaces, not for `std`. The stable seam is the
native **libc C ABI**, not the Rust std ABI.

This is not a reason not to do it; it is a reason to define the product as “a
pinned SDK”, not “a stable system std”. The compat IRIX 6.5 ABI is stable
(ADR-0008), but a Rust binary for it still carries its own `std`; the IRIX ABI
is stable, the Rust std version is not.

---

## 5. The native libc ABI: POSIX.1-2024 and the extensions

If the native libc implements the POSIX.1-2024 base that `std` needs, plus a
short, documented extension list, the native `std` port is small. If it
reproduces IRIX 6.5’s gaps, the port is the IRIX 6.5 patch set plus nine crate
patches. The native libc is therefore the load-bearing interface, and it must be
designed **for** `std`, not discovered by porting `std` to it.

### 5.1 POSIX.1-2024 is the native API baseline, not a binary ABI

The native libc’s normative API baseline should be **POSIX.1-2024** — IEEE Std
1003.1-2024, The Open Group Base Specifications Issue 8, the core of the Single
UNIX Specification, Version 5 (2024)
(<https://pubs.opengroup.org/onlinepubs/9799919799/mindex.html>). POSIX.1-2024
is Issue 8; the Rationale compares it with Issue 7
(<https://pubs.opengroup.org/onlinepubs/9799919799/xrat/V4_xsh_chap01.html>).
The standard is explicit that it defines an interface, not an implementation:

> “Interface, Not Implementation — POSIX.1-2024 defines an interface, not an
> implementation. No distinction is made between library functions and system
> calls; both are referred to as functions.”
>
> “Source, Not Object, Portability — POSIX.1-2024 has been written so that a
> program written and translated for execution on one conforming implementation
> may also be translated for execution on another conforming implementation.
> POSIX.1-2024 does not guarantee that executable (object or binary) code will
> execute under a different conforming implementation than that for which it was
> translated, even if the underlying hardware is identical.”
>
> — <https://pubs.opengroup.org/onlinepubs/9799919799/frontmatter/preface.html>

Moving to POSIX.1-2024 therefore does **not** give IRIX 7 a binary ABI. It gives
the native libc a portable C **source** contract. We still have to define, for
n32:

- C symbol names and the n32 calling convention (64-bit registers, 32-bit
  pointers, `$a0`–`$a3` plus the stack for arguments, `$v0` for the result);
- `size_t`, `ssize_t`, `off_t`, `ino_t`, `time_t`, `clockid_t`, `dev_t`,
  `mode_t`, `nlink_t`, `blkcnt_t`, `pid_t`, `uid_t`, `gid_t`, `socklen_t` and
  the rest of the POSIX typedefs, with fixed widths and signedness;
- `struct stat`, `struct dirent`, `struct timespec`, `struct timeval`,
  `struct sockaddr_*`, `struct msghdr`, `struct iovec`, `struct sigaction`,
  `siginfo_t`, `ucontext_t`, `pthread_*`, `DIR` and the other exposed layouts;
- the `errno` numbers and the per-thread `errno` location;
- thread-local storage: ELF TLS and `__tls_get_addr`, or the pthread-key
  implementation;
- process startup, `argc`/`argv`/`envp`/`auxv`, stack layout and `lang_start`;
- syscall/capability invocation numbers, the trap convention and the error
  convention;
- the dynamic loader, ELF program headers and `PT_GNU_EH_FRAME`.

POSIX leaves these implementation-defined. The native ABI spec must pin them.
This is exactly why the report distinguishes “the native libc implements
POSIX.1-2024” from “the native ABI is POSIX-2024”: the former is the API, the
latter is not a binary contract.

The native profile should be:

- **POSIX.1-2024 base**, limited to the facilities `std` and the v1 native
  applications need — not the whole standard;
- **selected XSI features** where they are useful (`lockf`, `utimes`, and the
  XSI-marked parts of the file and terminal interfaces);
- **a short, documented extension list** for the Linux/glibc facilities that
  `std` or ecosystem crates assume (§5.1.3);
- **not a claim of POSIX or XSI certification**. Certification requires the full
  standard, the conformance test suites, a conformance document and a vendor
  statement. v1 should publish a “POSIX.1-2024 profile” and a conformance
  matrix, not a certificate.

#### 5.1.1 What POSIX.1-2024 provides that `std` needs

The full list is the System Interfaces table of contents
(<https://pubs.opengroup.org/onlinepubs/9799919799/functions/toc.html>). The
facilities Rust `std` actually uses are:

| Area | POSIX.1-2024 facilities | `std` use |
|---|---|---|
| Processes | `fork`, `_Fork` (new), `posix_spawn` (`[SPN]`), `posix_spawn_file_actions_*` (including `addchdir`/`addfchdir`, new, `[SPN]`), `posix_spawnattr_*`, `exec*`, `wait`, `waitid`, `_exit`, `kill`, `getpid`, `getppid`, `setpgid`, `getpgid`, `tcgetpgrp`, `tcsetpgrp` | `std::process::Command`, `exit`, job control |
| Threads | `pthread_create`, `pthread_join`, `pthread_detach`, `pthread_self`, `pthread_equal`, `pthread_attr_*` (including `pthread_attr_getstack`), `pthread_key_*`, `pthread_once`, `pthread_mutex_*`, `pthread_cond_*`, `pthread_cond_clockwait` (new), `pthread_mutex_clocklock` (new), `pthread_rwlock_*` (including the new `pthread_rwlock_clockrdlock`/`clockwrlock`), `pthread_barrier_*`, `pthread_spin_*` | `std::thread`, `std::sync`, crates |
| TLS | `pthread_key_create`, `pthread_setspecific`, `pthread_getspecific`, `pthread_key_delete` | `std::thread_local` when `has-thread-local: false` |
| Clocks and timers | `clock_gettime` (`CLOCK_REALTIME`, `CLOCK_MONOTONIC`), `clock_getres`, `clock_nanosleep`, `nanosleep`, `timer_create`, `timer_settime`, `timer_gettime`, `timer_getoverrun`, `timer_delete` | `std::time`, `std::thread::sleep` |
| Files | `open`/`openat`, `O_CLOEXEC`/`O_CLOFORK` (the latter new), `F_DUPFD_CLOEXEC`, `close`/`posix_close` (new), `read`/`write`/`pread`/`pwrite`/`readv`/`writev`, `lseek`, `fstat`/`stat`/`lstat`/`fstatat`, `futimens`/`utimensat`, `mkdir`/`mkdirat`, `unlink`/`unlinkat`, `rename`/`renameat`, `link`/`linkat`, `symlink`/`symlinkat`, `readlink`/`readlinkat`, `chmod`/`fchmod`/`fchmodat`, `chown`/`fchown`/`lchown`/`fchownat`, `access`/`faccessat`, `opendir`/`fdopendir`/`dirfd`/`readdir`/`posix_getdents` (new), `realpath`, `getcwd`, `chdir`/`fchdir`, `statvfs`/`fstatvfs`, `pipe`/`pipe2` (new), `dup`/`dup2`/`dup3` (new) | `std::fs`, `std::io`, `std::process` |
| Memory | `malloc`/`calloc`/`realloc`/`free`, `posix_memalign`, `aligned_alloc` (new), `mmap` with `MAP_ANON`/`MAP_ANONYMOUS` (new in Issue 8; `[SHM]`), `munmap`, `mprotect`, `msync`, `posix_madvise` (`[ADV]`), `mlock`/`munlock`, `shm_open`/`shm_unlink` (`[SHM]`) | `std` allocator, `memmap2`, stack-overflow handler |
| Sockets | `socket`, `socketpair`, `bind`, `connect`, `listen`, `accept`, `accept4` (new), `send`/`recv`, `sendto`/`recvfrom`, `sendmsg`/`recvmsg`, `setsockopt`/`getsockopt`, `getsockname`/`getpeername`, `shutdown`, `sockatmark`, `getaddrinfo`/`freeaddrinfo`/`getnameinfo`/`gai_strerror`, `inet_pton`/`inet_ntop`, `if_nameindex`/`if_nametoindex`/`if_indextoname`, `poll`/`ppoll` (new), `pselect`/`select`; `SOCK_CLOEXEC`/`SOCK_CLOFORK`/`SOCK_NONBLOCK` (new), `MSG_NOSIGNAL` (POSIX-2008) | `std::net`, `std::os::unix::net`, mio/tokio |
| Signals | `sigaction`, `sigemptyset`/`sigaddset`/`sigfillset`/`sigdelset`/`sigismember`, `sigprocmask`, `pthread_sigmask`, `sigpending`, `sigsuspend`, `sigwait`/`sigwaitinfo`/`sigtimedwait`, `sigaltstack`, `sigqueue`, `raise`, `kill` | `std` runtime, `Command`, stack overflow |
| Environment and startup | `environ`, `getenv`/`setenv`/`unsetenv`/`putenv`, `getcwd`/`chdir`, `getpid`/`getppid`, `getuid`/`geteuid`/`getgid`/`getegid`, `sysconf`, `confstr`, `pathconf`/`fpathconf`, `uname` | `std::env`, `std::process` |
| Terminal and stdio | `isatty`, `ttyname`, `tcgetattr`/`tcsetattr`, `tcgetwinsize`/`tcsetwinsize` (new), `read`/`write`, `poll` | `std::io::stdio`, `std::process`, `terminal_size` |
| Errors | `errno`, `strerror`, `strerror_r`, `perror` | `std::io::Error` |
| Dynamic loading | `dlopen`, `dlsym`, `dlclose`, `dlerror`, `dladdr` (new) | `libloading`, backtrace symbolisation |
| Randomness | `getentropy` (new) | `std::random`, `rand`/`getrandom` crates |
| Device control | `posix_devctl` (new; Device Control option) | native drivers and `ctl`/`data`/`status`; `ioctl` is not in POSIX.1-2024 (§5.1.4) |

This table is deliberately the **subset** `std` needs, not the whole of
POSIX.1-2024. The standard also defines the shell and utilities, locales, the
C library, asynchronous I/O, message queues, semaphores, shared memory, realtime
signals, typed memory, tracing (removed in Issue 8) and many utilities that
`std` does not use. The native libc does not need to implement them for `std`;
they are separate product decisions.

Some of the entries are **option-group** members rather than base:
`shm_open`/`shm_unlink` `[SHM]`, `posix_madvise` `[ADV]`,
`posix_spawn_file_actions_addchdir` `[SPN]`, `posix_devctl` `[DC]`, `lockf` and
`utimes` `[XSI]`, and `pthread_attr_getstack`/`setstack` `[TSA]`/`[TSS]`. The
native profile in `native-libc.md` must mark which option groups it implements;
“POSIX.1-2024” alone does not imply every option.

At minimum, the v1 native profile needs `[SHM]` for
`MAP_ANON`/`MAP_ANONYMOUS` (the stack-overflow handler and `memmap2`), `[SPN]`
for `posix_spawn`, `[DC]` for `posix_devctl`, and `[TSA]`/`[TSS]` if the
stack-overflow handler uses `pthread_attr_getstack`. `[ADV]` (`posix_madvise`)
and `[XSI]` (`lockf`, `utimes`) are optional and should be listed as such.

#### 5.1.2 New or changed in Issue 8, and why `std` cares

The primary change list is the Rationale’s “Changes from Issue 7 to Issue 8”
and “New Features in Issue 8”
(<https://pubs.opengroup.org/onlinepubs/9799919799/xrat/V4_xsh_chap01.html#tag_22_01_01_01>).
The changes that matter to Rust `std` are:

| Facility | Issue 8 change | Why `std` cares |
|---|---|---|
| `MAP_ANON`/`MAP_ANONYMOUS` | Anonymous memory objects added by Austin Group Defect 850 (`mmap` change history: <https://pubs.opengroup.org/onlinepubs/9799919799/functions/mmap.html>; `[SHM]`) | The stack-overflow handler and `memmap2` need anonymous mappings; IRIX 6.5 had no `MAP_ANON` |
| `accept4`, `pipe2`, `dup3` | New in Issue 8 (accept/pipe/dup change histories; Defects 411, 1318, 1483, 1577: <https://pubs.opengroup.org/onlinepubs/9799919799/functions/accept.html>) | `std::net`, `std::process` and `std::io::pipe` can use the atomic CLOEXEC forms; the old `accept`/`pipe`/`dup` fallbacks remain for older kernels |
| `SOCK_CLOEXEC`/`SOCK_CLOFORK`/`SOCK_NONBLOCK` | New in Issue 8 (`socket` change history; Defects 411 and 1318: <https://pubs.opengroup.org/onlinepubs/9799919799/functions/socket.html>) | `std::net` can create sockets with atomic CLOEXEC/CLOFORK/nonblocking |
| `getentropy` | New in Issue 8 (<https://pubs.opengroup.org/onlinepubs/9799919799/functions/getentropy.html>) | `std::random` and `getrandom` need a source of entropy; no `/dev/urandom` patch |
| `posix_spawn_file_actions_addchdir`/`addfchdir` | New in Issue 8; derived from Solaris `_np` (<https://pubs.opengroup.org/onlinepubs/9799919799/functions/posix_spawn_file_actions_addchdir.html>) | `std::process::Command::current_dir` with `posix_spawn`; `std` already weak-links both the `_np` and POSIX names, so the native libc only has to provide the POSIX name |
| `pthread_cond_clockwait` | New in Issue 8 (<https://pubs.opengroup.org/onlinepubs/9799919799/functions/pthread_cond_clockwait.html>) | Precise `CLOCK_MONOTONIC` condvar timeouts; the current `std` path uses `pthread_condattr_setclock` + `pthread_cond_timedwait` |
| `pthread_mutex_clocklock` | New in Issue 8 (<https://pubs.opengroup.org/onlinepubs/9799919799/functions/pthread_mutex_clocklock.html>) | Precise mutex timeouts for crates; `std::sync::Mutex` does not expose timed lock directly |
| `pthread_rwlock_clockrdlock`/`clockwrlock` | New in Issue 8 (see the System Interfaces TOC) | Precise rwlock timeouts for crates |
| `sem_clockwait` | New in Issue 8 (<https://pubs.opengroup.org/onlinepubs/9799919799/functions/sem_clockwait.html>) | POSIX semaphores used by crates; not `std` |
| `posix_getdents` | New in Issue 8 (<https://pubs.opengroup.org/onlinepubs/9799919799/functions/posix_getdents.html>) | A descriptor-based directory reader; `std::fs::read_dir` can use `readdir` or this |
| `ppoll` | New in Issue 8, on the `poll` page (<https://pubs.opengroup.org/onlinepubs/9799919799/functions/ppoll.html>) | `poll` with a `timespec` timeout and signal mask; mio/tokio can use `poll` |
| `posix_close` | New in Issue 8, on the `close` page (<https://pubs.opengroup.org/onlinepubs/9799919799/functions/posix_close.html>) | Close semantics with `POSIX_CLOSE_RESTART`; `std` uses `close` |
| `posix_devctl` | New in Issue 8; Device Control option (<https://pubs.opengroup.org/onlinepubs/9799919799/functions/posix_devctl.html>) | Replaces the non-POSIX `ioctl`; new drivers use `ctl`/`data`/`status` |
| `tcgetwinsize`/`tcsetwinsize` | New in Issue 8 (<https://pubs.opengroup.org/onlinepubs/9799919799/functions/tcgetwinsize.html>) | Terminal size without `ioctl(TIOCGWINSZ)`; the `terminal_size` crate |
| `dladdr` | New in Issue 8 (<https://pubs.opengroup.org/onlinepubs/9799919799/functions/dladdr.html>) | Backtrace/module lookup; `dl_iterate_phdr` is still not POSIX |
| `_Fork` | New in Issue 8 (System Interfaces TOC) | Async-signal-safe fork; `std` uses `fork` |
| `O_CLOFORK`/`FD_CLOFORK` | New in Issue 8, Austin Group Defect 1318 (`open` change history: <https://pubs.opengroup.org/onlinepubs/9799919799/functions/open.html>) | Close-on-fork descriptors; useful for `fork`/`posix_spawn` and cell isolation |
| `reallocarray`, `memmem`, `strlcpy`, `strlcat` | New in Issue 8 (System Interfaces TOC) | Utility functions used by crates; not `std`-critical |
| `aligned_alloc`, `timespec_get`, C11 threads/atomics (`thrd_*`, `mtx_*`, `cnd_*`, `tss_*`, `call_once`, `atomic_*`) | New in Issue 8 (System Interfaces TOC) | Optional; `std` uses `posix_memalign`, `clock_gettime` and pthreads, not the C11 threads API |

`MSG_NOSIGNAL` is POSIX-2008 (the `send` change history), and
`O_CLOEXEC`/`F_DUPFD_CLOEXEC` are POSIX-2008; they are present in POSIX.1-2024
but not new in Issue 8. `accept4`, `pipe2`, `dup3`,
`SOCK_CLOEXEC`/`SOCK_CLOFORK`/`SOCK_NONBLOCK`, `O_CLOFORK`/`FD_CLOFORK`,
`getentropy`, `MAP_ANON`/`MAP_ANONYMOUS`, `ppoll`, `posix_getdents`,
`posix_close`, `posix_devctl`, `tcgetwinsize`/`tcsetwinsize`, `dladdr`,
`_Fork`, `reallocarray`, `memmem`, `strlcpy`, `strlcat`, the
`pthread_*clock*`/`sem_clockwait` family,
`posix_spawn_file_actions_addchdir`/`addfchdir` and the C11 additions are the
Issue 8 additions.

#### 5.1.3 What `std` assumes beyond POSIX.1-2024

These are the gaps where Rust’s Unix PAL, or a v1 ecosystem crate, assumes a
Linux/glibc or BSD facility that POSIX.1-2024 does not specify. Each is either a
documented native-libc extension or a small `std` patch. The extension list
should be part of the native ABI spec, not an accident. The method was to walk
the Unix PAL code paths (`library/std/src/sys/{pal/unix,fs,net,process,thread,sync,io}`)
and map each `libc::` symbol to the POSIX.1-2024 System Interfaces table of
contents; the rows below are the ones with no POSIX-2024 entry. POSIX-2024
includes `accept4`, `pipe2`, `dup3`, `SOCK_CLOEXEC`/`SOCK_CLOFORK`/
`SOCK_NONBLOCK`, `MSG_NOSIGNAL` (since POSIX-2008) and
`SO_RCVTIMEO`/`SO_SNDTIMEO`; those are **not** gaps and are not in this table.

| Assumption | Where `std`/crates use it | Native decision |
|---|---|---|
| `pthread_getattr_np` | `library/std/src/sys/pal/unix/stack_overflow/handler_signal.rs` lines 368, 611 | Not POSIX. POSIX has `pthread_attr_getstack` (on an attr), not a way to get the current thread’s attr. Provide `pthread_getattr_np` as a documented extension if stack-overflow detection is wanted; otherwise leave `irix7` out of the `handler_signal` list and use `handler_none`. |
| `pthread_setname_np` | `library/std/src/sys/thread/unix.rs` | Not POSIX. Optional extension; without it, thread names stay Rust-side. |
| `dl_iterate_phdr` | `library/backtrace` symbolisation (gimli) | Not POSIX. POSIX-2024 has `dladdr`; the prior art uses `__ehdr_start` and a custom `native_libraries`. Provide `dl_iterate_phdr` as an ELF extension, or use a `dladdr`/`__ehdr_start` backend; symbolised backtraces are optional for v1. |
| `flock` | `std::fs::File::lock` on platforms in its list | Not POSIX. POSIX has `fcntl` record locks and XSI `lockf`. Native libc should implement `fcntl(F_SETLK)`/`lockf`; either patch `std` to use them for `irix7` or leave `File::lock` `Unsupported` (do not fake it). |
| `madvise` | `memmap2` and some crates | Not POSIX; POSIX has `posix_madvise`. Provide `posix_madvise` and either a `madvise` extension or a crate patch. |
| `getrandom` | `getrandom`/`rand` crates | Not POSIX; POSIX-2024 has `getentropy`. Use `getentropy` in `std` and in the crate port; no `/dev/urandom` requirement. |
| `ioctl(FIONBIO)` | `std::net` nonblocking fallback on some targets | `ioctl` is not in POSIX.1-2024. `std` has an `fcntl(F_SETFL, O_NONBLOCK)` arm for Solaris/illumos; add `irix7` to that arm. |
| `ioctl(TIOCGWINSZ)` | `terminal_size` crate | `ioctl` is not in POSIX.1-2024; the standard has `tcgetwinsize`. Patch the crate or provide an `ioctl` extension for legacy. |
| `gettimeofday` | Some crates; `std` uses `clock_gettime` | Removed in Issue 8. Provide `gettimeofday` as a documented extension for legacy code, or patch the crate to `clock_gettime`. |
| `d_type` in `readdir`’s `struct dirent` | `library/std/src/sys/fs/unix.rs` `dirent64_min` | POSIX-2024’s `struct dirent` has only `d_ino`/`d_name`; the new `posix_dent` from `posix_getdents` has `d_type`. Use `posix_getdents` (preferred) or an extension/`lstat`; `std`’s `readdir` path needs an `irix7` arm. |
| `st_mtime`/`st_mtime_nsec` field spelling | `library/std/src/sys/fs/unix.rs` generic `FileAttr` | POSIX-2024 uses `st_mtim`/`st_atim`/`st_ctim`. Add an `irix7` arm using `st_mtim.tv_sec`/`tv_nsec`, or define the Linux spellings in `libc::stat`; prefer the POSIX fields and patch `std`. |
| `posix_spawn_file_actions_addchdir_np` | `library/std/src/sys/process/unix/unix.rs` weak symbol | POSIX-2024 has the non-`_np` `posix_spawn_file_actions_addchdir`. `std` already weak-links both names and tries `_np` first, then the POSIX name; provide the POSIX name, and optionally `_np`. No `std` patch is needed. |
| `/proc/self/exe` | `std::env::current_exe` | Not POSIX. Provide a native loader `getexecname` or a `/proc` extension; otherwise `std` returns an error. |
| `dlsym(RTLD_DEFAULT)` | non-ELF weak-symbol fallback; crates | POSIX-2024 has `dlopen`/`dlsym`; `RTLD_DEFAULT`/`RTLD_NEXT` are extensions. Native ELF can use true weak linkage; provide the extensions if crates need `dlsym` lookup. |

The important point is not that these gaps are large; it is that they are
**known and enumerable**. The prior art’s nine crate patches are mostly the
IRIX 6.5 versions of this table; a POSIX-2024 native libc removes the
`*at`/`futimens`/`O_CLOEXEC`/`MAP_ANON`/`CLOCK_MONOTONIC`/`getaddrinfo`/
`accept4`/`pipe2`/`SOCK_CLOEXEC`/`MSG_NOSIGNAL` gaps entirely. The remaining
entries are the extension and `std` field-spelling/naming gaps above.

The native libc does **not** need the Linux-only `epoll`, `eventfd`, `timerfd`,
`signalfd`, `inotify`, `statx`, `sendfile`, `splice` or `copy_file_range`.
POSIX-2024 gives `poll`/`ppoll`, POSIX timers, `sigwait`/`sigtimedwait`,
`stat`/`fstatat`, and `read`/`write`/`mmap`; that is enough for `std`, mio and
tokio. Crates that hard-require `inotify` or `epoll` need a crate-side port or a
documented extension, not a kernel feature.

#### 5.1.4 What POSIX.1-2024 removes, and what replaces it

The Rationale lists the removed functions
(<https://pubs.opengroup.org/onlinepubs/9799919799/xrat/V4_xsh_chap01.html#tag_22_01_01_04>).
The removals that affect a native libc or Rust crates are:

| Removed in Issue 8 | Replacement |
|---|---|
| `ioctl` | `posix_devctl` (Device Control option) for new code; `ioctl` remains a documented extension for legacy IRIX drivers and crates |
| `gettimeofday` | `clock_gettime(CLOCK_REALTIME)`; `gettimeofday` remains an extension for legacy code |
| `utime` | `futimens`/`utimensat` |
| `setitimer`/`getitimer` | POSIX timers (`timer_create` etc.) and `clock_nanosleep` |
| `sigset`, `sighold`, `sigrelse`, `sigignore`, `siginterrupt`, `sigpause` | `sigaction`, `sigprocmask`, `sigsuspend` |
| `ftw` | `nftw` |
| `tempnam` | `mkstemp`/`mkdtemp` |
| `rand_r` | `rand`/`random` (thread-safety via per-thread state) |
| `setpgrp` | `setpgid` |
| `ulimit` | `getrlimit`/`setrlimit` |
| `_longjmp`/`_setjmp`, `_tolower`/`_toupper`, `isascii`/`toascii` | Standard `longjmp`/`setjmp`, `tolower`/`toupper`, `isascii` replacement logic |
| `pthread_getconcurrency`/`pthread_setconcurrency` | No replacement; not used by `std` |
| `gets` | `fgets`/`getline` |
| STREAMS (`getmsg`, `putmsg`, `getpmsg`, `putpmsg`, `fattach`, `fdetach`) | None; STREAMS is legacy compatibility only (D31) |

The native libc can keep `ioctl` and `gettimeofday` as **documented
extensions** for legacy code and for crates that have not moved to
`posix_devctl`/`clock_gettime`. It should not use them in the POSIX base, and it
should not expose IRIX `ioctl` numbers to native code (§5.5).

#### 5.1.5 XSI/SUSv5 and certification

POSIX.1-2024 is the core of the Single UNIX Specification, Version 5 (2024);
the XSI option is the additional Single UNIX Specification surface
(<https://pubs.opengroup.org/onlinepubs/9799919799/mindex.html>). The native
profile should implement the POSIX base needed by `std`, plus selected XSI and
option-group features (`lockf`, `utimes`, `[SHM]`, `[SPN]`, `[DC]`,
`[TSA]`/`[TSS]` as required). It should **not** claim POSIX or XSI
certification. Certification requires the full standard, the conformance test
suites, a conformance document and a vendor statement; that is a multi-year
product-compliance effort, not an M9 deliverable. Publish:

- a **POSIX.1-2024 profile** document listing the implemented base facilities,
  the selected XSI facilities, and the extensions;
- a **conformance matrix** mapping each facility to its test (the `std`
  runtime suite, `posix_tests`, or a project test);
- a clear statement that unimplemented optional facilities are not claimed.

This is consistent with D1’s IRIX identity: POSIX is the portable source
contract, IRIX is the user experience and the compatibility container.

### 5.2 What the IRIX 6.5 headers show (and why the native ABI must differ)

The 6.5.7m capture is the compatibility oracle. It shows the gaps clearly:

| Facility | IRIX 6.5.7m evidence | Consequence for a native `std` if copied |
|---|---|---|
| `*at` family | No `openat`, `fstatat`, `mkdirat`, `unlinkat`, `renameat` or `fdopendir` in the headers; `dirfd` exists only as a BSD macro over `DIR` (`sys/dir.h` line 102), not as the POSIX function | `std::fs::Dir` uses the path-based fallback; `rustix`/`tempfile`/`terminal_size` blocked |
| `O_CLOEXEC` | `FD_CLOEXEC` exists (`sys/fcntl.h` line 198), `O_CLOEXEC` does not | non-atomic open/dup + `fcntl`; `F_DUPFD_CLOEXEC` absent |
| `accept4`/`SOCK_CLOEXEC` | Not in `sys/socket.h` | `accept` + `fcntl`; non-atomic |
| `pipe2` | Not in the headers | `pipe` + `fcntl`; non-atomic |
| `MSG_NOSIGNAL`/`SO_NOSIGPIPE` | Not in `sys/socket.h` | SIGPIPE must be ignored process-wide |
| `futimens`/`utimensat`/`UTIME_OMIT` | Only `utimes` (`sys/time.h` line 162) | descriptor-form `File::set_times` unsupported; one-timestamp sets unsupported |
| `CLOCK_MONOTONIC` | Only `CLOCK_REALTIME = 1` and `CLOCK_SGI_CYCLE = 2` (`sys/ptimers.h` lines 37–39) | `Instant` needs `CLOCK_SGI_CYCLE`; condvar needs the imprecise fallback |
| `getaddrinfo`/`getnameinfo`/`sockaddr_storage`/`socklen_t` | No `getaddrinfo`/`getnameinfo` declaration in `netdb.h` (only `gethostbyname`/`gethostbyaddr`, lines 106–127); no `sockaddr_storage`/`socklen_t` in the captured headers | no `ToSocketAddrs` hostname resolution; no IPv6-neutral API |
| `dirent.d_type` | `struct dirent` has only `d_ino`, `d_off`, `d_reclen`, `d_name` (`sys/dirent.h` lines 26–30) | `file_type()` needs `lstat` per entry |
| `pthread_getattr_np`/`pthread_attr_getstack` | `pthread_attr_setstacksize`/`getstacksize` exist (`pthread.h` lines 58–59), the getters do not | no stack-overflow handler; thread stack bounds unknown |
| `pthread_setname_np` | Absent | thread names are Rust-side only |
| `flock` | Declared in `sys/file.h` line 99; prior art reports it excludes nothing on 6.5.22 | `File::lock` would silently not lock |
| `SO_RCVTIMEO`/`SO_SNDTIMEO` | Constants exist (`sys/socket.h` lines 106–107); prior art reports `ENOPROTOOPT` on 6.5.22 | socket timeouts fail |
| `MAP_ANON`/`MAP_ANONYMOUS` | Not in `sys/mman.h`; only `MAP_PRIVATE`/`MAP_SHARED`/`MAP_AUTOGROW` (lines 25–31) | anonymous mappings need `/dev/zero`; `memmap2` patch |
| `F_DUPFD_CLOEXEC` | Absent | `dup` + `fcntl` |

The prior art documents these as real runtime gaps on 6.5.22, not just header
absences: `flock` returns success but excludes nothing, `SO_RCVTIMEO` is
rejected, `realpath` predates POSIX.1-2008 and will not allocate, and `madvise`
is a stub (prior art `docs/gaps.md` and `docs/crates.md`). The 6.5.7m runtime
behaviour of the same calls is **unverified**; the headers are the only oracle
in this repository, and the prior art’s runtime results are for 6.5.22.

POSIX.1-2024 closes most of these gaps: the `*at` family,
`futimens`/`utimensat`, `O_CLOEXEC`/`O_CLOFORK`, `F_DUPFD_CLOEXEC`,
`MAP_ANON`/`MAP_ANONYMOUS`,
`getentropy`, `posix_spawn_file_actions_addchdir`, `pthread_cond_clockwait`,
`posix_getdents`, `ppoll`, `posix_devctl`, `tcgetwinsize`,
`_SC_NPROCESSORS_ONLN`/`_SC_HOST_NAME_MAX` and the atomic socket/pipe forms
(`accept4`, `pipe2`, `dup3`, `SOCK_CLOEXEC`/`SOCK_CLOFORK`, `SOCK_NONBLOCK`,
`MSG_NOSIGNAL`) are in the standard. The remaining non-POSIX assumptions are
`pthread_getattr_np`, `pthread_setname_np`, `dl_iterate_phdr`, `flock`,
`madvise`, `getrandom`, `RTLD_DEFAULT`/`RTLD_NEXT`, `readdir`'s `d_type`,
`st_mtime_nsec` and `getexecname`; these are the extension list in §5.1.3. The
native libc should implement the POSIX base and only the extensions it actually
needs; it should not copy IRIX 6.5’s gaps.

### 5.3 The native libc surface required by `std`

The following table is the contract the native libc must meet for the v1 `std`
subset. “Required” means `std` or a v1 crate calls it; “optional” means a
`std` API or a crate can degrade without it. Unless marked “(extension)”, each
item is POSIX.1-2024 base or XSI as noted; the extension list is §5.1.3.

#### Process and tasks

| Symbol/contract | Required by | Notes |
|---|---|---|
| `posix_spawnp`, `posix_spawn_file_actions_init/destroy/adddup2/addclose/addopen`, `posix_spawn_file_actions_addchdir`/`addfchdir` (POSIX-2024; `_np` names are extensions), `posix_spawnattr_init/destroy/setflags/setsigdefault/setpgroup`, `_Fork` (new) | `std::process::Command` | The preferred path per D3; without it `Command` falls back to `fork` + `exec`, which copies the cell’s memory. `std` already weak-links both the POSIX and `_np` names; provide the POSIX name, and optionally `_np`. |
| `fork`, `execve`, `execvp`, `_exit`, `waitpid`, `waitid`, `kill`, `getpid`, `getppid`, `setpgid`, `getpgid`, `tcgetpgrp`, `tcsetpgrp` | `std::process`, shells, job control | `fork` is allowed for small cells; `posix_spawn` is the default |
| `pthread_create`, `pthread_join`, `pthread_detach`, `pthread_self`, `pthread_equal`, `pthread_attr_init/destroy/setstacksize/getstacksize/setguardsize/getguardsize`, `pthread_attr_getstack`, `pthread_key_create/delete/getspecific/setspecific`, `pthread_once`, `pthread_mutex_*`, `pthread_mutex_clocklock` (new), `pthread_cond_*`, `pthread_condattr_setclock`, `pthread_cond_clockwait` (new), `pthread_rwlock_*`, `pthread_rwlock_clockrdlock`/`clockwrlock` (new); `pthread_getattr_np` (extension) | `std::thread`, `std::sync`, crates | Pthreads over native tasks; `pthread_getattr_np` is only needed for the stack-overflow handler (not POSIX) |
| `pthread_setname_np` | `std::thread::Builder::name` | Optional; without it names stay Rust-side |
| `sched_yield`, `nanosleep`, `clock_nanosleep` | `std::thread::yield_now/sleep` | |
| `sigaction`, `sigemptyset`, `sigaddset`, `sigprocmask`, `sigaltstack`, `raise` | `std` runtime, `Command`, stack overflow | Native signal model is `Port` events; the libc presents POSIX signal shapes |
| `getrlimit`, `setrlimit` | `std::process`, crates | Optional for v1 |

#### Memory

| Symbol/contract | Required by | Notes |
|---|---|---|
| `malloc`, `calloc`, `realloc`, `free`, `posix_memalign` | `std` global allocator, `alloc` | `posix_memalign` is the normal `std` path; `memalign` only if POSIX coverage is incomplete |
| `mmap`, `mmap64`, `munmap`, `mprotect`, `msync`, `posix_madvise`, `mlock`, `munlock`, `shm_open`, `shm_unlink`, `sysconf(_SC_PAGESIZE)`; `MAP_ANON`/`MAP_ANONYMOUS` (POSIX-2024, `[SHM]`); `madvise`, `brk`, `sbrk`, `getpagesize` (extensions/implementation details) | `std`, allocators, `memmap2` | `MAP_ANON`/`MAP_ANONYMOUS` is now POSIX-2024 (`[SHM]`), so no `/dev/zero` workaround; `madvise` remains an extension, `posix_madvise` is the POSIX call |
| `MAP_PRIVATE`, `MAP_SHARED`, `MAP_FIXED`, `PROT_*`, `MAP_FAILED` | `std`, crates | Standard values; `MAP_FAILED` must be `*mut c_void` in `libc` |

#### Files

| Symbol/contract | Required by | Notes |
|---|---|---|
| `open`, `open64`, `openat`, `close`, `posix_close` (new), `read`, `write`, `pread`, `pwrite`, `lseek`, `lseek64`, `fsync`, `fdatasync`, `ftruncate`, `ftruncate64` | `std::fs`, `std::io` | `openat` is the key gap-closer; `O_CLOEXEC` (POSIX-2008) and `O_CLOFORK` (POSIX-2024) should be atomic |
| `stat`, `stat64`, `fstat`, `fstat64`, `lstat`, `lstat64`, `fstatat` | `std::fs` | `struct stat` must expose `st_dev`, `st_ino`, `st_mode`, `st_nlink`, `st_uid`, `st_gid`, `st_rdev`, `st_size`, `st_blksize`, `st_blocks`, `st_atim`, `st_mtim`, `st_ctim`; 64-bit `off_t`/`ino_t` |
| `mkdir`, `mkdirat`, `rmdir`, `unlink`, `unlinkat`, `rename`, `renameat`, `link`, `linkat`, `symlink`, `symlinkat`, `readlink`, `readlinkat`, `chmod`, `fchmod`, `fchmodat`, `chown`, `fchown`, `lchown`, `utimensat`, `futimens`, `realpath`, `getcwd`, `chdir`, `fchdir`, `access`, `faccessat`, `mkfifo`, `mknod`, `statvfs`, `fstatvfs`; `utimes` (XSI) | `std::fs`, `std::os::unix::fs` | `futimens`/`utimensat` with `UTIME_OMIT`/`UTIME_NOW`; `realpath` must allocate per POSIX.1-2008 |
| `opendir`, `readdir`, `readdir_r`/`readdir64`, `closedir`, `dirfd`, `fdopendir`, `posix_getdents` (new; `posix_dent.d_type`); `dirent.d_type` (extension) | `std::fs::read_dir` | POSIX-2024 gives `d_type` through `posix_dent`; `readdir`’s `struct dirent` does not. Use `posix_getdents` or an extension/`lstat`; `std` needs an `irix7` arm. |
| `O_CLOEXEC`, `O_CLOFORK` (new), `O_DIRECTORY`, `O_NOFOLLOW`, `O_NONBLOCK`, `O_APPEND`, `O_CREAT`, `O_TRUNC`, `O_EXCL`, `F_DUPFD_CLOEXEC`, `FD_CLOEXEC`, `FD_CLOFORK` (new), `AT_FDCWD`, `AT_SYMLINK_NOFOLLOW`, `AT_REMOVEDIR`, `UTIME_OMIT`, `UTIME_NOW` | `std::fs`, `std::os::unix::fs` | The modern constants are cheap to define in a new libc; `O_CLOFORK`/`FD_CLOFORK` help `fork`/`posix_spawn` isolation |
| `fcntl` (`F_SETLK`/`F_GETLK`), `lockf` (XSI); `flock` (extension) | `std::fs::File::lock` | POSIX record locks are `fcntl`/`lockf`; `flock` is not POSIX. Patch `std` for `irix7` or leave `File::lock` `Unsupported`; do not fake it. |

#### Networking

| Symbol/contract | Required by | Notes |
|---|---|---|
| `socket`, `socketpair`, `bind`, `connect`, `listen`, `accept`, `accept4` (new), `shutdown`, `send`, `recv`, `sendto`, `recvfrom`, `sendmsg`, `recvmsg`, `setsockopt`, `getsockopt`, `getsockname`, `getpeername`, `poll`, `ppoll` (new), `select`, `pselect`; `SOCK_CLOEXEC`/`SOCK_CLOFORK`/`SOCK_NONBLOCK` (new), `MSG_NOSIGNAL` (POSIX-2008) | `std::net`, `std::os::unix::net`, crates | POSIX-2024 provides all of these; no extension is needed. The old `accept`/`socket` + `fcntl` fallbacks remain for older kernels. |
| `getaddrinfo`, `freeaddrinfo`, `getnameinfo`, `gai_strerror`, `inet_pton`, `inet_ntop`, `if_nametoindex`, `if_indextoname` | `ToSocketAddrs`, `TcpStream::connect`, crates | Must support IPv4 and IPv6; the native network stack already has DNS (`kernel/src/dns.rs`) |
| `sockaddr_storage`, `socklen_t`, `sa_family_t`, `in6_addr`, `IPV6_V6ONLY`, `TCP_NODELAY`, `SO_REUSEADDR`, `SO_RCVTIMEO`, `SO_SNDTIMEO`, `MSG_NOSIGNAL` | `std::net` | All POSIX-2024/2008; socket timeouts must actually work (IRIX 6.5.22 rejected them with `ENOPROTOOPT`) |

#### Time, environment, errors, loading, randomness

| Symbol/contract | Required by | Notes |
|---|---|---|
| `clock_gettime` with `CLOCK_REALTIME` and `CLOCK_MONOTONIC`, `clock_getres`, `clock_nanosleep`, `nanosleep`, `timer_create`/`timer_settime`/`timer_gettime`/`timer_getoverrun`/`timer_delete`; `gettimeofday` (extension; removed in Issue 8) | `std::time`, `std::thread` | `Instant` uses `CLOCK_MONOTONIC`; the kernel already has a monotonic timebase (`docs/spec/architecture.md` §8.4); `gettimeofday` is only for legacy crates |
| `environ`, `getenv`, `setenv`, `unsetenv`, `putenv`, `argv`/`argc`, `sysconf` (including `_SC_NPROCESSORS_ONLN`, `_SC_HOST_NAME_MAX`), `confstr`, `pathconf`/`fpathconf` | `std::env`, `std::thread`, `std::net` | The cell loader supplies argv/env; the libc exposes `environ`; `_SC_NPROCESSORS_ONLN`/`_SC_HOST_NAME_MAX` are POSIX-2024, so no IRIX-style hostname patch |
| `__errno_location` (or a target-specific `__errno` link name), `strerror`, `strerror_r` | `std::io::Error` | Pick one and keep it stable; `std`’s default is `__errno_location` (`library/std/src/sys/io/error/unix.rs` lines 22–36) |
| `dlopen`, `dlsym`, `dlerror`, `dlclose`, `dladdr` (new); `RTLD_DEFAULT`/`RTLD_NEXT`, `dl_iterate_phdr` (extensions) | crates, backtrace, plugins | POSIX-2024 has the dynamic loader and `dladdr`; `RTLD_DEFAULT`/`RTLD_NEXT` and `dl_iterate_phdr` are the ELF/glibc extensions |
| `getentropy` (new); `getrandom` (extension), `/dev/urandom` (platform device) | `std::random`, `getrandom`, `rand` | `getentropy` is POSIX-2024 and is the preferred backend; no `/dev/urandom` patch |
| `write`, `read`, `isatty`, `poll`, `/dev/null`; `tcgetwinsize`/`tcsetwinsize` (new), `posix_devctl` (new) | `std::io::stdio`, runtime init, terminal, drivers | `/dev/null` is used by `std`’s startup; `tcgetwinsize` avoids `ioctl`; `posix_devctl` is the POSIX-2024 device-control call |

### 5.4 The native libc must be the capability enforcement point

`std::fs`, `std::net` and `std::process` are path/name based. In a capability
system, authority is a handle, not a name (D11). The native libc is where the
two meet:

- `open(path)` resolves in the cell’s namespace — by default the global
  IRIX-like namespace, optionally a private one (`docs/spec/architecture.md`
  §5.2) — and returns a `File` capability wrapped as an fd; a path does not
  grant authority.
- `socket()` returns a `Socket` capability; `connect` requires the network
  capability.
- `posix_spawnp` requires a spawn capability; the child inherits an attenuated
  capability set.
- `getcwd`/`chdir` are per-cell namespace state, not a global `/`.
- `chown`/`chmod`/`setuid` are capability checks, not ambient privilege.

If the native libc implements these as ambient POSIX, `std` becomes an
authority bypass. The libc, not `std`, must enforce D11. `std`’s `unsafe`
`FromRawFd` is the caller’s responsibility; the libc must still validate the
handle on every use.

### 5.5 The native libc must not expose IRIX 6.5 compatibility shims

Do not add `syssgi`, `sproc`, `arenal`, `procntl`, IRIX `ioctl` numbers or IRIX
`stat` layouts to the native libc. Those belong to the compat container
(`docs/spec/architecture.md` §6.3). A native program that needs them is a legacy
program and should run in the container. Keeping them out of the native libc is
what keeps the native ABI clean and makes the `std` port small.

---

## 6. Decision matrix

The options below are the five requested. “Works” and “breaks” are about the
product, not about whether a prototype can be made to compile.

### (a) Upstream `std` target with a libc-backed Unix PAL for IRIX 6.5/n32

**What it is.** An official or out-of-tree `target_os = "irix"`, n32 (and
optionally o32), `target_family = ["unix"]`, with a `libc` IRIX module generated
from the 6.5 headers, and a `std` patch adding IRIX arms. This is essentially
what `danifunker/rust-irixlibstd` proved on IRIX 6.5.22. It is the **IRIX 6.5
ABI**, not the native POSIX.1-2024 API.

**Works.** The prior art proves the model: `std` builds for o32 and n32, links
into IRIX executables, and runs 62 runtime stages with 995 layout assertions.
Files, threads, mutexes/condvars, processes, TCP/UDP, DNS, Unix sockets, TLS
(not in std), `catch_unwind` and backtraces work on 6.5.22. It requires no
kernel changes and no native OS.

**Breaks.** It is the wrong product for IRIX 7: it freezes the IRIX 6.5 ABI
into the native world, which D2 forbids. It also has a version-floor problem:
the project’s sysroot is 6.5.7m, while the prior art targets 6.5.22; SGI changed
the IPv6 ABI, added `getaddrinfo`/`sockaddr_storage`/`socklen_t` and changed
`IPV6_*` between those releases (prior art `docs/gaps.md`, “Version floor”).
A 6.5.22 std will not run on 6.5.7m; a 6.5.7m std is harder and has no prior
proof. It also carries the full 34-file `std` patch and nine crate patches,
`rustix`/`tempfile` stay blocked, and the `libc` module is header-derived, which
collides with ADR-0001/0009 if published.

**ABI.** IRIX 6.5 n32/o32; stable by ADR-0008; `std` is compiler-version-locked.
**Maintenance.** High: 34-file `std` patch + `libc` module + nine crate patches;
6.5.x floor churn.
**Toolchain.** LLVM + IRIX-patched LLD; no rustc fork; dynamic linking; GCC
backend unverified.
**Security.** Legacy process, MMU-isolated; ambient authority inside the
container; acceptable for compatibility, not for native.
**Milestone.** M5 (o32 syscall layer) + n32 compat + a broad IRIX libc; post-M7
for realistic coverage. Not M9.
**Verdict.** **Compatibility-only; not load-bearing.** Useful as a test harness
for the compat container and as a way to run Rust tools on IRIX 6.5. Do not let
it define the native ABI.

### (b) New native IRIX 7 PAL over the OS syscall/libc ABI

**What it is.** `target_os = "irix7"`, `target_family = ["unix"]`, a native
libc implementing the POSIX.1-2024 base plus the extension list, and a `std` PAL
that uses it. This is the product this report recommends.

**Works.** It fits D2–D4, D9 and D11–D13: the native ABI is new, the process is
a cell, the libc is Unix-shaped over capabilities, and `std` is a userland
library. It gives native Rust programs the ecosystem, the desktop, the MCP cell
and the shell. If the native libc provides the POSIX.1-2024 base plus the
extension list, the `std` patch is a target plus a handful of `cfg` arms and no
crate patches.

**Breaks.** It does not exist yet. It depends on M9’s native libc, cell loader,
capability table and IPC. It needs a cell entry that calls `std::rt::lang_start`,
a `posix_spawn` implementation, a pthread implementation over tasks, a TLS
strategy, an unwinder and a cell-scoped panic boundary. It cannot be built
before the native libc has a stable enough surface.

**ABI.** Native IRIX 7 n32; versioned; `std` compiler-locked; the stable seam is
the libc C ABI.
**Maintenance.** Low-to-medium if the libc is modern: target + libc module +
~10 small `std` cfg arms + unwinder. Upstreamable.
**Toolchain.** Pinned nightly + `-Zbuild-std` sysroot; no rustc fork; GCC
backend is the risk.
**Security.** Best fit: capability-scoped libc; std does not bypass it; no
ambient root.
**Milestone.** M9 (native libc/cell loader) + a new M9.5 feature for `std`.
**Verdict.** **Load-bearing; recommended.**

### (c) Forked/patched `rust-src`/`std` build shipped with the OS

**What it is.** Keep the upstream rustc, patch `rust-src` (target, `libc`
module, `std` PAL) and ship a `-Zbuild-std=std` sysroot or a prebuilt sysroot in
the SDK. This is a **vehicle**, not a product.

**Works.** It is available now; it needs no rustc fork; it is what the prior art
uses. It supports either the native or the compat target. It can be pinned and
reproduced; it can carry the MIPS unwinder patch. It is the only practical path
before an upstream target exists.

**Breaks.** Nightly churn; the patch must be rebased; `restricted_std` unless
the target is added to `build.rs`; users need the project toolchain or
`-Zbuild-std`; a prebuilt sysroot is compiler-version-locked; the `libc` patch
must live in the library workspace (the prior art’s `building.md` documents
this). It is not a substitute for an upstream target.

**ABI.** Same as the target; `std` compiler-locked.
**Maintenance.** Medium: a small patch for the native target; large for the
compat target.
**Toolchain.** `rust-src` + `-Zbuild-std`; no rustc fork; CI must rebuild std.
**Security.** Supply-chain risk; pin and verify.
**Milestone.** M0 for the mechanism; M9 for the native target; post-M5 for the
compat target.
**Verdict.** **Recommended vehicle for (b), not the end state.**

### (d) `std`-compatible shim over kernel/module services

**What it is.** A crate named `std` (or a `std` facade) that provides
`println!`, `Vec`, `String`, a few files and sockets over the module service
table, without libc or a process model.

**Works.** It can make a module or kernel cell feel like `std` for a small
subset; it uses `alloc`; it avoids a libc.

**Breaks.** It is not `std`; ecosystem crates will not compile against it
unless it reimplements the whole API; it has no threads, process, signals,
environment or real files; it would run in kernel mode and become part of the
TCB; it would be a second standard library to maintain. It cannot support
`tokio`, `ripgrep`, `gix` or a desktop.

**ABI.** Custom, not Rust std.
**Maintenance.** High, low payoff.
**Toolchain.** Low.
**Security.** Worst: kernel-mode std-shaped code with full authority.
**Milestone.** Now, but not recommended.
**Verdict.** **Rejected** for kernel/modules and for the native product.

### (e) No `std`; continue `core`/`alloc`

**What it is.** The current state: kernel and modules use `core`/`alloc`; host
tools use the host `std`; native cells would use `core`/`alloc` only.

**Works.** Smallest TCB; no maintenance; already proven. It is the correct
choice for the kernel, ROM, modules and drivers.

**Breaks.** No native Rust applications with files, threads, processes, sockets
or time; no ecosystem crates; no desktop/MCP/shell in Rust. It contradicts the
requirement that `std` be load-bearing.

**ABI.** N/A.
**Maintenance.** Low.
**Toolchain.** Current.
**Security.** Best.
**Milestone.** Current.
**Verdict.** **Fallback only** if M9 slips; not the product.

### Summary

| Option | Product fit | Native ABI | Maintenance | Toolchain cost | Security | Milestone | Verdict |
|---|---|---|---|---|---|---|---|
| (a) IRIX 6.5 libc-backed `std` | Wrong product for native; IRIX 6.5, not POSIX-2024 | IRIX 6.5 | High | LLVM + IRIX LLD | Legacy ambient | post-M5/M7 | Compat-only |
| (b) Native IRIX 7 PAL | **Right: native POSIX.1-2024 + extensions** | **Native** | Medium | Pinned nightly + build-std | **Capability** | M9+ | **Load-bearing** |
| (c) Patched rust-src | Vehicle for (a)/(b) | Target-dependent | Medium/high | `-Zbuild-std` | Supply chain | M0/M9 | **Vehicle** |
| (d) Shim | Wrong | None | High | Low | Kernel TCB | now | Rejected |
| (e) No std | Kernel-only | N/A | Low | Current | Best | now | Fallback |

**Recommended path:** (c) now, (b) as the product, (a) only for the compat
container/test track; upstream (b) later if it stabilises. (d) never for
kernel/modules. (e) only if M9 slips.

POSIX.1-2024 is **not a separate option**: it is the API baseline inside (b).
Option (a) remains IRIX 6.5, because that is the compat container’s contract.
(d) and (e) do not change; a shim is not POSIX, and `no_std` has no libc to be
POSIX-conformant.

---

## 7. Recommended architecture

### 7.1 Product split

```text
                    IRIX 7
                      |
        +-------------+----------------+
        |                              |
  Native world                    Compat container
  (new ABI)                       (IRIX 6.5 ABI)
        |                              |
  target_os="irix7"               target_os="irix"
  target_family=["unix"]          target_family=["unix"]
  native libc: POSIX.1-2024        IRIX 6.5 libc (clean-room)
  base + extensions
  std: native PAL                 std: hosted IRIX 6.5 PAL
  cells/apps                      legacy binaries + optional Rust tools
        |                              |
  kernel/ROM/modules: no_std       test harness only
  target_os="none"                 not the native ABI
```

### 7.2 Native `std` PAL: reuse `unix`, add a small `irix7` arm

The native libc is designed to the POSIX.1-2024 base plus the extension list,
so the existing `library/std/src/sys/pal/unix` is the right PAL. The native
target needs:

1. **A `libc` `irix7` module.** This is the native libc’s public header in Rust:
   types, constants, structs and function declarations. It is clean-room and
   publishable. It is the single largest new file, but it is generated from the
   native libc ABI spec, not hand-maintained twice.
2. **`std/build.rs`**: add `irix7` to the known-platform list (one line), so
   `std` is not `restricted_std`.
3. **`std/src/os/mod.rs` + `std/src/os/unix/mod.rs` + `std/src/os/irix7/mod.rs`**:
   re-export `std::os::unix` and add any native extensions (probably none in
   v1). This is the `os::irix7` module the prior art adds for IRIX.
4. **A handful of `cfg` arms** where the native libc deliberately differs from
   the generic Unix assumption:
   - `std/src/sys/fs/unix.rs`: an `irix7` arm for `FileAttr` times if the native
     `struct stat` uses POSIX `st_mtim` rather than `st_mtime`/`st_mtime_nsec`
     (recommended). If the native `libc::stat` exposes `st_mtime_nsec` as a
     real field, no arm is needed; do not design the ABI around a `std` quirk.
   - `std/src/sys/io/error/unix.rs`: the native errno link name if it is not
     `__errno_location` (prefer `__errno_location` to avoid the arm).
   - `std/src/sys/random/mod.rs`: an `irix7` arm selecting `getentropy`
     (POSIX-2024) or `unix_legacy` for `/dev/urandom`; do not require
     `getrandom`.
   - `std/src/sys/net/connection/socket/unix.rs` and `socket/mod.rs`:
     POSIX-2024 has `accept4`, `SOCK_CLOEXEC`/`SOCK_CLOFORK`,
     `SOCK_NONBLOCK` and `MSG_NOSIGNAL`; add `irix7` to those lists. The old
     `accept`/`socket` + `fcntl` fallbacks remain for older kernels, but the
     native libc should provide the atomic forms.
   - `std/src/sys/pipe/unix.rs`: POSIX-2024 has `pipe2` (and `dup3`); add
     `irix7` to the `pipe2` list. The old `pipe` + `fcntl` fallback remains for
     older kernels.
   - `std/src/sys/process/unix/unix.rs`: add `irix7` to the `posix_spawn` list;
     the native libc provides `posix_spawn_file_actions_addchdir` (POSIX-2024).
     `std` already weak-links both the `_np` and POSIX names and tries `_np`
     first, then the POSIX name; the `_np` alias is optional.
   - `std/src/sys/fs/unix.rs`: an `irix7` arm for `FileAttr` times using
     `st_mtim` (POSIX) rather than `st_mtime`/`st_mtime_nsec`; and either
     provide `dirent.d_type` as an extension or add `irix7` to the no-`d_type`
     list and use `lstat`.
   - `std/src/sys/paths/unix.rs`: `/proc/self/exe` is not POSIX; add a native
     loader `getexecname` arm or return an error.
   - `std/src/sys/pal/unix/stack_overflow/mod.rs`: add `irix7` to the
     `handler_signal` list only if the native libc provides the
     `pthread_getattr_np` **extension** (not POSIX), `sigaltstack` and `mmap`.
   - `std/src/sys/args/unix.rs`: add `irix7` to the `argc`/`argv` `imp` list.
   - `std/src/sys/thread/unix.rs`: `sysconf(_SC_NPROCESSORS_ONLN)` is
     POSIX-2024, so no arm is needed; return the configured count (1 on the
     uniprocessor) from the libc.
   - `std/src/sys/pal/unix/sync/condvar.rs` and
     `std/src/sys/pal/unix/time.rs`: `CLOCK_MONOTONIC`,
     `pthread_condattr_setclock` and `pthread_cond_clockwait` are POSIX-2024,
     so no fallback is needed; use the precise path.
5. **Unwinding**: `std` always links the `unwind` crate, and on a `unix`
   target it declares `_Unwind_*` even for `panic = "abort"` (§2.4). Provide
   `_Unwind_*` in the native runtime, or add `irix7` to the `unwinding` crate
   selection in `library/unwind/Cargo.toml` and
   `library/unwind/src/libunwind.rs`, and carry MIPS support in the vendored
   `unwinding` crate (or upstream it). `panic = "abort"` alone is not enough to
   avoid this.
6. **Backtrace** (optional): add `irix7` to the `backtrace` crate’s
   `dl_iterate_phdr`/`libs_unix` selection if the native libc provides
   `dl_iterate_phdr` and `PT_GNU_EH_FRAME`. For v1, `std::backtrace` can be
   compiled with `-Zbuild-std-features=` (no `backtrace`) or symbolisation can
   be deferred.

This is a **target + libc + ~10 cfg arms** patch, not a 34-file rewrite. The
exact count depends on how modern the native libc is; the table in §5.3 is the
contract.

### 7.3 `std` module behaviour on native IRIX 7

| `std` area | Native implementation | v1 status |
|---|---|---|
| `std::io` (stdio, Read/Write, Error) | `read`/`write`/`poll`, `isatty`, `__errno_location`, `strerror` | Yes |
| `std::fs` | `*at` family, `stat`, `posix_getdents`/`readdir`, `futimens`/`utimensat`, `realpath`; `posix_dent.d_type` or `lstat` | Yes |
| `std::net` | `socket`/`accept4`/`getaddrinfo`/`poll`/`ppoll`, IPv4+IPv6, working `SO_RCVTIMEO`, `MSG_NOSIGNAL`; `SOCK_CLOEXEC`/`SOCK_CLOFORK` | Yes |
| `std::process` | `posix_spawnp` + `posix_spawn_file_actions_addchdir` (POSIX-2024; optional `_np` alias), `waitpid`, `pipe`/`pipe2`, `_Fork` | Yes |
| `std::env` | `argv`/`environ` from cell loader, `setenv`/`unsetenv`, `getcwd`, loader `getexecname` (extension) | Yes |
| `std::time` | `clock_gettime(CLOCK_MONOTONIC/REALTIME)`, `nanosleep`; `gettimeofday` extension only | Yes |
| `std::thread` | pthreads over native tasks; stack size; `available_parallelism` from `_SC_NPROCESSORS_ONLN` | Yes |
| `std::sync` | pthread mutex/condvar, `pthread_cond_clockwait`/`pthread_mutex_clocklock`, queue `Once`/`RwLock`; mpsc | Yes |
| `std::random` | `getentropy` (POSIX-2024) | Yes |
| TLS | pthread keys first (`has-thread-local: false`); ELF TLS later (not POSIX; implementation-defined) | Yes (keys) |
| `std::panic` | `unwind` crate still needs `_Unwind_*` for abort; phase 2 adds `panic = "unwind"` with `unwinding` + MIPS support | Phase 1 abort; phase 2 unwind |
| `std::backtrace` | `dladdr` (POSIX-2024) and/or `dl_iterate_phdr` (extension) + `PT_GNU_EH_FRAME` + `gimli`; optional | Optional |
| `std::os::unix::net` | AF_UNIX over channels/sockets; `sendmsg`/`recvmsg` + `SCM_RIGHTS` | v1.1 |
| `std::os::unix::fs` | `MetadataExt`, `PermissionsExt`, `FileTypeExt`, `OpenOptionsExt` | Yes where libc supports |
| `File::lock` | `fcntl` record locks or XSI `lockf`; `flock` is an extension | Optional; do not fake |
| Terminal size | `tcgetwinsize`/`tcsetwinsize` (POSIX-2024); `ioctl(TIOCGWINSZ)` is a legacy extension | v1.1 for the crate |
| `std::os::unix::process::CommandExt` | `arg0`, `process_group`; `uid`/`gid`/`pre_exec` not meaningful in a capability OS | Partial; document |
| `std::os::unix::fs::chroot` | no ambient root | Omit/`Unsupported` |

### 7.4 The cell runtime entry

A native cell is a `#![no_main]` object. The SDK entry macro must:

1. receive a `CellStart` structure from the loader (argc/argv/envp, capability
   table, namespace, ABI version);
2. call `std::rt::lang_start` (or the equivalent `lang_start_internal`) with a
   `main` function and the captured argc/argv;
3. install a `catch_unwind` boundary at the cell entry, so a panic that escapes
   `main` terminates the cell, not the kernel;
4. call the libc’s `_exit` path, which returns the cell’s exit status to the
   loader.

This is the `std`-specific part of M9. It is not a `std` patch; it is the cell
SDK’s `entry!` macro. The module SDK’s `_start` is not the same thing: modules
are kernel code and use `module-abi`, not `std`.

### 7.5 Build and test

**Build a native `std` rlib:**

```sh
export RUSTC_BOOTSTRAP=1
export __CARGO_TESTS_ONLY_SRC_ROOT=/path/to/patched/rust-src/library
cargo build -Zjson-target-spec -Zbuild-std=std \
  --target targets/mips3-sgi-irix7-native-n32.json
```

The `libc` patch must live in the **library workspace** (`library/Cargo.toml`),
not the app workspace, because `-Zbuild-std` resolves `std`’s dependencies from
the library workspace. The prior art documents this in `docs/building.md`
(“The patch has to live in the *library workspace* rather than in your own
crate”). The pinned `rust-src` ships `library/vendor/` but not the source
replacement config for `build-std`; the project must supply the vendored
sources or a `[patch.crates-io]` entry, or the build resolves `libc`/`unwinding`
from crates.io (the first failure in §12.1 was a registry `libc`, not the
vendored one). Note also that the workspace `Cargo.lock` resolves `libc
0.2.191`, while `std` pins `0.2.189` (`library/std/Cargo.toml`); patch the copy
`std` resolves, not the workspace’s.

**Build a compat `std` rlib:**

```sh
cargo build -Zjson-target-spec -Zbuild-std=core,alloc,std,panic_unwind \
  --target targets/mips64-sgi-irix-n32.json
```

with the IRIX-patched LLD and sysroot for linking, and the same library
workspace patch. This is the prior art’s model; it is a separate track.

**Test strategy:**

| Layer | What | Where |
|---|---|---|
| Compile | `std` rlib for the target; no link | CI, host |
| ABI layout | `size_of`/`align_of`/`offset_of!` assertions for every native libc type | CI, host |
| PAL smoke | a crate that instantiates every v1 API (`fs`, `net`, `thread`, `process`, `env`, `time`, `sync`) | CI, cross-build |
| Runtime | the same crate as a native cell / compat process; 60+ staged tests | M9/QEMU/iris; compat container |
| Conformance | C and Rust print `sizeof`/`offsetof`/errno for the same ABI; compare | CI + target |
| Fuzz | parsers (ELF, EROFS, XFS, 9P, HTTP, MCP) unaffected | existing |
| Regression | `std` test suite subset; `ripgrep`/`tokio` smoke | M9.5/CI |

The prior art’s `tests/runtime` (62 stages, 0 failures on 6.5.22) and
`tests/layout` (995 assertions) are the model. The native version should have
fewer layout assertions (one ABI, not two) but the same staged runtime tests.

### 7.6 Preventing silent divergence

The failure mode to fear is not a compile error; it is a `std` that compiles and
then lies. The prior art’s `mio` bug is the exemplar: a target in neither the
`pipe2` nor the `pipe` list gets neither call and returns `[-1, -1]` as success,
which surfaces much later as an `fd != -1` panic (prior art `docs/crates.md`,
“mio”). The same class of bug exists in `std`’s per-OS `cfg` lists.

Mitigations:

1. **One ABI spec.** Define the native libc ABI in one machine-readable place
   (a `native-abi` crate with `repr(C)` structs and constants, plus a
   `native-libc.h` generated from it). Generate the `libc` `irix7` module and
   the C headers from the same spec. Do not hand-maintain two copies. This is
   the `module-abi` precedent: one crate, compile-time offsets, both sides link
   it (`docs/spec/modules.md` §4; `module-abi/src/lib.rs`).
2. **Layout assertions.** `const _: () = { assert!(size_of::<...>() == ...); ... }`
   for every type `std`/`libc` exposes. The prior art’s 995 assertions are the
   model. The native version can be smaller but must cover `stat`, `dirent`,
   `sockaddr_in6`, `msghdr`, `iovec`, `timespec`, `timeval`, `sigaction`,
   `pthread_*`, `off_t`/`ino_t`/`dev_t`/`mode_t`/`nlink_t`/`blkcnt_t`/
   `socklen_t`.
3. **cfg-arm audit.** A CI script that scans `library/std/src` for
   `cfg_select!`/`cfg(any(...))` arms listing Unix OSes and fails if `irix7` is
   neither listed nor explicitly excluded with a comment. This catches the
   “missing arm falls through to a wrong default” class. It is a read-only
   script; it does not require the OS.
4. **Runtime conformance.** A staged test binary that exercises every v1 API
   and prints a line per stage before running it, so a crash names the stage.
   Run it on the native cell loader and, for the compat target, in the compat
   container.
5. **Link-time completeness.** For the claimed v1 subset, the native libc must
   provide every symbol; a missing symbol is a link error, not a stub returning
   `ENOSYS`. Do not ship `Unsupported` stubs for `fs`, `net`, `thread` or
   `process` and call it supported.
6. **Target identity.** `target_os = "irix7"` vs `"irix"` prevents the compat
   std from being linked into native cells. Add a compile-time assertion in the
   native libc that it is not `target_os = "irix"` and vice versa.

---

## 8. Why this is load-bearing

### 8.1 Which ecosystem crates and applications need which pieces

The following is a realistic native-world set. “Needs” is the `std` surface, not
the crate’s whole dependency tree.

| Crate/application | Native `std` pieces | Native libc requirements | Status if §5.3 is met |
|---|---|---|---|
| `ripgrep` | `fs`, `thread`, `env`, `process`, `time`, `io` | `*at`/`stat`/`dirent`, pthreads, `mmap` with `MAP_ANON` | Works; `MAP_ANON` is POSIX-2024, so no `memmap2` patch |
| `tokio` | `net`, `thread`, `sync`, `time`, `io` | `poll`/`ppoll`, `getaddrinfo`; `pipe2`/`accept4`/`SOCK_CLOEXEC` are POSIX-2024 | Works with mio’s existing `poll` backend; a small cfg arm, not a new backend |
| `rustls` | `io`, `net` | `getentropy` (POSIX-2024) or `/dev/urandom`, sockets | Works |
| `gix` | `fs`, `net`, `process`, `time`, `thread` | `*at`, `getaddrinfo`, `posix_spawn` (all POSIX-2024) | Works; large dependency tree |
| `serde`/`serde_json` | `alloc` only | none | Works without `std` |
| `anyhow`/`thiserror` | `std::error`, `io` | `strerror`/`errno` | Works |
| `rayon`/`crossbeam` | `thread`, `sync` | pthreads | Works |
| `tempfile`/`terminal_size` | `fs` via `rustix` | `*at`, `fdopendir`, `dirfd`, `tcgetwinsize` (all POSIX-2024) | The OS API is present; `rustix` still needs a crate-side port because it has no generic-Unix tier. `tempfile` can use `mkstemp`; `terminal_size` can use `tcgetwinsize`. |
| `libloading` | `dlopen`/`dlsym` | dynamic loader (POSIX-2024), `dladdr` | Works; `RTLD_DEFAULT`/`RTLD_NEXT` are extensions |
| `getrandom`/`rand` | `getentropy` | `getentropy` (POSIX-2024) | A small crate patch may still be needed for the `target_os` arm, but no kernel gap |
| MCP cell | `net`, `io`, `fs`, `sync`, `thread`, `time` | sockets, `poll`/`ppoll`, `posix_spawn` | Works |
| Display server | `io`, `thread`, `sync`, `time` | pthreads, `poll`, `mmap` with `MAP_ANON` | Works |
| Shell/editor/file manager | `fs`, `io`, `env`, `process`, `thread`, `time` | `*at`, `posix_spawn`, `getaddrinfo` | Works |
| `cargo`/`rustc` on IRIX | everything, plus dynamic loading and huge memory | not v1 | Not a v1 goal; cross-compile on the host |

The native-world applications are not hypothetical: the architecture’s MCP
cell, display server, network/FS clients and desktop are all Rust
(`docs/spec/architecture.md` §4.3, §13, §14). Without `std`, each one would
reimplement the same operating-system layer in `no_std`, and the ecosystem
crates in the table would be unavailable.

### 8.2 Minimum viable `std` subset

For the M9.5 demo (“a native Rust cell opens a file, reads it, spawns a thread,
runs a command, connects to a TCP port, and prints”), the minimum is:

- `std::io` (stdio, `Read`/`Write`, `Error`);
- `std::fs` (`File`, `OpenOptions`, `metadata`, `read_dir`, `create/remove/rename`);
- `std::path`;
- `std::env` (`args`, `vars`, `current_dir`, `current_exe`);
- `std::process` (`Command`, `exit`, `status`) with `posix_spawn`;
- `std::thread` (`spawn`, `join`, `sleep`, `Builder`);
- `std::sync` (`Mutex`, `Condvar`, `RwLock`, `Once`, `mpsc`, `Arc`);
- `std::time` (`Instant`, `SystemTime`, `Duration`);
- `std::net` (`TcpListener`, `TcpStream`, `UdpSocket`, `ToSocketAddrs`);
- `std::os::unix` fd traits and `fs::{MetadataExt, PermissionsExt}`;
- `std::panic` with `catch_unwind` at the cell entry (abort first, unwind in
  phase 2).

Not required for the demo: `std::os::unix::net` (AF_UNIX), ancillary data,
`File::lock`, `peer_cred`, `pre_exec`, `chroot`, symbolised backtraces. These
can be v1.1 or explicitly `Unsupported`.

The native libc subset behind this demo is §5.3. The POSIX.1-2024 facilities
that make it small are `getentropy`, `posix_spawn_file_actions_addchdir`,
`pthread_cond_clockwait`, `posix_getdents`, `ppoll`, `posix_devctl`,
`tcgetwinsize`, `MAP_ANON`/`MAP_ANONYMOUS` and `O_CLOFORK`; the remaining
`std` assumptions are the extension list in §5.1.3.

### 8.3 Failure modes of a half-std

- **A `std` that compiles but returns `Unsupported`.** The `unsupported` PAL
  does this for some platforms; crates compile and fail at runtime. It is worse
  than no `std` because the failure is late and silent. Do not ship it for
  claimed modules.
- **A missing `cfg` arm falls through to a wrong default.** `pipe2` vs `pipe`,
  `accept4` vs `accept`, `F_DUPFD_CLOEXEC` vs `F_DUPFD`, `CLOCK_MONOTONIC` vs
  `CLOCK_REALTIME`, `SOCK_CLOEXEC` vs `fcntl`. The prior art’s mio bug is the
  exemplar. The cfg-arm audit (§7.6) is the mitigation.
- **ABI drift.** The `libc` bindings and the native libc headers disagree on a
  struct offset or an errno value; the program compiles and corrupts memory.
  Layout assertions and a C/Rust conformance test are the mitigation.
- **Wrong target.** A compat `std` (IRIX 6.5 syscalls) is linked into a native
  cell; the cell issues IRIX 6.5 `syscall` instructions or expects IRIX 6.5
  `struct stat`. Distinct `target_os` values and a compile-time assertion
  prevent this.
- **Panic escapes the cell.** `panic = "unwind"` without a cell-entry
  `catch_unwind` unwinds into the kernel’s stack or another cell. The cell
  runtime must catch at the boundary; the kernel must not have unwind tables
  for cell frames.
- **`std` in the kernel.** A single dependency accidentally pulls `std` into
  the kernel; the `#![no_std]`/`#[panic_handler]` conflict and the module size
  make it obvious, but CI should also assert that the kernel/ROM/module
  dependency graphs do not contain `std`.
- **`restricted_std` hides the problem.** Users add `#![feature(restricted_std)]`
  and believe the target is supported. Add the native target to
  `std/build.rs`; do not ship a feature-gated std.
- **POSIX-2024 removals.** A crate still calls `ioctl` or `gettimeofday`; if the
  native libc does not provide the documented extension, the program fails to
  link or gets `ENOSYS`. The extension policy in §5.1.3/§5.1.4 must be explicit
  and tested.
- **Certification creep.** Implementing a subset of POSIX.1-2024 and claiming
  POSIX or XSI conformance is false. Publish the profile and the conformance
  matrix, and state what is not claimed (§5.1.5).

### 8.4 Preventing divergence between the kernel ABI and `std`

The kernel ABI and the `std`/libc ABI are different seams, but they share types
and conventions. The rule is the same as `module-abi`: one source of truth.

- The native capability ABI is defined in a crate (the future `native-abi`),
  with `repr(C)` types, versioning and compile-time offsets. The native libc
  and the kernel both link it. `std` does not link it directly; `std` links
  `libc`, and `libc` is generated from it.
- The `libc` `irix7` module is generated from the native libc headers, which
  are generated from the same `native-abi` crate. There is no second
  hand-written copy.
- The cell SDK’s `entry!` uses the same `CellStart` type as the loader.
- The `module-abi` crate remains the single source of truth for kernel modules,
  and `std` is not allowed in that graph. Add a CI check for `std` in the
  kernel/module dependency graph.
- The compat container’s IRIX 6.5 ABI is a separate source of truth (the
  captured sysroot / clean-room IRIX libc), not the native `native-abi` crate.
  Do not merge them.

---

## 9. Sequencing and PR plan

### 9.1 Relative to M4/M5/M9

| Milestone | `std` work | Why |
|---|---|---|
| M0 | Keep the kernel target; add a CI probe that records the current `-Zbuild-std=std` failure (optional) | The mechanism exists; do not change the M0 gate |
| M1–M4 | None | No userland; kernel/modules are `no_std` |
| M5 | None required; the compat `std` is **post-M5** and optional | M5 is an o32 C hello + minimal IRIX libc; a Rust `std` needs n32, a much larger libc and the compat container |
| M6–M8 | Optional compat `std` test track after n32 libc coverage | The compat `std` is a test harness, not the native product |
| M9 | Native libc v0 + cell loader; add `m9-std` as a feature leaf | `std` needs the native libc and a cell entry |
| M9.5 (new feature or milestone) | Native `std` subset + conformance suite | The demo and acceptance in §8.2 |
| M10 | Full `std`, apps/desktop/MCP use it | Daily-driver requirement |

**Proposed M9.5 acceptance criteria** (add as a feature leaf under M9 in
`tree/roadmap.json`, not a renumbering of M9):

- [ ] `libc` `irix7` module compiles for the native target; the POSIX.1-2024
      profile is documented; all v1 structs have compile-time layout assertions.
- [ ] `std` builds with `-Zbuild-std=std` for `os = "irix7"` and is not
      `restricted_std`.
- [ ] A native cell uses `std::fs` to open/read a file, `std::thread` to spawn
      and join, `std::process::Command` to run a child, `std::net::TcpStream`
      to connect, and `std::time::Instant` to measure.
- [ ] `getentropy`, `posix_spawn_file_actions_addchdir`, `pthread_cond_clockwait`,
      `posix_getdents`, `ppoll`, `posix_devctl` and `tcgetwinsize` are present
      (or explicitly documented as extensions/unsupported).
- [ ] `catch_unwind` catches a panic at the cell entry (phase 2) or the cell
      aborts cleanly (phase 1).
- [ ] The runtime conformance suite passes on iris (or the native cell loader).
- [ ] The kernel/ROM/module dependency graphs do not contain `std`.
- [ ] The compat `std` (if built) is a separate target and does not share
      `target_os`.

### 9.2 First PR: implementable now, without faking userland

**Title:** `docs: define the native IRIX 7 std product and its libc contract`

**Contents:**

1. `docs/research/rust-std-irix7.md` — this report (evidence).
2. `docs/spec/native-libc.md` (new draft) — the §5.3 contract: signatures,
   types, error semantics, the POSIX.1-2024 profile, the extension list,
   “v1 / optional / never”, and the capability enforcement rules. Owner:
   kernel/compat; status draft. This fills the interface already listed as
   planned in `docs/plan/interfaces.md`.
3. An ADR PR in `irix7/project/docs/adr/` (ADR-0021, “native IRIX 7 std”) —
   the decision: native `std` is load-bearing; compat `std` is separate; target
   split; no `std` in kernel/modules; the native libc is designed for `std`;
   the compat `libc` bindings stay private. ADRs live in `irix7/project`, not
   this repository, so this is a separate review.
4. `tools/std-abi-surface.py` — a read-only script that parses
   `library/std/src` for `libc::` uses on Unix, classifies each as POSIX-2024
   base, XSI, or extension, and compares them to the symbols/types declared in
   `native-libc.md`; prints missing entries. Run in CI as a warning until the
   spec is accepted, then as a gate for the v1 subset.
5. No target JSON, kernel, module or Cargo changes.

**Why it is implementable now.** It is documentation plus a read-only analysis
tool. POSIX.1-2024 is a public standard, so the profile and the ABI checklist
can be written without userland, a native libc implementation or a cell loader.
It produces the contract that M9 implements, and it makes the `std` surface
reviewable before any code is written.

**Acceptance:** `python3 tools/std-abi-surface.py` reports zero missing entries
for the v1 subset; `mdbook build`, markdownlint and typos pass; the ADR is
reviewed by kernel/compat.

### 9.3 Later PR sequence

| PR | Milestone | Contents | Gate |
|---|---|---|---|
| 2 | M9 design | Native target JSON (`os=irix7`, `family=unix`, `abi=n32`) + `libc` `irix7` declarations (no implementations) + `std/build.rs`/`os::irix7`/cfg arms; POSIX-2024 profile in the spec; build `std` as an rlib (no link) | `cargo build -Zbuild-std=std` succeeds; layout assertions compile |
| 3 | M9 | Native libc implementations for the POSIX-2024 base + extensions; cell `entry!`; `CellStart`; `posix_spawn`/`posix_spawn_file_actions_addchdir`; `getentropy`; pthreads over tasks; TLS via pthread keys; `_Unwind_*` from the native runtime or the `unwinding` crate | A native cell prints, opens a file, spawns a thread |
| 4 | M9.5 | `std::process`, `std::net` (`poll`/`ppoll`), `std::fs` (`*at`/`posix_getdents`), `std::time`, `std::sync` (`pthread_cond_clockwait`); runtime conformance suite; `catch_unwind` boundary | The §9.1 demo on iris |
| 5 | M9.5 | `panic = "unwind"` with the `unwinding` crate + MIPS support; symbolised backtrace optional (`dladdr`/`dl_iterate_phdr`) | `catch_unwind`, panicking thread `join()` |
| 6 | post-v1.0 | Upstream the native target, `libc` module, `std` cfg arms, `unwinding` MIPS support and the POSIX-2024 profile | Tier 3 policy; libc/rust review |
| 7 | optional | Compat `std` target (`os=irix`, 6.5.22 floor) as a private/local test harness; do not publish the generated `libc` module | Compat container std tests |

### 9.4 Why not start with the compat target?

It is tempting because the prior art proves it. But:

- It targets 6.5.22, not the project’s 6.5.7m sysroot; the IPv6/getaddrinfo ABI
  changed between them.
- It needs n32 + a broad IRIX libc, which is post-M5/M7.
- It cannot define the native ABI; doing it first would create pressure to
  reuse IRIX 6.5 shapes in the native world, which is the exact mistake D2/D4
  warn against.
- It carries the high-maintenance 34-file patch and nine crate patches.

It is worth doing **after** the native target is defined, as a test harness for
the compat container and as a way to run Rust tools on IRIX 6.5. It is not the
first step.

---

## 10. Risks and open questions

### 10.1 Highest-risk unresolved question: the GCC backend

ADR-0018 makes `rustc_codegen_gcc` the primary backend, with LLVM parallel,
because GCC covers MIPS III + n32 where LLVM cannot do MIPS III + o32. ADR-0018
itself says the GCC backend is immature, with no verified MIPS support and
unsupported cross-targeting. `rustc_codegen_gcc`’s README describes the project
as WIP and expects ~100 failing UI tests
(<https://github.com/rust-lang/rustc_codegen_gcc>).

A native `std` is not load-bearing under ADR-0018 until it builds and runs
under the GCC backend on n32. The prior art, the existing M0 cross-build and
this report’s diagnostics all use LLVM. If the GCC backend cannot build
`-Zbuild-std=std` for n32, the project must either:

- amend ADR-0018 to make LLVM primary for userland and keep GCC as the
  parallel/portability backend, or
- accept an LLVM-only userland std temporarily and treat the GCC backend as a
  post-v1.0 portability goal.

This is the highest-risk unresolved question because it changes the toolchain
decision, not just the `std` patch. It should be answered with a spike before
M9.5: build `core`, `alloc` and `std` with `-Zcodegen-backend=rustc_codegen_gcc`
for the native target and run the layout/smoke tests.

### 10.2 Native libc surface and POSIX-2024 scope

The recommendation is to design the native libc to the **POSIX.1-2024 base**
plus a documented extension list. Two cautions:

- POSIX.1-2024 is a large standard. Implement the **profile** in §5.1.1, not
  the whole standard. Do not claim POSIX or XSI certification; publish a
  profile and a conformance matrix (§5.1.5).
- The removed `ioctl`/`gettimeofday` need an explicit extension policy: use
  `posix_devctl`/`clock_gettime` in the POSIX base, and keep `ioctl`/
  `gettimeofday` only as documented extensions for legacy code and crates that
  have not moved.
- The remaining non-POSIX assumptions in §5.1.3 (`pthread_getattr_np`,
  `pthread_setname_np`, `dl_iterate_phdr`, `flock`, `madvise`, `getrandom`,
  `RTLD_DEFAULT`/`RTLD_NEXT`, `readdir`'s `d_type`, `st_mtime_nsec`, `getexecname`) must
  each be explicitly provided as an extension or patched in `std`; do not let
  a missing `cfg` arm fall through to a wrong default.

If the project instead wants an IRIX-6.5-shaped native libc (for
“authenticity”), the `std` patch grows to the prior art’s 34 files, the nine
crate patches return, and `rustix`/`tempfile` stay blocked. This is a decision,
not a fact; it should be settled in the ADR before the libc is frozen.

`rustix` needs a separate note even under POSIX-2024: it has no generic-Unix
tier, so it still needs a crate-side `irix7` port. The POSIX-2024 `*at` family
removes the platform gap that made the port unsound, but it does not remove the
crate’s cfg work. `tempfile` can use `mkstemp`; `terminal_size` can use
`tcgetwinsize`; both need a crate arm, not a kernel feature.

### 10.3 No userland yet

M5 and M9 are not done. There is no native libc, no cell loader, no native
process model and no userland syscalls. The native ABI spec
(`docs/spec/native-abi.md`) and native libc spec (`docs/spec/native-libc.md`)
are still planned; `docs/plan/interfaces.md` lists both. `std` cannot run until
M9. The report’s plan is deliberately staged so that the first PR is docs, the
second compiles `std` against declarations, and only the third runs it. Do not
claim `std` works before a cell runs the conformance suite.

### 10.4 Clean-room and legal

- The native `libc` `irix7` module is clean-room (we define the ABI from the
  published POSIX.1-2024 specification and our own headers); publishable under
  GPL-3.0 with the usual MIT/Apache notices. POSIX.1-2024 is a public standard;
  implementing it from the specification is not an SGI-derived work.
- The compat `libc` module is generated from SGI headers. ADR-0001 forbids
  publishing SGI headers/sysroot; ADR-0009 forbids verbatim symbol or
  structure-name lists from the tree. A generated `libc` module is exactly such
  a list. Treat it as **private build input**, generated locally from the
  developer’s sysroot, gitignored; do not publish it in `irix7/os` and do not
  upstream it. The `std` cfg arms (target names) can be public; the bindings
  cannot.
- The prior art (`danifunker/rust-irixlibstd`, `atomchild411/rust-irix`) has no
  licence file (GitHub API reports `license: null`). Do not copy code from it.
  Use it as a reference for ABI facts and test plans; write the native code
  independently. `atomchild411/libc` is Apache-2.0 (upstream libc), but the
  IRIX-specific changes still need provenance review.
- The `unwinding` crate is MIT OR Apache-2.0; its MIPS support is not in the
  pinned version. Write the MIPS support independently from the DWARF/MIPS ABI
  or upstream it with a clean provenance record.

### 10.5 Nightly churn and patch maintenance

The `std` patch is against a pinned nightly. The IRIX 6.5 prior art needed 34
files against 1.99; the native patch is smaller but still moves with `std`’s
internals. Pin `nightly-2026-10-03`, rebase deliberately, and run the
conformance suite on every upgrade. R17 (nightly drift) applies.

### 10.6 Performance and memory

- `+soft-float` makes float-heavy code slow; hardware FP needs kernel FP
  context switching.
- 32-bit pointers and big-endian are fine for `std` but some crates assume
  little-endian or 64-bit atomics; test the corpus.
- `std`-linked cells are larger than 2 MiB; the M9 cell loader must allocate
  from the frame allocator, not the module area.
- `fork` in a SAS kernel copies the cell; `posix_spawn` is the default path.
- `std`’s global allocator and per-cell state are duplicated per cell; that is
  correct for “process = one cell”, but a shared libc would change the model.

### 10.7 Open questions

1. Is the native libc static per cell or a shared cell/library? (Start static;
   shared later.)
2. Does the native loader support ELF TLS? (Start pthread keys; ELF TLS later.)
3. Does the native target use hardware FP? (Start soft-float; separate kernel
   decision.)
4. What is the native cell image format — relocatable `.o`, flat image or ELF?
   How does `lang_start` get called?
5. What is the exact native syscall/capability invocation ABI, error model and
   blocking semantics?
6. Which POSIX.1-2024 profile and selected XSI facilities are in native libc
   v1? (§5.1.1/§5.3 are the proposal; do not implement the whole standard.)
7. Is `panic = "unwind"` in v1, or is abort acceptable for the first demo?
8. Does the compat `std` ship publicly or remain a private/local build input?
9. Does the project upstream the native target, `libc` module and `std` arms?
   Who maintains them (tier 3 requires a named maintainer)?
10. What is the exact IRIX 6.5 floor for the compat container: 6.5.7m (the
    sysroot) or 6.5.22 (the prior art)? The IPv6 ABI changed between them.
11. What is the licence/provenance of the prior-art MIPS unwinder, and can it
    be reimplemented cleanly?
12. Does `rustc_codegen_gcc` build `std` for n32? (See §10.1.)
13. Which POSIX-2024 new facilities does v1 adopt? The proposal is
    `getentropy`, `posix_spawn_file_actions_addchdir`, `pthread_cond_clockwait`,
    `posix_getdents`, `ppoll`, `posix_devctl`, `tcgetwinsize`, `O_CLOFORK` and
    `MAP_ANON`; the rest can wait.
14. What is the extension policy for the removed `ioctl`/`gettimeofday` and the
    non-POSIX `pthread_getattr_np`, `pthread_setname_np`, `dl_iterate_phdr`,
    `flock`, `madvise`, `getrandom`, `RTLD_DEFAULT`/`RTLD_NEXT`, `readdir`'s `d_type`,
    `st_mtime_nsec` and `getexecname`? Each must be provided or patched; none
    may fall through silently. (`accept4`, `pipe2`, `dup3`, `SOCK_CLOEXEC`,
    `SOCK_CLOFORK`, `SOCK_NONBLOCK` and `MSG_NOSIGNAL` are POSIX-2024/2008 and
    are base requirements, not extensions.)
15. Do we claim POSIX or XSI conformance? **No**; publish a profile and a
    conformance matrix, not a certificate (§5.1.5).

### 10.8 Facts that need a clean-room/ABI check

- Native `struct stat`, `dirent`, `sockaddr_in6`, `msghdr`, `iovec`, `timespec`,
  `timeval`, `sigaction`, `siginfo_t`, `ucontext_t`, `stack_t`, `pthread_*`
  layouts vs Rust `repr(C)`; compile-time assertions.
- `off_t`, `ino_t`, `dev_t`, `mode_t`, `nlink_t`, `blkcnt_t`, `socklen_t`,
  `pid_t`, `time_t`, `clockid_t` widths and signedness.
- `errno` location, values and `strerror` text; `EINTR` restart semantics.
- The native trap/capability invocation convention; the compat IRIX `$v0`/`$a3`
  convention. Do not mix them.
- Stack layout, argv/envp/auxv, TLS block layout.
- ELF program headers, `.eh_frame`/`PT_GNU_EH_FRAME`, `__ehdr_start`, dynamic
  loader/`dl_iterate_phdr` (the last is an extension, not POSIX-2024).
- `setjmp`/`longjmp`, signal frames, `sigaltstack` semantics.
- `posix_spawn` file actions, `posix_spawnattr` flags,
  `posix_spawn_file_actions_addchdir`/`addfchdir` (POSIX-2024) and the optional
  `_np` alias.
- `pthread_t`/`pthread_mutex_t`/`pthread_cond_t`/`pthread_key_t` sizes and
  initializers; `pthread_cond_clockwait`/`pthread_mutex_clocklock`/
  `pthread_rwlock_clock*`/`sem_clockwait` (POSIX-2024).
- `malloc`/`posix_memalign`/`aligned_alloc` alignment and `free` semantics.
- `MAP_ANON`/`MAP_ANONYMOUS` (POSIX-2024), `posix_madvise` vs `madvise`
  (extension), `O_CLOFORK`/`FD_CLOFORK` (POSIX-2024).
- `getentropy` (POSIX-2024) vs `getrandom` (extension) and `/dev/urandom`.
- `getaddrinfo`/`freeaddrinfo` ownership and IPv6 layout (native; compat 6.5.7m
  has no `getaddrinfo`).
- `SO_RCVTIMEO`/`SO_SNDTIMEO` semantics (native must work; compat 6.5.22
  reports `ENOPROTOOPT`).
- `fcntl` record locks / XSI `lockf` vs `flock` (extension); `File::lock`
  semantics.
- `CLOCK_MONOTONIC` (POSIX-2024; compat 6.5.7m lacks it).
- `ioctl` (not in POSIX.1-2024; extension) vs `posix_devctl` (POSIX-2024);
  `gettimeofday` (removed) vs `clock_gettime`.
- `tcgetwinsize`/`tcsetwinsize` (POSIX-2024) vs `ioctl(TIOCGWINSZ)`.
- `dirent.d_type` (extension) vs `lstat`; `posix_getdents` (POSIX-2024) vs
  `readdir`.
- `ppoll`/`poll` (POSIX-2024) and `pselect`/`select`.

---

## 11. Reconciliation with the ADRs and D1–D31

The numbered ADRs are in `github.com/irix7/project/docs/adr/`; all twenty were
read for this report. The local §2.1 table is a summary, not the ADR text.

### 11.1 ADR-by-ADR

| ADR | What it says (primary text) | Implication for `std` | Status |
|---|---|---|---|
| 0001 | Publish toolchain work only; no SGI sysroot, headers, binaries or tree excerpts | The compat `libc` bindings are header-derived; keep them private. The native `libc` is clean-room and publishable. | **Clarify** in the native-libc spec; no change to the ADR |
| 0002 | GCC 16.2 primary (superseded by 0019) | Toolchain lineage only | No change |
| 0003 | All ABIs from day one; n64 out of scope; o32/n32 proven | Native `std` is n32 first; o32 compat optional; n64 none. POSIX-2024 is an API baseline, not a new ABI. | No change |
| 0004 | Own rig, fresh 6.5.7m guest | The compat std’s oracle is 6.5.7m, not 6.5.22; this is a floor conflict | **Clarify** the compat floor |
| 0005 | Reference-first rebuild | `std` does not affect the stock rebuild | No change |
| 0006 | Dynamic-first smoke harness; IRIX 6.5 has no static libc | The **compat** std is dynamic; the **native** std is static-first (ADR-0007 principle). Scope the ADR to the IRIX 6.5 harness. | **Scope note**, no change |
| 0007 | Static-first rebuilt runtime; TLS off, LTO off | Native `std` can be static-first; the compat std uses the captured SGI libc dynamically. POSIX-2024 does not change the link model. | No change; aligns with static-first |
| 0008 | Backward-only compatibility; native binaries never run on 6.5 | Native `std` is not part of the 6.5 contract; compat `std` is a legacy process. **POSIX-2024 does not replace the IRIX 6.5 compat ABI.** | No change; explicit |
| 0009 | `irix7/os` publishes clean-room only | The compat `libc` module must not be published; the native one can be (implemented from the public POSIX-2024 spec). | **Clarify** the generated-bindings rule |
| 0010 | Rust monolith, n32/MIPS III; o32/n32 decode | Kernel `no_std`; native `std` is userland n32; compat `std` is o32/n32. | No change |
| 0011 | Capability substrate + compat container | Native `std` over a POSIX-2024 native libc with capabilities underneath; compat `std` over IRIX 6.5 syscalls. This is the decisive split. | No change; strongly supports §3 |
| 0012 | GPL-3.0 | `std`/`libc` MIT/Apache compatible; `unwinding` MIT/Apache; prior art unlicensed. POSIX-2024 is a public standard; implementing it is not an SGI-derived work. | No change; licence gate |
| 0013 | Reimplement everything; factory PROM; Rust PROM a stretch goal | The current assembly ROM is a later development; `std` is not in the ROM. | No change |
| 0014 | Harness-enforced clean-room | Native `libc`/`std` are clean-room; compat bindings are private. | **Clarify** |
| 0015 | Layered spec + similarity screen | The native ABI spec is the clean-room artefact; the compat bindings are not published. | **Clarify** |
| 0016 | o32 ECOFF bootstrap into n32 kernel | The actual ADR text says o32/ECOFF; `docs/spec/architecture.md` §2.1 says “Rust ROM replaces the factory PROM”. The current ROM is an assembly stub at `0xBFC00000` (`AGENTS.md`, `docs/spec/boot-protocol.md` §2). `std` is not linked into either. | **Documentation fix**, not a `std` change |
| 0017 | Modern open-source equivalents as references | The prior art is a reference, not copied code | No change |
| 0018 | GCC backend primary, LLVM parallel; MIPS unverified | `std` must be validated on GCC, or the ADR must be amended for userland | **Potential change**; §10.1 |
| 0019 | IRIX GCC fork on `rust-lang/gcc` 17 | Same as 0018 | **Potential change** |
| 0020 | Rebuild ladder: stock, modernised tree, Rust | `std` belongs to the Rust stage; no effect on the lower stages | No change |

**ADR-0016 note.** The external ADR is
`https://github.com/irix7/project/blob/main/docs/adr/0016-o32-ecoff-bootstrap.md`;
its text says the Indy’s ARCS PROM is o32/ECOFF and the bootstrap is o32. The
local `docs/spec/architecture.md` §2.1 summarises it as “Rust ROM replaces the
factory PROM; n32/MIPS III from the PROM window into the n32 kernel”, which is
not the same decision. The repository’s current ROM is a pure MIPS III assembly
stub executed from `0xBFC00000` (`AGENTS.md`; `docs/spec/boot-protocol.md` §2),
which supersedes both readings in practice. This should be corrected in a
separate docs PR. It does not affect `std`: the ROM and kernel are `no_std`, and
`std` is not a boot dependency.

### 11.2 D1–D31

Only the decisions with a `std` implication are listed; the rest are neutral.

| ID | Decision | Implication | Status |
|---|---|---|---|
| D2 | Native ABI is new; IRIX lives only in the compat container | **POSIX.1-2024 is the native source API; the native binary ABI is still ours; IRIX 6.5 lives only in the compat container.** | No change; reinforced by POSIX-2024 |
| D3 | Native process = one cell with a Unix personality | `std::process`/`thread`/`fs` map to cells/tasks/capabilities; `fork` is expensive, `posix_spawn` preferred; POSIX-2024 `posix_spawn_file_actions_addchdir`/`_Fork` help | No change; constrains native libc |
| D4 | Capability ABI underneath, Unix-shaped libc on top | **POSIX.1-2024 API above, capabilities underneath; the libc maps POSIX calls to capability invocations.** | No change; the native libc profile is the new ADR |
| D7 | First target is the iris default | n32/MIPS III/soft-float/R4400; `std` must run there | No change |
| D8 | Framekernel | `std` must not be in the privileged TCB; no kernel `std` | No change; forbids option (d) |
| D9 | Hybrid isolation: SAS for native cells, MMU for legacy | Native `std` shares the address space; a bug can corrupt other cells; compat `std` is MMU-isolated | No change |
| D10 | Core safety; drivers may use `unsafe` | `std` is safe userland; drivers stay `no_std` | No change |
| D11 | Capabilities: object + rights + revocation + delegation | Native libc enforces capability checks for `fs`/`net`/`process`; `std` must not bypass | No change; constrains libc |
| D12 | Synchronous IPC + shared memory | `std` blocking I/O maps to libc/IPC; async crates need mio/tokio, not std alone | No change |
| D13 | Cells load from Rust object files | Cell entry must call `std::rt::lang_start`; `std` is linked into the cell | No change; M9 integration |
| D14 | EDF + fair scheduling | `std::thread` maps to tasks; `available_parallelism` = 1 | No change |
| D16 | DMA-first, raw DMA accepted | `std` does not touch DMA; drivers stay `no_std` | No change |
| D17 | All drivers are de-privileged cells | Drivers may use `core`/`alloc`, not `std` (kernel mode, no process) | No change; forbids `std` in drivers |
| D18 | Userspace async; kernel event-driven | `std` is blocking; `tokio`/`mio` are separate crate work | No change |
| D20 | Minimal ROM + boot bundle | `std` is not in the ROM; bundle cells may use `std` if native | No change |
| D25 | Seeq + smoltcp; `/net` | Native libc sockets over the network cell; `getaddrinfo` required | No change; constrains native libc |
| D27 | Native display server | `std::io`/`thread`/`time`/`sync` for the server; no GUI std | No change |
| D30 | MCP full root, bearer token | MCP cell can use `std`; same security risk as D30 | No change |
| D31 | Plan 9 ideas selectively | `std::fs` sees the namespace; `/net` is synthetic; `std::net` uses sockets | No change |

**Net result:** D2–D4, D9, D11–D13, D17 and D30 already imply the recommended
split. POSIX.1-2024 is now recorded as **D32** in
[`docs/spec/architecture.md`](../../docs/spec/architecture.md): the native
libc's API baseline is POSIX.1-2024/SUSv5, while the IRIX 6.5 compatibility
container keeps its legacy ABI.  D2 makes the native ABI new, and D4 makes the
libc Unix-shaped over capabilities.  The profile and ABI obligations are in
[`docs/spec/posix.md`](../../docs/spec/posix.md); this report supplies the
evidence and the detailed std plan.  POSIX.1-2024 is the
chosen **source API** for that libc, not a replacement for the native binary ABI
or for the IRIX 6.5 compat container. The only governing-record changes are
ADR-0018/0019 (GCC backend), the documentation hygiene around ADR-0016, the
clean-room clarification for the compat `libc` bindings, and the new native
libc profile ADR (ADR-0021).

---

## 12. Recorded diagnostics and prior art

### 12.1 Diagnostics run for this report

Toolchain:

```text
$ rustc -vV
rustc 1.101.0-nightly (0abfedbc7 2026-10-02)
binary: rustc
commit-hash: 0abfedbc7cd4e725f126913880c95800394f7c37
commit-date: 2026-10-02
host: x86_64-unknown-linux-gnu
release: 1.101.0-nightly
LLVM version: 23.1.1

$ cargo -V
cargo 1.101.0-nightly (f3865b2a4 2026-09-29)
```

Current kernel target:

```text
$ rustc -Zunstable-options --print cfg --target targets/mips3-sgi-irix7-n32.json
target_abi="n32"
target_arch="mips64"
target_endian="big"
target_env=""
target_os="none"
target_pointer_width="32"
target_vendor="sgi"
panic="abort"
# no target_family; no `unix`
```

`-Zbuild-std=std` for the current kernel target:

```text
$ cargo build -Zjson-target-spec -Zbuild-std=std \
    --target targets/mips3-sgi-irix7-n32.json
error: none of the predicates in this `cfg_select` evaluated to true
  --> library/std/src/sys/alloc/mod.rs:66
error: none of the predicates in this `cfg_select` evaluated to true
  --> library/std/src/sys/io/error/mod.rs:1
error: Using no_threads implementation on a target with threads
  --> library/std/src/sys/sync/condvar/no_threads.rs:6
...
error: could not compile `std` (lib) due to 14 previous errors
```

A scratch target with `os = "irix"`, `target-family = ["unix"]`,
`panic = "abort"`, `has-thread-local = false`:

```text
$ rustc -Zunstable-options --print cfg --target irix-unix.json
target_env=""
target_family="unix"
target_os="irix"
unix

$ cargo build -Zjson-target-spec -Zbuild-std=std --target irix-unix.json
error[E0432]: unresolved import `unistd`
   --> libc-0.2.189/src/new/mod.rs:255:17
    |
255 |         pub use unistd::*;
error: could not compile `libc` (lib) due to 1 previous error
```

Interpretation:

1. The current kernel target is not a Unix `std` target, and its `std` build
   fails at compile time (good: it does not silently produce a half-std).
2. Merely adding `target_family = ["unix"]` selects the Unix PAL but immediately
   fails in `libc`, because there is no IRIX platform module. A target JSON
   change alone is not a `std` port.
3. The build used a registry `libc` (0.2.189), not the `rust-src` vendored copy;
   the library workspace must be patched or vendored for reproducibility. This
   matches the prior art’s `docs/building.md` note.

The exact failure for a native `irix7` target will differ once a `libc` module
exists; this report does not claim a full build. The scratch `os = "irix"`
target is used only to show the first `libc` failure; a native `irix7` target
has the same failure until an `irix7` `libc` module exists. The purpose of the
diagnostics is to pin the starting point and to disprove the “just flip a cfg”
shortcut.

### 12.2 Prior art

#### `danifunker/rust-irixlibstd`

<https://github.com/danifunker/rust-irixlibstd>

- A std-only port (no rustc fork) for IRIX 6.5.22, o32 and n32.
- `targets/mips-sgi-irix.json` and `targets/mips64-sgi-irix-n32.json`:
  `os = "irix"`, `target-family = ["unix"]`, `panic-strategy = "unwind"`,
  `has-thread-local: false`, `dynamic-linking: true`,
  `crt-objects-fallback: "false"`, `eh-frame-header: true`,
  `default-uwtable: true`, `linker-flavor: "unix-cc"`,
  `linker-is-gnu: false`, `relro-level: "off"`.
- `libc-irix/` is a generated IRIX `libc` module; `patches/std-irix.patch` is a
  34-file `std` patch; `unwinding-mips/` adds MIPS support to the pure-Rust
  `unwinding` crate; `backtrace-irix/` adds a `native_libraries` backend.
- `docs/status.md`: 62 runtime stages, 0 failures on 6.5.22 for o32 and n32;
  995 layout assertions; `panic = "unwind"` and `catch_unwind` work.
- `docs/gaps.md`: the `*at` family is absent; `flock` is a stub; close-on-exec
  is non-atomic; `futimens`/`utimensat` are absent; `CLOCK_MONOTONIC`,
  `pthread_condattr_setclock`, `pthread_setname_np`, `pthread_getattr_np`,
  `clock_nanosleep`, `sem_timedwait` and `pthread_mutex_timedlock` are absent;
  `getrandom`/`getentropy`/`arc4random_buf` are absent; `SO_RCVTIMEO` is
  `ENOPROTOOPT`; `realpath` predates POSIX.1-2008; `madvise` is a stub.
- These are exactly the facilities that POSIX.1-2024 adds or standardises:
  `*at`, `futimens`/`utimensat`, `O_CLOEXEC`/`O_CLOFORK`,
  `MAP_ANON`/`MAP_ANONYMOUS`, `getentropy`,
  `posix_spawn_file_actions_addchdir`, `pthread_cond_clockwait`,
  `posix_getdents`, `ppoll`, `posix_devctl`, `tcgetwinsize`, `dladdr`. The
  native target avoids the IRIX 6.5 patch set by implementing POSIX.1-2024; the
  remaining non-POSIX assumptions are the extension list in §5.1.3.
- `docs/crates.md`: 29 packages surveyed; 17 build unmodified, 9 need patches,
  3 are blocked (`rustix`, `tempfile`, `terminal_size`); `ripgrep` and `tokio`
  run.
- `docs/building.md`: the `libc` patch must live in the **library workspace**
  (`[patch.crates-io] libc = { path = ... }`), and `__CARGO_TESTS_ONLY_SRC_ROOT`
  points at the prepared `library/`; use the IRIX-patched LLD 18 and the
  captured sysroot.
- **Licence:** GitHub API reports `license: null`; there is no licence file in
  the repository. Do not copy code; use it as an ABI/test reference.

This is the strongest evidence that a libc-backed Unix PAL for IRIX 6.5 is
feasible. It is **not** evidence for the native IRIX 7 ABI, and its 6.5.22 floor
does not match the project’s 6.5.7m sysroot. It is also an **IRIX 6.5** port,
not a POSIX-2024 port: its 34-file `std` patch exists precisely because IRIX
6.5 lacks the `*at` family, `futimens`/`utimensat`, `O_CLOEXEC`, `MAP_ANON`,
`getentropy`, `posix_spawn_file_actions_addchdir`, `pthread_cond_clockwait`,
`posix_getdents`, `ppoll`, `posix_devctl`, `tcgetwinsize`, `dladdr`,
`accept4`, `pipe2`, `SOCK_CLOEXEC` and `MSG_NOSIGNAL` that POSIX.1-2024
supplies (or, for `MSG_NOSIGNAL`, POSIX-2008).

#### `atomchild411/rust-irix`

<https://github.com/atomchild411/rust-irix>

- A rustc fork with `mips64-sgi-irix` built in (N32, MIPS IV), std on Rust
  1.96.1, pkgsrc integration.
- `target/mips64-sgi-irix.json`: `os = "irix"`, `target-family = ["unix"]`,
  `panic-strategy = "unwind"`, `dynamic-linking: true`,
  `has-rpath: true`, `default-uwtable: true`, `no-default-libraries: false`.
- It proves that a rustc fork can carry a target and `std` in-tree, but it is a
  heavier maintenance model and it conflicts with the project’s no-rustc-fork
  conclusion. Do not adopt it; use it as prior art for the target fields and
  the std smoke tests.
- **Licence:** GitHub API reports `license: null`; the companion
  `atomchild411/libc` is Apache-2.0. Treat the IRIX-specific changes as
  unlicensed until reviewed.

#### Upstream status

- `rust-lang/rust` has no IRIX target in the platform-support list
  (<https://doc.rust-lang.org/rustc/platform-support.html>).
- `rust-lang/libc` 0.2.189 has no IRIX module; the `unix` dispatch’s final
  branch is “Unknown target_os” (`library/vendor/libc-0.2.189/src/unix/mod.rs`).
- The target tier policy requires a named maintainer, documentation, and (for a
  target that wants `std`) `cc` and `libc` support; it recommends adding a
  minimal `#![no_core]` target first
  (<https://doc.rust-lang.org/rustc/target-tier-policy.html#adding-a-new-target>).
  A native IRIX 7 target is a tier-3 candidate after v1.0, not a prerequisite.

---

## 13. Sources

### 13.1 Rust documentation

- Custom targets: <https://doc.rust-lang.org/rustc/targets/custom.html>
- Target tier policy: <https://doc.rust-lang.org/rustc/target-tier-policy.html>
- Platform support: <https://doc.rust-lang.org/rustc/platform-support.html>
- Adding a new target (rustc dev guide):
  <https://rustc-dev-guide.rust-lang.org/building/new-target.html>
- Cargo `build-std`: <https://doc.rust-lang.org/cargo/reference/unstable.html#build-std>
- Cargo `build-std-features`:
  <https://doc.rust-lang.org/cargo/reference/unstable.html#build-std-features>
- `cfg`/`target_family`/`target_os`/`target_env`:
  <https://doc.rust-lang.org/reference/conditional-compilation.html#unix-and-windows>
- `rustc_codegen_gcc`: <https://github.com/rust-lang/rustc_codegen_gcc>
- `unwinding`: <https://github.com/nbdd0121/unwinding>

### 13.2 Local `rust-src` source

All paths are relative to the pinned sysroot’s `lib/rustlib/src/rust/`:

- `library/std/build.rs` (lines 20–76)
- `library/std/src/lib.rs` (lines 216–226, 480)
- `library/std/Cargo.toml` (lines 36–39)
- `library/std/src/sys/pal/mod.rs` (lines 6–10)
- `library/std/src/sys/pal/unix/mod.rs` (lines 22–49)
- `library/std/src/sys/fs/mod.rs` (lines 8–18)
- `library/std/src/sys/fs/unix.rs` (lines 7–18, 275–293, 555–640, 1212, 1534–1620)
- `library/std/src/sys/thread/mod.rs` (lines 53–100)
- `library/std/src/sys/thread/unix.rs` (lines 55–151)
- `library/std/src/sys/sync/mutex/mod.rs` (lines 21–24)
- `library/std/src/sys/sync/condvar/mod.rs` (lines 18–21)
- `library/std/src/sys/sync/once/mod.rs` (lines 27–36)
- `library/std/src/sys/sync/rwlock/mod.rs` (lines 18–27)
- `library/std/src/sys/thread_local/mod.rs` (lines 44–58, 151–168)
- `library/std/src/sys/thread_local/key/unix.rs` (lines 22–60)
- `library/std/src/sys/io/error/unix.rs` (lines 22–36)
- `library/std/src/sys/random/mod.rs` (lines 1–130)
- `library/std/src/sys/pal/unix/time.rs` (lines 106–109)
- `library/std/src/sys/pal/unix/sync/condvar.rs` (lines 138–179)
- `library/std/src/sys/process/unix/unix.rs` (lines 193, 413, 426–442, 625–680)
- `library/std/src/sys/net/connection/socket/unix.rs` (lines 70–110, 252–280)
- `library/std/src/sys/net/connection/socket/mod.rs` (lines 85–110)
- `library/std/src/sys/pipe/unix.rs` (lines 14–39)
- `library/std/src/sys/paths/unix.rs` (lines 289–300)
- `library/std/src/sys/args/unix.rs` (lines 64–95)
- `library/std/src/sys/pal/unix/stack_overflow/mod.rs` (lines 4–42)
- `library/std/src/sys/pal/unix/stack_overflow/handler_signal.rs` (lines 368, 611)
- `library/unwind/Cargo.toml` (lines 19–20)
- `library/unwind/src/lib.rs` (lines 31–45)
- `library/unwind/src/libunwind.rs` (lines 4–9, 24–39)
- `library/panic_unwind/src/lib.rs` (lines 36–44)
- `library/vendor/libc-0.2.189/src/lib.rs` (unix dispatch)
- `library/vendor/libc-0.2.189/src/unix/mod.rs` (final “Unknown target_os” branch)
- `library/vendor/unwinding-0.2.10/Cargo.toml` (line 26)

### 13.3 Local repository

- `AGENTS.md` (ROM, modules, cache, PROM budget)
- `docs/spec/architecture.md` §2.1 (ADRs), §2.2 (D1–D31), §3.2–3.3, §4.2–4.4,
  §5.4–5.7, §6.1–6.4, §8.4–8.6, §10.2, §15.3
- `docs/spec/boot-protocol.md` §2 (handoff, CPU state)
- `docs/spec/modules.md` §3–§6 (module area, ABI, SDK)
- `docs/plan/milestones.md` M0, M5, M9
- `docs/plan/interfaces.md` (native ABI/native libc planned)
- `docs/plan/risks.md` R6, R17
- `docs/plan/testing.md` (test pyramid, CI)
- `docs/plan/security.md` (trust boundaries)
- `docs/toolchain.md` (current one-target/no-std description)
- `targets/mips3-sgi-irix7-n32.json`
- `Cargo.toml`, `Cargo.lock`
- `kernel/Cargo.toml`, `kernel/src/main.rs`, `kernel/src/module.rs`
- `modules/hello/Cargo.toml`, `modules/hello/linker.ld`,
  `modules/httpd/src/main.rs`
- `module-abi` (external, pinned `v0.6.0`): `Services`/`ModuleVtable`,
  `ABI_VERSION = 10`, compile-time offsets.

### 13.4 External ADRs

All under `https://github.com/irix7/project/blob/main/docs/adr/`:

- `0001-publish-toolchain-work-only.md`
- `0002-gcc-16.2-on-pdaxrom-base.md`
- `0003-all-abis-from-day-one.md`
- `0004-own-rig-fresh-6.5.7m-guest.md`
- `0005-reference-first-rebuild.md`
- `0006-dynamic-first-smoke-harness.md`
- `0007-static-first-rebuilt-runtime.md`
- `0008-irix7-inherits-backward-only-mandate.md`
- `0009-os-repo-publishes-clean-room-only.md`
- `0010-irix7-rust-monolith-n32.md`
- `0011-native-capability-substrate.md`
- `0012-irix7-gpl3.md`
- `0013-full-reimplementation-factory-prom.md`
- `0014-harness-enforced-clean-room.md`
- `0015-layered-spec-similarity-screen.md`
- `0016-o32-ecoff-bootstrap.md`
- `0017-modern-equivalents-as-references.md`
- `0018-gcc-backend-primary.md`
- `0019-irix-fork-on-rust-lang-gcc.md`
- `0020-rebuild-ladder.md`

### 13.5 IRIX 6.5 source (`/mnt/europa/irix7-sources/irix-655-src`)

- `f/root/usr/include/sys.s` (syscall numbers, `SYSVoffset = 1000`)
- `f/root/usr/include/sys/syscall.h` (trap/error convention)
- `f/root/usr/include/sys/stat.h` (`struct stat`, `struct stat64`)
- `f/root/usr/include/sys/dirent.h` (`struct dirent`, no `d_type`)
- `f/root/usr/include/sys/fcntl.h` (`FD_CLOEXEC`, no `O_CLOEXEC`)
- `f/root/usr/include/sys/mman.h` (no `MAP_ANON`)
- `f/root/usr/include/netdb.h` (`gethostbyname`; no `getaddrinfo`)
- `f/root/usr/include/pthread.h` (attributes; no `pthread_getattr_np`)
- `f/root/usr/include/sys/pthread.h` (`pthread_t`, mutex/cond/key types)
- `f/root/usr/include/time.h` (`clock_gettime`)
- `f/root/usr/include/sys/ptimers.h` (`CLOCK_REALTIME`, `CLOCK_SGI_CYCLE`)
- `f/root/usr/include/sys/socket.h` (`SO_RCVTIMEO`, `msghdr` variants)
- `f/root/usr/include/sys/file.h` (`flock` declaration)
- `f/root/usr/include/unistd.h`, `f/root/usr/include/sys/unistd.h` (`sysconf`)
- `f/irix/kern/sys/syscall.h` (kernel syscall table)

### 13.6 Prior art

- <https://github.com/danifunker/rust-irixlibstd> — `docs/status.md`,
  `docs/gaps.md`, `docs/upstreaming.md`, `docs/crates.md`, `docs/building.md`,
  `patches/std-irix.patch`, `targets/mips-sgi-irix.json`,
  `targets/mips64-sgi-irix-n32.json`.
- <https://github.com/atomchild411/rust-irix> — `README.md`,
  `target/mips64-sgi-irix.json`, `tests/std-smoke`.
- <https://github.com/atomchild411/libc> (Apache-2.0; IRIX branch).
- `rust-lang/rust` issue #104985 (32-bit MIPS target emits 64-bit
  instructions): <https://github.com/rust-lang/rust/issues/104985>.

### 13.7 POSIX.1-2024 / SUSv5

The Open Group Base Specifications Issue 8, IEEE Std 1003.1-2024:

- Overview: <https://pubs.opengroup.org/onlinepubs/9799919799/mindex.html>
- Preface (“Interface, Not Implementation”; “Source, Not Object, Portability”):
  <https://pubs.opengroup.org/onlinepubs/9799919799/frontmatter/preface.html>
- System Interfaces table of contents:
  <https://pubs.opengroup.org/onlinepubs/9799919799/functions/toc.html>
- Rationale, changes from Issue 7 to Issue 8, new/removed functions:
  <https://pubs.opengroup.org/onlinepubs/9799919799/xrat/V4_xsh_chap01.html>
- `mmap` (`MAP_ANON`/`MAP_ANONYMOUS`, anonymous memory objects):
  <https://pubs.opengroup.org/onlinepubs/9799919799/functions/mmap.html>
- `getentropy`: <https://pubs.opengroup.org/onlinepubs/9799919799/functions/getentropy.html>
- `posix_spawn_file_actions_addchdir`/`addfchdir`:
  <https://pubs.opengroup.org/onlinepubs/9799919799/functions/posix_spawn_file_actions_addchdir.html>
- `pthread_cond_clockwait`:
  <https://pubs.opengroup.org/onlinepubs/9799919799/functions/pthread_cond_clockwait.html>
- `pthread_mutex_clocklock`:
  <https://pubs.opengroup.org/onlinepubs/9799919799/functions/pthread_mutex_clocklock.html>
- `sem_clockwait`:
  <https://pubs.opengroup.org/onlinepubs/9799919799/functions/sem_clockwait.html>
- `posix_getdents`:
  <https://pubs.opengroup.org/onlinepubs/9799919799/functions/posix_getdents.html>
- `poll`/`ppoll`:
  <https://pubs.opengroup.org/onlinepubs/9799919799/functions/ppoll.html>
- `close`/`posix_close`:
  <https://pubs.opengroup.org/onlinepubs/9799919799/functions/posix_close.html>
- `posix_devctl`:
  <https://pubs.opengroup.org/onlinepubs/9799919799/functions/posix_devctl.html>
- `tcgetwinsize`:
  <https://pubs.opengroup.org/onlinepubs/9799919799/functions/tcgetwinsize.html>
- `dladdr`: <https://pubs.opengroup.org/onlinepubs/9799919799/functions/dladdr.html>
- `open` (`O_CLOEXEC`, `O_CLOFORK`, `FD_CLOFORK`):
  <https://pubs.opengroup.org/onlinepubs/9799919799/functions/open.html>
- `accept`/`accept4`:
  <https://pubs.opengroup.org/onlinepubs/9799919799/functions/accept.html>
- `pipe`/`pipe2`:
  <https://pubs.opengroup.org/onlinepubs/9799919799/functions/pipe.html>
- `dup`/`dup2`/`dup3`:
  <https://pubs.opengroup.org/onlinepubs/9799919799/functions/dup.html>
- `socket` (`SOCK_CLOEXEC`/`SOCK_CLOFORK`/`SOCK_NONBLOCK`):
  <https://pubs.opengroup.org/onlinepubs/9799919799/functions/socket.html>
- `send` (`MSG_NOSIGNAL`):
  <https://pubs.opengroup.org/onlinepubs/9799919799/functions/send.html>
- `futimens`/`utimensat`/`utimes`:
  <https://pubs.opengroup.org/onlinepubs/9799919799/functions/futimens.html>
- `sysconf` (`_SC_NPROCESSORS_ONLN`, `_SC_HOST_NAME_MAX`, `_SC_PAGESIZE`):
  <https://pubs.opengroup.org/onlinepubs/9799919799/functions/sysconf.html>
- `pthread_attr_getstack`/`pthread_attr_setstack`:
  <https://pubs.opengroup.org/onlinepubs/9799919799/functions/pthread_attr_getstack.html>
- `lockf` (XSI): <https://pubs.opengroup.org/onlinepubs/9799919799/functions/lockf.html>
- `posix_madvise`:
  <https://pubs.opengroup.org/onlinepubs/9799919799/functions/posix_madvise.html>
- `shm_open`:
  <https://pubs.opengroup.org/onlinepubs/9799919799/functions/shm_open.html>

POSIX.1-2024 **includes** `accept4`, `pipe2`, `dup3`, `SOCK_CLOEXEC`,
`SOCK_CLOFORK`, `SOCK_NONBLOCK` and `MSG_NOSIGNAL` (the last since
POSIX-2008); they are base requirements, not extensions. It does **not**
include `pthread_getattr_np`, `pthread_setname_np`, `dl_iterate_phdr`, `flock`,
`madvise`, `getrandom`, `RTLD_DEFAULT`/`RTLD_NEXT`, `readdir`'s `d_type`,
`st_mtime_nsec` or `getexecname`; these are the extensions in §5.1.3. It
**removes** `ioctl` and `gettimeofday`; the replacements and extensions are in
§5.1.4.

---

## Appendix A: Native target checklist

Before the native target is committed:

- [ ] `os = "irix7"`, `target-family = ["unix"]`, `abi = "n32"`.
- [ ] The POSIX.1-2024 profile and extension list are in `native-libc.md`.
- [ ] `std/build.rs` knows `irix7`; `std` is not `restricted_std`.
- [ ] `libc` has an `irix7` module generated from the native ABI spec.
- [ ] `std` rlib builds; the PAL smoke crate compiles.
- [ ] Layout assertions cover the native structs.
- [ ] The cell `entry!` calls `lang_start` and catches panics at the boundary.
- [ ] `posix_spawnp` + `posix_spawn_file_actions_addchdir`, pthreads,
      `*at`, `O_CLOEXEC`/`O_CLOFORK`, `futimens`/`utimensat`,
      `CLOCK_MONOTONIC`, `getaddrinfo`, `MAP_ANON`, `getentropy`,
      `pthread_cond_clockwait`, `posix_getdents`, `ppoll`, `posix_devctl`,
      `tcgetwinsize`, `accept4`, `pipe2`, `dup3`,
      `SOCK_CLOEXEC`/`SOCK_CLOFORK`/`SOCK_NONBLOCK` and `MSG_NOSIGNAL` are
      implemented.
- [ ] The documented extensions (`pthread_getattr_np`, `pthread_setname_np`,
      `dl_iterate_phdr`, `flock`, `madvise`, `getrandom`, `RTLD_DEFAULT`/
      `RTLD_NEXT`, `d_type`, `st_mtime_nsec`, `getexecname`, `ioctl`,
      `gettimeofday`) are each either provided or patched in `std`.
- [ ] The runtime conformance suite passes on iris.
- [ ] The kernel/ROM/module graphs contain no `std`.
- [ ] The compat target is separate and not linked into native cells.

## Appendix B: Compat target checklist (optional, private)

- [ ] Choose the floor: 6.5.7m (sysroot) or 6.5.22 (prior art). Do not mix.
- [ ] Generate the `libc` IRIX module from the chosen sysroot, privately.
- [ ] Patch `std` for IRIX (`target_os = "irix"`), using the prior art as a
      reference but not copying its unlicensed code.
- [ ] Build with `-Zbuild-std=core,alloc,std,panic_unwind` and the IRIX-patched
      LLD.
- [ ] Run the staged conformance suite in the compat container.
- [ ] Do not publish the generated bindings; do not define the native ABI from
      this target.
