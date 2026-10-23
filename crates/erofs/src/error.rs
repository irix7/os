// SPDX-License-Identifier: GPL-3.0-only
//! Errors returned by the EROFS reader.

use core::fmt;

/// An EROFS reader error.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum Error {
    /// The block device could not satisfy a read.
    Io,
    /// A structure was shorter than its on-disk definition.
    Truncated,
    /// The superblock magic was not `0xE0F5E1E2`.
    BadMagic,
    /// An unknown incompatible feature bit was set.
    UnsupportedFeature(u32),
    /// An inode uses a data layout this reader does not implement yet.
    UnsupportedLayout(u8),
    /// The superblock was internally inconsistent.
    InvalidSuperblock(&'static str),
    /// An inode was internally inconsistent.
    InvalidInode(&'static str),
    /// A directory block was malformed.
    InvalidDirectory(&'static str),
    /// A path component did not exist.
    NotFound,
    /// The inode was not a directory.
    NotDirectory,
    /// The inode was not a regular file.
    NotRegularFile,
    /// The inode was not a symbolic link.
    NotSymlink,
    /// The requested range extended past the end of the file.
    OutOfBounds,
    /// A buffered read was larger than the reader's safety limit.
    TooLarge,
    /// A directory name was longer than the format allows.
    NameTooLong,
    /// The allocation for a file or directory failed.
    OutOfMemory,
    /// A path feature is not implemented.
    Unsupported(&'static str),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io => f.write_str("block device read failed"),
            Self::Truncated => f.write_str("on-disk structure is truncated"),
            Self::BadMagic => f.write_str("not an EROFS image"),
            Self::UnsupportedFeature(bits) => {
                write!(
                    f,
                    "unsupported EROFS incompatible feature bits 0x{bits:08x}"
                )
            }
            Self::UnsupportedLayout(layout) => {
                write!(f, "unsupported EROFS inode data layout {layout}")
            }
            Self::InvalidSuperblock(reason) => write!(f, "invalid EROFS superblock: {reason}"),
            Self::InvalidInode(reason) => write!(f, "invalid EROFS inode: {reason}"),
            Self::InvalidDirectory(reason) => write!(f, "invalid EROFS directory: {reason}"),
            Self::NotFound => f.write_str("path component not found"),
            Self::NotDirectory => f.write_str("inode is not a directory"),
            Self::NotRegularFile => f.write_str("inode is not a regular file"),
            Self::NotSymlink => f.write_str("inode is not a symbolic link"),
            Self::OutOfBounds => f.write_str("read past the end of the file"),
            Self::TooLarge => f.write_str("file is too large to buffer"),
            Self::NameTooLong => f.write_str("directory name is too long"),
            Self::OutOfMemory => f.write_str("out of memory"),
            Self::Unsupported(what) => write!(f, "unsupported: {what}"),
        }
    }
}

/// A result from the EROFS reader.
pub type Result<T> = core::result::Result<T, Error>;
