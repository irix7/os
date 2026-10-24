# Native IRIX 7 libc — interface specification

**Status:** draft
**Version:** 0.1
**Owner:** ABI / libc
**Related:** [`posix.md`](posix.md) (API baseline), [`architecture.md`](architecture.md) §5 (native ABI and
process model) and §6 (compatibility container), [`elf.md`](elf.md) (ELF loader contract),
[`driver-interface.md`](driver-interface.md) (`ctl`/`data`/`status` and `posix_devctl`),
[`modules.md`](modules.md) (kernel modules are `no_std`), and the
[`std` research report](../research/rust-std-irix7.md).

**Machine-readable manifest:** [`native-libc-symbols.json`](native-libc-symbols.json).
**Rust layout source:** [`crates/native-abi`](../../crates/native-abi/README.md) — provisional v0 types,
layouts and constants, with host layout tests.
**Audit tool:** [`../../tools/std-abi-surface.py`](../../tools/std-abi-surface.py).

## 1. Scope and status

This document is the draft interface contract for the **native IRIX 7 libc**.  It owns the
n32 C binary ABI: symbol names, type widths, public layouts, `errno`, TLS, startup, the
libc-to-capability boundary and the conformance rules.  The **source API** is
[POSIX.1-2024](posix.md) plus a documented XSI subset and a short extension list.  POSIX
defines the C source contract; it does not define this binary contract.

This is a specification, not an implementation and not a certification claim.  No native
libc exists yet: M9 builds the first one, and M9.5 builds the first native `std` subset
([`../plan/milestones.md`](../plan/milestones.md)).  Every statement below is a
requirement on that work, not a report of shipped behaviour.

The document deliberately separates three things:

1. **Decided** — the native ABI is new, n32, big-endian, capability-backed, and distinct
   from the IRIX 6.5 compatibility ABI.
2. **Proposed for review** — the POSIX profile, XSI subset and extension list.  These are
   specific enough to implement and review, but v0.1 is a draft.
3. **Open** — the exact `struct stat` size, the capability invocation numbers, the errno
   numeric values, the cell start structure and the dynamic-loading model.  Each is marked
   open in §9 with a recommended default.  Do not implement an open item as if it were
   frozen.

The compatibility container is **not** covered.  Its IRIX 6.5 libc ABI, `sproc`, `syssgi`,
`procntl`, IRIX `ioctl` numbers, `struct stat` and `rld` conventions stay in the container
and are never exposed by the native libc (report §5.5).

## 2. Two targets, one kernel

The native world and the compatibility container are separate products with separate
targets, separate libcs and separate source contracts (D2, D4, D32).

| Property | Native IRIX 7 | IRIX 6.5 compatibility | Kernel, ROM, modules |
|---|---|---|---|
| Target file | `targets/mips3-sgi-irix7-native-n32.json` (committed) | `targets/mips64-sgi-irix-n32.json` (proposed; o32 later) | `targets/mips3-sgi-irix7-n32.json` (existing) |
| Rust cfg identity | `target_os = "irix7"`, `target_family = ["unix"]` | `target_os = "irix"`, `target_family = ["unix"]` | `target_os = "none"`, no `target_family` |
| Source API | POSIX.1-2024 + selected XSI + extensions | IRIX 6.5 libc and headers | none |
| Binary ABI | native n32 (this document) | IRIX 6.5 o32/n32 exactly | kernel/module ABI |
| Rust runtime | `std` from a pinned, patched `rust-src` | separate `std`, optional test track | `core` + `alloc` only |
| Process model | one cell with a Unix personality (D3) | legacy MMU process | kernel thread |
| Panic | `unwind` (phase 2); `abort` bring-up (phase 1) | `unwind` or `abort` | `abort` |
| Runs on | native cell loader (M9) | compatibility container | iris / Indy |

Rules:

- The kernel, ROM and loadable modules stay `no_std`.  They do not link this libc, and
  `std` must not appear in their dependency graphs (D8, D17).
- A native build must never take a `target_os = "irix"` arm; a compat build must never take
  an `irix7` arm.  The native libc carries a compile-time assertion that it is not being
  built for `target_os = "irix"`, and the compat libc carries the inverse.
- The native target JSON is committed at `targets/mips3-sgi-irix7-native-n32.json`; its
  cfg identity is fixed here, and future field changes are ABI/toolchain changes, not
  cosmetic edits.  The std research report recommends exactly this split (report §3.4).
- `std` is compiler-version-locked.  The stable seam is this C ABI, not a Rust ABI; the OS
  ships a pinned toolchain and sysroot (report §4.3).

## 3. POSIX.1-2024 base profile

The native libc implements the **POSIX.1-2024 base** (IEEE Std 1003.1-2024, Base
Specifications Issue 8, the core of SUSv5), limited to the facilities native applications
and Rust `std` need.  It is not the whole standard.  The profile is versioned; each entry in
the manifest records its class and whether v1 promises it.

### 3.1 Profile areas

