// SPDX-License-Identifier: GPL-3.0-only
//! EROFS inodes.

use crate::codec::{u16_at, u32_at, u64_at, ByteOrder};
use crate::error::{Error, Result};
use crate::superblock::Superblock;

/// On-disk inode slot size.  Node IDs are indices in units of this size.
pub const INODE_SLOT_SIZE: u64 = 32;
/// Size of a compact inode.
pub const INODE_COMPACT_SIZE: usize = 32;
/// Size of an extended inode.
pub const INODE_EXTENDED_SIZE: usize = 64;
/// Block address meaning "no full data blocks".
pub const EROFS_NULL_ADDR: u64 = 0xffff_ffff;

/// Inode data layout (`i_format` bits 1..3).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DataLayout {
    /// Uncompressed data, no inline tail.
    Plain,
    /// Compressed data with non-compact indexes.
    CompressedFull,
    /// Uncompressed data with an inline tail.
    Inline,
    /// Compressed data with compact indexes.
    CompressedCompact,
    /// Chunk-based data.
    ChunkBased,
    /// A layout value this reader does not know.
    Unknown(u8),
}

impl DataLayout {
    fn from_bits(bits: u8) -> Self {
        match bits {
            0 => Self::Plain,
            1 => Self::CompressedFull,
            2 => Self::Inline,
            3 => Self::CompressedCompact,
            4 => Self::ChunkBased,
            other => Self::Unknown(other),
        }
    }

    /// Whether the layout stores uncompressed data in plain blocks.
    #[must_use]
    pub fn is_flat(self) -> bool {
        matches!(self, Self::Plain | Self::Inline)
    }

    /// Whether the layout uses compressed physical clusters.
    #[must_use]
    pub fn is_compressed(self) -> bool {
        matches!(self, Self::CompressedFull | Self::CompressedCompact)
    }

    /// The raw `i_format` datalayout value.
    #[must_use]
    pub fn raw_bits(self) -> u8 {
        match self {
            Self::Plain => 0,
            Self::CompressedFull => 1,
            Self::Inline => 2,
            Self::CompressedCompact => 3,
            Self::ChunkBased => 4,
            Self::Unknown(bits) => bits,
        }
    }
}

/// The type of a directory entry or inode.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FileType {
    /// Unknown file type.
    Unknown,
    /// Regular file.
    Regular,
    /// Directory.
    Directory,
    /// Character device.
    CharDevice,
    /// Block device.
    BlockDevice,
    /// FIFO.
    Fifo,
    /// Socket.
    Socket,
    /// Symbolic link.
    Symlink,
}

impl FileType {
    pub(crate) fn from_erofs(value: u8) -> Self {
        match value {
            1 => Self::Regular,
            2 => Self::Directory,
            3 => Self::CharDevice,
            4 => Self::BlockDevice,
            5 => Self::Fifo,
            6 => Self::Socket,
            7 => Self::Symlink,
            _ => Self::Unknown,
        }
    }

    fn from_mode(mode: u16) -> Self {
        match mode & 0xf000 {
            0x8000 => Self::Regular,
            0x4000 => Self::Directory,
            0x2000 => Self::CharDevice,
            0x6000 => Self::BlockDevice,
            0x1000 => Self::Fifo,
            0xc000 => Self::Socket,
            0xa000 => Self::Symlink,
            _ => Self::Unknown,
        }
    }
}

/// A decoded EROFS inode.
#[derive(Clone, Debug)]
pub struct Inode {
    /// Node ID.
    pub nid: u64,
    /// Raw `i_format` field.
    pub format: u16,
    /// Inline xattr slot count.
    pub xattr_icount: u16,
    /// POSIX mode.
    pub mode: u16,
    /// Hard link count.
    pub nlink: u32,
    /// File size in bytes.
    pub size: u64,
    /// Owner UID.
    pub uid: u32,
    /// Owner GID.
    pub gid: u32,
    /// Modification time in seconds since the Unix epoch.
    pub mtime: u64,
    /// Modification time nanoseconds.
    pub mtime_nsec: u32,
    /// Data layout.
    pub layout: DataLayout,
    /// First data block for flat layouts; compressed block count for
    /// compressed layouts.
    pub start_blk: u64,
    /// Inode size: 32 for compact, 64 for extended.
    pub inode_size: u8,
    /// Inline xattr size in bytes.
    pub xattr_size: u16,
    /// Offset of the inline tail in the image.
    pub inline_offset: u64,
    /// File type derived from `mode`.
    pub file_type: FileType,
}

