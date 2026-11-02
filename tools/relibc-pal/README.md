# relibc IRIX 7 PAL

The native libc is the `irix7-pal` branch of
<https://github.com/irix7/relibc>.  The branch is the source of truth for the
PAL; this directory holds the pinned commit, the build script for the C probe
and a review mirror of the change.

| Item | Value |
|---|---|
| Branch | `irix7-pal` |
| Commit | `8a7e044f584945dbf5fdbad11fd76e16daa420b9` |
| Series base | `a36041ac5201ede0c3853d548de5aa7fbfb0c708` |
| Mirror | [`patches/`](patches/) (`0001` functional, `0002` formatting, `0003` file calls, `0004` poll and `O_NONBLOCK`, `0005` writable ramfs, `0006` futex, `0007` pthreads, `0008` sleep, `0009` fork/exec/wait) |
| Probe sources | [`user/relibc-probe/probe.c`](../../user/relibc-probe/probe.c), [`user/relibc-thread-probe/probe.c`](../../user/relibc-thread-probe/probe.c), [`user/relibc-futex-probe/probe.c`](../../user/relibc-futex-probe/probe.c), [`user/relibc-pthread-probe/probe.c`](../../user/relibc-pthread-probe/probe.c), [`user/relibc-sleep-probe/probe.c`](../../user/relibc-sleep-probe/probe.c), [`user/relibc-forkexec-probe/probe.c`](../../user/relibc-forkexec-probe/probe.c) and [`user/relibc-forkexec-child/probe.c`](../../user/relibc-forkexec-child/probe.c) |

The mirror is generated with `git format-patch a36041ac..8a7e044f` and applies
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
11. Sleep.  `nanosleep` maps to a real `SYS_NANOSLEEP` park: the kernel sleeps
    the calling thread on a CP0 Count deadline and a cooperative peer runs
    meanwhile.  One park lasts at most 60 s so the deadline stays inside half
    the 32-bit Count range; the PAL re-arms with the remainder for a longer
    interval and leaves `rmtp` untouched because there are no signals.
    `clock_nanosleep` is implemented over the same primitive for
    `CLOCK_REALTIME` and `CLOCK_MONOTONIC`, converting `TIMER_ABSTIME` to a
    relative interval; it returns the error number directly as POSIX requires,
    and `thrd_sleep` adapts that to C11's `0`/`-1`/other-negative contract.

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
`libcrt0.a`, compiles the C probe, the thread probe, the futex probe, the
pthread probe, the sleep probe and the two fork/exec images, strips each ELF
and pads 256 KiB `probe.raw`, `thread-probe.raw`, `futex-probe.raw`,
`pthread-probe.raw`, `sleep-probe.raw`, `forkexec-probe.raw` and
`forkexec-child.raw` images.  It records the relibc commit in
`BUILD-PROVENANCE.txt`.  An existing checkout needs its submodules initialised
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

The fork/exec probes attach the parent at SCSI target 2 and the child at target
3:

```toml
[scsi.2]
path = "/tmp/opencode/relibc-pal-work/out/forkexec-probe.raw"
cdrom = false

[scsi.3]
path = "/tmp/opencode/relibc-pal-work/out/forkexec-child.raw"
cdrom = false
```

The parent expects:

```text
fork: pid=1 child=2 actor=...
exec: entry=0x10000000
CHILD-OK
process: exit pid=2 status=0x00000007
FORK-EXEC-OK
user: exit syscall code=0x00000000
kernel: userland returned to kernel console
```

The child inherits fd 3 (a ramfs file) and verifies that fd 4, marked
`FD_CLOEXEC` by the parent, is closed after `execve`.  The parent checks that
`waitpid(pid, ..., WNOHANG)` reports 0 before the child runs, that the blocking
wait returns the child pid with status 7, and that the inherited open file
description still shares its offset.

The sleep probe (`sleep-probe.raw`) expects:

```text
SLEEP-PROBE: start
sleep: wait tid=1 limit=20ms
SLEEP-PROBE: solo-elapsed=20ms
thread: create by=1 tid=2 epc=0x10000040 sp=0x2001FFE0 tcb=0x20020000
sleep: wait tid=2 limit=20ms
sleep: park tid=2 limit=19ms
sleep: expiry tid=2 limit=19ms elapsed=20ms
SLEEP-PROBE: counter=1097 elapsed=42ms worker-elapsed=43ms
SLEEP-PROBE: join-elapsed=19ms
SLEEP-PROBE: checks=48 failures=0
SLEEP-OK
user: exit syscall code=0x00000000
kernel: userland returned to kernel console
```

Timer and scheduling jitter on the emulator host changes the millisecond
values; the checks are bounds, not fixed numbers.

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

- Threads are cooperative: no preemption.  A thread runs until it yields,
  blocks on a futex or a sleep, retires or the image exits.  A pthread
  program must yield or block for its peers to make progress.  A sleeper is
  resumed at the next scheduling point after its timer deadline, so a peer
  that never yields also never sees the switch.
- `poll`, a blocking read or write and a futex wait are bounded by the
  kernel's 100 ms poll limit; the libc re-parks a futex wait whose bounded
  expiry precedes its deadline.
- `nanosleep` parks the calling thread on a CP0 Count deadline, so a
  cooperative peer runs while one thread sleeps.  One park lasts at most 60 s
  and the libc re-arms with the remainder.  With no runnable peer the kernel
  arms Compare for the deadline and executes `wait`, so a solo sleep idles the
  CPU instead of spinning.
- `pthread_cancel` cannot deliver a signal, so cancellation with the target
  enabled returns `ENOSYS`.  `pthread_key_delete` removes the key from the
  calling thread's map only.  A detached thread's TCB and stack are not
  reclaimed because the kernel's `munmap` is a bump-allocator no-op.
- No signals or TLS block.
- `fork`, `execve` and `waitpid` are the local cell fast path.  A process is
  one cell with a Unix personality; its `pid` is local to the replica and the
  POSIX surface does not expose the global `Job`/actor handle.  Remote
  spawn/wait, copy-on-write, job control, more than eight local cells and the
  socket API are not implemented.
- `getentropy` succeeds but the kernel zero-fills the buffer.
- `mmap` is the kernel's bump allocator; `munmap`/`mprotect` are metadata-only.
- The kernel's mutable ramfs is the only filesystem: 32 inodes, 4096 bytes
  per file and 63-byte names.  There are no links, symbolic links,
  permissions, ownership or timestamps beyond the fixture metadata.
