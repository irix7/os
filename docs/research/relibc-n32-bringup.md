# relibc n32 bring-up spike

Status: **gate passed (research)**. 2026-10-25.

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
  ABI.  The provisional `irix7-pal` scaffold replaces it but has not been booted.
- Nightly-only, custom target, and QEMU-specific clone validation.

## IRIX PAL scaffold (2026-10-25)

A provisional `irix7-pal` branch in the fork replaces the Linux `sc` backend
with an `irix7` module.  It builds `librelibc.a` for the native target and links
a C hello and an fs probe with no undefined symbols and no FP instructions.

This is **not a working PAL**.  Neither binary has been executed on IRIX 7: the
host has no system-mode MIPS emulator, and the probe cannot be meaningfully run
under user-mode QEMU because IRIX syscall 15 (`getpid`) collides with Linux n32
`mknod`.  The scaffold also guesses errno because the kernel has no error
channel, has no TLS/thread pointer (MIPS III has no `rdhwr`), does not pass an
auxiliary vector, and has a private 128-byte `stat` layout that differs from
relibc's 136-byte n32 layout.  Signals, sockets, processes, `lseek` and the
remaining POSIX surface are still `ENOSYS` walls.

The branch is at <https://github.com/irix7/relibc/tree/irix7-pal>.  The next
gate is a bootable probe on iris with a real negative-errno channel and a shared
`stat`/directory ABI, not a success claim from linking alone.

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
