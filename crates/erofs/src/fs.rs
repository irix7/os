// SPDX-License-Identifier: GPL-3.0-only
//! The readers: block device, filesystem, directories and paths.

use alloc::vec::Vec;
use core::cmp::min;

use crate::codec::{u16_at, u64_at, LittleEndian};
use crate::error::{Error, Result};
use crate::inode::{DataLayout, FileType, Inode, EROFS_NULL_ADDR, INODE_SLOT_SIZE};
use crate::superblock::{Superblock, SUPER_BASE_SIZE, SUPER_OFFSET};

/// A byte-addressed block device.
///
/// EROFS asks for whole blocks (and inline metadata), so the device does not
/// need a block size of its own.  The HTTP range reader and a SCSI disk driver
/// both implement this trait.
pub trait BlockDevice {
    /// Read exactly `buf.len()` bytes starting at byte `offset`.
    fn read_at(&mut self, offset: u64, buf: &mut [u8]) -> Result<()>;
}

impl BlockDevice for &[u8] {
    fn read_at(&mut self, offset: u64, buf: &mut [u8]) -> Result<()> {
        let start = usize::try_from(offset).map_err(|_| Error::Io)?;
        let end = start.checked_add(buf.len()).ok_or(Error::Io)?;
        let source = self.get(start..end).ok_or(Error::Io)?;
        buf.copy_from_slice(source);
        Ok(())
    }
}

/// A directory entry.
#[derive(Clone, Debug)]
pub struct DirEntry {
    /// Node ID of the referenced inode.
    pub nid: u64,
    /// Entry name, without a terminating NUL.
    pub name: Vec<u8>,
    /// File type from the directory entry.
    pub file_type: FileType,
}

impl DirEntry {
    /// The entry name as UTF-8, if it is valid UTF-8.
    #[must_use]
    pub fn name_str(&self) -> Option<&str> {
        core::str::from_utf8(&self.name).ok()
    }
}

/// A mounted read-only EROFS image.
#[derive(Debug)]
pub struct Erofs<D: BlockDevice> {
    dev: D,
    sb: Superblock,
}

impl<D: BlockDevice> Erofs<D> {
    /// Mount an image by reading its superblock.
    pub fn mount(mut dev: D) -> Result<Self> {
        let mut raw = [0u8; SUPER_BASE_SIZE];
        dev.read_at(SUPER_OFFSET, &mut raw)?;
        let sb = Superblock::parse(&raw)?;
        Ok(Self { dev, sb })
    }

    /// The decoded superblock.
    #[must_use]
    pub fn superblock(&self) -> &Superblock {
        &self.sb
    }

    /// The filesystem block size in bytes.
    #[must_use]
    pub fn block_size(&self) -> usize {
        self.sb.block_size() as usize
    }

    /// Read the root directory inode.
    pub fn root(&mut self) -> Result<Inode> {
        self.read_inode(self.sb.root_nid)
    }

    /// Read an inode by node ID.
    pub fn read_inode(&mut self, nid: u64) -> Result<Inode> {
        if nid == 0 {
            return Err(Error::InvalidInode("nid is zero"));
        }
        if nid & (1 << 63) != 0 {
            return Err(Error::Unsupported("metabox inode"));
        }

        let blocksize = u64::from(self.sb.block_size());
        let meta_start = u64::from(self.sb.meta_blkaddr)
            .checked_mul(blocksize)
            .ok_or(Error::InvalidInode("metadata offset"))?;
        let offset = nid
            .checked_mul(INODE_SLOT_SIZE)
            .and_then(|slot| meta_start.checked_add(slot))
            .ok_or(Error::InvalidInode("inode offset"))?;

        let mut raw = [0u8; 64];
        self.dev.read_at(offset, &mut raw[..32])?;
        let format = u16_at::<LittleEndian>(&raw[..32], 0).ok_or(Error::Truncated)?;
        let inode_size = if format & 1 == 0 { 32 } else { 64 };
        if inode_size == 64 {
            self.dev.read_at(offset + 32, &mut raw[32..])?;
        }
        Inode::parse::<LittleEndian>(&raw[..inode_size], nid, &self.sb)
    }

