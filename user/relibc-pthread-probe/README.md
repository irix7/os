# relibc-linked pthread probe

`probe.c` is linked against the relibc IRIX 7 PAL and exercises
`pthread_create`, `pthread_join`, `pthread_self`, `pthread_mutex_lock`/
`unlock` and pthread-specific data on the cooperative kernel.  It reuses the
fixed 256 KiB image slot and linker script of
[`../relibc-probe`](../relibc-probe).

The probe prints `PTHREAD-PROBE: start`, and `PTHREADS-OK` only when every
check passes.  It exits 0 on a pass and 1 on a failure.

- `pthread_self()` in main is a non-null `pthread_t`, and each thread's
  `pthread_self()` is stable across yields and distinct from the other's.
- `pthread_create` creates a worker on a relibc-mapped stack and TCB, returns
  the worker's `pthread_t`, and passes the start argument unchanged.
- Main and the worker both run 16 rounds of `lock; counter++; unlock;
  sched_yield()` on one static-initialised `PTHREAD_MUTEX_INITIALIZER`.  The
  final counter is 32, so the increments were serialised and neither thread
  stalled.
- A second create/join brings the counter to 64, proving the join released the
  first worker's TCB and thread-table entry and that a second worker can use
  the reused kernel thread slot.
- The worker stores a pointer with `pthread_setspecific` and reads it back.
  Its key destructor runs once per worker when the thread exits.  Main's key
  map stays null, so the pthread-specific value did not leak between threads.
- The worker sets `errno` to 77, yields 16 times and still reads 77.  Main's
  `errno` stays 55 across the create, the yields and the join.
- `pthread_join` returns the value from the worker's start routine.

The probe declares only the POSIX surface it uses; the opaque pthread types
match relibc's `src/header/bits_threads` layout (`pthread_t` is a pointer,
`pthread_key_t` is an unsigned long and `pthread_mutex_t` is 12 bytes).
