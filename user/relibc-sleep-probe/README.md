# relibc-linked sleep probe

`probe.c` is linked against the relibc IRIX 7 PAL and exercises
`nanosleep` and `clock_nanosleep` through the kernel's parked-sleep gate from
`docs/spec/native-abi.md` §5.9.  It reuses the fixed 256 KiB image slot and
linker script of [`../relibc-probe`](../relibc-probe).

The probe prints `SLEEP-PROBE: start`, and `SLEEP-OK` only when every check
passes.  It exits 0 on a pass and 1 on a failure.

- `nanosleep` rejects a nanosecond field of `1e9` and a negative interval with
  `EINVAL`; a zero interval returns without parking.
- A solo sleep of 20 ms lasts at least its interval, so the kernel's
  no-runnable-peer path (arm Compare, execute `wait`) still honours the
  deadline.
- A worker sleeps 20 ms while main runs a `sched_yield`/increment loop.  The
  main-side elapsed time and the worker's own `clock_gettime` span both show
  at least the interval, the counter has advanced, and the worker's return
  value proves it woke.
- `pthread_join` on a sleeping worker finishes in well under the kernel's
  100 ms futex poll bound: the sleep deadline, not the bound, ends the join.
  The kernel's no-peer futex wait parks until the sleeper's earlier deadline,
  then hands the CPU to it.
- `clock_nanosleep` works relative on `CLOCK_REALTIME`, absolute on
  `CLOCK_MONOTONIC`, and returns at once for a deadline in the past.  Unknown
  flags, unknown clocks and out-of-range intervals return `EINVAL` directly,
  not through `errno`.
- An uninterrupted `nanosleep` leaves the remaining-time pointer untouched,
  because the kernel has no signals and never reports an interruption.

There are no gate stubs to assemble: both calls are mapped in the relibc PAL,
so the probe links only `probe.c`.
