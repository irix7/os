// SPDX-License-Identifier: GPL-3.0-only
//! A read-only-lower, RAM-upper overlay for IRIX 7.
//!
//! The overlay joins a caller-supplied [`Lower`] tree, normally the read-only
//! EROFS root, with an in-memory upper tree of files and directories.  It is
//! the groundwork for writable `/tmp`, `/etc` and `/home` mounts on the M4
//! root image.
//!
//! The lower tree is never modified.  There is therefore no copy-up: a write
//! to any path stores a complete new file in the upper tree, and the lower
//! bytes are only consulted when no upper file shadows them.  Removing a file
//! records a whiteout so a lower file cannot reappear through the merged view.
//!
//! Paths are normalised absolute paths: a leading `/` is required, duplicate
//! slashes and `.` components collapse, and a `..` component that walks above
//! the root is rejected with [`Error::EscapesRoot`].
//!
//! The crate is `no_std` + `alloc`, has no unsafe code and no external
//! dependencies.  It is an in-memory namespace, not a VFS: permissions,
//! timestamps, symbolic links and renaming are outside its contract.
//!
//! See [`docs/spec/overlay.md`](https://github.com/irix7/os/blob/main/docs/spec/overlay.md).

#![no_std]
#![deny(unsafe_code)]

extern crate alloc;

mod error;
mod lower;
mod overlay;
mod path;

pub use error::{Error, Result};
pub use lower::{DirEntry, EntryKind, Lower};
pub use overlay::{Overlay, Stat};
pub use path::normalise;