| Area | Facilities in the v1 profile |
|---|---|
| Process and signals | `fork`, `_Fork`, `posix_spawn` `[SPN]`, `posix_spawn_file_actions_*` (including `addchdir`/`addfchdir`), `posix_spawnattr_*`, `execve`, `execvp`, `waitpid`, `waitid`, `_exit`, `kill`, `killpg`, `getpid`, `getppid`, `setpgid`, `getpgid`, `setsid`, `tcgetpgrp`, `tcsetpgrp`, `sigaction`, signal sets, `sigprocmask`, `pthread_sigmask`, `sigaltstack`, `raise` |
| Threads | `pthread_create`, `pthread_join`, `pthread_detach`, `pthread_self`, `pthread_equal`, attributes (including `pthread_attr_getstack` `[TSA]`/`setstack` `[TSS]`), `pthread_key_*`, `pthread_once`, `pthread_mutex_*`, `pthread_mutex_clocklock`, `pthread_cond_*`, `pthread_condattr_setclock`, `pthread_cond_clockwait`, `sched_yield` |
| Clocks and timers | `clock_gettime` (`CLOCK_REALTIME`, `CLOCK_MONOTONIC`), `clock_getres`, `nanosleep`, `clock_nanosleep` (optional) |
| Memory | `malloc`, `calloc`, `realloc`, `free`, `posix_memalign`, `aligned_alloc`, `mmap` with `MAP_ANON`/`MAP_ANONYMOUS` `[SHM]`, `munmap`, `mprotect`, `msync`, `posix_madvise` `[ADV]` (optional) |
| Files and metadata | the `*at` family, `O_CLOEXEC`/`O_CLOFORK`, `F_DUPFD_CLOEXEC`, `posix_close`, `read`/`write`/`pread`/`pwrite`/`readv`/`writev`/`preadv`/`pwritev`, `lseek`, `fstat`/`stat`/`lstat`/`fstatat`, `futimens`/`utimensat`, `mkdirat`, `unlinkat`, `renameat`, `linkat`, `symlinkat`, `readlinkat`, `fchmodat`, `fchownat`, `faccessat`, `opendir`/`fdopendir`/`dirfd`/`readdir`/`posix_getdents`, `realpath`, `getcwd`/`chdir`/`fchdir`, `statvfs`/`fstatvfs`, `pipe`/`pipe2`, `dup`/`dup2`/`dup3`, `fcntl` |
| Networking | `socket`, `socketpair`, `bind`, `connect`, `listen`, `accept`, `accept4`, `send`/`recv`, `sendto`/`recvfrom`, `setsockopt`/`getsockopt`, `getsockname`/`getpeername`, `shutdown`, `poll`/`ppoll`, `getaddrinfo`/`freeaddrinfo`/`getnameinfo`/`gai_strerror`, `inet_pton`/`inet_ntop`; `SOCK_CLOEXEC`/`SOCK_CLOFORK`/`SOCK_NONBLOCK`, `MSG_NOSIGNAL` |
| Environment | `environ`, `getenv`/`setenv`/`unsetenv`/`putenv`, `getuid`/`geteuid`/`getgid`/`getegid`, `sysconf`, `confstr`, `pathconf`/`fpathconf`, `uname`, `getpwuid_r` |
| Terminal and stdio | `isatty`, `ttyname`, `tcgetattr`/`tcsetattr`, `tcgetwinsize`/`tcsetwinsize` (optional in v1), `posix_devctl` `[DC]`, `/dev/null`, the standard file descriptors |
| Errors | `errno` (through `__errno_location`), `strerror`, `strerror_r`, `perror` |
| Randomness | `getentropy` |
| Dynamic loading | `dlopen`/`dlsym`/`dlclose`/`dlerror`/`dladdr`; post-v1 (see §4.8) |

The table is a summary; [`native-libc-symbols.json`](native-libc-symbols.json) is the
authority for individual names.  It currently has **642 entries**, of which **526** are
promised for v1.

### 3.2 Option groups

POSIX option groups are not implied by “POSIX.1-2024”.  The v1 profile selects:

| Option | v1 | Why |
|---|---|---|
| `[SHM]` | yes | `MAP_ANON`/`MAP_ANONYMOUS` for the allocator, `memmap2` and the stack-overflow handler |
| `[SPN]` | yes | `posix_spawn` is the preferred native process creation path (D3) |
| `[DC]` | yes | `posix_devctl` replaces `ioctl` for native drivers |
| `[TSA]` / `[TSS]` | yes | `pthread_attr_getstack`/`setstack` for stack bounds |
| `[ADV]` | optional | `posix_madvise` is small but not needed by v1 `std` |
| `[XSI]` | selected subset | see §3.3 |
| `[AIO]`, `[MQ]`, `[PS]`, `[RTS]`, `[SEM]`, `[TMO]`, `[TYM]` | no | outside the v1 product; see §6.3 |

### 3.3 Selected XSI subset

The v1 XSI surface is deliberately short:

- `lockf` and `utimes` (record locks and the legacy timestamp call);
- `chroot` (capability-checked; it returns `EPERM` without a namespace capability);
- the `S_IF*` file-type macros and `WCOREDUMP`, which XSI marks and `std` uses;
- the XSI-marked parts of the file and terminal interfaces that the base profile already
  needs.

`mknod`, `sysmp`, `sproc`, `arenal`, `procntl` and the IRIX `ioctl` set are **not** in the
v1 XSI subset.  The IRIX names are compatibility-container only.

The project does **not** claim XSI certification.  The subset is a documented
implementation choice for the native world, not a conformance statement.

### 3.4 Documented extensions

An extension is a name outside POSIX.1-2024 that the native libc provides on purpose,
because `std` or a v1 crate assumes it.  It is versioned like every other symbol and must
be listed in the manifest.  No extension may silently change a POSIX call's semantics.

| Extension | Decision | Why |
|---|---|---|
| `pthread_getattr_np` | provided | the signal stack-overflow handler needs the current thread's stack bounds; POSIX only has `pthread_attr_getstack` on an attribute |
| `pthread_setname_np` | provided | `std::thread::Builder::name`; without it the name stays Rust-side |
| `dl_iterate_phdr` | optional, post-v1 | ELF module iteration for symbolised backtraces; POSIX-2024 has `dladdr` |
| `flock` | **not provided** | `std::fs::File::lock` must use `fcntl` record locks or return `Unsupported`; a fake success is forbidden |
| `madvise` | provided | `memmap2` and crates; `posix_madvise` is the POSIX call |
| `getrandom` | provided | `getrandom`/`rand` crate compatibility; `getentropy` is the v1 backend |
| `RTLD_DEFAULT`, `RTLD_NEXT` | optional, post-v1 | ELF symbol lookup for crates |
| `struct dirent.d_type` | provided | `std::fs` and `readdir` want it; POSIX-2024 only guarantees `d_type` in `posix_dent`.  The extension field is documented in §4.1 |
| `st_mtime_nsec` field spelling | **not provided** | the native `struct stat` uses POSIX `st_atim`/`st_mtim`/`st_ctim`; `std` gets an `irix7` arm |
| `getexecname` | provided | `std::env::current_exe`; the cell loader knows the image path; no `/proc` requirement |
| `ioctl` | provided for legacy code | removed in Issue 8; `posix_devctl` is the native call; IRIX `ioctl` numbers are not exposed |
| `gettimeofday` | provided for legacy code | removed in Issue 8; `clock_gettime(CLOCK_REALTIME)` is the native call |
| `setgroups` | provided | `std::process::Command` and Unix tools; capability-checked |
| `FIOCLEX`, `FIONBIO`, `TIOCGWINSZ` | provided | the `ioctl` requests `std` and `terminal_size` use on generic Unix; prefer `fcntl(F_SETFD/F_SETFL)` and `tcgetwinsize` |
| `__errno_location` | provided | the stable `errno` entry point `std` expects |
| 64-suffixed aliases (`open64`, `stat64`, …) | optional | native `off_t`/`ino_t` are already 64-bit; aliases exist only for source compatibility |

### 3.5 What is not implemented

`aio_*`, POSIX message queues, semaphores (optional), typed memory, the trace option
(removed in Issue 8), STREAMS, and the Linux-only `epoll`, `eventfd`, `timerfd`,
`signalfd`, `inotify`, `statx`, `sendfile`, `splice` and `copy_file_range` are outside the
v1 profile.  They are recorded as `unsupported` in the manifest so that a crate does not
mistake their absence for an oversight.  A crate that needs one gets a documented error or
a crate-side port, not a kernel feature.

