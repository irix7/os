# EROFS module

A loadable module that mounts the EROFS fixture image on **SCSI target 2 at
LBA 0**, reads `/etc/motd`, and prints it through the console service.  It
links `crates/erofs` and `crates/block-cache` by path and reads sectors through
the ABI version 10 `block_read` service.

The image is the compact `crates/erofs/tests/fixtures/root.erofs` fixture
(SHA-256 `9e3fd4b55c042224b9d31631c2099909c6eabdeadbcbf74b5116320071f0f522`)
written to LBA 0 of target 2.  Without that disk the module reports a mount
failure and returns status 1.

## Build

The module is cross-built for n32 with `build-std` because it is `no_std`:

```bash
export CARGO_TARGET_MIPS3_SGI_IRIX7_N32_RUSTFLAGS="-Clink-arg=-Tmodules/erofs/linker.ld"
cargo build --release -Z build-std=core,alloc -Zjson-target-spec \
  --target targets/mips3-sgi-irix7-n32.json -p erofs-module

HOST="$(rustc -vV | sed -n 's/^host: //p')"
"$(rustc --print sysroot)/lib/rustlib/$HOST/bin/llvm-objcopy" -O binary \
  target/mips3-sgi-irix7-n32/release/erofs-module erofs.module
```

`llvm-objcopy -O binary` produces the flat image the loader expects: text at
`0x8840_0000`, no ELF header, and `.bss` omitted (the loader zeroes the module
area before the download).  The current image is 79,160 bytes with SHA-256
`c144d25e801befee8888f5b99506fe2ddf4550ca5cb211f9596e28bf30774c31`; check the
result stays under the 2 MiB module area before serving it.

## Load

Serve `erofs.module` over HTTPS with its SHA-256 and load it from the console:

```text
module https://<host>/erofs.module <sha256>
```

Expected output on a machine with the fixture disk attached:

```text
erofs: start
erofs: mounted target 2 (block size 4096)
erofs: /etc/motd (18 bytes)
Welcome to IRIX 7
overlay: /tmp/motd (10 bytes)
RAM upper
overlay: whiteout ok
erofs: done
module: returned 0
```
