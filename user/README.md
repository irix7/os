# Native user images

These are small **native IRIX 7** n32 user images.  They are not IRIX 6.5
compatibility binaries.  They use either the tiny `user/libc` Rust API or the
42-symbol C ABI shim in `crates/irix7-libc`; the production POSIX libc will be
the [relibc fork](https://github.com/irix7/relibc).

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
The full table is documented in [`../docs/spec/elf.md`](../docs/spec/elf.md).

The hello ELF is built at `-O1` because the n32 backend currently
miscompiles this libc write/exit flow at `-Oz` (the post-write continuation can
be replaced by a break).  The image remains under the loader's 256 KiB limit.

The loader builds the conventional initial stack: `argc` at `$sp`, followed by
the `argv` and `envp` pointer arrays, with `"hello"`, `"PATH=/"` and `"HOME=/"`
above them.  `$a0`, `$a1` and `$a2` hold `argc`, `argv` and `envp` on entry.
`write` sends bytes to the console and the console service mirrors them to
serial and telnet; fd 0/1/2 are console descriptors, and the ramfs/`/dev/null`
descriptors are available to the std image.  The std heap is a 1 MiB anonymous
window mapped with two 256 KiB page pairs; the loader frees the image and stack
pairs on exit.

Build it from the repository root:

```sh
export PATH=/mnt/europa/rust-toolchain/bin:/mnt/europa/rust-toolchain/cargo/bin:$PATH
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