### 3.6 Not a certification claim

Implementing a subset of POSIX.1-2024 is not POSIX or XSI certification.  Certification
needs the full standard, the conformance suites, a conformance document and a vendor
statement.  The project publishes this profile and a conformance matrix instead.  The
position is the same as [`posix.md`](posix.md) §8.

## 4. The n32 binary ABI

The native ABI is n32, MIPS III, big-endian, `ILP32` with 64-bit registers.  It is **new**:
it does not reuse IRIX 6.5 syscall numbers, `struct stat`, `errno` values, `rld`
conventions or trap convention.  Those belong to the compatibility container.

### 4.1 Data model and public types

Fixed widths:

| Type | Width | Signedness | Notes |
|---|---:|---|---|
| `size_t` | 32 | unsigned | n32 pointers are 32-bit |
| `ssize_t` | 32 | signed | |
| `off_t` | 64 | signed | no `_FILE_OFFSET_BITS` switch; `off_t` is always 64-bit |
| `ino_t` | 64 | unsigned | |
| `dev_t` | 64 | unsigned | |
| `mode_t` | 32 | unsigned | |
| `nlink_t` | 32 | unsigned | |
| `blkcnt_t` | 64 | signed | |
| `blksize_t` | 32 | signed | |
| `pid_t` | 32 | signed | |
| `uid_t`, `gid_t` | 32 | unsigned | |
| `time_t` | 64 | signed | no 2038 switch |
| `clockid_t` | 32 | signed | |
| `socklen_t` | 32 | unsigned | |
| `sa_family_t` | 16 | unsigned | |
| `id_t` | 32 | unsigned | |
| `long` | 32 | signed | ILP32 |
| `long long` | 64 | signed | 8-byte aligned |

Every public struct is `repr(C)`-compatible, big-endian, free of bitfields, and padded by
the libc with zero bytes.  64-bit fields are 8-byte aligned.  The stack is 16-byte aligned
at a public function call.  Variadic functions follow the n32 C convention.

Required public structs (the list is the ABI surface, not a suggestion):

| Group | Structs |
|---|---|
| Files | `stat`, `statvfs`, `dirent`, `posix_dent`, `DIR` (opaque), `FILE` (opaque) |
| Time | `timespec`, `timeval`, `itimerspec`, `sigevent` |
| Signals | `sigaction`, `siginfo_t`, `sigset_t`, `stack_t`, `ucontext_t` |
| Threads | `pthread_attr_t`, `pthread_mutex_t`, `pthread_mutexattr_t`, `pthread_cond_t`, `pthread_condattr_t`, `pthread_rwlock_t`, `pthread_rwlockattr_t`, `pthread_barrier_t`, `pthread_barrierattr_t`, `pthread_spinlock_t`; `pthread_t`, `pthread_key_t` are scalars |
| Sockets | `sockaddr`, `sockaddr_in`, `sockaddr_in6`, `sockaddr_storage`, `sockaddr_un`, `msghdr`, `cmsghdr`, `iovec`, `pollfd`, `fd_set`, `addrinfo`, `linger` |
| Terminal | `termios`, `winsize` |
| Environment | `passwd`, `utsname`, `rlimit` |
| Loading | `Dl_info` (post-v1) |
| Optional | `sem_t` |

Recommended sizes and offsets (items marked open are pinned later by the `native-abi`
crate and its layout assertions):

| Struct | Requirement |
|---|---|
| `timespec` | `tv_sec` (`time_t`, 64-bit), `tv_nsec` (32-bit signed), 16 bytes with padding |
| `timeval` | `tv_sec` (64-bit), `tv_usec` (32-bit signed), 16 bytes with padding |
| `sockaddr_in` | 16 bytes; `sin_family`, `sin_port`, `sin_addr`, zero padding |
| `sockaddr_in6` | 28 bytes; `sin6_family`, `sin6_port`, `sin6_flowinfo`, `sin6_addr`, `sin6_scope_id` |
| `sockaddr_storage` | at least 128 bytes, 8-byte aligned |
| `sockaddr_un` | `sun_family` plus a 108-byte `sun_path`; 110 bytes (open: confirm with the VFS path limit) |
| `iovec` | 8 bytes (32-bit base, 32-bit length) |
| `pollfd` | 8 bytes (`fd: i32`, `events: i16`, `revents: i16`) |
| `winsize` | 8 bytes (four `u16`) |
| `sigset_t` | 128 bytes (1024 signals); open: confirm with the scheduler's signal limit |
| `stack_t` | 12 bytes (`ss_sp`, `ss_size`, `ss_flags`) |
| `rlimit` | 16 bytes (`rlim_cur`, `rlim_max`, both 64-bit) |
| `stat` | `st_dev`, `st_ino`, `st_mode`, `st_nlink`, `st_uid`, `st_gid`, `st_rdev`, `st_size`, `st_blksize`, `st_blocks`, `st_atim`, `st_mtim`, `st_ctim`; total size open, recommended 128 bytes |
| `dirent` | `d_ino` (64-bit), `d_off` (64-bit), `d_reclen` (16-bit), `d_type` (8-bit, extension), `d_name` (256 bytes); open: `NAME_MAX` |
| `posix_dent` | `d_ino`, `d_off`, `d_reclen`, `d_type`, `d_name[]`; the POSIX-2024 directory record |
| `sigaction` | handler union, `sa_mask`, `sa_flags`, an optional restorer pointer; open: exact order and size |
| `siginfo_t` | at least 128 bytes; open: the signal payload union |
| `ucontext_t` | open: must match the signal frame; size TBD |
| `pthread_*` | opaque storage with the natural alignment of a pointer; exact sizes open, recommended 32 bytes each |
| `fd_set` | 128 bytes (1024 descriptors); open: `FD_SETSIZE` |

`struct stat` uses the POSIX field names `st_atim`, `st_mtim`, `st_ctim`, not the Linux
`st_atime`/`st_mtime`/`st_mtime_nsec` spelling.  `std` needs a small `irix7` arm for the
timestamp fields; do not add the Linux spelling to the ABI to avoid that patch.

`struct dirent` carries `d_type` as a **documented extension**; POSIX-2024 only guarantees
`d_ino` and `d_name` there.  Native `std` may use either `d_type` or `posix_getdents`, whose
`posix_dent` has the standard type field.  `readdir_r` is a compatibility extension; the
native reader is `posix_getdents` (or `readdir`).

### 4.2 Symbol naming and versioning

