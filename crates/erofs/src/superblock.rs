// SPDX-License-Identifier: GPL-3.0-only
//! The EROFS superblock.

use crate::codec::{u16_at, u32_at, u64_at, ByteOrder, LittleEndian};
use crate::error::{Error, Result};

/// Offset of the superblock from the start of the image.
pub const SUPER_OFFSET: u64 = 1024;
/// Size of the base superblock.  Extended slots follow it.
pub const SUPER_BASE_SIZE: usize = 128;
/// EROFS magic number, little-endian on disk.
pub const MAGIC: u32 = 0xE0F5_E1E2;

/// `sb_csum`: the superblock carries a CRC32C.
pub const FEATURE_COMPAT_SB_CHKSUM: u32 = 0x0000_0001;
/// `mtime`: inodes carry modification times.
pub const FEATURE_COMPAT_MTIME: u32 = 0x0000_0002;
/// `xattr_filter`.
pub const FEATURE_COMPAT_XATTR_FILTER: u32 = 0x0000_0004;

/// `lz4_0padding`.
pub const FEATURE_INCOMPAT_LZ4_0PADDING: u32 = 0x0000_0001;
/// `compr_cfgs` / `big_pcluster`.
pub const FEATURE_INCOMPAT_COMPR_CFGS: u32 = 0x0000_0002;
/// `chunked_file`.
pub const FEATURE_INCOMPAT_CHUNKED_FILE: u32 = 0x0000_0004;
/// `device_table` / `compr_head2`.
pub const FEATURE_INCOMPAT_DEVICE_TABLE: u32 = 0x0000_0008;
/// `ztailpacking`.
pub const FEATURE_INCOMPAT_ZTAILPACKING: u32 = 0x0000_0010;
/// `fragments` / `dedupe`.
pub const FEATURE_INCOMPAT_FRAGMENTS: u32 = 0x0000_0020;
/// `xattr_prefixes`.
pub const FEATURE_INCOMPAT_XATTR_PREFIXES: u32 = 0x0000_0040;
/// 48-bit block addresses.
pub const FEATURE_INCOMPAT_48BIT: u32 = 0x0000_0080;
/// `metabox`.
pub const FEATURE_INCOMPAT_METABOX: u32 = 0x0000_0100;

/// Every incompatible feature bit defined by the current format.
pub const KNOWN_INCOMPAT: u32 = FEATURE_INCOMPAT_LZ4_0PADDING
    | FEATURE_INCOMPAT_COMPR_CFGS
    | FEATURE_INCOMPAT_CHUNKED_FILE
    | FEATURE_INCOMPAT_DEVICE_TABLE
    | FEATURE_INCOMPAT_ZTAILPACKING
    | FEATURE_INCOMPAT_FRAGMENTS
    | FEATURE_INCOMPAT_XATTR_PREFIXES
    | FEATURE_INCOMPAT_48BIT
    | FEATURE_INCOMPAT_METABOX;

/// An EROFS superblock, decoded into native fields.
#[derive(Clone, Debug)]
pub struct Superblock {
    /// Superblock checksum (CRC32C over the superblock with this field zeroed).
    pub checksum: u32,
    /// Compatible feature bits.  Unknown bits are safe to ignore.
    pub feature_compat: u32,
    /// Log2 of the filesystem block size.
    pub blkszbits: u8,
    /// Number of 16-byte extended superblock slots.
    pub sb_extslots: u8,
    /// Node ID of the root directory.
    pub root_nid: u64,
    /// Total inode count.
    pub inos: u64,
    /// Base seconds for compact inode modification times.
    pub epoch: u64,
    /// Fixed nanoseconds for compact inode modification times.
    pub fixed_nsec: u32,
    /// Time the image was built, as an offset from `epoch`.
    pub build_time: u32,
    /// Total filesystem blocks.
    pub blocks: u64,
    /// First block of the shared metadata area.
    pub meta_blkaddr: u32,
    /// First block of the shared xattr area.
    pub xattr_blkaddr: u32,
    /// Filesystem UUID.
    pub uuid: [u8; 16],
    /// Volume label, NUL-padded.
    pub volume_name: [u8; 16],
    /// Incompatible feature bits.
    pub feature_incompat: u32,
    /// LZ4 maximum distance (when compression is configured).
    pub lz4_max_distance: u16,
    /// Number of extra devices.
    pub extra_devices: u16,
    /// Offset of the device table, in device slots.
    pub devt_slotoff: u16,
    /// Log2 of the directory block size (0 means filesystem block size).
    pub dirblkbits: u8,
    /// Number of long xattr name prefixes.
    pub xattr_prefix_count: u8,
    /// Start of the long xattr name prefixes.
    pub xattr_prefix_start: u32,
    /// Node ID of the packed inode.
    pub packed_nid: u64,
    /// 48-bit root node ID.
    pub root_nid_8b: u64,
}

impl Superblock {
    /// Decode a little-endian superblock from the 128 bytes at image offset 1024.
    pub fn parse(bytes: &[u8]) -> Result<Self> {
        Self::parse_with::<LittleEndian>(bytes)
    }

