// SPDX-License-Identifier: GPL-3.0-only
//! The read-only lower namespace handed to the overlay.

use alloc::string::String;
use alloc::vec::Vec;

use crate::Result;

/// The kind of node an entry names.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EntryKind {
    /// A regular file.
    File,
    /// A directory.
    Directory,
}

/// A directory entry in a [`Lower`] tree.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DirEntry {
    /// The entry name, which must be a single path component.
    pub name: String,
    /// Whether the entry is a file or a directory.
    pub kind: EntryKind,
}

/// A read-only lower namespace.
///
/// A caller implements this trait for the EROFS root (or another read-only
/// source).  The overlay only ever passes normalised absolute paths, and the
/// lower must resolve symbolic links itself: only regular files and
/// directories are part of this contract.
///
/// Both methods use `Option` to distinguish a missing (or wrong-kind) path
/// from a genuine lower failure:
///
/// * `Ok(None)` means the path is absent, or is not the requested kind;
/// * `Err(Error::Lower(_))` means the lower could not answer.
///
/// The trait is object-safe, and the blanket implementation for `&mut T`
/// lets an [`Overlay`](crate::Overlay) borrow a `&mut dyn Lower`.
pub trait Lower {
    /// Read a whole regular file from the lower tree.
    ///
    /// Returns `Ok(None)` when `path` does not exist or is not a regular file.
    fn read_file(&mut self, path: &str) -> Result<Option<Vec<u8>>>;

    /// List a directory's direct children.
    ///
    /// Returns `Ok(None)` when `path` does not exist or is not a directory.
    /// Entry names must be single components, not paths.
    fn read_dir(&mut self, path: &str) -> Result<Option<Vec<DirEntry>>>;
}

impl<T: Lower + ?Sized> Lower for &mut T {
    fn read_file(&mut self, path: &str) -> Result<Option<Vec<u8>>> {
        (**self).read_file(path)
    }

    fn read_dir(&mut self, path: &str) -> Result<Option<Vec<DirEntry>>> {
        (**self).read_dir(path)
    }
}