- Public functions use the POSIX C names exactly.  Leading underscores are reserved for
  the native runtime: `_start`, `_exit`, `_Fork`, `__errno_location`, and the ABI version
  symbol.  Do not add IRIX's historical underscore spellings.
- No `sproc`, `syssgi`, `arenal`, `procntl`, `sysmp` or IRIX `ioctl` names in the native
  libc (§1).
- The recommended v1 model is **static-first**: the libc is linked into the cell, so there
  is no dynamic symbol table and no symbol versioning yet.  This follows ADR-0007's
  static-first principle and the report's recommended default (report §10.7); the shared
  model is open (§9).
- When the libc becomes shared, it uses `libc.so.0` as the soname and a single version node,
  `IRIX7_1.0`, with append-only symbol versions.  Removing or changing a symbol's layout
  requires a new soname.  The timing of the shared libc is open (§9).
- The libc exposes an ABI version for diagnostics and for the loader:
  `unsigned long __irix7_libc_abi_version(void)`, returning `1` for the v1 ABI.
  The name is a recommended default, not a POSIX name; the cell ABI version is a separate
  number owned by the loader.
- Rust `libc` bindings are generated from the same `native-abi` crate and the C headers.
  There is no second hand-written copy (report §7.6).

### 4.3 `errno`

- `errno` is accessed through `int *__errno_location(void)`.  The returned pointer is
  per-thread and is never shared between tasks.
- `errno` starts at zero in each new task and is not cleared by successful calls.
- Functions that return `-1` set `errno`; pthread functions return the error number and do
  not set `errno` (POSIX rule).
- The numeric values are **open**.  Recommended default: the Linux/asm-generic values,
  because Rust crates and test suites commonly assume them, with `EWOULDBLOCK == EAGAIN`
  and `ENOTSUP == EOPNOTSUPP`.  Do **not** inherit IRIX 6.5's values; the compat container
  keeps those.
- The `E*` set in the manifest is the complete v1 set.  Adding an error number is additive;
  changing one is a breaking ABI change.
- `strerror` returns a stable English string; `strerror_r` uses the POSIX (int-returning)
  form.  The GNU form is not part of the native ABI.
- `EINTR` is returned when a signal interrupts a blocking call.  The libc does not restart
  automatically; `SA_RESTART` is honoured by the signal layer, and `std` retries where it
  needs to.

### 4.4 Thread-local storage

- v1 uses **pthread keys**, not ELF TLS: `pthread_key_create`, `pthread_setspecific`,
  `pthread_getspecific`, `pthread_key_delete`.  The native target sets
  `has-thread-local: false`, which selects `std`'s pthread-key implementation.
- `PTHREAD_KEYS_MAX` is at least 128; `PTHREAD_DESTRUCTOR_ITERATIONS` is at least 4.
  (Recommended defaults; confirm in the implementation.)
- `errno` uses an internal key (or a reserved per-task slot) created before any other
  thread-local state.  The first key is reserved for `errno`.
- ELF TLS, `PT_TLS` and `__tls_get_addr` are a later loader feature.  When they arrive,
  `has-thread-local` may become `true`, but the pthread-key ABI stays for source
  compatibility.  This is open (§9) and does not block M9.
- The thread pointer convention and the static TLS block layout belong to the loader
  contract, not to POSIX; the cell loader and the libc must agree on one.

### 4.5 Startup, `_start`, argv/envp/auxv

The native cell loader starts an image at `_start`.  The initial stack follows the
System V/IRIX shape:

```text
sp -> argc
      argv[0] .. argv[argc-1], NULL
      envp[0] .. envp[n-1], NULL
      auxv[0].a_type, auxv[0].a_val, ..., AT_NULL, 0
```

The initial `sp` is 16-byte aligned.  Each pointer-sized slot is 4 bytes; the auxv pairs
are 8 bytes each.

