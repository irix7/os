// SPDX-License-Identifier: GPL-3.0-only
//! Errors returned by the overlay.

use core::fmt;

/// An overlay error.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum Error {
    /// A path did not start with `/`.
    NotAbsolute,
    /// A `..` component walked above the root.
    EscapesRoot,
    /// A path contained a component the overlay cannot represent.
    InvalidPath(&'static str),
    /// The path did not exist in the merged namespace.
    NotFound,
    /// The path named a directory, but a file was required.
    IsDirectory,
    /// The path named a file, but a directory was required.
    NotDirectory,
    /// A directory still had entries in the merged namespace.
    NotEmpty,
    /// A file was already in the way of a directory.
    AlreadyExists,
    /// An allocation for an upper entry failed.
    OutOfMemory,
    /// The lower filesystem failed to serve a request.
    Lower(&'static str),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotAbsolute => f.write_str("path is not absolute"),
            Self::EscapesRoot => f.write_str("path escapes the root"),
            Self::InvalidPath(reason) => write!(f, "invalid path: {reason}"),
            Self::NotFound => f.write_str("path not found"),
            Self::IsDirectory => f.write_str("path is a directory"),
            Self::NotDirectory => f.write_str("path is not a directory"),
            Self::NotEmpty => f.write_str("directory is not empty"),
            Self::AlreadyExists => f.write_str("file already exists"),
            Self::OutOfMemory => f.write_str("out of memory"),
            Self::Lower(reason) => write!(f, "lower filesystem error: {reason}"),
        }
    }
}

/// A result from the overlay.
pub type Result<T> = core::result::Result<T, Error>;
