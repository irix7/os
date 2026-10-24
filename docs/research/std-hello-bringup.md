# First Rust `std` image on IRIX 7

Status: **bring-up result**. 2026-10-25.

## Result

A static Rust `std` binary now runs on IRIX 7 under iris. It prints through the
kernel console, reads its initial argument vector, and exits with status 0:

```
userelf: entry=0x10000360
user: entering user mode
hello from std
arg=hello
slept_ms=10
time=1792872464
file=hello from the IRIX 7 ramfs
write=ok
after_write=rewritten by std
chdir=ok
cwd=/etc
relative=Welcome to IRIX 7.
stat_size=17
entry=hello.txt
entry=etc
entry=dev
user: exit syscall code=0x00000000
kernel: userland returned to kernel console
```

The unstripped ELF has **no undefined symbols**. It is linked at
`0x10000000`, is a 32-bit big-endian MIPS ELF with n32 ABI flags, and is
stripped for the 256 KiB target-2 image.

This is the **S1 gate** from the Rust `std` research: it proves the target,
toolchain, libc ABI shim, loader and minimal syscall path. It is not a complete
POSIX environment.  The production C ABI layer is now the
[relibc fork](https://github.com/irix7/relibc); its n32 Linux/QEMU gate passes
([report](relibc-n32-bringup.md)).

## What was added

- `crates/irix7-libc`: a provisional 42-symbol C ABI shim for `std` bring-up.
  It implements the abort-mode hello symbol set: console write, a bump
  allocator, string/memory primitives, single-threaded pthread mutex/cond/key
  stubs, and `_Unwind_*` stubs. The libc strategy is to fork **relibc** later;
  this crate is a validation shim, not the final libc.
- `user/std-hello`: the first static `std` probe.
- Kernel syscall cases for `open`, `close`, `fcntl`, `dup`, `getcwd`, `poll`,
  `signal`, `getentropy`, `clock_gettime`, `getpid`, `read`, `fstat` and `stat`.
- A tiny bring-up fd table (`kernel/src/fd.rs`) for console, `/dev/null`, two
  ramfs files (`/hello.txt`, `/etc/motd`), directory iteration and `chdir`.
- A 256 KiB loader image, a four-page user stack and 64 KiB SCSI read chunks.
- A fixed 1 MiB heap backed by two 256 KiB page pairs, so std's allocator does
  not exhaust the 48-entry JTLB with 4 KiB mappings.
- The kernel stack top moved from `0x88200000` to `0x88300000`.  The old top
  left too little gap above `.bss`; deep boot stacks could overwrite the
  loader's pair counter and cause an intermittent out-of-bounds cleanup panic.
- `clock_gettime`: `CLOCK_REALTIME` from the RTC/NTP wall clock and
  `CLOCK_MONOTONIC` from CP0 Count.
- `nanosleep` as a CP0 Count busy-wait inside the syscall, enough for
  `std::thread::sleep`.
- `std::fs::metadata`, `std::fs::read_dir`, `std::fs::read` and
  `std::fs::write`.
- `chdir`/`getcwd` and relative path lookup in the tiny namespace.
- NUL-terminated initial `argv`/`envp` strings. Without the terminators,
  `std::env::args()` walked off the top of the user stack.
- `has-thread-local: false` on the native target, keeping pthread-key TLS.

## Reproduction outline

The `std` source and libc declarations come from the patched spike tree
prepared by `tools/std-spike/build-std.sh`. The image is built with:

```
cargo build --release -Zbuild-std=std,panic_abort -Zbuild-std-features= \
  -Zjson-target-spec \
  --target targets/mips3-sgi-irix7-native-n32.json
```

with `-Clink-arg=-Tuser/hello/linker.ld`, `-e__start`, `--gc-sections`, and
empty `libc.a`/`libm.a`/`librt.a`/`libpthread.a` archives so rustc's default
Unix `-l` flags resolve to our shim. The image is stripped, padded to 256 KiB
and attached at SCSI target 2.

## Limitations

- Single-threaded only; `pthread_create` is not implemented.
- A small two-file ramfs with a root directory and mutable file data; no
  general filesystem or process creation.
- `getentropy` returns zeroes (fine for the smoke test, not a secure RNG).
- Signals are no-ops; no termios; no sockets.
- `fork` is not present; the process model will be `posix_spawn`.
- The real libc will be a **relibc fork** with an `irix7` backend, a 64-bit
  `time_t`/`off_t` n32 ABI, and the MIPS n32 CRT/TLS/pthread port.

## Next gates

1. ~~`std::fs` over a ramfs and `std::time` with real clocks.~~ (bring-up done)
2. `std::thread` with native threads/TLS.
3. `std::process::Command` over `posix_spawn`.
4. termios + poll + Reedline.
5. Brush, then a reduced Nushell on 128 MB.
