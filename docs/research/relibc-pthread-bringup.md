# pthreads on the cooperative thread gate

Status: **bring-up result**. 2026-11-02.  Tracks
[irix7/os#318](https://github.com/irix7/os/issues/318).

## Result

A C program linked against `librelibc.a` from `irix7-pal` creates a worker,
shares a `pthread_mutex_t` with it, stores a pthread-specific value, joins and
receives the worker's return value, then creates and joins a second worker.
It prints `PTHREADS-OK` and exits 0 on iris 0.1.13:

```text
kernel: starting userland from SCSI target 2
userelf: entry=0x10000000
user: entering user mode
PTHREAD-PROBE: start
thread: create by=1 tid=2 epc=0x10000040 sp=0x2001FFE0 tcb=0x20020000
thread: exit tid=2 status=0x00000000
thread: create by=1 tid=2 epc=0x10000040 sp=0x20030FE0 tcb=0x20031000
thread: exit tid=2 status=0x00000000
PTHREAD-PROBE: counter=64 expected=64
PTHREAD-PROBE: main-self=268524400 worker-self=537071664
PTHREADS-OK
PTHREAD-PROBE: checks=29 failures=0
user: exit syscall code=0x00000000
kernel: userland returned to kernel console
```

The ROM is 490,980 bytes with SHA-256
`df6d1f8b7b3393d3bced0b375c755aefae78891732de4eb0bc40839ba98d3184`.  The
kernel is unchanged from `ada0dd2`; the new work is entirely in the PAL and
the libc-side pthread layer.

| Image | ELF bytes | Raw SHA-256 |
|---|---|---|
| `pthread-probe.raw` | 154,760 | `bdf16ffbcbefe1f87d0b0da80f8617f0cf3dd9b2708e88f5684dd94d530c1723` |
| `thread-probe.raw` | 125,264 | `006be187c310c8c50adc2486387ac71f742fa61e4c0836d9d333179c69a5356e` |
| `futex-probe.raw` | 126,756 | `39b16f7b7eaad96300fae6f42830161b10cc5c64c94fab5c1da305a1a85c76ee` |
| `probe.raw` | 244,648 | `71aad1175028a214ae314c6f8ca0bbe2f49d9497bb6b610dc9e0f2772ddda525` |
| `std-hello.raw` | unchanged | `983a28e371ff5e2f9d9e5d968b1457c482ebb0eb91429dba8fd62ec93da15808` |

## Fork

| Item | Value |
|---|---|
| Branch | `irix7-pal` in <https://github.com/irix7/relibc> |
| Commit | `729bdc3bbbc9c4cb1fc2ca1ffeac92ea7d551383` |
| Parent | `5c78551d891fe199d9933638152f194c50c9c4f2` |
| DCO | `Signed-off-by: Jupiter Belic <belikh@users.noreply.github.com>` |
| Mirror | [`tools/relibc-pal/patches/`](../../tools/relibc-pal/patches/) |

The branch is the source of truth.  This repository pins the commit in
[`tools/relibc-pal/build.sh`](../../tools/relibc-pal/build.sh) and the
[`README`](../../tools/relibc-pal/README.md); the patches are a review mirror.

## What the PAL changes

1. **TCB layout.** `$28` still points at the 16-byte ABI header.  relibc's
   `Tcb` and `Pthread` are appended at `$28 + 16`, both for the main thread in
   crt0's BSS and for workers allocated with `Tcb::new`.  `pthread::init`
   constructs the main record; `GenericTcb::arch_read` reads `$28 + 16 + offset`.
2. **Thread creation.** `pthread_create` keeps relibc's existing
   `[shim, entry, arg, tcb, mutex]` stack frame.  `rlct_clone` calls
   `SYS_THREAD_CREATE` with a small assembly entry that loads that frame and
   calls `new_thread_shim`.  The returned kernel tid is written into the new
   `Pthread` before the creator yields, so `gettid` is correct when the worker
   first runs.
3. **Join.** `pthread_join` waits on the `Pthread`'s libc-owned exit word with
   `futex_wait`.  The worker posts its start routine's return value and retires
   with `SYS_THREAD_EXIT`; the joiner wakes, receives the value and releases
   the joined worker's TCB and stack.  The kernel's slot exit status is not
   used and no kernel join call is needed.
4. **Thread identity.** `sched_yield` maps to `SYS_YIELD`, `exit_thread` to
   `SYS_THREAD_EXIT`, and `gettid` reads the current thread's tid.
   `pthread_self` and `__errno_location` read `$28`, so `errno` is per-thread.
5. **Mutex.** `RlctMutex`'s existing fast path and futex contention path now
   work because the owner word is the per-thread tid.  The IRIX 7 cached-tid
   `SingleThread<Cell<u32>>` shim is gone.
6. **Per-thread libc state.** The pthread key map, cleanup handler list,
   `__cxa_thread_atexit` list and C++ thread destructor list move from
   process-wide statics into the `Pthread` record on this target.
7. **Stack default.** The default stack is 64 KiB because the kernel's
   anonymous heap is a fixed 1 MiB shared with relibc's allocator; the POSIX
   minimum is still 64 KiB.

## Build

```sh
export RUSTUP_HOME=/tmp/opencode/relibc-n32/rustup/rustup
export CARGO_HOME=/tmp/opencode/relibc-n32/rustup/cargo
export PATH=/nix/store/j1938gsbvbhkyz4kbmvjk56r479mkw3s-gcc-wrapper-16.2.0/bin:$PATH
export RELIBC_SRC=/tmp/opencode/relibc-pal-src

tools/m2-rig/build.sh /tmp/opencode/pthreads-work/rig
tools/relibc-pal/build.sh /tmp/opencode/pthreads-work/out
```

The relibc build pins `729bdc3b` and refuses a different checkout.  The C
compiler is Bootlin `mips64-n32--uclibc--stable-2026.08-1` (GCC 15.3.0,
binutils 2.45.1); relibc uses its pinned `nightly-2026-05-24`.

## Run

Attach the image at SCSI target 2 and boot the merged kernel:

```toml
banks = [64, 0, 0, 0]
no_audio = true

[machine]
profile = "indy_ip24"

[scsi]

[scsi.2]
path = "/tmp/opencode/pthreads-work/out/pthread-probe.raw"
cdrom = false
```

```sh
LD_LIBRARY_PATH=/nix/store/gscx8xwsib0prqif61k2bd0j4hq515rd-libglvnd-1.7.0/lib:/nix/store/yclidnx4racav6sbgy292qg5m6xwr7br-alsa-lib-1.2.16.1/lib \
IRIS_NO_IDLE=1 /tmp/opencode/iris-0.1.13/iris --config pthread.toml --ci \
  --serial-log pthread-serial.log --monitor-port 18901 \
  --ci-socket /tmp/opencode/pthreads-work/pthread.sock \
  --prom /tmp/opencode/pthreads-work/rig/irix7-rom.bin \
  --nvram /tmp/opencode/pthreads-work/pthread-nvram.bin
/tmp/opencode/iris-0.1.13/iris-ci --socket /tmp/opencode/pthreads-work/pthread.sock \
  status --wait-ready 30
/tmp/opencode/iris-0.1.13/iris-ci --socket /tmp/opencode/pthreads-work/pthread.sock start
```

The pthread probe's serial tail is above.  The other images run unchanged:

```text
THREAD-PROBE: checks=20 failures=0
THREADS-OK
user: exit syscall code=0x00000000
```

```text
FUTEX-PROBE: checks=32 failures=0
FUTEX-OK
user: exit syscall code=0x00000000
```

```text
IRIX7-RELIB-PROBE-4: checks=226 failures=0
user: exit syscall code=0x00000000
```

```text
hello from std
user: exit syscall code=0x00000000
kernel: userland returned to kernel console
```

## What the probe proves

- `pthread_self()` is non-null in main, stable across yields, and distinct from
  the worker's.  `pthread_create` returns the same pointer the worker sees.
- `pthread_create` maps a stack and TCB, passes the start argument, and records
  the kernel tid.
- One `PTHREAD_MUTEX_INITIALIZER` mutex serialises 16 rounds from each thread;
  the counter is 32 after the first join and 64 after a second create/join.
  Every lock and unlock succeeds.
- `pthread_setspecific`/`pthread_getspecific` return the worker's value only in
  the worker.  The key destructor runs once per worker.  Main's key map stays
  null.
- The worker's `errno` stays 77 across 16 yields; main's stays 55 across both
  creates and joins.
- `pthread_join` returns the worker's start-routine value.  The second join
  proves the first worker's TCB and thread-table entry were released and the
  kernel tid can be reused.
- The pre-existing thread, futex, relibc and `std-hello` probes still pass.

## Limits

- Cooperative scheduling: no preemption and no timer wake-up.  A thread runs
  until it yields, blocks on a futex, retires or the image exits.
- The kernel's `munmap` is a bump-allocator no-op.  A joined worker's records
  are dropped from the libc tables, but the 1 MiB heap window is not recycled;
  a detached thread cannot free a stack it is still running on.
- `pthread_cancel` with cancellation enabled returns `ENOSYS` because the
  kernel has no signal delivery.  `pthread_key_delete` clears the calling
  thread's value only.
- `fork`/`exec`, signals, condition variables beyond the futex primitives,
  process tables and dynamic linking remain out of scope.