- `_start` is provided by the cell CRT (or the SDK's `entry!` macro), not by the Rust
  `std` runtime.  It reads `argc`/`argv`/`envp`/`auxv` from the stack and calls the
  libc's start routine, which initialises `errno`, the TLS key, the standard descriptors,
  the allocator and the capability table view, then calls the cell entry.
- The cell entry calls `std::rt::lang_start` with `argc`/`argv` so that
  `std::env::args` works.  A `#![no_main]` cell is the normal shape; the SDK macro supplies
  `_start` and a `catch_unwind` boundary at the cell edge.
- `__libc_start_main` is **not** part of the native contract; it is a glibc-shaped
  compatibility name and is marked `compat-only` in the manifest.
- The loader passes a `CellStart` structure with at least: ABI version, `argc`, `argv`,
  `envp`, `auxv`, the capability table, the namespace root, and the requested stack size.
  The exact layout is open and belongs to the native ABI (`native-abi.md`); the libc must
  not define a second copy.
- Recommended `auxv` keys: `AT_PAGESZ`, `AT_ENTRY`, `AT_PHDR`, `AT_PHNUM`, `AT_BASE`
  (zero for a static cell), `AT_RANDOM`, `AT_EXECFN`, `AT_UID`, `AT_EUID`, `AT_GID`,
  `AT_EGID`, `AT_HWCAP`, `AT_NULL`.  The exact set is open.
- C `__attribute__((constructor))`/`destructor` support is optional in v1.  Rust `std`
  does not need it; a C library that does gets it when the native CRT grows an init array.
- `_start` must never return.  It calls `_exit` (or the cell exit path) with the
  program's status.

### 4.6 Syscall and capability entry

- Native code does **not** execute the IRIX `syscall` instruction.  The libc is the only
  component that performs capability invocations, and it does so through the native ABI
  crate (`native-abi`): typed `Channel` calls and object invocations with rights checks
  (D11, D12).
- The exact invocation numbers, register convention, blocking semantics and error model are
  **open**; they are owned by `native-abi.md` (planned) and the M9 kernel work, not by this
  document.  The libc must not expose raw invocation numbers to C or Rust callers.
- The libc maps a native error to `-1`/`errno` (or to a pthread error number).  A capability
  failure is `EPERM`, `EACCES` or `EBADF` as appropriate; it is never a silent success.
- Blocking calls block the calling task on a `Port` or `Channel`.  The POSIX cancellation
  points are the points at which a pending cancellation may be delivered.
- The compatibility container keeps the IRIX `$v0`/`$a3` trap convention.  The two
  conventions must never be mixed; the distinct `target_os` values make a mix a build
  error, and the libc carries a compile-time assertion.

### 4.7 Threads, cancellation and `fork`

- `pthread_create` creates a native task; `pthread_join` waits on its exit event;
  `pthread_detach` releases the task handle.  Threads share the cell's address space and
  capability table.
- Cancellation is **deferred** at the POSIX cancellation points.  Asynchronous cancellation
  and cancellation of a thread blocked in a capability invocation are optional; if they are
  not implemented, the call returns `ENOTSUP` rather than hanging.  `pthread_setcancelstate`,
  `pthread_setcanceltype`, `pthread_testcancel`, `pthread_cleanup_push` and
  `pthread_cleanup_pop` are the API.
- `fork` clones the cell's memory (copy-on-write where the SAS kernel can support it) and
  its capability table.  `posix_spawn` is the default for new processes because a fork of a
  large cell is expensive (D3, report §5.4).
- Between `fork` and `exec`, only async-signal-safe functions may be called.  The libc must
  not allocate, take a lock or use stdio in that window.  `_Fork` is the async-signal-safe
  variant and does not run `pthread_atfork` handlers.
- `pthread_atfork(prepare, parent, child)` handlers run in the POSIX order: prepare in
  reverse registration order, parent and child in registration order.  A child handler must
  not assume that locks held by vanished threads are usable.
- `execve` replaces the cell image and resets signal dispositions and the thread set.  It
  keeps the capability table only as far as the loader grants it; a child's capabilities
  are attenuated from the parent's, never amplified.
- The exact stack size defaults (`PTHREAD_STACK_MIN`, the main-thread stack, the guard
  size) are recommended defaults pending the loader contract: 64 KiB minimum, 2 MiB
  default, one guard page.  `std` queries the values it needs through the pthread API.

### 4.8 Dynamic loading

- The recommended v1 model is static: no `dlopen`, no shared libc, no `rld`.  A cell is one
  image linked against the static libc.  This is open (§9); the rest of this section
  describes the post-v1 loading model.
- The native loader will gain `PT_DYNAMIC`, `DT_NEEDED`, symbol resolution,
  `PT_GNU_EH_FRAME`, `dlopen`/`dlsym`/`dlclose`/`dlerror`, `dladdr` and the `RTLD_*` flags
  after v1.  `dl_iterate_phdr`, `RTLD_DEFAULT` and `RTLD_NEXT` are extensions.
- `dladdr` is POSIX-2024 and is the preferred module lookup for backtraces; `dl_iterate_phdr`
  is the ELF extension and is optional.
- The shared libc, when it exists, uses the soname and version node in §4.2.  The native
  loader is not the compat container's `rld`; the compat loader stays in `elf.md` and the
  container.
- `getexecname` (extension) is the loader-provided executable path for
  `std::env::current_exe`.  A cell without it returns an error from `current_exe`, which is
  acceptable; `/proc/self/exe` is not a native requirement.

### 4.9 Capability enforcement

The libc is the enforcement point between POSIX names and capabilities (D4, D11, report
§5.4).  A name never grants authority; a capability does.

| POSIX operation | Check |
|---|---|
| `open`, `openat`, `stat`, `fstatat`, `opendir`, `chdir` | namespace resolution; the resulting `File` capability must carry the requested rights |
| `read`, `write`, `pread`, `pwrite`, `mmap` of a file | the fd's `File` rights; a read-only fd cannot write |
| `mmap`, `mprotect`, `munmap` | `VMAR`/`VMO` `map`/`read`/`write` rights; `PROT_EXEC` needs `execute` |
| `socket`, `bind`, `connect`, `listen`, `accept` | `Socket` rights plus the network capability; bind/listen also need the port grant |
| `posix_spawn`, `execve`, `_Fork` + `exec` | spawn/exec capability; the child inherits an attenuated set |
| `chown`, `fchown`, `lchown`, `chmod`, `fchmod`, `setuid`, `setgid`, `setgroups`, `chroot` | the matching capability; otherwise `EPERM`; there is no ambient root |
| `kill`, `killpg` | a signal capability for the target job, or membership in the same job |
| `ioctl`, `posix_devctl` | a `Device` capability for the target; `posix_devctl` is the native call |
| `SCM_RIGHTS` fd passing | `transfer`/`dup` rights on the fd |
| `dlopen` (post-v1) | a load capability for the image |
| `getentropy` | no capability; it is a system service, rate-limited if needed |

`FromRawFd`/`from_raw_fd` is `unsafe` in Rust and the caller owns the contract, but the libc
still validates the handle generation on every use.  A stale or forged handle returns
`EBADF`; it is never dereferenced.

## 5. The API surface required by Rust `std`

`std` on Unix is the whole Unix PAL, not a minimal subset.  A `target_family = "unix"`
target gets `std::io`, `std::fs`, `std::net`, `std::process`, `std::env`, `std::time`,
`std::thread`, `std::sync`, the `std::os::unix` extension traits, and the `libc` crate
(report §2.1).  The native libc must therefore cover the names in the manifest, not a
convenience subset.

The audit of the pinned `rust-src` (`nightly-2026-10-03`) finds **197 files**, **731**
`libc::` references and **362 distinct names** for this cfg model.  The counts move with
`std`; rerun the tool after a toolchain bump.

### 5.1 Process

- Functions: `fork`, `execvp`, `execve`, `_exit`, `exit`, `abort`, `waitpid`, `waitid`,
  `kill`, `killpg`, `getpid`, `getppid`, `setpgid`, `getpgid`, `setsid`, `tcgetpgrp`,
  `tcsetpgrp`, `posix_spawn*`, `_Fork`, `sigaction`, `sigemptyset`, `sigaddset`,
  `sigprocmask`, `pthread_sigmask`, `sigaltstack`, `raise`, `signal`.
- Types and constants: `pid_t`, `siginfo_t`, `sigset_t`, `stack_t`, `ucontext_t`, `SIG*`,
  `W*`, `EXIT_SUCCESS`, `EXIT_FAILURE`.
- `std` currently takes the `fork` + `exec` fallback because `irix7` is not in its
  `posix_spawn` list.  The target patch adds it; `posix_spawn_file_actions_addchdir` is the
  POSIX-2024 name and `std` already weak-links it.

### 5.2 Threads and synchronisation

- Functions: `pthread_create`, `pthread_join`, `pthread_detach`, `pthread_self`,
  `pthread_equal`, `pthread_attr_*`, `pthread_key_*`, `pthread_once`,
  `pthread_mutex_*` (including `pthread_mutex_clocklock`), `pthread_cond_*` (including
  `pthread_cond_clockwait` and `pthread_condattr_setclock`), `pthread_rwlock_*` (optional),
  `sched_yield`, `nanosleep`.
- Types and constants: `pthread_*_t`, `PTHREAD_*_INITIALIZER`, `PTHREAD_STACK_MIN`.
- `std` uses pthread mutexes and condition variables, the queue implementation for
  `Once`/`RwLock`, and pthread keys for thread-local storage.  `available_parallelism`
  comes from `sysconf(_SC_NPROCESSORS_ONLN)`, which returns the configured count (1 on a
  uniprocessor).
- `pthread_getattr_np` and `pthread_setname_np` are the extensions in §3.4.

### 5.3 Memory

- Functions: `malloc`, `calloc`, `realloc`, `free`, `posix_memalign`, `aligned_alloc`,
  `mmap`, `munmap`, `mprotect`, `msync`, `posix_madvise` (optional), `mlock`/`munlock`
  (optional), `shm_open`/`shm_unlink` (optional), `madvise` and `getpagesize` (extensions).
- Types and constants: `MAP_ANON`, `MAP_ANONYMOUS`, `MAP_PRIVATE`, `MAP_SHARED`,
  `MAP_FIXED`, `MAP_FAILED`, `PROT_*`, `MS_*`, `POSIX_MADV_*`.
- Anonymous mappings come from `MAP_ANON`/`MAP_ANONYMOUS`; no `/dev/zero` workaround.
  `std`'s allocator uses `posix_memalign` for over-aligned allocations.

### 5.4 Files

- Functions: `open`, `openat`, `close`, `posix_close`, `read`, `write`, `pread`, `pwrite`,
  `readv`, `writev`, `preadv`, `pwritev`, `lseek`, `fsync`, `fdatasync`, `ftruncate`,
  `stat`, `fstat`, `lstat`, `fstatat`, `futimens`, `utimensat`, `mkdir`, `mkdirat`,
  `unlink`, `unlinkat`, `rename`, `renameat`, `link`, `linkat`, `symlink`, `symlinkat`,
  `readlink`, `readlinkat`, `chmod`, `fchmod`, `fchmodat`, `chown`, `fchown`, `lchown`,
  `fchownat`, `access`, `faccessat`, `opendir`, `fdopendir`, `dirfd`, `readdir`,
  `readdir_r` (extension), `closedir`, `posix_getdents`, `realpath`, `getcwd`, `chdir`,
  `fchdir`, `statvfs`, `fstatvfs`, `pipe`, `pipe2`, `dup`, `dup2`, `dup3`, `fcntl`,
  `lockf` (XSI), `utimes` (XSI), `chroot` (XSI, capability-checked), `mkstemp`, `mkdtemp`.
- Types and constants: `stat`, `dirent`, `posix_dent`, `statvfs`, `mode_t`, `off_t`,
  `ino_t`, `dev_t`, `nlink_t`, `blkcnt_t`, `blksize_t`, the `O_*`, `F_*`, `AT_*`, `DT_*`,
  `SEEK_*`, `S_*` and `UTIME_*` sets.
- `std::fs::Dir` uses the `openat` form; the native libc must not force the path fallback.
  `futimens`/`utimensat` must support `UTIME_NOW` and `UTIME_OMIT`.
- `File::lock` uses `fcntl` record locks or `lockf`.  `flock` is not provided; a fake
  success is forbidden.
- `/proc/self/exe` is not required; `getexecname` is the extension.

### 5.5 Networking

- Functions: `socket`, `socketpair`, `bind`, `connect`, `listen`, `accept`, `accept4`,
  `shutdown`, `send`, `recv`, `sendto`, `recvfrom`, `sendmsg`/`recvmsg` (optional),
  `setsockopt`, `getsockopt`, `getsockname`, `getpeername`, `poll`, `ppoll`,
  `getaddrinfo`, `freeaddrinfo`, `getnameinfo`, `gai_strerror`, `inet_pton`, `inet_ntop`,
  `if_nametoindex`, `if_indextoname`.
- Types and constants: `sockaddr*`, `sockaddr_storage`, `socklen_t`, `sa_family_t`,
  `msghdr`, `cmsghdr`, `iovec`, `pollfd`, `addrinfo`, `AF_*`, `SOCK_*`, `SOL_SOCKET`,
  `SO_*`, `MSG_*`, `IPPROTO_*`, `IP_*`, `TCP_NODELAY`, `POLL*`, `SHUT_*`.
- IPv4 and IPv6 are both required.  `SO_RCVTIMEO`/`SO_SNDTIMEO` must actually work; the
  IRIX 6.5.22 `ENOPROTOOPT` behaviour is not acceptable in the native world.
- `std` currently uses `socket` + `fcntl(FD_CLOEXEC)` and `accept` + `fcntl` because
  `irix7` is not in the atomic-form lists.  Its pipe code likewise uses `pipe` + `fcntl`
  until `irix7` joins the `pipe2` list.  The target patch adds `accept4`, `pipe2`,
  `SOCK_CLOEXEC`, `SOCK_CLOFORK`, `SOCK_NONBLOCK` and `MSG_NOSIGNAL`.  These are
  POSIX-2024/2008, not extensions.

### 5.6 Time

- Functions: `clock_gettime`, `clock_getres`, `nanosleep`, `clock_nanosleep` (optional),
  `timer_*` (optional), `gettimeofday` (extension).
- Types and constants: `timespec`, `timeval`, `clockid_t`, `CLOCK_REALTIME`,
  `CLOCK_MONOTONIC`.
- `Instant` uses `CLOCK_MONOTONIC`; the kernel already has a monotonic timebase.  The
  condition-variable path uses `pthread_condattr_setclock` and `pthread_cond_timedwait`
  (or `pthread_cond_clockwait`) with the monotonic clock.

### 5.7 Environment

- Functions: `environ`, `getenv`, `setenv`, `unsetenv`, `putenv`, `getuid`, `geteuid`,
  `getgid`, `getegid`, `sysconf`, `confstr`, `pathconf`, `fpathconf`, `uname`,
  `getpwuid_r`, `getcwd`, `chdir`, `getexecname` (extension).
- Types and constants: `passwd`, `utsname`, `_SC_*` (`_SC_PAGESIZE`,
  `_SC_NPROCESSORS_ONLN`, `_SC_HOST_NAME_MAX`, `_SC_GETPW_R_SIZE_MAX`).
- The cell loader supplies `argc`/`argv`/`envp`; the libc exposes `environ`.  `getcwd` and
  `chdir` are per-cell namespace state.
- `std::env::home_dir` uses `getpwuid_r`; it is not a security boundary.

### 5.8 Randomness

- `getentropy` is the v1 backend (POSIX-2024).  `getrandom` is a compatibility extension.
- No `/dev/urandom` requirement.

### 5.9 Terminal and standard I/O

- Functions: `isatty`, `ttyname`, `tcgetattr`, `tcsetattr`, `tcgetwinsize`/`tcsetwinsize`
  (optional), `posix_devctl` `[DC]`, `read`, `write`, `poll`.
- Types and constants: `termios`, `winsize`, `STDIN_FILENO`, `STDOUT_FILENO`,
  `STDERR_FILENO`.
- `std`'s runtime opens `/dev/null` to sanitise closed standard descriptors and ignores
  `SIGPIPE`.  `ioctl(TIOCGWINSZ)` remains an extension for the `terminal_size` crate;
  `tcgetwinsize` is the native call.

### 5.10 Errors and loading

- `__errno_location`, the `E*` set, `strerror`, `strerror_r`, `perror`.
- Loading (`dlopen`/`dlsym`/`dlclose`/`dlerror`/`dladdr`) is post-v1 (§4.8);
  `dl_iterate_phdr`, `RTLD_DEFAULT` and `RTLD_NEXT` are optional extensions.
- Symbolised backtraces are optional for v1.  `std` always links the `unwind` crate, so the
  native runtime still needs `_Unwind_*` (or the `unwinding` crate with MIPS support) even
  with `panic = "abort"` (report §2.4).  That is a runtime requirement, not a libc symbol.

### 5.11 The audit and the target patch

The names above are checked by:

```sh
python3 tools/std-abi-surface.py          # warning mode
python3 tools/std-abi-surface.py --strict # gate, once the profile is accepted
```

The tool walks the module tree, evaluates `cfg` for the `irix7` model, follows
`cfg_select!`, resolves the `libc as netc as c` aliases, and compares the resulting 362
names with the manifest.  On the pinned `rust-src` the current result is:

```text
scanned: 197 files, 731 libc references, 362 distinct names
MISSING: 0
UNCLASSIFIED: 0
UNUSED: 164
```

The unused entries are mostly v1 promises that the unpatched `std` does not reach yet:
`posix_spawn*`, `getentropy`, `pipe2`, `accept4`, `SOCK_CLOEXEC`, `pthread_cond_clockwait`,
`mmap`, the `MAP_*`/`PROT_*` constants and the crate-facing types.  They are the checklist
for the target patch and for M9.5.  The tool is a name-level audit; it does not prove a
signature, layout or behaviour (§7.2).

## 6. Status: v1, optional, unsupported and extension

### 6.1 v1

v1 is the native libc that M9 builds and M9.5 tests.  It is the set of manifest entries with
`v1: true` (526 of 642 at v0.1).  The v1 acceptance criteria are:

- every entry marked `v1` is present at link time, with no `ENOSYS` stubs for claimed
  facilities;
- layout assertions pass for every public struct and scalar;
- a native cell opens and reads a file, spawns and joins a thread, runs a child with
  `posix_spawn`, connects a TCP socket, reads the monotonic clock and prints;
- `std` builds for `os = "irix7"` and is not `restricted_std`;
- the kernel, ROM and module dependency graphs contain no `std`;
- the conformance matrix maps each v1 area to a test, and the profile is published.

### 6.2 Optional and post-v1

Optional entries (`v1: false`, class `posix-2024`, `xsi` or `extension`) are documented but
not promised: dynamic loading, `shm_open`, POSIX semaphores, `posix_madvise`, `mlock`,
POSIX timers, rwlocks/barriers/spinlocks, terminal-size and AF_UNIX ancillary data, and the
64-suffixed aliases.  An optional facility that is not implemented must be absent at link
time or return a documented error; it must not return a fake success.

### 6.3 Unsupported

The `unsupported` class records facilities the project deliberately does not provide:
Linux-only `epoll`/`eventfd`/`timerfd`/`signalfd`/`inotify`/`statx`/`sendfile`/`splice`,
STREAMS (`getmsg`/`putmsg`/`fattach`/`fdetach`) and POSIX async I/O.  They are listed so a
port can see the decision, not because the native kernel is expected to grow them.

### 6.4 Compatibility-only

The `compat-only` class records IRIX 6.5 names that belong to the compatibility container:
`sproc`, `syssgi`, `procntl`, `arenal`, `sysmp`, `CLOCK_SGI_CYCLE` and
`__libc_start_main`.  They are never exported by the native libc.

### 6.5 Extension policy

- An extension is added only with a manifest entry, a written reason and a test.
- It uses a distinct, documented name; it does not overload a POSIX name.
- It never silently changes a POSIX call's semantics, and it never turns a failure into a
  success.
- It is versioned with the libc ABI and removed only through the deprecation policy (§8).
- The extension list is reviewed with the profile; it is not a back door for IRIX 6.5
  compatibility.

## 7. Conformance and layout tests

### 7.1 Test layers

| Layer | What it proves | Where |
|---|---|---|
| Layout | `size_of`, `align_of`, `offset_of!` for every public struct and scalar; C `sizeof`/`offsetof` agrees | `native-abi` crate + host tests |
| Errno | every `E*` name and value, the `EWOULDBLOCK`/`EAGAIN` and `ENOTSUP`/`EOPNOTSUPP` aliases, `strerror` text | host + target |
| ABI surface | every v1 manifest name is present; no unintended symbol | link step + `nm` |
| Capability | rights, revocation, stale-handle `EBADF`, attenuation, `EPERM` without authority | unit + fuzz |
| POSIX subset | one smoke test per area; negative tests for bad descriptors, `EINTR`, short I/O, invalid pointers | C and Rust |
| `std` PAL | `std::fs`, `std::net`, `std::process`, `std::thread`, `std::sync`, `std::time`, `std::env` on a native cell | CI cross-build + iris |
| Runtime | staged binary that prints one line per stage before running it | M9/M9.5 |
| Audit | every `libc::` name `std` uses is in the manifest | `tools/std-abi-surface.py` |
| Kernel hygiene | no `std` in the kernel/ROM/module graphs | CI dependency check |

Layout tests are mandatory, not advisory.  The IRIX 6.5 prior art needed 995 assertions for
two ABIs; the native ABI needs fewer, but it must cover `stat`, `dirent`, `posix_dent`,
`sockaddr_in6`, `sockaddr_storage`, `msghdr`, `iovec`, `timespec`, `timeval`, `sigaction`,
`siginfo_t`, `ucontext_t`, `stack_t`, every `pthread_*` type, and the scalar widths in
§4.1.

The manifest is the source for the surface test: a v1 entry without an implementation is a
link error; an implementation without a manifest entry is a manifest bug.

### 7.2 What the audit does not prove

`tools/std-abi-surface.py` is a **name-level** audit.  It does not compile anything, does
not expand macros other than `cfg_select!`, does not evaluate `cfg!()` in expressions, and
does not understand `cfg` conditions hidden in std's helper macros except the ones listed in
the tool.  It cannot prove a function signature, a struct layout, an `errno` value, a
capability check or a runtime behaviour.  A clean run means only that every name `std`
mentions has a written decision in the manifest.  Treat it as a coverage check, not as an
ABI checker or a conformance suite.

The tool is warning-only in CI while the profile is a draft.  `--strict` is the later gate;
it fails on a referenced name that is missing from v1 or absent from the manifest.

### 7.3 Cross-checks

- The C headers and the Rust `libc` module are generated from the same `native-abi` crate.
- A C program and a Rust program print the same `sizeof`/`offsetof`/`errno` values, and the
  outputs are diffed on the host and on iris.
- The compat container is cross-checked only at the kernel object boundary; it never shares
  the native struct layouts (report §8.4).
- A target-identity assertion fails the build if the native libc is compiled for
  `target_os = "irix"` or the compat libc for `target_os = "irix7"`.

## 8. Versioning policy

- The specification is versioned independently of the libc.  v0.1 is a draft; v1.0 is the
  accepted profile.  A breaking change to the profile is a new major version, not a silent
  edit (the same rule as [`../plan/interfaces.md`](../plan/interfaces.md)).
- The libc ABI version starts at 1 (`__irix7_libc_abi_version()` returns 1).  Additive
  symbols keep the version; a changed layout, a removed symbol or a changed `errno` value
  needs a new major version and, when shared, a new soname.
- Public structs are append-only only if the size is reserved and the new field fits in the
  reserved space with a zero value; otherwise it is a new major version.  Do not grow
  `struct stat` or `struct dirent` in place.
- `errno` values are append-only.  The aliases (`EWOULDBLOCK`/`EAGAIN`,
  `ENOTSUP`/`EOPNOTSUPP`) are fixed.
- A deprecated symbol stays for one major version; removing it is a new major version.
  The manifest records the status.
- A new POSIX edition is a new profile version.  The project tracks Issue 8 now; Issue 9,
  if it arrives, is reviewed as a new profile, not applied silently.
- The compatibility container has its own IRIX 6.5 versioning and is not affected by this
  policy.  The kernel capability ABI is versioned separately by `native-abi`/the cell
  loader.

## 9. Open decisions and recommended defaults

These are the decisions this draft does **not** freeze.  The recommended default is what M9
should implement unless review says otherwise.

| # | Decision | Recommended default | Owner | Blocks |
|---|---|---|---|---|
| 1 | Commit the native target JSON (`os = "irix7"`, family `unix`, n32) | **done**: `targets/mips3-sgi-irix7-native-n32.json`; the kernel target is unchanged | kernel / ABI | PR 2 |
| 2 | Static or shared native libc | static per cell for v1; shared `libc.so.0` later | ABI | v1 |
| 3 | TLS model | pthread keys for v1 (`has-thread-local: false`); ELF TLS later | ABI / loader | v1 |
| 4 | Floating point | soft-float for v1; hardware FP is a separate kernel decision | kernel | v1 |
| 5 | Cell image format and `CellStart` | `#![no_main]` cell with an SDK `entry!`; exact layout in `native-abi` | kernel / ABI | M9 |
| 6 | Capability invocation numbers, trap and error model | define in `native-abi.md`; never expose them to C | kernel | M9 |
| 7 | Numeric `errno` values | Linux/asm-generic values, with the POSIX aliases | ABI | before 1.0 |
| 8 | Exact `struct stat`, `pthread_*`, `sigaction`, `siginfo_t`, `ucontext_t` sizes | pin with layout assertions; recommended sizes in §4.1 | ABI | before 1.0 |
| 9 | Panic model | `abort` for the first cell; `unwind` with the `unwinding` crate in phase 2 | ABI / toolchain | M9.5 |
| 10 | Shared libc and `dlopen` | post-v1; `dladdr` first, `dl_iterate_phdr` optional | ABI | post-v1 |
| 11 | Selected XSI subset approval | as listed in §3.3 | ABI | before 1.0 |
| 12 | Backtrace symbolisation | optional for v1; `dladdr`/`dl_iterate_phdr` later | ABI | post-v1 |
| 13 | `__irix7_libc_abi_version` name and value | keep the name, value 1 | ABI | before 1.0 |
| 14 | Auxv key set | §4.5 recommended set | ABI / loader | M9 |
| 15 | GCC backend for userland `std` | spike `rustc_codegen_gcc` for n32; amend ADR-0018 if needed | toolchain | M9.5 |
| 16 | Upstream the native target and libc module | after v1; tier-3 requires a named maintainer | ABI | post-v1 |
| 17 | Compatibility floor (6.5.7m sysroot vs 6.5.22 prior art) | compat-only; do not mix | compat | compat track |

The IRIX 6.5 compatibility `std` is a separate track and not a dependency of this
specification (report §3.3, §9.4).  It never defines the native ABI.

## 10. References

- [`posix.md`](posix.md) — the adopted POSIX.1-2024 baseline, profile and ABI obligations.
- [`../research/rust-std-irix7.md`](../research/rust-std-irix7.md) — the std research report
  (evidence, target matrix, §5.1/§5.3 surface, §9.2 first-PR plan).
- [`architecture.md`](architecture.md) §2 (D2, D3, D4, D11, D32), §5 (native ABI) and §6
  (compatibility container).
- [`elf.md`](elf.md) — the IRIX ELF loader contract (compat container; not the native loader).
- [`driver-interface.md`](driver-interface.md) — `ctl`/`data`/`status`; `posix_devctl` is the
  POSIX-2024 device-control call.
- [`modules.md`](modules.md) — loadable modules are kernel-mode `no_std`; they do not link
  this libc.
- The Open Group, *POSIX.1-2024 / Base Specifications, Issue 8*:
  <https://pubs.opengroup.org/onlinepubs/9799919799/>.
- The Open Group, *Single UNIX Specification, Version 5 (2024)*.
- Rust Project, *The Rust Standard Library*, the Rustc Dev Guide and the pinned `rust-src`
  for `nightly-2026-10-03`.
- [`native-libc-symbols.json`](native-libc-symbols.json) — the machine-readable manifest.
- [`../../tools/std-abi-surface.py`](../../tools/std-abi-surface.py) — the name-level audit.

## Appendix A: Manifest summary (v0.1)

| Class | Entries | Meaning |
|---|---:|---|
| `posix-2024` | 573 | POSIX.1-2024 base |
| `xsi` | 13 | selected XSI subset |
| `extension` | 32 | documented non-POSIX names |
| `compat-only` | 7 | IRIX 6.5 container only |
| `unsupported` | 17 | deliberately not provided |
| **Total** | **642** | **526 marked `v1`** |

Areas: files 159, network 115, process 109, threads 84, errors 53, memory 39, env 21,
time 21, loading 13, terminal 14, base 8, random 3, startup 3.

The counts are generated from the manifest; if they disagree, the manifest is right.
