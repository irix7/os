# Overlay contract

**Status:** draft
**Version:** 0.1
**Owner:** storage
**Implementation:** [`crates/overlay`](../../crates/overlay)

## 1. Scope

This document fixes the contract for the read-only-lower, RAM-upper overlay that
will provide writable `/tmp`, `/etc` and `/home` mounts on the M4 EROFS root.

The overlay is **in memory only**.  Upper files, upper directories and whiteouts
live in `BTreeMap`/`BTreeSet` structures owned by the `Overlay`, and they are
lost when it is dropped.  There is no write-back, no journal and no persistence.
A persistent upper layer is later work.

It is not a VFS.  Inodes, permissions, ownership, timestamps, extended
attributes, symbolic links, hard links, rename and truncate are outside this
contract.  Only regular files and directories are modelled.

## 2. Lower contract

The caller implements `Lower` for the EROFS root:

```rust
pub trait Lower {
    fn read_file(&mut self, path: &str) -> Result<Option<Vec<u8>>>;
    fn read_dir(&mut self, path: &str) -> Result<Option<Vec<DirEntry>>>;
}
```

Both methods receive normalised absolute paths.  `Ok(None)` means the path is
absent or is not the requested kind; `Err(Error::Lower(_))` means the lower
could not answer.  The lower must resolve symbolic links itself and must return
single-component names from `read_dir`; the overlay rejects other names with
`Error::Lower("invalid directory entry name")`.

The trait is object-safe.  A blanket implementation for `&mut T` lets an
`Overlay` borrow a `&mut dyn Lower` when the caller wants dynamic dispatch.

## 3. Upper tree and whiteouts

The upper tree has three parts:

* `files`: full file contents keyed by normalised path.  A write always stores
  the complete file.
* `dirs`: directories created with `create_dir`.
* `whiteouts`: paths removed from the merged namespace.

`write_file` requires the parent directory to exist in the merged namespace and
rejects a visible directory at the target path with `Error::IsDirectory`.  A
whiteout at the target is cleared, so writing after `remove_file` replaces the
lower file with new upper contents.

`remove_file` removes the upper file, if any, and records a whiteout.  The
whiteout hides the lower file and any later lookup of the path returns
`Error::NotFound`.

`remove_dir` requires the directory to be empty in the merged view.  It records
a whiteout for the directory, which hides the lower directory and its whole
subtree.  Recreating the directory with `create_dir` clears only the whiteout at
that path; whiteouts for its former children remain, so removed lower children
do not reappear.

A whiteout at any ancestor hides a path.  An upper file at an ancestor makes
every path below it invisible as well.

## 4. Path normalisation

`normalise` is the single path parser used by every operation:

* a leading `/` is required, otherwise `Error::NotAbsolute`;
* duplicate slashes, trailing slashes and `.` components are removed;
* `..` removes the preceding component;
* a `..` with no preceding component walks above the root and returns
  `Error::EscapesRoot`;
* an embedded NUL returns `Error::InvalidPath`;
* the root normalises to `/`.

Thus `//tmp///a/./b/` becomes `/tmp/a/b`, `/a/../b` becomes `/b`, and `/..` is
rejected.

## 5. Lookup and merge

Lookup checks, in order:

1. a whiteout at the path or an ancestor;
2. an upper file at an ancestor;
3. an upper file at the path;
4. an upper directory at the path, or the root;
5. the lower tree, by listing the parent directory.

`read_dir` starts with the direct upper children, then adds lower children whose
names are not shadowed by an upper file or directory and not hidden by a
whiteout at the child path.  The result is sorted by name, so callers see a
deterministic order independent of the lower's listing order.

## 6. Copy-up

There is no copy-up.  The lower is read-only, so the overlay never needs to
preserve lower metadata in the upper tree: writing a path simply stores the full
bytes in RAM and shadows the lower file.  Reading a path that has no upper file
streams through `Lower::read_file` and does not cache the result.

This is acceptable for `/tmp`, small `/etc` files and per-user `/home` state.
Large writes consume RAM proportional to their size; a block-backed upper layer
is future work.

## 7. Errors

| Error | Meaning |
|---|---|
| `NotAbsolute` | The path did not start with `/` |
| `EscapesRoot` | A `..` component walked above the root |
| `InvalidPath` | A path contained an unrepresentable component |
| `NotFound` | The path is absent in the merged namespace |
| `IsDirectory` | A file operation found a directory |
| `NotDirectory` | A directory operation found a file |
| `NotEmpty` | `remove_dir` found visible children |
| `AlreadyExists` | `create_dir` found a file at the path |
| `OutOfMemory` | A fallible upper-tree allocation failed |
| `Lower` | The lower filesystem failed to answer |

## 8. Tests

`crates/overlay/tests/overlay.rs` implements `MemoryLower` and covers lower
reads, upper shadowing, whiteouts, directory merging and removal, path
normalisation, `..` rejection, missing paths, empty files and object-safe lower
implementations.  The tests run on the host with `cargo test -p overlay`.
