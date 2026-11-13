# First C probe on the native syscall gate

Status: **bring-up result**. 2026-11-02.  Tracks
[irix7/os#288](https://github.com/irix7/os/issues/288).

## Result

A freestanding n32 C binary boots on the Indy through the existing user-image
path, enters the new native gate (`syscall 0x1F7`), and exits 0:

```text
kernel: starting userland from SCSI target 2
userelf: entry=0x10000000
user: entering user mode
C-PROBE: check=ok sp 16-aligned
C-PROBE: check=ok argc == 1
C-PROBE: check=ok argv[0] present
C-PROBE: check=ok auxv aligned
C-PROBE: check=ok AT_PAGESZ == 4096
C-PROBE: check=ok AT_ENTRY == _start
C-PROBE: check=ok AT_RANDOM present
C-PROBE: check=ok AT_RANDOM non-zero
C-PROBE: check=ok AT_CLKTCK == 100
C-PROBE: check=ok AT_SECURE == 0
C-PROBE: check=ok AT_EXECFN present
C-PROBE: check=ok AT_PLATFORM present
C-PROBE: check=ok TCB self
C-PROBE: check=ok TCB dtv == 0
C-PROBE: check=ok $28 == TCB
C-PROBE: check=ok errno at TP+8
C-PROBE: marker=IRIX7-C-PROBE-1
C-PROBE: check=ok write returns length
C-PROBE: check=ok $28 preserved across syscall
C-PROBE: write=ok
C-PROBE: check=ok getpid == 1
C-PROBE: check=ok stat(/hello.txt) == 0
C-PROBE: check=ok stat is regular
C-PROBE: check=ok stat st_nlink == 1
C-PROBE: check=ok stat st_size == 28
C-PROBE: check=ok stat st_blksize == 4096
C-PROBE: check=ok stat st_blocks == 1
C-PROBE: stat size=28 mode=0100644 blksize=4096
C-PROBE: check=ok fstat(999) == -1
C-PROBE: check=ok fstat status == E_BADF
C-PROBE: check=ok fstat errno == EBADF
C-PROBE: badf status=5 errno=9
C-PROBE: check=ok stat(/missing) == -1
C-PROBE: check=ok stat status == E_NOENT
C-PROBE: check=ok stat errno == ENOENT
C-PROBE: notfound status=2 errno=2
C-PROBE: tcb=0x10002270 auxv=0x7ff03f48
C-PROBE: done rc=0
user: exit syscall code=0x00000000
kernel: userland returned to kernel console
```

## What the probe proves

- **A real C binary, not a Rust shim.** The image is clang n32 big-endian
  MIPS III, linked with `rust-lld` and no libc. `user/c-probe/abi.h` is the C
  oracle and `_Static_assert`s every `stat`, `dirent`, TCB and auxv offset at
  compile time.
- **A deterministic error channel.** `fstat(999)` returns status `CELL_E_BADF`
  (5), which the C stub maps through the shared `status_errno[]` table to
  `EBADF` (9) and returns `-1`. `stat("/missing")` independently returns status
  `CELL_E_NOENT` (2) and errno `ENOENT` (2), so the result is not a constant
  failure value.
- **The 136-byte n32 `struct stat`.** The kernel writes
  `native_abi::stat`, the same layout as relibc's n32
  `src/header/sys_stat/mod.rs`; the probe checks size, mode, link count, size,
  block size and blocks. `struct dirent` (280 bytes) is pinned by static
  assertions even though this probe does not read a directory.
- **Auxiliary vector on the stack.** The loader builds
  `argc/argv/envp/auxv` and passes `$a3 = auxv`. The probe checks `AT_PAGESZ`,
  `AT_ENTRY`, `AT_RANDOM`, `AT_CLKTCK`, `AT_SECURE`, `AT_EXECFN` and
  `AT_PLATFORM`.
- **A main-thread TLS convention without `rdhwr`.** `_start` builds the 16-byte
  TCB at `main_tcb` and sets `$28`; `__errno_location` is `TP + 8`, and `$28`
  survives a syscall.
- **The existing user-image path.** No new loader: the image is a 256 KiB raw
  SCSI target-2 file, exactly like `user/hello` and `std-hello`.

## Reproduction

```sh
# Kernel, ROM and module (from the repository root)
nix develop --command bash -lc '
  export PATH=/nix/store/j1938gsbvbhkyz4kbmvjk56r479mkw3s-gcc-wrapper-16.2.0/bin:$PATH
  tools/m2-rig/build.sh /tmp/opencode/upal/rig'

# C probe
CLANG=/nix/store/jmgw1yz3xm3bpgrk9r84jkcynskdbd40-clang-21.1.8/bin/clang \
LLD=$HOME/.rustup/toolchains/nightly-2026-10-03-x86_64-unknown-linux-gnu/lib/rustlib/x86_64-unknown-linux-gnu/bin/rust-lld \
  user/c-probe/build.sh /tmp/opencode/upal/cprobe
```

The iris config attaches `cprobe/probe.raw` as SCSI target 2:

```toml
banks = [64, 0, 0, 0]
no_audio = true

[machine]
profile = "indy_ip24"

[scsi]

[scsi.2]
path = "/tmp/opencode/upal/probe.raw"
cdrom = false
```

```sh
IRIS_NO_IDLE=1 iris --config probe.toml --ci --serial-log probe-serial.log \
  --monitor-port 18892 --ci-socket /tmp/opencode/upal/probe.sock \
  --prom /tmp/opencode/upal/rig/irix7-rom.bin \
  --nvram /tmp/opencode/upal/probe-nvram.bin
iris-ci --socket /tmp/opencode/upal/probe.sock status --wait-ready 30
iris-ci --socket /tmp/opencode/upal/probe.sock start
```

`--headless` is not used: on iris 0.1.13 it leaves the REX3 initialisation in a
GIO-timeout loop on this kernel. The probe takes about two minutes of guest time
because every console fragment flushes the graphics console.

## Limits

- This probe is freestanding C, not linked against relibc.  The separate
  [`relibc-probe`](../../user/relibc-probe/probe.c) is the relibc-linked
  counterpart; see [`relibc-pal-bringup.md`](relibc-pal-bringup.md).
- The native gate is the v0 flat form in
  [`../spec/native-abi.md`](../programming/native-abi.md) §5.8. The capability
  invocation encoding is not implemented; the gate will be replaced, not
  versioned alongside it.
- `fcntl` and `signal` are no-op successes (`CELL_OK`); the bring-up has no
  per-fd flags and no signal machinery.  `mmap` is a bump allocator over a
  fixed 1 MiB window; `munmap`/`mprotect` are metadata-only; `readdir` is the
  one-entry bring-up call, not `getdents`.
- The static Rust `std` shim now uses the same native gate and status map; the
  separate `user/hello` loader smoke image still uses the plain bring-up
  `syscall` path.
- No threads, signals, processes, sockets or termios. The kernel remains
  single-threaded.

## relibc-side change now landed

The relibc-linked PAL has booted; see
[`relibc-pal-bringup.md`](relibc-pal-bringup.md). The fork branch
`irix7-pal` at `d484536c49fed26a5377d2c511962b891e1c8dfb` now:

1. Uses `syscall 0x1F7`, reads the status from `$v0` and the value from `$v1`,
   and maps through the shared status table to `Err(Errno(...))`.
2. Deletes `KernelStat`/`copy_stat` and passes the 136-byte n32 `struct stat`
   directly.
3. Sets `$28` to a TCB with `self` at 0, `dtv` 0 at 4, `errno` at 8 and
   libc-private at 12; `__errno_location` returns `TP+8`.
4. Uses the System V stack and `$a3 = auxv`.
5. Keeps `getdents` synthesised over the one-entry `readdir` call.

The status table is
[`crates/native-abi/src/status.rs`](../../crates/native-abi/src/status.rs); the
C probe's copy is [`user/c-probe/abi.h`](../../user/c-probe/abi.h).
