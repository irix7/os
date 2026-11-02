# relibc PAL on the native n32 kernel

Status: **bring-up result**. 2026-11-02.  Tracks
[irix7/os#288](https://github.com/irix7/os/issues/288) and
[irix7/os#302](https://github.com/irix7/os/issues/302).  Supersedes the
scaffold limits recorded in
[`relibc-n32-bringup.md`](relibc-n32-bringup.md) and
[`c-probe-bringup.md`](c-probe-bringup.md).

## Result

A C program linked against `librelibc.a` from the `irix7-pal` branch boots on
iris 0.1.13 through SCSI target 2, prints `IRIX7-RELIB-PROBE-3`, exercises the
native file and readiness surface and exits 0:

```text
kernel: starting userland from SCSI target 2
userelf: entry=0x10000000
user: entering user mode
IRIX7-RELIB-PROBE-3
IRIX7-RELIB-PROBE-3: stat size=28 mode=100644 blksize=4096 blocks=1
IRIX7-RELIB-PROBE-3: tp=0x1002c7e0 auxv pagesz=4096
IRIX7-RELIB-PROBE-3: checks=142 failures=0
user: exit syscall code=0x00000000
kernel: userland returned to kernel console
sash> m1-post: mesh=0/0/0 actor=08006912345600000000000000000001
```

The image was run on the kernel built from this tree at `cd48028` plus this
slice.  The ROM is 487,180 bytes with SHA-256
`930494bd6db328484ec2f766632a800b79da0aa85aaefcdce7c175f23a35dd0d`.  The
pinned probe build (`probe.raw`) is 248,092 bytes with SHA-256
`71c586d738a3722aacd17a12a356dbee6ec126c29b9ec9d008c60bcecf160371`; it
embeds relibc `acd6c496` and 142 checks passed on it.

The earlier `IRIX7-RELIB-PROBE-2` result and its hashes are recorded in the
previous revision of this file; those 110 checks are a subset of this run.

## Fork

| Item | Value |
|---|---|
| Branch | `irix7-pal` in <https://github.com/irix7/relibc> |
| Commit | `acd6c4966c249510154158e05c19b8f846a70bb5` |
| Parent | `d5fd3d04c88fac4117132ae25d3959f6e55277b7` |
| DCO | `Signed-off-by: Jupiter Belic <belikh@users.noreply.github.com>` |
| Mirror | [`tools/relibc-pal/patches/`](../../tools/relibc-pal/patches/) |

The branch is the source of truth.  This repository pins the commit in
[`tools/relibc-pal/README.md`](../../tools/relibc-pal/README.md) and its build
script; the patches are review mirror only.

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
5. **`getdents` and directory cookies.** `getdents` maps to the kernel call,
   which fills whole `dirent` records and advances the descriptor's cookie.
   `d_off` is the cookie of the next entry and `dir_seek` writes it back with
   `lseek`, so `telldir`/`seekdir` work.  The one-entry `readdir` synthesis is
   gone.
6. **The file surface.** `lseek` (`SEEK_SET`/`CUR`/`END`/`DATA`/`HOLE`),
   `pread`, `pwrite`, `dup2` and `pipe2` map to the native calls.  The 64-bit
   offset is split across two argument registers, and `pread`/`pwrite` pass a
   16-byte `{ buf, len, offset }` block.
7. **Close-on-exec.** `fcntl F_GETFD`/`F_SETFD` carry real state,
   `O_CLOEXEC` is recorded on `open`, and `dup`/`dup2` clear it on the new
   descriptor.
8. **Readiness and `O_NONBLOCK`.** `poll` passes its millisecond timeout to the
   kernel, which evaluates real pipe and console readiness.  `fcntl
   F_GETFL`/`F_SETFL` carry the access mode and `O_NONBLOCK` per open file
   description; `dup` shares them.  A non-blocking read of an empty pipe or an
   idle console is `EAGAIN`; a blocking one rechecks readiness for the kernel's
   fixed 100 ms bound and then does the same.

The first five items are the `d484536c` commit.  Items 5–7 and the file calls
are `d5fd3d04`.  Item 8 is `acd6c496`.

The first commit also carries two follow-on fixes:

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

The script verifies `acd6c496`, builds `librelibc.a` and `libcrt0.a`, then
links [`user/relibc-probe/probe.c`](../../user/relibc-probe/probe.c) with
`--gc-sections`.  The unstripped link is about 260 KiB; stripping leaves a
241 KiB ELF and the 256 KiB raw image is padded.

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
  `write`, `stat`, `fstat`, `getpid`, `getauxval`, `__errno_location`, `open`,
  `lseek`, `pread`, `pwrite`, `dup`, `dup2`, `fcntl`, `pipe`, `pipe2`, `poll`,
  `opendir`, `readdir`, `seekdir`, `telldir` and `posix_getdents`, and enters
  through relibc's `crt0::_start`.  `_Static_assert`s pin the 136-byte `stat`,
  the 280-byte `dirent`, the 8-byte `pollfd` and their field offsets at compile
  time.
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
- **The file surface.** `lseek` moves and reports the position in every mode,
  `pread`/`pwrite` transfer at an explicit offset without moving it, `dup` and
  `dup2` share the open file description (a seek through one moves the other),
  and `dup2` from a bad descriptor is `EBADF`.
- **Close-on-exec is state.** `open(O_CLOEXEC)` is visible in `F_GETFD`,
  `F_SETFD` clears and sets it, and `dup`/`dup2` clear it on the new descriptor.
- **The pipe contract.** A pipe round-trips bytes; a non-blocking empty read
  with a live writer is `EAGAIN`; a write after the reader closes is `EPIPE`;
  closing every writer makes the reader report end-of-file; `lseek` on a pipe
  is `ESPIPE`; `pipe2(O_CLOEXEC)` marks both ends and unknown flags are
  `EINVAL`.
- **`O_NONBLOCK` is state, not a label.** `F_GETFL` reports `O_RDWR` for an
  `open(O_RDWR)` file, `F_SETFL` sets and clears `O_NONBLOCK`, `dup` sees the
  same flag through the shared description, and a non-blocking read of an
  empty pipe is `EAGAIN`.
- **`poll` reports real readiness.** On a live empty pipe, a zero-timeout poll
  of the read end returns 0 and the write end returns `POLLOUT`.  Data gives
  `POLLIN` on the read end.  With two records, only the ready one is counted
  and `revents` is written per record.  Closing every writer gives `POLLHUP`
  and no `POLLIN`; closing the reader gives `POLLERR` on the write end.  A
  closed descriptor is `POLLNVAL`, a negative descriptor is ignored, and a
  zero-count poll returns 0.
- **The negative timeout is bounded.** A `poll(-1)` on an idle pipe returns 0
  after the kernel's 100 ms cap; the same call on the always-writable console
  returns immediately.
- **Console input is readable when present.** The console reports `POLLOUT`
  unconditionally.  A separate temporary build polled fd 0, received a `Z`
  sent with `iris-ci serial-send`, read it back and exited 0; the serial
  receiver feeds the same input queue as the PS/2 keyboard.  The committed
  probe checks the `POLLOUT` half deterministically so CI needs no keystroke.
- **Directory cookies.** One `posix_getdents` call returns three records in a
  1024-byte buffer with `d_off` 1, 2 and 3, the next returns 4 and 5, the call
  at the end returns 0, and `lseek` to cookie 1 resumes at `..`.  `d_reclen`
  is 280, `d_type` is filled and `d_ino` is stable across the seek.  A buffer
  smaller than one record is `EINVAL`.  `readdir`, `seekdir` and `telldir`
  reach the same entries through the wrapper.
- **The loader path is unchanged.** The image is a padded 256 KiB raw SCSI
  target-2 file, like `user/c-probe` and `std-hello`.

## `std-hello` regression

The committed `packages/userland/std-hello.raw`
(SHA-256 `983a28e371ff5e2f9d9e5d968b1457c482ebb0eb91429dba8fd62ec93da15808`)
runs unchanged on this ROM and exits 0:

```text
kernel: starting userland from SCSI target 2
userelf: entry=0x10001080
user: entering user mode
hello from std
arg=hello
slept_ms=10
time=1793604922
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

The image was rebuilt in `cd48028` against the 136-byte `stat` and the native
gate.  The earlier failure in `core::fmt::write` was the 128-byte
`KernelStat` overrun described in the previous revision of this file; it is
closed.

## Console input check

The committed probe tests `POLLOUT` and pipe readiness.  To check the readable
side of the console without making CI depend on a keystroke, a temporary
program outside the repository polled fd 0 with `POLLIN` and read one byte.
The run printed:

```text
user: entering user mode
IRIX7-CONSOLE-INPUT-READY
IRIX7-CONSOLE-INPUT-GOT Z
user: exit syscall code=0x00000000
```

`Z` was injected with `iris-ci serial-send --no-cr Z` after the ready marker.
The serial receiver and the PS/2 keyboard feed the same console queue, so
either source makes `POLLIN` true.

## Limits

- Single-threaded.  No threads, ELF TLS block, signals or process table.  The
  `SingleThread` wrappers are a truthful statement of that kernel state, not a
  general threading solution.
- There is no scheduler to wake a blocked task.  `poll` rechecks readiness and
  a "blocking" read or write rechecks it, but both give up after the fixed
  100 ms bound and return `EAGAIN`.  A negative `poll` timeout is clamped to
  that bound, not infinite.
- `poll` is a readiness recheck, not a registration API: there is no wait
  queue and no cross-thread wakeup.  The console and pipe state are the only
  event sources.
- Console input is the PS/2 keyboard and the serial receiver.  The telnet
  console is a kernel console and does not feed a user image.
- `fork`, `execve`, `waitpid` and the socket API remain `ENOSYS`.
- `getentropy` succeeds but the kernel zero-fills the buffer.
- `mmap` is the kernel's bump allocator; `munmap`/`mprotect` are metadata-only.
- The console/ramfs fd table is the only filesystem; there is no file creation,
  unlinking or permissions.
- `uname` describes the ABI, not the running kernel; there is no `uname`
  syscall.
- The fork's Linux/QEMU gate still builds but was not re-run under
  `qemu-mipsn32` for this change; the host control build passes.
