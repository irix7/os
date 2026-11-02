# relibc-linked per-cell sleep probe

`probe.c` is linked against the relibc IRIX 7 PAL and proves that a
`nanosleep` in a parent cell lets a runnable child cell run.  It is the
process-level counterpart of [`../relibc-sleep-probe`](../relibc-sleep-probe):
that probe covers a sleeping thread and a runnable sibling thread in one cell;
this one covers two cells after `fork`.  It reuses the fixed 256 KiB image slot
and linker script of [`../relibc-probe`](../relibc-probe).

The parent creates a non-blocking pipe, forks, and sleeps 20 ms.  The child
increments a counter 2,000 times, writes the final value to the pipe and exits
with status 42.  The parent then checks:

- the pipe already holds the counter, so the child ran before the parent woke;
- the value equals the loop count, so the child finished the work;
- the parent's `CLOCK_MONOTONIC` span is at least 20 ms and below a relaxed
  upper bound;
- `waitpid(pid, &status, WNOHANG)` reaps an already-exited child and reports
  status 42.

The pipe is non-blocking on purpose.  A blocking read would wait for the child
and hide the scheduling bug; with `O_NONBLOCK`, a child that did not run during
the sleep leaves the pipe empty and the probe fails with a missing counter.

The probe prints `SLEEP-CELL-PROBE: counter=... expected=... elapsed=...ms
checks=... failures=0`, then `SLEEP-CELL-OK`, and exits 0.  A failure prints
the failed check and `SLEEP-CELL-FAILED`, and exits 1.

```sh
tools/relibc-pal/build.sh /tmp/opencode/relibc-pal-work/out
```

Attach `sleep-cell-probe.raw` to SCSI target 2.  No child image is needed: the
child is a fork of the parent image.  See
[`tools/relibc-pal/README.md`](../../tools/relibc-pal/README.md).
