# `std` bring-up image

This is the first static Rust `std` program for the native n32 target.  It is a
bring-up probe, not a general-purpose userland.

It needs the patched `rust-src` and libc tree prepared by
`tools/std-spike/build-std.sh`, because upstream Rust has no n32 target and no
IRIX 7 `libc` module.  Once that tree exists, from this directory:

```sh
export __CARGO_TESTS_ONLY_SRC_ROOT=/mnt/europa/rust-src-std-spike/library
export RUSTFLAGS='-Clink-arg=-T/mnt/europa/os/user/hello/linker.ld \
  -Clink-arg=-e__start -Clink-arg=--gc-sections \
  -Clink-arg=-L/mnt/europa/target-std-hello/syslib'
cargo build --release -Zbuild-std=std,panic_abort -Zbuild-std-features= \
  -Zjson-target-spec \
  --target ../../targets/mips3-sgi-irix7-native-n32.json
```

The link needs empty `libc.a`, `libm.a`, `librt.a` and `libpthread.a` archives
in the `syslib` directory; the actual C symbols come from
`crates/irix7-libc`.  Strip the ELF, pad it to 256 KiB, and attach it at SCSI
target 2; the kernel enters it automatically after boot.

The provisional libc shim is deliberately single-threaded.  The kernel provides
console descriptors, `/dev/null` and a small read/write ramfs; it has no process
table, pthread/TLS, signals or termios.  The production POSIX layer will be a
relibc fork; see
[`docs/spec/userland-services.md`](../../docs/spec/userland-services.md) and
[`docs/research/std-hello-bringup.md`](../../docs/research/std-hello-bringup.md).
