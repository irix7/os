# relibc PAL on the native n32 kernel

Status: **bring-up result**. 2026-11-02.  Tracks
[irix7/os#288](https://github.com/irix7/os/issues/288) and its final userland
slice.  Supersedes the scaffold limits recorded in
[`relibc-n32-bringup.md`](relibc-n32-bringup.md) and
[`c-probe-bringup.md`](c-probe-bringup.md).

## Result

A C program linked against `librelibc.a` from the `irix7-pal` branch boots on
iris 0.1.13 through SCSI target 2, prints `IRIX7-RELIB-PROBE-1`, performs
`write`, `stat` and two failing calls that set `errno` through
`__errno_location`, and exits 0:

```text
kernel: starting userland from SCSI target 2
userelf: entry=0x10000000
user: entering user mode
IRIX7-RELIB-PROBE-1
IRIX7-RELIB-PROBE-1: stat size=28 mode=100644 blksize=4096 blocks=1
IRIX7-RELIB-PROBE-1: tp=0x10026710 auxv pagesz=4096 failures=0
user: exit syscall code=0x00000000
kernel: userland returned to kernel console
sash> m1-post: mesh=0/0/0 actor=08006912345600000000000000000001
```

The image was run on the merged kernel at
`e13afe52204a71fdbe157067f2db11afd9631e5a`; the rebuilt ROM is 483,540 bytes
with SHA-256
`eb980e92fc24198a442dbb29674902e0c89396da8db3b7ffe60e93dddf64366e`.  The
probe image (`probe.raw`) is
`5679fb31888a24215d6c23a9543b7af78ce1ee43d9f5193f8d84919fc11dd1a7`.

## Fork

| Item | Value |
|---|---|
| Branch | `irix7-pal` in <https://github.com/irix7/relibc> |
| Commit | `d484536c49fed26a5377d2c511962b891e1c8dfb` |
| Parent | `a36041ac5201ede0c3853d548de5aa7fbfb0c708` |
| DCO | `Signed-off-by: Jupiter Belic <belikh@users.noreply.github.com>` |
| Mirror | [`tools/relibc-pal/patches/`](../../tools/relibc-pal/patches/) |

The branch is the source of truth.  This repository pins the commit in
[`tools/relibc-pal/README.md`](../../tools/relibc-pal/README.md) and its build
script; the patch is review mirror only.

## What the branch changes

1. **Native gate.** `syscall0..4` become one `syscall 0x1F7` path: syscall
   number in `$2`, arguments in `$4`–`$7`, `CELL_E_*` status in `$2`, value in
   `$3`.  A single `STATUS_ERRNO` table maps the status to `Err(Errno(...))`;
   the kernel never returns a libc `errno`.
2. **Shared `stat`.** `KernelStat`/`copy_stat` are deleted.  relibc's 136-byte
   n32 `struct stat` is passed to `fstat`/`stat` directly; the kernel writes
   that layout.
3. **TCB and errno.** `_start` builds the 16-byte main TCB (`self` at 0, `dtv` 0
   at 4, `errno` at 8, libc-private at 12) and sets `$28`.  `__errno_location`
   and the internal errno cell are `TP + 8`.  MIPS III has no `rdhwr`, and the
   target keeps `has-thread-local: false`; every `#[thread_local]` in the
   library becomes a plain static on this target, with `SingleThread` stating
   the single-threaded invariant at each declaration.
4. **Auxiliary vector.** `_start` stores `$a3`; `Stack::auxv` returns it, and
   `getauxval` reads the kernel's native vector numbering
   (`AT_PAGESZ` 1, `AT_ENTRY` 2, `AT_CLKTCK` 13, and so on).
5. **`getdents`.** Still synthesised over the kernel's one-entry
   `readdir(fd, index, name, len)`; there is no directory cookie to advance.

Two follow-on fixes are part of the same commit:

- `futex_wait`/`futex_wake` are no-ops.  The kernel has no futex and one
  thread; `ENOSYS` there made `printf` panic on the first mutex unlock.
- The MIPS n32 `crt0` sets the TCB before calling `relibc_crt0`, so the first
  errno use has a valid thread pointer.

## Build

Toolchain:

| Component | Version |
|---|---|
| Rust | `1.98.0-nightly (23a3312d9 2026-05-23)`, relibc's `nightly-2026-05-24` + `rust-src` |
| C compiler | Bootlin `mips64-n32--uclibc--stable-2026.08-1`, GCC 15.3.0, binutils 2.45.1 |
| Target | `targets/mips3-sgi-irix7-native-n32.json` (n32, big-endian, MIPS III, soft float) |
| Flags | `-mno-abicalls -fno-pic -mabi=n32 -msoft-float -G0`; `-G0` for the native image |

```sh
export RUSTUP_HOME=/path/to/rustup
export CARGO_HOME=/path/to/cargo
export MIPS_CC=/path/to/mips64-buildroot-linux-uclibc-gcc
export MIPS_AR=/path/to/mips64-buildroot-linux-uclibc-ar
export RELIBC_SRC=/tmp/opencode/relibc-pal-src

tools/relibc-pal/build.sh /tmp/opencode/relibc-pal-work/out
```

The script verifies `d484536c`, builds `librelibc.a` and `libcrt0.a`, then
links [`user/relibc-probe/probe.c`](../../user/relibc-probe/probe.c) with
`--gc-sections`.  The unstripped link is about 250 KiB; stripping leaves a
223 KiB ELF and the 256 KiB raw image is padded.

## Run

```toml
banks = [64, 0, 0, 0]
no_audio = true

[machine]
profile = "indy_ip24"

[scsi]

[scsi.2]
path = "/tmp/opencode/relibc-pal-work/out/probe.raw"
cdrom = false
```

```sh
IRIS_NO_IDLE=1 /tmp/opencode/iris-0.1.13/iris --config probe.toml --ci \
  --serial-log probe-serial.log --monitor-port 18893 \
  --ci-socket /tmp/opencode/relibc-pal-work/probe.sock \
  --prom /tmp/opencode/relibc-pal-work/rig/irix7-rom.bin \
  --nvram /tmp/opencode/relibc-pal-work/probe-nvram.bin
/tmp/opencode/iris-0.1.13/iris-ci --socket /tmp/opencode/relibc-pal-work/probe.sock \
  status --wait-ready 30
/tmp/opencode/iris-0.1.13/iris-ci --socket /tmp/opencode/relibc-pal-work/probe.sock start
```

`--headless` is not used: on iris 0.1.13 it leaves REX3 initialisation in a
GIO-timeout loop on this kernel.

## What the probe proves

- **A relibc-linked C binary, not the shim.** The image calls relibc's
  `write`, `stat`, `fstat`, `getpid`, `getauxval` and `__errno_location`, and
  enters through relibc's `crt0::_start`.  `_Static_assert`s pin the 136-byte
  `stat` and its field offsets at compile time.
- **The gate and its error channel.** `fstat(999)` returns status
  `CELL_E_BADF` (5), which the PAL maps to `EBADF` (9) and returns as `-1`.
  `stat("/missing")` independently returns `CELL_E_NOENT` (2) -> `ENOENT` (2),
  so the result is not a constant failure value.
- **The TCB convention.** `$28` is non-zero and `__errno_location()` equals
  `$28 + 8`; the probe reads `$28` in C and compares the pointers.
- **The `$a3` auxiliary vector.** `getauxval` reports `AT_PAGESZ == 4096`,
  `AT_ENTRY == &_start` and `AT_CLKTCK == 100`, values only the loader can
  supply.
- **The 136-byte `stat`.** `stat("/hello.txt")` reports a regular file with one
  link, 28 bytes, a 4096-byte block size and one block.
- **The loader path is unchanged.** The image is a padded 256 KiB raw SCSI
  target-2 file, like `user/c-probe` and `std-hello`.

## `std-hello` regression

The same ROM booted the existing static Rust `std` image.  Rebuilding
`tools/build-std-hello.sh` reproduced the existing `std-hello.raw`
byte-for-byte (SHA-256
`983a28e371ff5e2f9d9e5d968b1457c482ebb0eb91429dba8fd62ec93da15808`):

```text
hello from std
arg=hello
slept_ms=10
time=1793597182
file=hello from the IRIX 7 ramfs
write=ok
after_write=rewritten by std
chdir=ok
cwd=/etc
relative=Welcome to IRIX 7.
stat_size=17
dev_null=ok
entry=hello.txt
entry=etc
entry=dev
user: exit syscall code=0x00000000
kernel: userland returned to kernel console
```

The std shim still uses its own `$gp` startup; the relibc PAL is the
`-mno-abicalls -G0` image.

## Limits

- Single-threaded.  No threads, ELF TLS block, signals or process table.  The
  `SingleThread` wrappers are a truthful statement of that kernel state, not a
  general threading solution.
- `getdents` yields one entry per call and has no directory cookie.  `lseek`,
  `pread`/`pwrite`, `dup2`, `fork`, `execve`, `waitpid`, `pipe2` and the socket
  API remain `ENOSYS`.
- `getentropy` succeeds but the kernel zero-fills the buffer.
- `mmap` is the kernel's bump allocator; `munmap`/`mprotect` are metadata-only.
- The console/ramfs fd table is the only filesystem.
- `uname` describes the ABI, not the running kernel; there is no `uname`
  syscall.
- The fork's Linux/QEMU gate still builds but was not re-run under
  `qemu-mipsn32` for this change; the host control build passes.
