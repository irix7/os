# Native user images

These are small **native IRIX 7** n32 user images.  They are not IRIX 6.5
compatibility binaries.  They use either the tiny `user/libc` Rust API, the
42-symbol C ABI shim in `crates/irix7-libc`, or the production
[relibc fork](https://github.com/irix7/relibc) `irix7-pal` branch.

## native libc v0

`user/libc` is a freestanding C ABI layer over the provisional syscalls.  It
provides `write`, `_exit`, anonymous `mmap`, a one-way `malloc`/`calloc`/`realloc`
family, the `mem*` and basic string primitives, and `__errno_location`.  It is
not the POSIX.1-2024 native libc yet.  The kernel now has a small fd table,
read/write ramfs, `stat` and directory iteration for the std bring-up image, but
there is still no process table, pthread/TLS, signal or capability layer.

## hello

`user/hello` is the ELF-loader smoke image.  It is linked at `0x1000_0000` and
uses the provisional kernel syscall codes:

| `$v0` | Arguments | Meaning |
|---|---|---|
| 1 | `$a0` fd, `$a1` buffer, `$a2` length | write; `$v0` returns the length or -1. The kernel copies from a recorded user range. |
| 2 | `$a0` status | exit; the kernel returns to the telnet console |
| 3 | `$a1` length (other arguments ignored) | anonymous `mmap`; returns zeroed heap pages at `0x2000_0000` or -1 |
| 4 | `$a0` address, `$a1` length | `munmap`; v0 returns 0 and the heap is one-way |
| 5 | `$a0` address, `$a1` length, `$a2` protection | `mprotect`; v0 returns 0 (pages are already read/write) |

The kernel also implements the provisional `read`, `open`, `close`, `fcntl`,
`dup`, `getcwd`, `chdir`, `poll`, `signal`, `getentropy`, `clock_gettime`,
`getpid`, `fstat`, `stat`, `readdir` and `nanosleep` calls used by the std image.
The full table is documented in [`../docs/programming/elf.md`](../docs/programming/elf.md).

The hello ELF is built at `-O1` because the n32 backend currently
miscompiles this libc write/exit flow at `-Oz` (the post-write continuation can
be replaced by a break).  The image remains under the loader's 256 KiB limit.

The loader builds the conventional initial stack: `argc` at `$sp`, followed by
the `argv` and `envp` pointer arrays, then the auxiliary-vector pairs.  The
strings `"hello"`, `"PATH=/"`, `"HOME=/"`, `"mips"` and a 16-byte `AT_RANDOM`
value sit above them.  `$a0`, `$a1`, `$a2` and `$a3` hold `argc`, `argv`, `envp`
and `auxv` on entry; `$28` is zero until the startup code sets the thread
pointer.
`write` sends bytes to the console and the console service mirrors them to
serial and telnet; fd 0/1/2 are console descriptors, and the ramfs/`/dev/null`
descriptors are available to the std image.  The std heap is a 1 MiB anonymous
window mapped with two 256 KiB page pairs; the loader frees the image and stack
pairs on exit.

Build it in the development shell:

```sh
nix develop

cd user/hello
CARGO_TARGET_DIR=/tmp/irix7-user-hello-target \
CARGO_TARGET_MIPS3_SGI_IRIX7_NATIVE_N32_RUSTFLAGS="-Clink-arg=-T$PWD/linker.ld" \
  cargo build --release -Zbuild-std=core -Zjson-target-spec \
  --target ../../targets/mips3-sgi-irix7-native-n32.json
```

The resulting ELF is `$CARGO_TARGET_DIR/mips3-sgi-irix7-native-n32/release/hello`.
`tools/build-user-hello.sh` does the same build and also creates a 256 KiB
raw SCSI image.
To run it on iris, place it at LBA 0 of a raw SCSI disk image and attach that
image as SCSI target 2.  The kernel enters it automatically after boot; the
telnet `userelf` command reloads the same image.

The kernel reads up to 256 KiB from target 2, so the raw image is padded to
256 KiB.  The image uses a tiny ramfs fixture, not a general filesystem.

## std bring-up image

`user/std-hello` is the first static Rust `std` image.  It exercises args, time,
sleep, a read/write ramfs, `stat`, `read_dir`, `chdir` and relative paths.  See
[`std-hello/README.md`](std-hello/README.md) and the
[bring-up report](../docs/research/std-hello-bringup.md).

## c-probe

`user/c-probe` is the freestanding n32 C probe for the native syscall ABI.  It
is not linked against relibc: it contains its own `_start`, syscall stubs and
the C oracle for the 136-byte `struct stat`, the 280-byte `struct dirent`, the
`CELL_E_*` status map, the auxiliary-vector keys and the 16-byte TCB header.
It calls `write`, `getpid`, `stat`, `fstat` and `exit` through `syscall 0x1F7`,
checks two deterministic negative results (`EBADF` and `ENOENT`), and exits 1
if any check fails.

Build and run it from the repository root:

```sh
CLANG=/path/to/clang user/c-probe/build.sh /tmp/c-probe
# attach /tmp/c-probe/probe.raw as SCSI target 2 and boot iris
```

The kernel enters target 2 automatically after `BOOT-READY`.  The expected
serial transcript is in
[`docs/research/c-probe-bringup.md`](../docs/research/c-probe-bringup.md).

## relibc probe

`user/relibc-probe` is the first C program linked against `librelibc.a` from the
`irix7-pal` branch.  It uses relibc's crt0, `write`, `stat`, `fstat`, `getpid`,
`getauxval` and `__errno_location`; it prints `IRIX7-RELIB-PROBE-1`, checks the
136-byte `struct stat`, `$28`/`TP+8`, the `$a3` auxiliary vector, `EBADF` and
`ENOENT`, and exits 0.  Build and run instructions are in
[`../tools/relibc-pal/README.md`](../tools/relibc-pal/README.md).
