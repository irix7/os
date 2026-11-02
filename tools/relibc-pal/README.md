# relibc IRIX 7 PAL

The native libc is the `irix7-pal` branch of
<https://github.com/irix7/relibc>.  The branch is the source of truth for the
PAL; this directory holds the pinned commit, the build script for the C probe
and a review mirror of the change.

| Item | Value |
|---|---|
| Branch | `irix7-pal` |
| Commit | `729bdc3bbbc9c4cb1fc2ca1ffeac92ea7d551383` |
| Series base | `a36041ac5201ede0c3853d548de5aa7fbfb0c708` |
| Mirror | [`patches/`](patches/) (`0001` functional, `0002` formatting, `0003` file calls, `0004` poll and `O_NONBLOCK`, `0005` writable ramfs, `0006` futex, `0007` pthreads) |
| Probe sources | [`user/relibc-probe/probe.c`](../../user/relibc-probe/probe.c), [`user/relibc-thread-probe/probe.c`](../../user/relibc-thread-probe/probe.c), [`user/relibc-futex-probe/probe.c`](../../user/relibc-futex-probe/probe.c), [`user/relibc-pthread-probe/probe.c`](../../user/relibc-pthread-probe/probe.c) |

The mirror is generated with `git format-patch a36041ac..729bdc3b` and applies
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
8. The writable ramfs: `openat` passes the creation mode and applies the
   process umask, `unlinkat` maps to `unlink` or `rmdir` (`AT_REMOVEDIR`),
   `mkdirat` maps to `mkdir`, `renameat2` maps to `rename`, and `ftruncate`
   splits the 64-bit length across two argument registers.  `umask` keeps
   process state.
9. `futex_wait`/`futex_wake` map to `SYS_FUTEX_WAIT`/`SYS_FUTEX_WAKE`.
   `futex_wait` takes relibc's absolute `CLOCK_MONOTONIC` deadline, converts it
   to the native relative millisecond timeout, returns `EAGAIN` for a changed
   word and `ETIMEDOUT` at a real deadline, and re-parks when the kernel's
   100 ms poll bound expires early or when the caller waits indefinitely.
   `futex_wake` returns the kernel's woken count.  Before this, both calls were
   no-ops, so a mutex unlock had no waiter to tell and a wait reported success
   without blocking.
10. pthreads.  `$28` still points at the 16-byte ABI header; relibc's `Tcb`
    and `Pthread` records are appended after it, both for the main thread and
    for mmap-allocated workers.  `pthread_self` and `__errno_location` are
    therefore per-thread.  `pthread_create` keeps relibc's existing stack
    frame and maps `SYS_THREAD_CREATE` through `rlct_clone`, recording the
    returned kernel tid before the creator can yield.  `pthread_join` waits on
    the thread's libc-owned exit word with `futex_wait`; the worker posts its
    start routine's return value and retires with `SYS_THREAD_EXIT`.
    `sched_yield` maps to `SYS_YIELD`, `exit_thread` to `SYS_THREAD_EXIT`, and
    `gettid` reads the current thread's tid.  The mutex owner word is
    therefore per-thread and the existing fast path and futex contention path
    work.  Key maps, cleanup lists, thread-exit handlers and the C++ thread
    destructor list live in the Pthread record.  The default stack is 64 KiB
    because the kernel's anonymous heap is a fixed 1 MiB shared with the
    allocator.

`printf` uses the generic path and completes.

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
`libcrt0.a`, compiles the C probe, the thread probe, the futex probe and the
pthread probe, strips each ELF and pads 256 KiB `probe.raw`,
`thread-probe.raw`, `futex-probe.raw` and `pthread-probe.raw` images.  It
records the relibc commit in `BUILD-PROVENANCE.txt`.  An existing checkout
needs its submodules initialised (`git submodule update --init --recursive`).

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
IRIX7-RELIB-PROBE-4
IRIX7-RELIB-PROBE-4: stat size=28 mode=100644 blksize=4096 blocks=1
IRIX7-RELIB-PROBE-4: tp=0x1002f3b0 auxv pagesz=4096
IRIX7-RELIB-PROBE-4: checks=226 failures=0
user: exit syscall code=0x00000000
kernel: userland returned to kernel console
```

The pthread probe (`pthread-probe.raw`) expects:

```text
PTHREAD-PROBE: start
PTHREAD-PROBE: counter=64 expected=64
PTHREAD-PROBE: main-self=0x10010E50 worker-self=0x20031060
PTHREADS-OK
PTHREAD-PROBE: checks=29 failures=0
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
and `POLLNVAL`, and a console that is always `POLLOUT`.  It adds the writable
ramfs: `O_CREAT`/`O_TRUNC`/`O_EXCL`/`O_APPEND`, write and read back,
`ftruncate` and `truncate`, `rename`, `unlink`, `mkdir`/`rmdir` with
`ENOTEMPTY`, a live `getdents` walk, `chdir` into a created directory, an
unlinked inode still readable through its descriptor, and `EEXIST`,
`ENOTDIR`, `EISDIR`, `ENOENT` and `ENOSPC`.

## Limits

- Threads are cooperative: no preemption and no timer wakeup.  A thread runs
  until it yields, blocks on a futex, retires or the image exits.  A pthread
  program must yield or block for its peers to make progress.
- `poll`, a blocking read or write and a futex wait are bounded by the
  kernel's 100 ms poll limit; the libc re-parks a futex wait whose bounded
  expiry precedes its deadline.
- `pthread_cancel` cannot deliver a signal, so cancellation with the target
  enabled returns `ENOSYS`.  `pthread_key_delete` removes the key from the
  calling thread's map only.  A detached thread's TCB and stack are not
  reclaimed because the kernel's `munmap` is a bump-allocator no-op.
- No signals, TLS block or process table.
- `fork`, `execve`, `waitpid` and the socket API remain `ENOSYS`.
- `getentropy` succeeds but the kernel zero-fills the buffer.
- `mmap` is the kernel's bump allocator; `munmap`/`mprotect` are metadata-only.
- The kernel's mutable ramfs is the only filesystem: 32 inodes, 4096 bytes
  per file and 63-byte names.  There are no links, symbolic links,
  permissions, ownership or timestamps beyond the fixture metadata.
