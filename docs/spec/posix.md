# Native POSIX API baseline

**Status:** proposed
**Version:** 0.1
**Owner:** ABI / libc
**Related:** [`architecture.md`](architecture.md) §5, [`elf.md`](elf.md), [`modules.md`](modules.md), [`../plan/milestones.md`](../plan/milestones.md) M5/M9

## 1. Decision

The **native** IRIX 7 libc targets **POSIX.1-2024** (IEEE Std 1003.1-2024, the core of
The Open Group Base Specifications Issue 8 and Single UNIX Specification, Version 5
(2024)), plus a documented subset of XSI interfaces.  POSIX.1-2024 replaces the ad-hoc
"Unix-shaped" list in [`architecture.md`](architecture.md) §5.5 as the normative API
baseline for new software.

This does **not** change the legacy IRIX compatibility container.  IRIX 6.5 o32 and n32
binaries keep the IRIX 6.5 ABI, syscall numbers, libc layouts and `sproc` semantics.  They
are a separate target and a separate source contract.  POSIX.1-2024 is not retrofitted
onto the compatibility container.

POSIX is a **source-level API and environment specification**, not a binary ABI.  It does
not define struct offsets, symbol versioning, TLS layout, syscall numbers or the startup
sequence.  IRIX 7 still has to own those n32 definitions, test them and version them.

## 2. Scope

In scope for the native API profile:

- Base Definitions and System Interfaces;
- the POSIX shell and utilities, when the native userland exists;
- the POSIX threads, realtime, clock, signal, process, filesystem, socket and terminal
  interfaces needed by Rust and by native applications;
- selected XSI interfaces that are useful on an Indy, each recorded in the profile;
- the capability mapping underneath each operation (D4).

Out of scope for POSIX:

- graphics, audio, window systems, database and record I/O;
- a promise that every optional POSIX option is implemented in v1;
- binary compatibility with another POSIX system;
- POSIX certification in the first release.  The project will document the profile and
  test it, but certification is a separate commercial and legal decision.

## 3. Why move up

- **Rust `std` needs a modern, documented Unix surface.**  The native std target can reuse
  the Unix PAL and a libc instead of inventing a parallel language runtime.
- **New software should not inherit 1990s API accidents.**  The compatibility container
  already preserves those; the native world can use `openat`, `fstatat`,
  `clock_gettime`, `posix_spawn`, modern thread and synchronisation interfaces, and the
  2024 error and type rules.
- **A dated native API would be a permanent tax.**  Every future libc addition would be
  an exception; POSIX.1-2024 gives a stable, externally reviewed baseline.
- **It separates the two worlds cleanly.**  "IRIX 6.5 compatible" and "POSIX.1-2024
  native" are different contracts, with different targets, tests and release promises.

## 4. Two targets, not one

| Property | Native | IRIX 6.5 compatibility |
|---|---|---|
| API source | POSIX.1-2024 + selected XSI | IRIX 6.5 libc and syscall ABI |
| Binary ABI | new n32 IRIX 7 ABI | IRIX 6.5 o32/n32 exactly |
| Process personality | native cell with Unix personality (D3) | legacy MMU process |
| Rust target | hosted target, `std` planned | compat target, separate sysroot |
| Goal | source portability | binary compatibility (ADR-0008) |
| Certification | profile, not certified | not applicable |

The **kernel and ROM** remain `no_std` and do not link this libc.  The hosted native
target is `targets/mips3-sgi-irix7-native-n32.json` (`target_os = "irix7"`, Unix
family); the kernel target stays `targets/mips3-sgi-irix7-n32.json` (`target_os =
"none"`), and the compatibility target stays separate.

The draft binary contract that pins those n32 choices — symbols, layouts, `errno`, TLS,
startup, capability entry and the extension list — is [`native-libc.md`](native-libc.md).

## 5. API profile

The profile is versioned and machine-readable.  Each entry records:

- the POSIX.1-2024 name and section;
- the native symbol and ABI version;
- whether it is required, optional or not implemented;
- the capability operation behind it;
- the test that proves it.

The initial required set is driven by the first applications and by Rust `std`:

| Area | Examples of the 2024 interfaces to audit |
|---|---|
| Process | `fork`, `execve`, `posix_spawn`, `waitpid`, `_exit`, process groups |
| Threads | `pthread_create`, `pthread_join`, mutexes, condition variables, rwlocks, TLS |
| Memory | `mmap`, `munmap`, `mprotect`, `brk` (compat only if required) |
| Files | `openat`, `read`, `write`, `pread`, `pwrite`, `fstatat`, `getdents`, `renameat`, `unlinkat` |
| Metadata | `stat`, `fstatat`, timestamps, modes, ownership, `realpath`, `readlinkat` |
| I/O readiness | `poll`, `select`, `ppoll`, non-blocking descriptors |
| Signals | `sigaction`, `sigprocmask`, `sigwait`, `kill`, signal-safe I/O |
| Time | `clock_gettime`, `clock_nanosleep`, `nanosleep`, timers |
| Network | `socket`, `bind`, `connect`, `listen`, `accept`, `getaddrinfo`, `send`/`recv` |
| Environment | `environ`, `getenv`, `setenv`, locale, `sysconf`, `pathconf` |
| Startup | `_start`, `__libc_start_main`, arguments, environment, auxv, TLS setup |
| Terminal | `isatty`, `tcgetattr`, `tcsetattr`, standard streams |
| Random / security | `getrandom` equivalent, `getentropy` if adopted, `fork` safety |

