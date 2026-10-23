# EROFS test fixtures

These are golden images generated with **erofs-utils 1.9.4** and checked with
`fsck.erofs`.  They are committed so the host tests do not need `mkfs.erofs` in CI.

| Image | Shape |
|---|---|
| `root.erofs` | Compact inodes, flat inline data, 4 KiB blocks |
| `extended.erofs` | The same tree with `-Eforce-inode-extended` (64-byte inodes) |
| `48bit.erofs` | The same tree with `-E48bit` (the 48-bit feature, small root nid) |
| `lz4.erofs` | The same tree plus `many.bin`, LZ4 with compact indexes and `-Eztailpacking` |
| `lz4-full.erofs` | The same compressed tree with `-Elegacy-compress` (8-byte full indexes) |
| `lz4-big.erofs` | The same compressed tree with `-C 16384` (16 KiB big pclusters) |

The uncompressed source tree is:

```text
/etc/motd          "Welcome to IRIX 7\n"
/hello.txt         "hello, erofs\n"
/big.bin           9000 bytes, byte i = (i * 7) mod 256
/empty             empty regular file
/dir/nested.txt    "nested\n"
/link              symlink to hello.txt
```

`big.bin` is deliberately larger than two blocks: its first 8192 bytes are flat
blocks and its final 808 bytes are an inline tail, so the reader exercises both
paths and a read that crosses the boundary.

The three compressed images use a copy of that tree with one extra file:

```text
/many.bin          153400 bytes, 2600 lines of
                   "line NNNNNNNN: the quick brown fox jumps over the lazy dog\n"
```

Its 38 logical clusters compress to three 4 KiB pclusters plus an inline tail
pcluster in `lz4.erofs` and `lz4-full.erofs`, so the tests cross pcluster and
extent boundaries.  `lz4-big.erofs` stores the whole file in one 16 KiB
pcluster that decodes to 153400 bytes; it exercises the `CBLKCNT` chain and the
`compr_cfgs` superblock slot.

The images are reproducible: `build.sh` creates the trees and runs:

```sh
mkfs.erofs -T 1700000000 --all-time --all-root -U clear -b 4096 \
    root.erofs "$src"
mkfs.erofs ... -Eforce-inode-extended extended.erofs "$src"
mkfs.erofs ... -E48bit 48bit.erofs "$src"

mkfs.erofs ... -zlz4 -Eztailpacking lz4.erofs "$zsrc"
mkfs.erofs ... -zlz4 -Elegacy-compress -Eztailpacking lz4-full.erofs "$zsrc"
mkfs.erofs ... -zlz4 -C 16384 lz4-big.erofs "$zsrc"
```

`-Elegacy-compress` is obsolete for production images, but it is the only way
to ask `mkfs.erofs` for the full 8-byte index layout, so it is kept here to
test that decoding path.

Run `./build.sh` with `mkfs.erofs` and `fsck.erofs` in `PATH` (for example
`nix shell nixpkgs#erofs-utils`).  The committed images have these SHA-256
digests:

```text
9e3fd4b55c042224b9d31631c2099909c6eabdeadbcbf74b5116320071f0f522  root.erofs
d6ae6b9811b3af2f43e795909b63794393f92e784c96c4dcff4df53b4eea8d75  extended.erofs
68a199a2eb2f5cf0c45be0aec4c7d5f4caec11752d561268cef63a73df2973f1  48bit.erofs
0a4939cf8602a3296d750c979f5c991714ee14499bc7008129d58c8e00417def  lz4.erofs
4570ef680e6575e35be98f8587b6e32f4c8e03fd87ea69e57c746282e4725318  lz4-full.erofs
efa7dae97a2f49324b7b3bcd48e62d31c93b39fa6b185a95f591465b33f295f5  lz4-big.erofs
```
