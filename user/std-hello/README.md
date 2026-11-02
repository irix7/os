# `std` bring-up image

This is the first static Rust `std` program for the native n32 target.  It is a
bring-up probe, not a general-purpose userland.

It needs the patched `rust-src` and libc tree prepared by
`tools/std-spike/build-std.sh`, because upstream Rust has no n32 target and no
IRIX 7 `libc` module. From the repository root:

```sh
nix develop
tools/std-spike/build-std.sh
tools/build-std-hello.sh /tmp/irix7-std-hello
```

`build-std.sh` copies the pinned `rust-src` to `target/std-spike/` and applies
the patches under `tools/std-spike/patches/`; `build-std-hello.sh` links the
probe against the resulting rlib and writes a 256 KiB SCSI image. `STD_SPIKE_WORK`
overrides the scratch root for both scripts.

The link needs empty `libc.a`, `libm.a`, `librt.a` and `libpthread.a` archives;
`build-std-hello.sh` creates them in `syslib/`. The actual C symbols come from
`crates/irix7-libc`. Attach the raw image at SCSI target 2; the kernel enters it
automatically after boot.

The provisional libc shim is deliberately single-threaded.  The kernel provides
console descriptors, `/dev/null` and a small read/write ramfs; it has no process
table, pthread/TLS, signals or termios.  The production POSIX layer will be a
relibc fork; see
[`docs/spec/userland-services.md`](../../docs/spec/userland-services.md) and
[`docs/research/std-hello-bringup.md`](../../docs/research/std-hello-bringup.md).