    /// Read part of an inode's data.
    ///
    /// The range is checked against the inode size.  Flat plain and flat inline
    /// layouts are supported; compressed and chunk-based inodes return
    /// [`Error::UnsupportedLayout`].
    pub fn read_at(&mut self, inode: &Inode, offset: u64, buf: &mut [u8]) -> Result<()> {
        let end = offset
            .checked_add(buf.len() as u64)
            .ok_or(Error::OutOfBounds)?;
        if end > inode.size {
            return Err(Error::OutOfBounds);
        }
        if buf.is_empty() {
            return Ok(());
        }

        match inode.layout {
            DataLayout::Plain => {
                if inode.start_blk == EROFS_NULL_ADDR {
                    return Err(Error::InvalidInode("null data block"));
                }
                self.read_blocks(inode.start_blk, offset, buf)
            }
            DataLayout::Inline => {
                let blocksize = u64::from(self.sb.block_size());
                let full = inode.size - (inode.size % blocksize);
                let mut done = 0usize;
                let mut pos = offset;
                if pos < full {
                    if inode.start_blk == EROFS_NULL_ADDR {
                        return Err(Error::InvalidInode("null data block"));
                    }
                    let count = min(buf.len() as u64, full - pos) as usize;
                    self.read_blocks(inode.start_blk, pos, &mut buf[..count])?;
                    done = count;
                    pos += count as u64;
                }
                if done < buf.len() {
                    let tail = pos
                        .checked_sub(full)
                        .ok_or(Error::InvalidInode("inline offset"))?;
                    let offset = inode
                        .inline_offset
                        .checked_add(tail)
                        .ok_or(Error::InvalidInode("inline offset"))?;
                    self.dev.read_at(offset, &mut buf[done..])?;
                }
                Ok(())
            }
            other => Err(Error::UnsupportedLayout(other.raw_bits())),
        }
    }

    fn read_blocks(&mut self, start_blk: u64, offset: u64, buf: &mut [u8]) -> Result<()> {
        let blocksize = u64::from(self.sb.block_size());
        let mut pos = offset;
        let mut done = 0usize;
        while done < buf.len() {
            let block = start_blk
                .checked_add(pos / blocksize)
                .ok_or(Error::OutOfBounds)?;
            let in_block = (pos % blocksize) as usize;
            let count = min(
                blocksize as usize - in_block,
                buf.len().saturating_sub(done),
            );
            let byte = block
                .checked_mul(blocksize)
                .and_then(|base| base.checked_add(in_block as u64))
                .ok_or(Error::OutOfBounds)?;
            self.dev.read_at(byte, &mut buf[done..done + count])?;
            pos += count as u64;
            done += count;
        }
        Ok(())
    }

    /// Read and buffer a whole inode's data.
    ///
    /// Files larger than 64 MiB are rejected by [`Error::TooLarge`]; use
    /// [`Self::read_at`] to stream a large file instead.
    pub fn read_file(&mut self, inode: &Inode) -> Result<Vec<u8>> {
        const MAX_BUFFERED_FILE: u64 = 64 * 1024 * 1024;
        if inode.size > MAX_BUFFERED_FILE {
            return Err(Error::TooLarge);
        }
        let len = usize::try_from(inode.size).map_err(|_| Error::TooLarge)?;
        let mut data = Vec::new();
        data.try_reserve_exact(len)
            .map_err(|_| Error::OutOfMemory)?;
        data.resize(len, 0);
        if len != 0 {
            self.read_at(inode, 0, &mut data)?;
        }
        Ok(data)
    }

