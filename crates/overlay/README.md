# overlay

A `no_std` + `alloc` overlay with a read-only lower tree and an in-memory upper
tree.

The overlay is groundwork for writable `/tmp`, `/etc` and `/home` mounts on the
M4 EROFS root.  A caller implements `Lower` for the EROFS reader; the overlay
adds upper files, upper directories and whiteouts, and merges the two views.

```rust
use overlay::{Lower, Overlay};

let mut fs = Overlay::new(erofs_lower);
fs.create_dir("/tmp")?;
fs.write_file("/tmp/hello", b"hello")?;
assert_eq!(fs.read_file("/tmp/hello")?, b"hello");
fs.remove_file("/tmp/hello")?;
assert!(!fs.exists("/tmp/hello")?);
```

The lower tree is never modified, so there is no copy-up: a write stores the
complete file in RAM.  Removing a file records a whiteout, and removing a
directory records a whiteout for the whole lower subtree.  Nothing is persisted;
the upper tree disappears when the `Overlay` is dropped.

Paths must be absolute.  Normalisation collapses duplicate slashes and `.`,
applies `..` within the root, and rejects `..` that would escape the root.

The API is deliberately small:

| Operation | Purpose |
|---|---|
| `read_file` | Upper file first, then the lower tree |
| `write_file` | Store a complete upper file |
| `remove_file` | Remove an upper file and whiteout the lower path |
| `create_dir` | Create an upper directory |
| `remove_dir` | Remove an empty directory and whiteout the lower path |
| `read_dir` / `list` | Merge upper and lower directory entries |
| `exists` / `is_dir` / `is_file` | Typed path lookup |
| `stat` | Report the node kind |

It is not a VFS: there are no inodes, permissions, timestamps, symbolic links or
rename.  See [`docs/spec/overlay.md`](../../docs/spec/overlay.md) for the
contract.

The nineteen host tests cover lower reads, upper shadowing, whiteouts, directory
merging and removal, path normalisation, `..` rejection, missing paths, empty
files and object-safe lower implementations.
