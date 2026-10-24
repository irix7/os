# Native user images

These are small **native IRIX 7** n32 user images.  They are not IRIX 6.5
compatibility binaries and they do not use a libc yet.

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

The loader builds the conventional initial stack: `argc` at `$sp`, followed by
the `argv` and `envp` pointer arrays, with `"hello"`, `"PATH=/"` and `"HOME=/"`
above them.  `$a0`, `$a1` and `$a2` hold `argc`, `argv` and `envp` on entry.
`write` currently sends bytes to the serial console (visible through MCP
`serial.read`); there is no fd table, only fd 1 and 2 are accepted, and the
buffer is capped at 256 bytes.  The provisional heap is a contiguous 128 KiB
anonymous range; the loader frees its page pairs on exit.

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
`tools/build-user-hello.sh` does the same build and also creates an 80 KiB
raw SCSI image.
To run it on iris, place it at LBA 0 of a raw SCSI disk image and attach that
image as SCSI target 2, then use the telnet `userelf` command.

The kernel reads up to 80 KiB from target 2, so the raw image can be padded to
80 KiB; the loader does not use a filesystem yet.
