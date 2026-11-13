# relibc-linked cooperative thread probe

`probe.c` is linked against the relibc IRIX 7 PAL and exercises the raw
cooperative thread gate from `docs/programming/native-abi.md` §5.9.  It reuses the
fixed 256 KiB image slot and linker script of
[`../relibc-probe`](../relibc-probe).

The probe prints `THREAD-PROBE: start`, and `THREADS-OK` only when every check
passes.  It exits 0 on a pass and 1 on a failure.

- `$28` in `main` is relibc's 16-byte TCB and `__errno_location()` is `TP + 8`.
- `SYS_YIELD` with no other runnable thread returns success without switching.
- `SYS_THREAD_CREATE` rejects a stack top outside a mapped user range with
  `CELL_E_FAULT`; a valid create returns tid 2 for the first worker.
- The worker runs on a 16 KiB static stack and with a static TCB in `.bss`,
  checks that its `$28` is that TCB and that its argument is the TCB pointer,
  and writes a distinct `errno` value there.
- Main and the worker increment separate shared counters.  The worker yields
  seven times; main runs between every yield and yields back.  The worker then
  sets a flag, writes its libc word and retires with `SYS_THREAD_EXIT`.
- Main's `$28` and `errno` are unchanged after the worker's exit, and the two
  TCBs are distinct addresses.

`native.S` holds the three raw gate stubs.  The relibc PAL does not expose a
thread-create API yet, so the probe calls the gate directly; `exit_thread` and
`sched_yield` are mapped in the PAL itself.
