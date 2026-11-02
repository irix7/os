# Parked sleeps on the cooperative thread gate

Status: **bring-up result**. 2026-11-02.  Tracks
[irix7/os#322](https://github.com/irix7/os/issues/322).

## Result

`nanosleep` and `clock_nanosleep` now park the calling thread on a CP0 Count
deadline.  A cooperative peer runs while the sleeper is parked, and a sleeper
with no peer parks the CPU in `wait` instead of spinning.  A relibc-linked C
program creates a worker that sleeps 20 ms, increments a shared counter from
main while it sleeps, joins, and prints `SLEEP-OK` on iris 0.1.13:

```text
user: entering user mode
SLEEP-PROBE: start
sleep: wait tid=1 limit=20ms
SLEEP-PROBE: solo-elapsed=70ms
thread: create by=1 tid=2 epc=0x10000040 sp=0x2001FFE0 tcb=0x20020000
futex: wait tid=1 addr=0x7FF03C88 value=0x00000002 limit=100ms
futex: wake addr=0x7FF03C88 count=2147483647 woke=1
sleep: wait tid=2 limit=20ms
sleep: park tid=2 limit=19ms
sleep: expiry tid=2 limit=19ms elapsed=20ms
futex: wake addr=0x20020050 count=2147483647 woke=0
thread: exit tid=2 status=0x00000000
SLEEP-PROBE: counter=3489 elapsed=22ms worker-elapsed=22ms worker-before=36239 worker-after=36261
thread: create by=1 tid=2 epc=0x10000040 sp=0x20030FE0 tcb=0x20031000
futex: wait tid=1 addr=0x7FF03C88 value=0x00000002 limit=100ms
futex: wake addr=0x7FF03C88 count=2147483647 woke=1
sleep: wait tid=2 limit=15ms
sleep: park tid=2 limit=14ms
futex: no-peer tid=1 addr=0x20031050 limit=100ms
sleep: expiry tid=2 limit=14ms elapsed=18ms
futex: wait tid=1 addr=0x20031050 value=0x00000000 limit=82ms
futex: wake addr=0x20031050 count=2147483647 woke=1
thread: exit tid=2 status=0x00000000
thread: create by=1 tid=2 epc=0x10000040 sp=0x20041FE0 tcb=0x20042000
futex: wait tid=1 addr=0x7FF03C88 value=0x00000002 limit=100ms
futex: wake addr=0x7FF03C88 count=2147483647 woke=1
sleep: wait tid=2 limit=15ms
sleep: park tid=2 limit=14ms
futex: no-peer tid=1 addr=0x20042050 limit=100ms
sleep: expiry tid=2 limit=14ms elapsed=15ms
futex: wait tid=1 addr=0x20042050 value=0x00000000 limit=99ms
futex: wake addr=0x20042050 count=2147483647 woke=1
thread: exit tid=2 status=0x00000000
thread: create by=1 tid=2 epc=0x10000040 sp=0x20052FE0 tcb=0x20053000
futex: wait tid=1 addr=0x7FF03C88 value=0x00000002 limit=100ms
futex: wake addr=0x7FF03C88 count=2147483647 woke=1
sleep: wait tid=2 limit=15ms
sleep: park tid=2 limit=14ms
futex: no-peer tid=1 addr=0x20053050 limit=100ms
sleep: expiry tid=2 limit=14ms elapsed=15ms
futex: wait tid=1 addr=0x20053050 value=0x00000000 limit=91ms
futex: wake addr=0x20053050 count=2147483647 woke=1
thread: exit tid=2 status=0x00000000
SLEEP-PROBE: join-elapsed=45ms
sleep: wait tid=1 limit=10ms
sleep: wait tid=1 limit=9ms
sleep: wait tid=1 limit=5ms
SLEEP-PROBE: checks=48 failures=0
SLEEP-OK
user: exit syscall code=0x00000000
kernel: userland returned to kernel console
```

The three `thread: create`/`thread: exit` pairs after the counter line are the
three immediate-join rounds; `join-elapsed` is the fastest of them.  This run
was taken while the emulator host was heavily loaded, so the solo sleep and
the joins show more jitter than the deadline itself; the probe's checks are
bounds and the kernel's `sleep: expiry ... elapsed=` line is the measured
interval.

The ROM is 491,464 bytes with SHA-256
`0ae559de89feb2b6d39cc689e15228b569737520db5df00ec4be6331ccb205cc`.

| Image | ELF bytes | Raw SHA-256 |
|---|---|---|
| `sleep-probe.raw` | 143,924 | `ff0894cfdde6666633827638dcdf1807a318731405cc1006f7d17239e6bce349` |
| `thread-probe.raw` | 122,928 | `e3e1c8f0f46d28c000d6ed97275916f05176d87352c8ef365b54d74175af5976` |
| `futex-probe.raw` | 124,224 | `aad47fee05a1dcd79e18d5376bbf98325c72ee1c4376f15aa8832ed3fa3a9beb` |
| `pthread-probe.raw` | 146,660 | `5eff5742da6fd5b8b6f1fe8004df63d1156065a2b647350e48e52710ca29f0fb` |
| `probe.raw` | 238,416 | `787b22e430ec2233203b7c20f3688964ba77b72e5bf2715ec414bacf461221e3` |
| `std-hello.raw` | unchanged | `983a28e371ff5e2f9d9e5d968b1457c482ebb0eb91429dba8fd62ec93da15808` |

## Kernel

- `thread::State` gains `Sleeping`.  A sleeper reuses the futex deadline
  fields: `wait_start` is the Count sample and `wait_limit` the interval in
  ticks.  A deadline scan at every scheduling point marks an expired sleeper
  runnable with `CELL_OK` in its saved `$v0`; a futex waiter still re-checks
  its word and reports `CELL_E_AGAIN`/`CELL_E_TIMEDOUT`.
- `SYS_NANOSLEEP` validates and converts the caller's `timespec`, then calls
  `thread::sleep_current`.  A runnable peer is switched to and the sleeper
  resumes through its saved frame with `EPC` advanced past the gate.  With no
  runnable peer the call parks the CPU instead: `traps::wait_until` programs
  CP0 Compare for the deadline and executes `wait` with `EXL` cleared and
  `KSU` set to kernel for the window.  Count is re-read after every interrupt
  and the loop re-arms as needed.
- A parked futex wait with no waker no longer spins.  It waits on the timer
  until its own deadline or a sleeping peer's earlier one.  When the sleeper
  expires first, the waiter parks for the rest of its budget and switches to
  it, so `pthread_join` on a sleeping worker is woken by the sleep deadline.
- `thread::exit_current` waits out the nearest parked deadline with the timer
  rather than spinning, and resumes the peer before reporting the image as
  finished.
- One park lasts at most `SLEEP_LIMIT_MS` (60 s).  The bound keeps every
  deadline inside half the 32-bit Count range and lets the libc re-arm with
  the remainder for a longer interval.  `wait_until` keeps a 1000-tick margin
  (`~30 µs`) between Count and Compare before programming the timer, because
  a Compare value already behind Count only matches after the 32-bit wrap on
  hardware.

## Fork

| Item | Value |
|---|---|
| Branch | `irix7-pal` in <https://github.com/irix7/relibc> |
| Commit | `bf5fee5e29a7a5054fa3cf82f6c5916de4b45b19` |
| Parent | `729bdc3bbbc9c4cb1fc2ca1ffeac92ea7d551383` |
| DCO | `Signed-off-by: Jupiter Belic <belikh@users.noreply.github.com>` |
| Mirror | [`tools/relibc-pal/patches/0008-irix7-sleep-on-the-native-nanosleep-gate-and-add-clo.patch`](../../tools/relibc-pal/patches/0008-irix7-sleep-on-the-native-nanosleep-gate-and-add-clo.patch) |

The branch is the source of truth.  This repository pins the commit in
[`tools/relibc-pal/build.sh`](../../tools/relibc-pal/build.sh) and the
[`README`](../../tools/relibc-pal/README.md).

## What the PAL changes

1. **`nanosleep`.** `SYS_NANOSLEEP` is a real park.  The PAL validates the
   interval, sleeps it in at most 60 s chunks, and leaves `rmtp` untouched:
   there are no signals, so the call is never interrupted.
2. **`clock_nanosleep`.** Implemented over `clock_gettime` and the PAL
   `nanosleep` for `CLOCK_REALTIME` and `CLOCK_MONOTONIC`.  A relative request
   is passed through; `TIMER_ABSTIME` is converted to an interval against the
   same clock, and an already-passed deadline returns 0 without sleeping.
   POSIX returns the error number directly, so an unknown clock, unknown flag
   or out-of-range interval is `EINVAL` without touching `errno`.
3. **`thrd_sleep`.** C11's wrapper now adapts the POSIX return value to
   `0`/`-1`/other-negative (no signal can interrupt, so the `-1` branch is
   unreachable).

## Build

```sh
export RUSTUP_HOME=/tmp/opencode/relibc-n32/rustup/rustup
export CARGO_HOME=/tmp/opencode/relibc-n32/rustup/cargo
export PATH=/nix/store/j1938gsbvbhkyz4kbmvjk56r479mkw3s-gcc-wrapper-16.2.0/bin:$PATH
export RELIBC_SRC=/tmp/opencode/relibc-pal-src

tools/m2-rig/build.sh /tmp/opencode/sleep-work/rig
tools/relibc-pal/build.sh /tmp/opencode/sleep-work/out
```

The relibc build pins `bf5fee5e` and refuses a different checkout.  The C
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
path = "/tmp/opencode/sleep-work/out/sleep-probe.raw"
cdrom = false
```

```sh
IRIS_NO_IDLE=1 iris --config sleep.toml --ci --serial-log sleep-serial.log \
  --monitor-port 18903 --ci-socket /tmp/opencode/sleep-work/sleep.sock \
  --prom /tmp/opencode/sleep-work/rig/irix7-rom.bin \
  --nvram /tmp/opencode/sleep-work/sleep-nvram.bin
iris-ci --socket /tmp/opencode/sleep-work/sleep.sock status --wait-ready 30
iris-ci --socket /tmp/opencode/sleep-work/sleep.sock start
```

The other images run unchanged:

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
PTHREADS-OK
PTHREAD-PROBE: checks=29 failures=0
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

- `nanosleep` rejects `tv_nsec >= 1e9` and a negative interval with `EINVAL`,
  and a zero interval returns without parking.
- A solo 20 ms sleep returns after its deadline through the kernel's
  Compare + `wait` path (the recorded run measured 70 ms under host load).
  No other thread exists, so nothing but the timer ends it.
- A worker that sleeps 20 ms does not stop main: main's yield-and-increment
  loop advances the counter (3489 in this run, and the check only requires a
  non-zero count) while the worker's own `clock_gettime` span covers at least
  the requested interval.  The worker's return value proves it woke and set
  its flag.
- Three immediate-join rounds on a 15 ms sleeper finish in tens of
  milliseconds.  The kernel transcript shows why: the no-peer futex wait
  printed `limit=100ms`, the sleeper expired after 14–18 ms, and the futex
  then parked with the remaining 82–99 ms — the sleeper's deadline, not the
  futex poll bound, ended the wait.
- `clock_nanosleep` works relative on `CLOCK_REALTIME`, absolute on
  `CLOCK_MONOTONIC`, and returns at once for a deadline in the past.  Unknown
  flags, unknown clocks and out-of-range intervals return `EINVAL` directly.
- An uninterrupted `nanosleep` leaves the caller's remaining-time word
  untouched.

## Limits

- Threads are cooperative: no preemption.  A sleeper is resumed at the next
  scheduling point after its deadline, so a peer that never yields also never
  sees the switch.  The kernel's own timer wait is the exception: it resumes
  the running thread at its own deadline.
- One park lasts at most 60 s.  `nanosleep` re-arms with the remainder, but a
  caller of the raw gate must do the same.
- No signals: `nanosleep` is never interrupted, `rmtp` is never written, and
  `clock_nanosleep` cannot return `EINTR`.
- `CLOCK_MONOTONIC` is CP0 Count divided by the nominal 33 MHz, so it wraps
  every ~130 s.  This predates the slice and only affects a caller that
  computes an absolute deadline across the wrap.
- Timer delivery and host scheduling jitter on iris stretch short sleeps
  under load; the probe says "at least the interval" and bounds the upper end
  generously for that reason.
- `pthread_cond_timedwait` still uses the futex primitive; it can now rely on
  a real `clock_nanosleep` for its fallback waits but was not part of this
  slice.
- No process table, signals, `fork`/`exec` or preemption.
