// SPDX-License-Identifier: GPL-3.0-only
//! The readers: block device, filesystem, directories and paths.

use alloc::borrow::Cow;
use alloc::string::ToString;
use alloc::vec::Vec;
use core::cmp::min;

use crate::codec::{u16_at, u64_at, LittleEndian};
#[cfg(feature = "lz4")]
use crate::compress::CompressedInfo;
use crate::error::{Error, Result};
use crate::inode::{DataLayout, FileType, Inode, EROFS_NULL_ADDR, INODE_SLOT_SIZE};
use crate::superblock::{Superblock, SUPER_BASE_SIZE, SUPER_OFFSET};

/// The number of symbolic links a single path resolution may follow.
///
/// A longer chain, including a link that points at itself, returns
/// [`Error::SymlinkLoop`].
pub const MAX_SYMLINK_DEPTH: usize = 8;

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
    /// layouts are read directly from the device; compressed inodes are mapped
    /// and decompressed one pcluster at a time.  Chunk-based inodes return
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
            DataLayout::CompressedFull | DataLayout::CompressedCompact => {
                self.read_compressed_dispatch(inode, offset, buf)
            }
            other => Err(Error::UnsupportedLayout(other.raw_bits())),
            other => Err(Error::UnsupportedLayout(other.raw_bits())),
        }
    }

    fn read_compressed_dispatch(
        &mut self,
        inode: &Inode,
        offset: u64,
        buf: &mut [u8],
    ) -> Result<()> {
        #[cfg(feature = "lz4")]
        {
            self.read_compressed(inode, offset, buf)
        }
        #[cfg(not(feature = "lz4"))]
        {
            let _ = (offset, buf);
            Err(Error::UnsupportedLayout(inode.layout.raw_bits()))
        }
    }

    /// Read the compression map header for an inode.
    #[cfg(feature = "lz4")]
    fn compression_info(&mut self, inode: &Inode) -> Result<CompressedInfo> {
        let map_header = inode
            .inline_offset
            .checked_add(7)
            .ok_or(Error::InvalidInode("map header offset"))?
            & !7;
        let mut raw = [0u8; 8];
        self.dev.read_at(map_header, &mut raw)?;
        let Self { dev, sb } = self;
        CompressedInfo::parse(&raw, inode, sb, &mut |offset, buf| dev.read_at(offset, buf))
    }

    /// Read a compressed range, one pcluster extent at a time.
    ///
    /// A read that crosses a pcluster boundary decodes each pcluster again
    /// only when the range requires it; there is no decompression cache.
    #[cfg(feature = "lz4")]
    fn read_compressed(&mut self, inode: &Inode, offset: u64, buf: &mut [u8]) -> Result<()> {
        let info = self.compression_info(inode)?;
        let zero_padding = self.sb.has_zero_padding();
        let blocksize = u64::from(self.sb.block_size());
        let Self { dev, sb: _ } = self;
        let mut read = |offset: u64, buf: &mut [u8]| dev.read_at(offset, buf);
        let mut done = 0usize;
        while done < buf.len() {
            let pos = offset + done as u64;
            let extent = info.map(inode, pos, &mut read)?;
            let start = pos
                .checked_sub(extent.la)
                .ok_or(Error::InvalidCompressedData("extent offset"))?;
            let needed = start
                .checked_add((buf.len() - done) as u64)
                .ok_or(Error::OutOfBounds)?;
            let data = info.decompress(&extent, needed, zero_padding, blocksize, &mut read)?;
            let start = usize::try_from(start)
                .map_err(|_| Error::InvalidCompressedData("extent offset"))?;
            if start >= data.len() {
                return Err(Error::InvalidCompressedData("extent misses the read"));
            }
            let count = min(buf.len() - done, data.len() - start);
            buf[done..done + count].copy_from_slice(&data[start..start + count]);
            done += count;
        }
        Ok(())
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

    /// Resolve a slash-separated path from the root without following symlinks.
    ///
    /// Empty components and `.` are skipped.  `..` walks back to the directory
    /// the resolution descended from, including at the root, where it stays at
    /// the root.  A path that ends at a symbolic link returns the link inode
    /// itself; use [`Self::lookup_path_follow`] to resolve the target.
    pub fn lookup_path(&mut self, path: &str) -> Result<Inode> {
        self.resolve_path(path, false)
    }

    /// Resolve a slash-separated path from the root, following symlinks.
    ///
    /// Paths are always resolved from the root, with or without a leading
    /// slash, so a relative path is not relative to a caller directory.  A
    /// relative link target is resolved from the directory holding the link;
    /// an absolute target starts again at the root.  At most
    /// [`MAX_SYMLINK_DEPTH`] links are followed; a longer chain returns
    /// [`Error::SymlinkLoop`].
    pub fn lookup_path_follow(&mut self, path: &str) -> Result<Inode> {
        self.resolve_path(path, true)
    }

    fn resolve_path(&mut self, path: &str, follow: bool) -> Result<Inode> {
        let mut inode = self.root()?;
        // The directories resolution has descended through, innermost last.
        // EROFS has no parent pointer, so `..` walks back through this stack.
        let mut parents: Vec<Inode> = Vec::new();
        // The components still to visit, top of the stack first.  Owned
        // components hold a symlink target; borrowed ones point into `path`.
        let mut pending: Vec<Cow<'_, str>> = path.rsplit('/').map(Cow::Borrowed).collect();
        let mut followed = 0usize;

        while let Some(component) = pending.pop() {
            match component.as_ref() {
                "" | "." => {}
                ".." => {
                    if !inode.is_dir() {
                        return Err(Error::NotDirectory);
                    }
                    if let Some(parent) = parents.pop() {
                        inode = parent;
                    }
                }
                name => {
                    if !inode.is_dir() {
                        return Err(Error::NotDirectory);
                    }
                    let child = self
                        .lookup(&inode, name.as_bytes())?
                        .ok_or(Error::NotFound)?;
                    if !follow || !child.is_symlink() {
                        parents.push(inode);
                        inode = child;
                        continue;
                    }

                    let target = self.read_link(&child)?;
                    let target = core::str::from_utf8(&target)
                        .map_err(|_| Error::InvalidInode("symlink target is not UTF-8"))?;
                    if target.is_empty() {
                        return Err(Error::NotFound);
                    }
                    followed += 1;
                    if followed > MAX_SYMLINK_DEPTH {
                        return Err(Error::SymlinkLoop);
                    }
                    if target.starts_with('/') {
                        inode = self.root()?;
                        parents.clear();
                    }
                    // A relative target continues from the link's directory,
                    // which is still `inode`.  Push its components in reverse
                    // so the stack pops them in order, ahead of what is left.
                    pending.extend(
                        target
                            .split('/')
                            .rev()
                            .map(|part| Cow::Owned(part.to_string())),
                    );
                }
            }
        }

        Ok(inode)
    }

    /// Read a whole file by path, following symbolic links.
    pub fn read_file_path(&mut self, path: &str) -> Result<Vec<u8>> {
        let inode = self.lookup_path_follow(path)?;
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
