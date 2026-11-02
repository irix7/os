# relibc IRIX 7 PAL

The native libc is the `irix7-pal` branch of
<https://github.com/irix7/relibc>.  The branch is the source of truth for the
PAL; this directory holds the pinned commit, the build script for the C probe
and a review mirror of the change.

| Item | Value |
|---|---|
| Branch | `irix7-pal` |
| Commit | `acd6c4966c249510154158e05c19b8f846a70bb5` |
| Series base | `a36041ac5201ede0c3853d548de5aa7fbfb0c708` |
| Mirror | [`patches/`](patches/) (`0001` functional, `0002` formatting, `0003` file calls, `0004` poll and `O_NONBLOCK`) |
| Probe source | [`user/relibc-probe/probe.c`](../../user/relibc-probe/probe.c) |

The mirror is generated with `git format-patch a36041ac..acd6c496` and applies
on top of `a36041ac`.  It exists for review; the build script pins and verifies
the branch commit, not the patch.

## What the branch changes

1. `syscall 0x1F7` replaces the flat provisional syscall.  The kernel returns
   `CELL_E_*` in `$2` and the value in `$3`; the PAL maps the status through
   `STATUS_ERRNO` to `Err(Errno(...))`.
2. relibc's 136-byte n32 `struct stat` is passed directly to `fstat`/`stat`.
   `KernelStat` and `copy_stat` are deleted.
3. `_start` builds the 16-byte main TCB (`self` at 0, `dtv` at 4, `errno` at 8,
   libc-private at 12) and sets `$28`.  `errno` and `__errno_location` are
   `TP + 8`.  MIPS III has no `rdhwr`, and the target has
   `has-thread-local: false`, so every `#[thread_local]` in the library becomes a
   plain static on this target.
4. `$a3` carries the auxiliary vector.  `_start` stores it, `Stack::auxv`
   returns it, and `getauxval` uses the native vector numbering.
5. `lseek`, `pread`, `pwrite`, `dup2`, `pipe2` and `getdents` map to the
   native calls.  `getdents` fills whole `dirent` records and advances the
   kernel's cookie; `d_off` is the next cookie and `dir_seek` writes it back
   through `lseek`, so `telldir`/`seekdir` work.  The one-entry `readdir`
   synthesis is gone.
6. `fcntl F_GETFD`/`F_SETFD` carry real close-on-exec state, `O_CLOEXEC` is
   recorded on `open`, and `dup`/`dup2` clear it.
7. `poll` passes its millisecond timeout to the kernel, which evaluates real
   pipe and console readiness and bounds a negative timeout.
   `fcntl F_GETFL`/`F_SETFL` carry the access mode and `O_NONBLOCK` per open
   file description; `dup` shares them, and a non-blocking read of an empty
   pipe or an idle console is `EAGAIN`.

`futex_wait` and `futex_wake` are no-ops: the kernel has no futex and there is
one thread, so returning `ENOSYS` makes every mutex unlock panic.  `printf`
uses the generic path and now completes.

The target is `targets/mips3-sgi-irix7-native-n32.json`: n32, big-endian,
MIPS III, soft float, `-mno-abicalls -G0`.  The `std` shim remains the abicalls
exception.

## Build

```sh
# Pinned C toolchain: Bootlin mips64-n32--uclibc--stable-2026.08-1
# (GCC 15.3.0, binutils 2.45.1).  Pinned Rust: nightly-2026-05-24
# with rust-src, as in relibc's rust-toolchain.toml.
export MIPS_CC=/path/to/mips64-buildroot-linux-uclibc-gcc
export MIPS_AR=/path/to/mips64-buildroot-linux-uclibc-ar
export RELIBC_SRC=/tmp/opencode/relibc-pal-src

tools/relibc-pal/build.sh /tmp/opencode/relibc-pal-work/out
```

The script clones `irix7-pal` if `RELIBC_SRC` is absent, refuses to build a
different commit unless `RELIBC_COMMIT` says so, builds `librelibc.a` and
`libcrt0.a`, compiles the C probe, strips the ELF and pads a 256 KiB
`probe.raw`.  It records the relibc commit in `BUILD-PROVENANCE.txt`.  An
existing checkout needs its submodules initialised
(`git submodule update --init --recursive`).

## Run

Attach `probe.raw` as SCSI target 2 and boot the merged kernel:

```toml
banks = [64, 0, 0, 0]
no_audio = true

[machine]
profile = "indy_ip24"

[scsi]

[scsi.2]
path = "/tmp/opencode/relibc-pal-work/out/probe.raw"
cdrom = false
```

```sh
IRIS_NO_IDLE=1 iris --config probe.toml --ci --serial-log probe-serial.log \
  --monitor-port 18893 --ci-socket /tmp/opencode/relibc-pal-work/probe.sock \
  --prom rig/irix7-rom.bin --nvram probe-nvram.bin
iris-ci --socket /tmp/opencode/relibc-pal-work/probe.sock status --wait-ready 30
iris-ci --socket /tmp/opencode/relibc-pal-work/probe.sock start
```

The expected tail is:

```text
IRIX7-RELIB-PROBE-3
IRIX7-RELIB-PROBE-3: stat size=28 mode=100644 blksize=4096 blocks=1
IRIX7-RELIB-PROBE-3: tp=0x1002c7e0 auxv pagesz=4096
IRIX7-RELIB-PROBE-3: checks=142 failures=0
user: exit syscall code=0x00000000
kernel: userland returned to kernel console
```

The probe keeps the startup checks (marker, `argc`/`argv`, `getpid`,
`AT_PAGESZ`/`AT_ENTRY`/`AT_CLKTCK`, `$28`, `__errno_location`, `stat`,
`fstat(999)` with `EBADF`, `stat("/missing")` with `ENOENT`) and the file
surface: `lseek` in all mode, `pread`/`pwrite`, shared offsets across
`dup`/`dup2`, `F_GETFD`/`F_SETFD`, `pipe`/`pipe2` with `EAGAIN` and `EPIPE`,
and a directory read in whole records with `d_off` cookies, then resumed with
`lseek` and walked through `readdir`/`seekdir`/`telldir`.  It adds descriptor
readiness: `F_GETFL`/`F_SETFL` with `O_NONBLOCK` shared across `dup`, `poll`
with a zero and a negative timeout, `POLLIN`, `POLLOUT`, `POLLHUP`, `POLLERR`
and `POLLNVAL`, and a console that is always `POLLOUT`.

## Limits

- Single-threaded: no threads, TLS block, signals or process table.
- There is no scheduler wakeup.  `poll` and a blocking read or write recheck
  readiness for at most 100 ms and then return `EAGAIN`; a negative `poll`
  timeout is clamped to that bound.
- `fork`, `execve`, `waitpid` and the socket API remain `ENOSYS`.
- `getentropy` succeeds but the kernel zero-fills the buffer.
- `mmap` is the kernel's bump allocator; `munmap`/`mprotect` are metadata-only.
- The kernel's console/ramfs fd table is the only filesystem.