    /// Read a directory's entries.
    pub fn read_dir(&mut self, dir: &Inode) -> Result<Vec<DirEntry>> {
        if !dir.is_dir() {
            return Err(Error::NotDirectory);
        }
        let data = self.read_file(dir)?;
        let blocksize = self.block_size();
        let mut entries = Vec::new();
        let mut offset = 0usize;

        while offset < data.len() {
            let end = min(offset + blocksize, data.len());
            let block = &data[offset..end];
            if block.len() < 12 {
                return Err(Error::InvalidDirectory("short directory block"));
            }
            let first_nameoff = u16_at::<LittleEndian>(block, 8)
                .ok_or(Error::InvalidDirectory("missing first name offset"))?
                as usize;
            if first_nameoff < 12
                || first_nameoff > block.len()
                || !first_nameoff.is_multiple_of(12)
            {
                return Err(Error::InvalidDirectory("invalid first name offset"));
            }
            let count = first_nameoff / 12;

            for index in 0..count {
                let dirent = index * 12;
                let nid = u64_at::<LittleEndian>(block, dirent)
                    .ok_or(Error::InvalidDirectory("missing nid"))?;
                let nameoff = u16_at::<LittleEndian>(block, dirent + 8)
                    .ok_or(Error::InvalidDirectory("missing name offset"))?
                    as usize;
                let file_type = FileType::from_erofs(
                    *block
                        .get(dirent + 10)
                        .ok_or(Error::InvalidDirectory("missing file type"))?,
                );
                let next = if index + 1 < count {
                    u16_at::<LittleEndian>(block, (index + 1) * 12 + 8)
                        .ok_or(Error::InvalidDirectory("missing next name offset"))?
                        as usize
                } else {
                    block.len()
                };

                if nameoff < first_nameoff || nameoff > next || next > block.len() {
                    return Err(Error::InvalidDirectory("invalid name range"));
                }
                let raw_name = &block[nameoff..next];
                let name_len = raw_name
                    .iter()
                    .position(|&byte| byte == 0)
                    .unwrap_or(raw_name.len());
                if name_len == 0 {
                    return Err(Error::InvalidDirectory("empty name"));
                }
                if name_len > 255 {
                    return Err(Error::NameTooLong);
                }

                entries.push(DirEntry {
                    nid,
                    name: raw_name[..name_len].to_vec(),
                    file_type,
                });
            }

            offset = end;
        }

        Ok(entries)
    }

    /// Find a name in a directory.
    pub fn lookup(&mut self, dir: &Inode, name: &[u8]) -> Result<Option<Inode>> {
        if !dir.is_dir() {
            return Err(Error::NotDirectory);
        }
        for entry in self.read_dir(dir)? {
            if entry.name == name {
                return self.read_inode(entry.nid).map(Some);
            }
        }
        Ok(None)
    }

    /// Resolve a slash-separated path from the root.
    ///
    /// Empty components and `.` are skipped.  `..` is not implemented yet and
    /// returns [`Error::Unsupported`]; callers should normalise paths before
    /// asking the reader.
    pub fn lookup_path(&mut self, path: &str) -> Result<Inode> {
        let mut inode = self.root()?;
        for component in path.split('/') {
            if component.is_empty() || component == "." {
                continue;
            }
            if component == ".." {
                return Err(Error::Unsupported("parent directory lookup"));
            }
            inode = self
                .lookup(&inode, component.as_bytes())?
                .ok_or(Error::NotFound)?;
        }
        Ok(inode)
    }

    /// Read a whole file by path.
    pub fn read_file_path(&mut self, path: &str) -> Result<Vec<u8>> {
        let inode = self.lookup_path(path)?;
        self.read_file(&inode)
    }

    /// Read a symbolic link's target by path.
    pub fn read_link(&mut self, inode: &Inode) -> Result<Vec<u8>> {
        if !inode.is_symlink() {
            return Err(Error::NotSymlink);
        }
        self.read_file(inode)
    }
}