    /// Decode a superblock using the supplied byte order.
    ///
    /// Standard EROFS images are little-endian.  The generic form exists so the
    /// decoder can be tested against a big-endian copy of the same fields.
    pub fn parse_with<O: ByteOrder>(bytes: &[u8]) -> Result<Self> {
        if bytes.len() < SUPER_BASE_SIZE {
            return Err(Error::Truncated);
        }

        let magic = u32_at::<O>(bytes, 0).ok_or(Error::Truncated)?;
        if magic != MAGIC {
            return Err(Error::BadMagic);
        }

        let checksum = u32_at::<O>(bytes, 4).ok_or(Error::Truncated)?;
        let feature_compat = u32_at::<O>(bytes, 8).ok_or(Error::Truncated)?;
        let blkszbits = bytes[12];
        let sb_extslots = bytes[13];
        if !(9..=16).contains(&blkszbits) {
            return Err(Error::InvalidSuperblock("invalid block size"));
        }
        let blocksize = 1u32 << blkszbits;
        if !(512..=65536).contains(&blocksize) {
            return Err(Error::InvalidSuperblock("block size out of range"));
        }

        let feature_incompat = u32_at::<O>(bytes, 80).ok_or(Error::Truncated)?;
        let unknown = feature_incompat & !KNOWN_INCOMPAT;
        if unknown != 0 {
            return Err(Error::UnsupportedFeature(unknown));
        }
        if feature_incompat & FEATURE_INCOMPAT_METABOX != 0 {
            return Err(Error::UnsupportedFeature(FEATURE_INCOMPAT_METABOX));
        }
        let extra_devices = u16_at::<O>(bytes, 86).ok_or(Error::Truncated)?;
        if feature_incompat & FEATURE_INCOMPAT_DEVICE_TABLE != 0 || extra_devices != 0 {
            return Err(Error::UnsupportedFeature(FEATURE_INCOMPAT_DEVICE_TABLE));
        }

        let is_48bit = feature_incompat & FEATURE_INCOMPAT_48BIT != 0;
        // The 48-bit root nid field is only written when the root nid does not
        // fit in 16 bits.  Its presence doubles as the flag for the 48-bit
        // blocks_hi field, which shares its offset with rootnid_2b.
        let root_nid_8b = u64_at::<O>(bytes, 112).ok_or(Error::Truncated)?;
        let (root_nid, blocks_hi) = if is_48bit && root_nid_8b != 0 {
            (
                root_nid_8b,
                u16_at::<O>(bytes, 14).ok_or(Error::Truncated)? as u64,
            )
        } else {
            (u16_at::<O>(bytes, 14).ok_or(Error::Truncated)? as u64, 0)
        };
        if root_nid == 0 {
            return Err(Error::InvalidSuperblock("root nid is zero"));
        }

        let blocks_lo = u32_at::<O>(bytes, 36).ok_or(Error::Truncated)? as u64;
        let blocks = blocks_lo | (blocks_hi << 32);

        let mut uuid = [0u8; 16];
        uuid.copy_from_slice(bytes.get(48..64).ok_or(Error::Truncated)?);
        let mut volume_name = [0u8; 16];
        volume_name.copy_from_slice(bytes.get(64..80).ok_or(Error::Truncated)?);

        Ok(Self {
            checksum,
            feature_compat,
            blkszbits,
            sb_extslots,
            root_nid,
            inos: u64_at::<O>(bytes, 16).ok_or(Error::Truncated)?,
            epoch: u64_at::<O>(bytes, 24).ok_or(Error::Truncated)?,
            fixed_nsec: u32_at::<O>(bytes, 32).ok_or(Error::Truncated)?,
            build_time: u32_at::<O>(bytes, 108).ok_or(Error::Truncated)?,
            blocks,
            meta_blkaddr: u32_at::<O>(bytes, 40).ok_or(Error::Truncated)?,
            xattr_blkaddr: u32_at::<O>(bytes, 44).ok_or(Error::Truncated)?,
            uuid,
            volume_name,
            feature_incompat,
            lz4_max_distance: u16_at::<O>(bytes, 84).ok_or(Error::Truncated)?,
            extra_devices,
            devt_slotoff: u16_at::<O>(bytes, 88).ok_or(Error::Truncated)?,
            dirblkbits: bytes[90],
            xattr_prefix_count: bytes[91],
            xattr_prefix_start: u32_at::<O>(bytes, 92).ok_or(Error::Truncated)?,
            packed_nid: u64_at::<O>(bytes, 96).ok_or(Error::Truncated)?,
            root_nid_8b,
        })
    }

    /// The filesystem block size in bytes.
    #[must_use]
    pub fn block_size(&self) -> u32 {
        1u32 << self.blkszbits
    }

    /// Whether the image uses 48-bit block addresses.
    #[must_use]
    pub fn has_48bit(&self) -> bool {
        self.feature_incompat & FEATURE_INCOMPAT_48BIT != 0
    }

    /// Whether the superblock carries a CRC32C checksum.
    #[must_use]
    pub fn has_checksum(&self) -> bool {
        self.feature_compat & FEATURE_COMPAT_SB_CHKSUM != 0
    }

    /// Whether compact inodes carry modification times.
    #[must_use]
    pub fn has_mtime(&self) -> bool {
        self.feature_compat & FEATURE_COMPAT_MTIME != 0
    }

    /// Whether the image carries an xattr name filter.
    #[must_use]
    pub fn has_xattr_filter(&self) -> bool {
        self.feature_compat & FEATURE_COMPAT_XATTR_FILTER != 0
    }
}
