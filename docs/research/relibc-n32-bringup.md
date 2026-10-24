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
- The n32 syscall backend is Linux, not the IRIX 7 native ABI; the IRIX PAL
  replaces it.
- Nightly-only, custom target, and QEMU-specific clone validation.

## Repository artefacts

The patched source, patch set, pristine `sc` 0.2.7 source, test sources and full
report live in the dedicated repository:

<https://github.com/irix7/relibc> (branch `irix7-n32`).

It is a research branch, not yet wired into the kernel or release build.

## Next

1. Add the target spec and relibc patch queue to the forked relibc.
2. Replace the Linux `sc` backend with an `irix7` PAL over the native capability
   syscalls.
3. Add a mips64 `crt0` to relibc proper.
4. Keep the IRIX `std` shim as the bring-up path until the relibc link succeeds.