This table is a starting audit, not a claim that all 2024 facilities exist or that every
optional interface is required.  The profile is the authority.

## 6. Native ABI contract

The native ABI is n32, MIPS III, big-endian, `ILP32` with 64-bit registers.  POSIX does
not define the following; IRIX 7 must:

1. **Symbols and versioning.**  A stable symbol set with an ABI version and a policy for
   adding symbols without silent layout changes.
2. **Types and struct layouts.**  `struct stat`, `struct timespec`, `struct sigaction`,
   `struct sockaddr`, `fd_set`, `DIR`, `FILE` and every public C layout.  Compile-time
   assertions are mandatory, as the prior IRIX std work demonstrates.
3. **errno.**  A complete mapping from native errors to POSIX names and values, defined
   once in a shared header/crate.
4. **TLS.**  The native thread pointer convention, static TLS block, dynamic TLS and the
   initial-exec/local-exec model.
5. **Startup.**  `_start`, stack alignment, argv/envp/auxv layout, constructors and the
   `__libc_start_main` handoff.
6. **Syscall entry.**  The native trap instruction, register convention, restart and
   cancellation semantics.  This ABI is not the IRIX 6.5 syscall ABI.
7. **Capability mapping.**  `open`, `mmap`, `socket`, `ioctl` and device operations map
   to capabilities, not to ambient root.  A POSIX name is an API, never an authority
   grant (D4, D11).
8. **Thread cancellation and fork.**  The interaction between `fork`, threads, locks,
   `pthread_atfork` and async-signal safety must be specified before it is needed.

## 7. Rust `std`

The native target is intended to support the real Rust `std`, not a lookalike crate:

- build a hosted target with `target_family = "unix"` so the Unix PAL is selected;
- provide a libc implementation for that target (a `libc` port or a compatible native
  libc crate);
- build `std` with `-Z build-std=std` from a pinned/patchable `rust-src`, or upstream the
  target when the ABI is stable;
- keep the kernel, ROM and loadable modules on `core`/`alloc`; they must not link std;
- test `std::thread`, `std::sync`, `std::fs`, `std::net`, `std::process`, `std::env`,
  `std::time`, TLS, panic and backtrace behaviour against the native ABI.

This is a downstream consumer of the native POSIX profile, not a substitute for it.  A
`std`-compatible shim over kernel/module services is a separate product and must not be
called "Rust std".

## 8. Conformance and tests

- **Layout tests** for every public C type and constant, generated from one ABI source.
- **Smoke tests** in C and Rust for each required API area.
- **Negative tests** for invalid pointers, bad descriptors, `EINTR`, short I/O, signal
  interruption and fork/thread interactions.
- **Cross-check** the native profile against the IRIX 6.5 container only at the boundary
  (the kernel objects), never by sharing C ABI layouts.
- **Certification** is not claimed.  If it is pursued later, use The Open Group
  certification test suite and record the exact profile tested.

## 9. Sequencing

1. **Profile and ABI records (can start now).**  Freeze the POSIX.1-2024 profile and the
   C layout decisions; add compile-time assertions.  No kernel userland is required.
2. **Native process and syscall entry (M5/M9 dependency).**  A cell with a Unix
   personality, user MMU context, TLS and a trap that can implement `write`, `exit`,
   memory and basic process calls.
3. **Native libc.**  Implement the required POSIX surface over capabilities; start with
   the subset needed by a hello-world Rust binary.
4. **Rust std target.**  Build the hosted target and `std`, then grow the test corpus.
5. **Shell and utilities.**  POSIX shell/utilities follow the native userland; they are
   not a prerequisite for std.
6. **Optional XSI and certification.**  Only after the base profile is stable.

## 10. Open questions

- Exact hosted target name and `target_os` value; how to keep the compat target distinct.
- Whether to use the upstream `unix` PAL with a native libc or a dedicated PAL; which
  Linux/glibc assumptions must be removed.
- The native TLS and unwinding model on MIPS III.
- Which XSI interfaces are in the first profile.
- Whether POSIX shell/utilities are part of the first native release or a later layer.
- The certification and trademark position.

## 11. Sources

- The Open Group, *POSIX.1-2024 / Base Specifications, Issue 8*:
  <https://pubs.opengroup.org/onlinepubs/9799919799/> (the standard explicitly excludes
  graphics, databases, record I/O, object/binary portability and resource availability).
- The Open Group, *Single UNIX Specification, Version 5 (2024)*.
- Rust Project, *The Rust Standard Library* and the Rustc Dev Guide porting/PAL
  documentation; the installed `rust-src` for the pinned nightly.