impl Inode {
    /// Decode an inode from its raw on-disk bytes.
    ///
    /// `raw` must contain `inode_size` bytes starting at the inode location;
    /// `nid` and the superblock are used to derive the inline-data offset.
    pub fn parse<O: ByteOrder>(raw: &[u8], nid: u64, sb: &Superblock) -> Result<Self> {
        if raw.len() < INODE_COMPACT_SIZE {
            return Err(Error::Truncated);
        }

        let format = u16_at::<O>(raw, 0).ok_or(Error::Truncated)?;
        let inode_size = if format & 1 == 0 {
            INODE_COMPACT_SIZE
        } else {
            INODE_EXTENDED_SIZE
        };
        if raw.len() < inode_size {
            return Err(Error::Truncated);
        }

        let xattr_icount = u16_at::<O>(raw, 2).ok_or(Error::Truncated)?;
        let xattr_size = if xattr_icount == 0 {
            0u16
        } else {
            let slots = xattr_icount
                .checked_sub(1)
                .ok_or(Error::InvalidInode("xattr count"))?;
            12u16
                .checked_add(
                    slots
                        .checked_mul(4)
                        .ok_or(Error::InvalidInode("xattr size"))?,
                )
                .ok_or(Error::InvalidInode("xattr size"))?
        };

        let mode = u16_at::<O>(raw, 4).ok_or(Error::Truncated)?;
        let file_type = FileType::from_mode(mode);
        let is_dir = file_type == FileType::Directory;
        let layout = DataLayout::from_bits(((format >> 1) & 0x7) as u8);

        let i_nb = u16_at::<O>(raw, 6).ok_or(Error::Truncated)?;
        let (size, mtime, mtime_nsec, uid, gid, nlink, start_lo) = if inode_size == 32 {
            (
                u32_at::<O>(raw, 8).ok_or(Error::Truncated)? as u64,
                sb.epoch
                    .saturating_add(u32_at::<O>(raw, 12).ok_or(Error::Truncated)? as u64),
                0,
                u16_at::<O>(raw, 24).ok_or(Error::Truncated)? as u32,
                u16_at::<O>(raw, 26).ok_or(Error::Truncated)? as u32,
                if !is_dir && format & 0x10 != 0 {
                    1
                } else {
                    u32::from(i_nb)
                },
                u32_at::<O>(raw, 16).ok_or(Error::Truncated)? as u64,
            )
        } else {
            (
                u64_at::<O>(raw, 8).ok_or(Error::Truncated)?,
                u64_at::<O>(raw, 32).ok_or(Error::Truncated)?,
                u32_at::<O>(raw, 40).ok_or(Error::Truncated)?,
                u32_at::<O>(raw, 24).ok_or(Error::Truncated)?,
                u32_at::<O>(raw, 28).ok_or(Error::Truncated)?,
                u32_at::<O>(raw, 44).ok_or(Error::Truncated)?,
                u32_at::<O>(raw, 16).ok_or(Error::Truncated)? as u64,
            )
        };

        let mut start_blk = start_lo;
        let has_high = sb.has_48bit()
            && (inode_size == INODE_EXTENDED_SIZE || (!is_dir && format & 0x10 != 0));
        if has_high {
            start_blk |= u64::from(i_nb) << 32;
        }

        let blocksize = u64::from(sb.block_size());
        let meta_start = u64::from(sb.meta_blkaddr)
            .checked_mul(blocksize)
            .ok_or(Error::InvalidInode("metadata offset"))?;
        let slot = nid
            .checked_mul(INODE_SLOT_SIZE)
            .ok_or(Error::InvalidInode("inode offset"))?;
        let inline_offset = meta_start
            .checked_add(slot)
            .and_then(|offset| offset.checked_add(inode_size as u64))
            .and_then(|offset| offset.checked_add(u64::from(xattr_size)))
            .ok_or(Error::InvalidInode("inline offset"))?;

        Ok(Self {
            nid,
            format,
            xattr_icount,
            mode,
            nlink,
            size,
            uid,
            gid,
            mtime,
            mtime_nsec,
            layout,
            start_blk,
            inode_size: inode_size as u8,
            xattr_size,
            inline_offset,
            file_type,
        })
    }

    /// Whether this is a directory.
    #[must_use]
    pub fn is_dir(&self) -> bool {
        self.file_type == FileType::Directory
    }

    /// Whether this is a regular file.
    #[must_use]
    pub fn is_regular(&self) -> bool {
        self.file_type == FileType::Regular
    }

    /// Whether this is a symbolic link.
    #[must_use]
    pub fn is_symlink(&self) -> bool {
        self.file_type == FileType::Symlink
    }
}
