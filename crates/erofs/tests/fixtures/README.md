# EROFS test fixtures

These are golden images generated with **erofs-utils 1.9.4** and checked with
`fsck.erofs`.  They are committed so the host tests do not need `mkfs.erofs` in CI.

| Image | Shape |
|---|---|
| `root.erofs` | Compact inodes, flat inline data, 4 KiB blocks |
| `extended.erofs` | The same tree with `-Eforce-inode-extended` (64-byte inodes) |
| `48bit.erofs` | The same tree with `-E48bit` (the 48-bit feature, small root nid) |

The source tree is:

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

The images are reproducible: `build.sh` creates the tree and runs:

```sh
mkfs.erofs -T 1700000000 --all-time --all-root -U clear -b 4096 \
    root.erofs "$src"
mkfs.erofs ... -Eforce-inode-extended extended.erofs "$src"
mkfs.erofs ... -E48bit 48bit.erofs "$src"
```

Run `./build.sh` with `mkfs.erofs` and `fsck.erofs` in `PATH` (for example
`nix shell nixpkgs#erofs-utils`).  The committed images have these SHA-256
digests:

```text
9e3fd4b55c042224b9d31631c2099909c6eabdeadbcbf74b5116320071f0f522  root.erofs
d6ae6b9811b3af2f43e795909b63794393f92e784c96c4dcff4df53b4eea8d75  extended.erofs
68a199a2eb2f5cf0c45be0aec4c7d5f4caec11752d561268cef63a73df2973f1  48bit.erofs
```
