# relibc n32 bring-up spike

Status: **gate passed (research)**. 2026-10-25.

> **2026-11-02 update:** the kernel provides the v0 native gate, the
> `CELL_E_*` status channel, the 136-byte relibc `stat` layout, the auxv stack
> and the `$28` TCB convention.  A freestanding C probe booted first, and the
> relibc-linked PAL now boots too; see
> [`c-probe-bringup.md`](c-probe-bringup.md) and
> [`relibc-pal-bringup.md`](relibc-pal-bringup.md).  The PAL commit is
> `d484536c49fed26a5377d2c511962b891e1c8dfb` on the `irix7-pal` branch.

Upstream relibc is not n32-ready, but a contained port has been demonstrated.  A
patched relibc built a static, big-endian, n32 `libc.a`, and four smoke tests ran
under `qemu-mipsn32`:

| Test | Result |
|---|---|
| `hello`, `printf`, `argc`/`argv` | pass |
| `__thread` TLS initial image and read/write | pass |
| `setjmp`/`longjmp` using relibc's mipsn32 assembly | pass |
| `pthread_create`/`join` with per-thread TLS | pass |

The raw result included:

```text
hello: argc=1 tls=1234
setjmp: r=9 jumped=1 tls=5678
main: tls=100
thread: tls=42
joined: ret=43 main_tls=100
```

## What it proves

The expensive unknowns for forking relibc are resolved: n32 big-endian codegen,
static linking, TLS layout (`TLS_DTV_AT_TP`, `TLS_TCB_OFFSET 0x7000`), MIPS
pthread clone argument order, the n32 syscall table, `setjmp`, and per-thread
TLS.  The port is about 224 changed lines plus a generated syscall backend; an
IRIX PAL would need the same architecture seam.

## What it does not prove

- Static linking only; no dynamic MIPS relocations or ld.so path.
- Signal/ucontext types are placeholders; delivery and `sigreturn` are untested.
- `long double` varargs and `%Lf` are approximate/untested.
- 64-bit syscall arguments are truncated by `usize` casts.
- The headers are not a full sysroot.
- The n32 syscall backend used for the QEMU gate is Linux, not the IRIX 7 native
  ABI.  The `irix7-pal` branch replaces it and now boots; see
  [`relibc-pal-bringup.md`](relibc-pal-bringup.md).
- Nightly-only, custom target, and QEMU-specific clone validation.

## IRIX PAL scaffold (2026-10-25)

A provisional `irix7-pal` branch in the fork replaces the Linux `sc` backend
with an `irix7` module.  It builds `librelibc.a` for the native target and links
a C hello and an fs probe with no undefined symbols and no FP instructions.

At that point this was **not a working PAL**.  Neither binary had been executed
on IRIX 7: the host had no system-mode MIPS emulator, and the probe could not be
meaningfully run under user-mode QEMU because IRIX syscall 15 (`getpid`)
collides with Linux n32 `mknod`.  The scaffold also guessed errno because the
kernel had no error channel, had no TLS/thread pointer (MIPS III has no
`rdhwr`), passed no auxiliary vector, and used a private 128-byte `stat` layout
that differed from relibc's 136-byte n32 layout.  The 2026-11 kernel work
replaces all four; the PAL commit
`d484536c49fed26a5377d2c511962b891e1c8dfb` now boots on iris.  See
[`c-probe-bringup.md`](c-probe-bringup.md) and
[`relibc-pal-bringup.md`](relibc-pal-bringup.md).  Signals, sockets, processes,
`lseek` and the remaining POSIX surface are still `ENOSYS` walls.

The branch is at <https://github.com/irix7/relibc/tree/irix7-pal>.  The gate
described above has passed: a relibc-linked probe now runs with the negative
errno channel and the shared `stat` ABI.

## Repository artefacts

The patched source, patch set, pristine `sc` 0.2.7 source, test sources and full
report live in the dedicated repository:

<https://github.com/irix7/relibc> (branch `irix7-n32`); the PAL scaffold is on
`irix7-pal`.  Neither is wired into the kernel or release build.

## Next

1. Harden the IRIX PAL scaffold: negative-errno channel, TLS/thread pointer,
   auxiliary vector and a shared `stat`/directory ABI.
2. Boot the linked fs probe on iris and only then replace the Linux `sc` gate.
3. Add the target spec and relibc patch queue to the forked relibc.
4. Add a mips64 `crt0` to relibc proper.
5. Keep the IRIX `std` shim as the bring-up path until the relibc link succeeds.
