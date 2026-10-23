// SPDX-License-Identifier: GPL-3.0-only
//! A read-only EROFS reader for IRIX 7.
//!
//! The crate is `no_std` and uses `alloc`.  It decodes the standard
//! little-endian EROFS on-disk format on a big-endian machine through an
//! explicit [`ByteOrder`] codec; there are no native-endian loads.
//!
//! The supported profile is deliberately small:
//!
//! - compact and extended inodes;
//! - flat plain and flat inline data;
//! - directories and symbolic links;
//! - 48-bit block addresses for flat data;
//! - one device (the primary image).
//!
//! Compressed clusters, chunk-based files and metabox images are recognised and
//! rejected with a typed error.  They are the next layer, not a silent
//! misinterpretation.
//!
//! See [`docs/spec/erofs.md`](https://github.com/irix7/os/blob/main/docs/spec/erofs.md).

#![no_std]
#![deny(unsafe_code)]

extern crate alloc;

pub mod codec;
mod error;
mod fs;
mod inode;
mod superblock;

pub use codec::{BigEndian, ByteOrder, LittleEndian};
pub use error::{Error, Result};
pub use fs::{BlockDevice, DirEntry, Erofs};
pub use inode::{DataLayout, FileType, Inode, EROFS_NULL_ADDR};
pub use superblock::{Superblock, MAGIC, SUPER_BASE_SIZE, SUPER_OFFSET};
