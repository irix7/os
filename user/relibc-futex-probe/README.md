# relibc-linked futex probe

`probe.c` is linked against the relibc IRIX 7 PAL and exercises the raw
cooperative futex gate from `docs/programming/native-abi.md` §5.9.  It reuses the
fixed 256 KiB image slot and linker script of
[`../relibc-probe`](../relibc-probe).

The probe prints `FUTEX-PROBE: start`, and `FUTEX-OK` only when every check
passes.  It exits 0 on a pass and 1 on a failure.

- `SYS_FUTEX_WAKE` with no waiter returns 0, and a zero count wakes nothing.
- `SYS_FUTEX_WAIT` compares the 32-bit word before parking: a mismatch returns
  `CELL_E_AGAIN`, even with a zero timeout.  An unaligned address is
  `CELL_E_INVAL` and an address outside the image's mapped ranges is
  `CELL_E_FAULT`.
- A worker parks on a word with a negative timeout.  Main sets the word,
  `SYS_FUTEX_WAKE(address, 1)` returns 1, and the worker resumes inside
  `futex_wait` with `CELL_OK`.  The woken worker does not run until main
  yields, which is the cooperative scheduling contract.
- Two workers park on one word; a wake count of one returns 1 and resumes
  exactly one of them.  A second wake resumes the other, and a third returns 0.
- A wait with no waker reaches its bounded deadline with the word unchanged
  and returns `CELL_E_TIMEDOUT`.
- A wait whose word changed without a wake re-checks the word at its deadline
  and returns `CELL_E_AGAIN`.
- With no thread left that could wake it, a negative timeout is bounded by the
  poll rule rather than infinite: the call returns after about 100 ms with
  `CELL_E_TIMEDOUT`.  A zero timeout probes the word and returns
  `CELL_E_TIMEDOUT` without parking.

`native.S` holds the raw gate stubs.  `futex_wait` and `futex_wake` are mapped
in the relibc PAL itself; the probe calls the gate directly because the PAL
does not yet expose a thread-create API.
