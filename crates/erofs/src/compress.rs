// SPDX-License-Identifier: GPL-3.0-only
//! EROFS compressed-file mapping and the LZ4 block decoder.
//!
//! EROFS stores compressed files as *physical clusters* (pclusters).  Each
//! logical cluster has an index entry that either names a pcluster or points
//! back at the index entry that does.  A pcluster can cover several logical
//! clusters, and the last one can live inline in the metadata block
//! (`ztailpacking`).
//!
//! The layout is decoded from the SGI host's big-endian CPU through the same
//! explicit little-endian codec as the rest of the crate.  The mapping follows
//! `z_erofs_map_blocks_iter` in Linux `fs/erofs/zmap.c`; the LZ4 decoder reads
//! the standard LZ4 block format and, like `LZ4_decompress_safe_partial`,
//! stops once the requested output prefix is full.
//!
//! Supported:
//!
//! - compressed-full (datalayout 1) with 8-byte logical-cluster indexes;
//! - compressed-compact (datalayout 3), including the compacted 2-byte index
//!   packs;
//! - multiple logical clusters per pcluster (big pclusters with `CBLKCNT`);
//! - inline pclusters from `ztailpacking`;
//! - LZ4 and plain (shifted) pclusters with the `lz4_0padding` zero padding.
//!
//! Fragments, encoded extents, interlaced pclusters and non-LZ4 algorithms are
//! rejected with [`Error::UnsupportedCompression`].

use alloc::vec::Vec;

use crate::codec::{u16_at, u32_at, LittleEndian};
use crate::error::{Error, Result};
use crate::inode::{DataLayout, Inode};
use crate::superblock::Superblock;

/// The last pcluster is stored inline in the metadata block.
const ADVISE_INLINE_PCLUSTER: u16 = 0x0008;
/// The first pcluster of a head may span several blocks.
const ADVISE_BIG_PCLUSTER_1: u16 = 0x0002;
/// The same, for head type 2.
const ADVISE_BIG_PCLUSTER_2: u16 = 0x0004;
/// Compact 2-byte index packs are in use (datalayout 3 only).
const ADVISE_COMPACTED_2B: u16 = 0x0001;
/// Encoded extents (datalayout 1 only, the 48-bit feature).
const ADVISE_EXTENTS: u16 = 0x0001;
/// Interlaced pcluster layout.
const ADVISE_INTERLACED_PCLUSTER: u16 = 0x0010;
/// The pcluster lives in the packed inode (fragments).
const ADVISE_FRAGMENT_PCLUSTER: u16 = 0x0020;
/// Every advise bit this reader knows about.
const KNOWN_ADVISE: u16 = ADVISE_COMPACTED_2B
    | ADVISE_BIG_PCLUSTER_1
    | ADVISE_BIG_PCLUSTER_2
    | ADVISE_INLINE_PCLUSTER
    | ADVISE_INTERLACED_PCLUSTER
    | ADVISE_FRAGMENT_PCLUSTER;

/// Logical cluster types.
const LCLUSTER_PLAIN: u8 = 0;
const LCLUSTER_HEAD1: u8 = 1;
const LCLUSTER_NONHEAD: u8 = 2;
const LCLUSTER_HEAD2: u8 = 3;

/// A non-head entry with this bit stores a compressed block count, not a
/// distance to its head.
const LI_D0_CBLKCNT: u16 = 1 << 11;
/// The pcluster refers to partial decompressed data (fragments).
const LI_PARTIAL_REF: u16 = 1 << 15;

/// Maximum encoded size of one pcluster, in bytes.
const PCLUSTER_MAX_SIZE: u64 = 1024 * 1024;
/// Maximum decoded size of one pcluster, in bytes.
const PCLUSTER_MAX_DSIZE: u64 = 12 * 1024 * 1024;
/// Size of the on-disk `z_erofs_map_header`.
const MAP_HEADER_SIZE: u64 = 8;

/// The first byte of a logical cluster.
fn lcn_start(lcn: u64, bits: u64, clusterofs: u32) -> Result<u64> {
    lcn.checked_mul(1u64 << bits)
        .and_then(|base| base.checked_add(u64::from(clusterofs)))
        .ok_or(Error::InvalidCompressedData("logical cluster offset"))
}

/// The first byte after a logical cluster.
fn lcn_end(lcn: u64, bits: u64) -> Result<u64> {
    lcn.checked_add(1)
        .and_then(|next| next.checked_mul(1u64 << bits))
        .ok_or(Error::InvalidCompressedData("logical cluster offset"))
}

/// The compression algorithm of one pcluster.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Algorithm {
    /// An LZ4 block.
    Lz4,
    /// Stored uncompressed ("shifted").
    Plain,
}

/// One compressed extent, already translated to file and image offsets.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Extent {
    /// First logical byte covered by this extent.
    pub la: u64,
    /// Number of logical bytes covered.
    pub llen: u64,
    /// Physical byte offset of the pcluster.
    pub pa: u64,
    /// Encoded length of the pcluster in bytes.
    pub plen: u64,
    /// How to decode the pcluster.
    pub algorithm: Algorithm,
}

/// A decoded logical-cluster index entry.
#[derive(Clone, Copy, Debug)]
struct Record {
    lcn: u64,
    kind: u8,
    clusterofs: u32,
    pblk: u32,
    delta0: u16,
    delta1: u16,
    compressedblks: u16,
    nextpackoff: u64,
}

/// A tail pcluster stored inline in the metadata block.
#[derive(Clone, Copy, Debug)]
struct InlineTail {
    head_lcn: u64,
    offset: u64,
}

/// The parsed `z_erofs_map_header` and index layout of one inode.
#[derive(Clone, Debug)]
pub(crate) struct CompressedInfo {
    layout: DataLayout,
    advise: u16,
    algorithm: [u8; 2],
    lclusterbits: u8,
    lcluster_count: u64,
    index_start: u64,
    idata_size: u16,
    tail: Option<InlineTail>,
}

impl CompressedInfo {
    /// Parse the map header that follows an inode and, for inline pclusters,
    /// locate the tail pcluster.
    ///
    /// `raw` is the 8-byte map header read from the image.
    pub(crate) fn parse<F>(raw: &[u8], inode: &Inode, sb: &Superblock, read: &mut F) -> Result<Self>
    where
        F: FnMut(u64, &mut [u8]) -> Result<()>,
    {
        if raw.len() < MAP_HEADER_SIZE as usize {
            return Err(Error::Truncated);
        }
        let layout = inode.layout;
        if !layout.is_compressed() {
            return Err(Error::InvalidInode("not a compressed inode"));
        }

        let advise = u16_at::<LittleEndian>(raw, 4).ok_or(Error::Truncated)?;
        let algorithm_byte = *raw.get(6).ok_or(Error::Truncated)?;
        let clusterbits = *raw.get(7).ok_or(Error::Truncated)?;
        if clusterbits & 0x80 != 0 {
            return Err(Error::UnsupportedCompression("packed inode fragment"));
        }
        if clusterbits & 0x0f != 0 {
            // The cluster size only differs from the block size in rare images
            // whose index layout this reader has not been tested against.
            return Err(Error::UnsupportedCompression("logical cluster size"));
        }
        if advise & !KNOWN_ADVISE != 0 {
            return Err(Error::UnsupportedCompression("map header advise bits"));
        }
        if advise & ADVISE_FRAGMENT_PCLUSTER != 0 {
            return Err(Error::UnsupportedCompression("fragments"));
        }
        if advise & ADVISE_INTERLACED_PCLUSTER != 0 {
            return Err(Error::UnsupportedCompression("interlaced pcluster"));
        }
        if layout == DataLayout::CompressedFull && advise & ADVISE_EXTENTS != 0 {
            return Err(Error::UnsupportedCompression("encoded extents"));
        }
        if layout == DataLayout::CompressedCompact
            && (advise & ADVISE_BIG_PCLUSTER_1 != 0) != (advise & ADVISE_BIG_PCLUSTER_2 != 0)
        {
            return Err(Error::InvalidCompressedData(
                "inconsistent big pcluster advise",
            ));
        }

        let lclusterbits = sb.blkszbits;
        let blocksize = u64::from(sb.block_size());
        if inode.size == 0 {
            return Err(Error::InvalidInode("compressed inode is empty"));
        }
        let lcluster_count = (inode.size - 1) / blocksize + 1;

        let map_header = inode
            .inline_offset
            .checked_add(7)
            .ok_or(Error::InvalidInode("map header offset"))?
            & !7;
        let index_start = map_header
            .checked_add(MAP_HEADER_SIZE)
            .ok_or(Error::InvalidInode("index offset"))?
            + if layout == DataLayout::CompressedFull {
                8
            } else {
                0
            };
        // The full layout keeps an 8-byte legacy alignment gap after the map
        // header; the compact layout starts the first pack immediately.

        let idata_size = if advise & ADVISE_INLINE_PCLUSTER != 0 {
            let size = u16_at::<LittleEndian>(raw, 2).ok_or(Error::Truncated)?;
            if size == 0 {
                return Err(Error::InvalidCompressedData("inline pcluster size"));
            }
            size
        } else {
            0
        };

        let mut info = Self {
            layout,
            advise,
            algorithm: [algorithm_byte & 0x0f, algorithm_byte >> 4],
            lclusterbits,
            lcluster_count,
            index_start,
            idata_size,
            tail: None,
        };
        if idata_size != 0 {
            let tail = info.find_inline_tail(inode, read)?;
            info.tail = Some(tail);
        }
        Ok(info)
    }

    /// Find the head logical cluster that owns the last byte of the file and
    /// the offset of the inline pcluster data.
    ///
    /// The inline data follows the index entry of the lcluster containing the
    /// last byte, which is the end of the whole index area.  `nextpackoff` is
    /// captured before any lookback, matching `EROFS_GET_BLOCKS_FINDTAIL`.
    fn find_inline_tail<F>(&self, inode: &Inode, read: &mut F) -> Result<InlineTail>
    where
        F: FnMut(u64, &mut [u8]) -> Result<()>,
    {
        let bits = u64::from(self.lclusterbits);
        let ofs = inode.size - 1;
        let lcn = ofs >> bits;
        let endoff = (ofs & ((1u64 << bits) - 1)) as u32;
        let record = self.load(lcn, read)?;
        let offset = record.nextpackoff;
        let head_lcn = if record.kind != LCLUSTER_NONHEAD && endoff >= record.clusterofs {
            record.lcn
        } else {
            let distance = if record.kind == LCLUSTER_NONHEAD {
                record.delta0
            } else {
                1
            };
            self.lookback(record.lcn, distance, read)?.lcn
        };
        Ok(InlineTail { head_lcn, offset })
    }

    /// Follow a non-head entry back to the head that owns its pcluster.
    fn lookback<F>(&self, lcn: u64, distance: u16, read: &mut F) -> Result<Record>
    where
        F: FnMut(u64, &mut [u8]) -> Result<()>,
    {
        let mut lcn = lcn;
        let mut distance = distance;
        loop {
            if distance == 0 {
                return Err(Error::InvalidCompressedData("zero lookback distance"));
            }
            let step = u64::from(distance);
            if lcn < step {
                return Err(Error::InvalidCompressedData("lookback past start"));
            }
            lcn -= step;
            let record = self.load(lcn, read)?;
            if record.kind == LCLUSTER_NONHEAD {
                distance = record.delta0;
                continue;
            }
            return Ok(record);
        }
    }

    /// Map one file offset to the extent that contains it.
    pub(crate) fn map<F>(&self, inode: &Inode, offset: u64, read: &mut F) -> Result<Extent>
    where
        F: FnMut(u64, &mut [u8]) -> Result<()>,
    {
        if offset >= inode.size {
            return Err(Error::OutOfBounds);
        }
        let bits = u64::from(self.lclusterbits);
        let lcn = offset >> bits;
        if lcn >= self.lcluster_count {
            return Err(Error::OutOfBounds);
        }
        let endoff = (offset & ((1u64 << bits) - 1)) as u32;
        let record = self.load(lcn, read)?;
        let mut end = lcn_end(record.lcn, bits)?;
        let head;
        let la;
        if record.kind != LCLUSTER_NONHEAD && endoff >= record.clusterofs {
            head = record;
            la = lcn_start(record.lcn, bits, record.clusterofs)?;
            if self.idata_size != 0 && end > inode.size {
                end = inode.size;
            }
        } else {
            let distance = if record.kind == LCLUSTER_NONHEAD {
                record.delta0
            } else {
                end = lcn_start(record.lcn, bits, record.clusterofs)?;
                1
            };
            head = self.lookback(record.lcn, distance, read)?;
            la = lcn_start(head.lcn, bits, head.clusterofs)?;
        }
        let llen = end
            .checked_sub(la)
            .ok_or(Error::InvalidCompressedData("extent end before start"))?;
        if llen == 0 || llen > PCLUSTER_MAX_DSIZE {
            return Err(Error::InvalidCompressedData("invalid extent length"));
        }

        let (pa, plen, algorithm) = if let Some(tail) = self.tail {
            if head.lcn == tail.head_lcn {
                let inline_len = u64::from(self.idata_size);
                let in_block = tail.offset % (1u64 << self.lclusterbits);
                if in_block + inline_len > (1u64 << self.lclusterbits) {
                    return Err(Error::InvalidCompressedData(
                        "inline pcluster crosses a block",
                    ));
                }
                (tail.offset, inline_len, self.algorithm_for(head.kind)?)
            } else {
                let blocks = self.compressed_blocks(&head, inode, read)?;
                let pa = u64::from(head.pblk)
                    .checked_mul(u64::from(1u32 << self.lclusterbits))
                    .ok_or(Error::InvalidCompressedData("pcluster offset"))?;
                (
                    pa,
                    u64::from(blocks) << self.lclusterbits,
                    self.algorithm_for(head.kind)?,
                )
            }
        } else {
            let blocks = self.compressed_blocks(&head, inode, read)?;
            let pa = u64::from(head.pblk)
                .checked_mul(u64::from(1u32 << self.lclusterbits))
                .ok_or(Error::InvalidCompressedData("pcluster offset"))?;
            (
                pa,
                u64::from(blocks) << self.lclusterbits,
                self.algorithm_for(head.kind)?,
            )
        };
        if plen == 0 || plen > PCLUSTER_MAX_SIZE {
            return Err(Error::InvalidCompressedData("invalid pcluster length"));
        }

        Ok(Extent {
            la,
            llen,
            pa,
            plen,
            algorithm,
        })
    }

    /// Decode a prefix of one pcluster.
    ///
    /// `needed` is the number of logical bytes from the start of the extent
    /// that the caller will consume; the decoder never produces more than that,
    /// which keeps a one-byte read inside a large pcluster cheap.
    pub(crate) fn decompress<F>(
        &self,
        extent: &Extent,
        needed: u64,
        zero_padding: bool,
        blocksize: u64,
        read: &mut F,
    ) -> Result<Vec<u8>>
    where
        F: FnMut(u64, &mut [u8]) -> Result<()>,
    {
        let len = usize::try_from(needed.min(extent.llen)).map_err(|_| Error::TooLarge)?;
        if len == 0 {
            return Err(Error::InvalidCompressedData("empty extent read"));
        }
        let mut out = Vec::new();
        out.try_reserve_exact(len).map_err(|_| Error::OutOfMemory)?;
        out.resize(len, 0);

        match extent.algorithm {
            Algorithm::Plain => {
                if len as u64 > extent.plen {
                    return Err(Error::InvalidCompressedData(
                        "plain pcluster shorter than its extent",
                    ));
                }
                read(extent.pa, &mut out)?;
            }
            Algorithm::Lz4 => {
                let encoded_len = usize::try_from(extent.plen).map_err(|_| Error::TooLarge)?;
                let mut encoded = Vec::new();
                encoded
                    .try_reserve_exact(encoded_len)
                    .map_err(|_| Error::OutOfMemory)?;
                encoded.resize(encoded_len, 0);
                read(extent.pa, &mut encoded)?;

                let input = if zero_padding {
                    strip_zero_padding(&encoded, extent.pa, blocksize)?
                } else {
                    &encoded[..]
                };
                lz4::decompress_prefix(input, &mut out)?;
            }
        }
        Ok(out)
    }

    /// The number of physical blocks in a head's pcluster.
    fn compressed_blocks<F>(&self, head: &Record, inode: &Inode, read: &mut F) -> Result<u16>
    where
        F: FnMut(u64, &mut [u8]) -> Result<()>,
    {
        let big1 = self.advise & ADVISE_BIG_PCLUSTER_1 != 0;
        let big2 = self.advise & ADVISE_BIG_PCLUSTER_2 != 0;
        let next_lcn = head.lcn + 1;
        if (head.kind == LCLUSTER_HEAD1 && !big1)
            || ((head.kind == LCLUSTER_PLAIN || head.kind == LCLUSTER_HEAD2) && !big2)
            || lcn_start(head.lcn + 1, u64::from(self.lclusterbits), 0)? >= inode.size
        {
            return Ok(1);
        }
        if head.compressedblks != 0 {
            return Ok(head.compressedblks);
        }
        let next = self.load(next_lcn, read)?;
        if next.kind == LCLUSTER_NONHEAD {
            if next.delta0 != 1 {
                return Err(Error::InvalidCompressedData("bogus pcluster block count"));
            }
            if next.compressedblks != 0 {
                return Ok(next.compressedblks);
            }
        }
        Ok(1)
    }

    /// Resolve the algorithm of a head pcluster.
    fn algorithm_for(&self, kind: u8) -> Result<Algorithm> {
        match kind {
            LCLUSTER_PLAIN => Ok(Algorithm::Plain),
            LCLUSTER_HEAD1 | LCLUSTER_HEAD2 => {
                let algorithm = if kind == LCLUSTER_HEAD1 {
                    self.algorithm[0]
                } else {
                    self.algorithm[1]
                };
                match algorithm {
                    0 => Ok(Algorithm::Lz4),
                    1 => Err(Error::UnsupportedCompression("lzma")),
                    2 => Err(Error::UnsupportedCompression("deflate")),
                    3 => Err(Error::UnsupportedCompression("zstd")),
                    _ => Err(Error::UnsupportedCompression("unknown algorithm")),
                }
            }
            _ => Err(Error::InvalidCompressedData("head without an algorithm")),
        }
    }

    /// Load one logical-cluster index entry.
    fn load<F>(&self, lcn: u64, read: &mut F) -> Result<Record>
    where
        F: FnMut(u64, &mut [u8]) -> Result<()>,
    {
        if lcn >= self.lcluster_count {
            return Err(Error::InvalidCompressedData("logical cluster out of range"));
        }
        match self.layout {
            DataLayout::CompressedFull => self.load_full(lcn, read),
            DataLayout::CompressedCompact => self.load_compact(lcn, read),
            _ => Err(Error::InvalidInode("not a compressed inode")),
        }
    }

    /// Load an entry from the 8-byte full-index array.
    fn load_full<F>(&self, lcn: u64, read: &mut F) -> Result<Record>
    where
        F: FnMut(u64, &mut [u8]) -> Result<()>,
    {
        let pos = self
            .index_start
            .checked_add(lcn.checked_mul(8).ok_or(Error::Truncated)?)
            .ok_or(Error::Truncated)?;
        let nextpackoff = pos.checked_add(8).ok_or(Error::Truncated)?;
        let mut raw = [0u8; 8];
        read(pos, &mut raw)?;

        let advise = u16_at::<LittleEndian>(&raw, 0).ok_or(Error::Truncated)?;
        let kind = (advise & 3) as u8;
        let mut record = Record {
            lcn,
            kind,
            clusterofs: 0,
            pblk: 0,
            delta0: 0,
            delta1: 0,
            compressedblks: 0,
            nextpackoff,
        };
        if kind == LCLUSTER_NONHEAD {
            record.clusterofs = 1u32 << self.lclusterbits;
            let mut delta0 = u16_at::<LittleEndian>(&raw, 4).ok_or(Error::Truncated)?;
            record.delta1 = u16_at::<LittleEndian>(&raw, 6).ok_or(Error::Truncated)?;
            if delta0 & LI_D0_CBLKCNT != 0 {
                if self.advise & (ADVISE_BIG_PCLUSTER_1 | ADVISE_BIG_PCLUSTER_2) == 0 {
                    return Err(Error::InvalidCompressedData(
                        "pcluster block count without big pclusters",
                    ));
                }
                record.compressedblks = delta0 & !LI_D0_CBLKCNT;
                delta0 = 1;
            }
            record.delta0 = delta0;
        } else {
            if advise & LI_PARTIAL_REF != 0 {
                return Err(Error::UnsupportedCompression("partial reference"));
            }
            record.clusterofs = u32::from(u16_at::<LittleEndian>(&raw, 2).ok_or(Error::Truncated)?);
            record.pblk = u32_at::<LittleEndian>(&raw, 4).ok_or(Error::Truncated)?;
        }
        if record.kind != LCLUSTER_NONHEAD && record.clusterofs >= (1u32 << self.lclusterbits) {
            return Err(Error::InvalidCompressedData("cluster offset out of range"));
        }
        Ok(record)
    }

    /// The bounds of the compact index pack that holds `lcn`.
    ///
    /// Returns `(pack_start, pack_size, shift, index)` where `index` is the
    /// entry number within the pack and `shift` is 1 for 2-byte entries and 2
    /// for 4-byte entries.
    fn compact_pack(&self, lcn: u64) -> Result<(u64, u64, u8, u64)> {
        let total = self.lcluster_count;
        let initial = ((32 - self.index_start % 32) / 4) & 7;
        let compacted_2b = if self.advise & ADVISE_COMPACTED_2B != 0 && initial < total {
            (total - initial) & !15
        } else {
            0
        };

        let mut pos = self.index_start;
        let mut local = lcn;
        let mut shift = 2u8;
        if local >= initial {
            pos = pos.checked_add(initial * 4).ok_or(Error::Truncated)?;
            local -= initial;
            if local < compacted_2b {
                shift = 1;
            } else {
                pos = pos.checked_add(compacted_2b * 2).ok_or(Error::Truncated)?;
                local -= compacted_2b;
            }
        }
        let entry_size = 1u64 << shift;
        pos = pos
            .checked_add(local.checked_mul(entry_size).ok_or(Error::Truncated)?)
            .ok_or(Error::Truncated)?;
        let pack_size = if shift == 1 { 32 } else { 8 };
        let pack_start = pos & !(pack_size - 1);
        Ok((pack_start, pack_size, shift, (pos - pack_start) >> shift))
    }

    /// Decode one entry inside a compact index pack.
    fn compact_entry(pack: &[u8], index: u64, encodebits: u32, lobits: u32) -> Result<(u16, u8)> {
        let bit = index
            .checked_mul(u64::from(encodebits))
            .ok_or(Error::Truncated)?;
        let byte = usize::try_from(bit / 8).map_err(|_| Error::Truncated)?;
        let shift = (bit % 8) as u32;
        let end = byte.checked_add(4).ok_or(Error::Truncated)?;
        let bytes = pack.get(byte..end).ok_or(Error::Truncated)?;
        let value = u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]) >> shift;
        let value = value & ((1u32 << encodebits) - 1);
        let lo = (value & ((1u32 << lobits) - 1)) as u16;
        let kind = ((value >> lobits) & 3) as u8;
        Ok((lo, kind))
    }

    /// Load an entry from the compact index packs.
    fn load_compact<F>(&self, lcn: u64, read: &mut F) -> Result<Record>
    where
        F: FnMut(u64, &mut [u8]) -> Result<()>,
    {
        let (pack_start, pack_size, shift, index) = self.compact_pack(lcn)?;
        let vcnt = if shift == 1 { 16u64 } else { 2u64 };
        let encodebits = if shift == 1 { 14u32 } else { 16u32 };
        let lobits = u32::from(self.lclusterbits).max(12);
        let size = usize::try_from(pack_size).map_err(|_| Error::Truncated)?;
        let mut pack = [0u8; 32];
        read(pack_start, &mut pack[..size])?;
        let pack = &pack[..size];

        let (lo, kind) = Self::compact_entry(pack, index, encodebits, lobits)?;
        let mut record = Record {
            lcn,
            kind,
            clusterofs: 0,
            pblk: 0,
            delta0: 0,
            delta1: 0,
            compressedblks: 0,
            nextpackoff: pack_start.checked_add(pack_size).ok_or(Error::Truncated)?,
        };
        if kind == LCLUSTER_NONHEAD {
            record.clusterofs = 1u32 << self.lclusterbits;
            if lo & LI_D0_CBLKCNT != 0 {
                if self.advise & ADVISE_BIG_PCLUSTER_1 == 0 {
                    return Err(Error::InvalidCompressedData(
                        "pcluster block count without big pclusters",
                    ));
                }
                record.compressedblks = lo & !LI_D0_CBLKCNT;
                record.delta0 = 1;
                return Ok(record);
            }
            if index + 1 != vcnt {
                record.delta0 = lo;
                return Ok(record);
            }
            // The last entry of a pack stores the distance to the next head
            // (delta[1]); the distance to this entry's own head is derived
            // from the previous entry.
            let (previous, previous_kind) =
                Self::compact_entry(pack, index - 1, encodebits, lobits)?;
            let delta0 = if previous_kind != LCLUSTER_NONHEAD {
                0
            } else if previous & LI_D0_CBLKCNT != 0 {
                1
            } else {
                previous
            };
            record.delta0 = delta0 + 1;
            return Ok(record);
        }

        record.clusterofs = u32::from(lo);
        if record.clusterofs >= (1u32 << self.lclusterbits) {
            return Err(Error::InvalidCompressedData("cluster offset out of range"));
        }

        // Count the pcluster blocks that precede this head by walking the
        // entries before it in the pack.  The trailing 32-bit word of the pack
        // is the base pcluster address.
        let big_pcluster = self.advise & ADVISE_BIG_PCLUSTER_1 != 0;
        let mut blocks = 0u16;
        let mut i = index as i64;
        if !big_pcluster {
            blocks = 1;
            while i > 0 {
                i -= 1;
                let (lo, entry_kind) = Self::compact_entry(pack, i as u64, encodebits, lobits)?;
                if entry_kind == LCLUSTER_NONHEAD {
                    i -= i64::from(lo);
                }
                if i >= 0 {
                    blocks = blocks
                        .checked_add(1)
                        .ok_or(Error::InvalidCompressedData("pcluster count"))?;
                }
            }
        } else {
            while i > 0 {
                i -= 1;
                let (lo, entry_kind) = Self::compact_entry(pack, i as u64, encodebits, lobits)?;
                if entry_kind == LCLUSTER_NONHEAD {
                    if lo & LI_D0_CBLKCNT != 0 {
                        i -= 1;
                        blocks = blocks
                            .checked_add(lo & !LI_D0_CBLKCNT)
                            .ok_or(Error::InvalidCompressedData("pcluster count"))?;
                        continue;
                    }
                    if lo <= 1 {
                        return Err(Error::InvalidCompressedData("big pcluster distance"));
                    }
                    i -= i64::from(lo) - 2;
                    continue;
                }
                blocks = blocks
                    .checked_add(1)
                    .ok_or(Error::InvalidCompressedData("pcluster count"))?;
            }
        }
        let base = u32_at::<LittleEndian>(pack, size - 4).ok_or(Error::Truncated)?;
        record.pblk = base
            .checked_add(u32::from(blocks))
            .ok_or(Error::InvalidCompressedData("pcluster address"))?;
        Ok(record)
    }
}

/// Drop the leading zero padding of a pcluster.
///
/// With the `lz4_0padding` feature the LZ4 stream ends at the end of the
/// pcluster and the space before it is zero-filled.  As in the kernel, only
/// the first block is searched.
fn strip_zero_padding(data: &[u8], offset: u64, blocksize: u64) -> Result<&[u8]> {
    let in_block = offset % blocksize;
    let limit = (blocksize - in_block).min(data.len() as u64) as usize;
    let first = data[..limit]
        .iter()
        .position(|&byte| byte != 0)
        .ok_or(Error::InvalidCompressedData("pcluster is all padding"))?;
    Ok(&data[first..])
}

/// The LZ4 block-format decoder.
pub(crate) mod lz4 {
    use crate::error::{Error, Result};

    /// Decode an LZ4 block, filling `output` completely.
    ///
    /// Decoding stops as soon as `output` is full, so the caller may ask for a
    /// prefix of a pcluster that holds more data; this is the behaviour of
    /// `LZ4_decompress_safe_partial`.  Input after the point where the output
    /// filled is not examined.
    pub(crate) fn decompress_prefix(input: &[u8], output: &mut [u8]) -> Result<()> {
        if output.is_empty() {
            return Ok(());
        }
        let mut ip = 0usize;
        let mut op = 0usize;
        while op < output.len() {
            let token = *input
                .get(ip)
                .ok_or(Error::InvalidCompressedData("truncated LZ4 token"))?;
            ip += 1;

            let mut literals = usize::from(token >> 4);
            if literals == 15 {
                literals = literals
                    .checked_add(read_length(input, &mut ip)?)
                    .ok_or(Error::InvalidCompressedData("literal length overflow"))?;
            }
            let available = output.len() - op;
            let take = literals.min(available);
            let source = input
                .get(ip..ip + take)
                .ok_or(Error::InvalidCompressedData("truncated LZ4 literals"))?;
            output[op..op + take].copy_from_slice(source);
            ip += take;
            op += take;
            if literals > take {
                // The output filled in the middle of the literal run.
                return Ok(());
            }
            if op == output.len() {
                return Ok(());
            }

            let low = *input
                .get(ip)
                .ok_or(Error::InvalidCompressedData("truncated LZ4 offset"))?;
            let high = *input
                .get(ip + 1)
                .ok_or(Error::InvalidCompressedData("truncated LZ4 offset"))?;
            ip += 2;
            let offset = usize::from(u16::from_le_bytes([low, high]));
            if offset == 0 || offset > op {
                return Err(Error::InvalidCompressedData("invalid LZ4 match offset"));
            }

            let mut length = usize::from(token & 0x0f);
            if length == 15 {
                length = length
                    .checked_add(read_length(input, &mut ip)?)
                    .ok_or(Error::InvalidCompressedData("match length overflow"))?;
            }
            let length = length
                .checked_add(4)
                .ok_or(Error::InvalidCompressedData("match length overflow"))?;
            let take = length.min(output.len() - op);
            let start = op - offset;
            for index in 0..take {
                output[op + index] = output[start + index];
            }
            op += take;
            if length > take {
                // The output filled in the middle of the match.
                return Ok(());
            }
        }
        Ok(())
    }

    /// Read an LZ4 extended length: 255 bytes add 255 each, the final byte is
    /// the remainder.
    fn read_length(input: &[u8], position: &mut usize) -> Result<usize> {
        let mut length = 0usize;
        loop {
            let byte = *input
                .get(*position)
                .ok_or(Error::InvalidCompressedData("truncated LZ4 length"))?;
            *position += 1;
            length = length
                .checked_add(usize::from(byte))
                .ok_or(Error::InvalidCompressedData("LZ4 length overflow"))?;
            if byte != 255 {
                return Ok(length);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::lz4::decompress_prefix;
    use super::strip_zero_padding;
    use crate::error::Error;
    use alloc::vec;
    use alloc::vec::Vec;

    fn decode(input: &[u8], len: usize) -> Vec<u8> {
        let mut out = vec![0u8; len];
        decompress_prefix(input, &mut out).expect("decode");
        out
    }

    #[test]
    fn literals_only() {
        assert_eq!(decode(&[0x50, b'h', b'e', b'l', b'l', b'o'], 5), b"hello");
    }

    #[test]
    fn literals_and_overlapping_match() {
        // Four literals followed by a 9-byte match at distance 4.
        let block = [0x45, b'a', b'b', b'c', b'd', 0x04, 0x00];
        assert_eq!(decode(&block, 13), b"abcdabcdabcda");
    }

    #[test]
    fn prefix_stops_before_the_end_of_the_block() {
        let block = [0x45, b'a', b'b', b'c', b'd', 0x04, 0x00];
        assert_eq!(decode(&block, 6), b"abcdab");
    }

    #[test]
    fn truncated_literals_are_rejected() {
        let mut out = vec![0u8; 8];
        assert!(matches!(
            decompress_prefix(&[0x80, b'a'], &mut out),
            Err(Error::InvalidCompressedData(_))
        ));
    }

    #[test]
    fn zero_match_offset_is_rejected() {
        let mut out = vec![0u8; 8];
        assert!(matches!(
            decompress_prefix(&[0x10, b'a', 0x00, 0x00], &mut out),
            Err(Error::InvalidCompressedData(_))
        ));
    }

    #[test]
    fn match_before_the_output_start_is_rejected() {
        let mut out = vec![0u8; 8];
        assert!(matches!(
            decompress_prefix(&[0x20, b'a', b'b', 0x05, 0x00], &mut out),
            Err(Error::InvalidCompressedData(_))
        ));
    }

    #[test]
    fn missing_token_is_rejected() {
        let mut out = vec![0u8; 4];
        assert!(matches!(
            decompress_prefix(&[], &mut out),
            Err(Error::InvalidCompressedData(_))
        ));
    }

    #[test]
    fn long_literal_run_uses_extension_bytes() {
        let mut block = vec![0xf0, 20];
        block.extend_from_slice(&[b'x'; 35]);
        assert_eq!(decode(&block, 35), vec![b'x'; 35]);
    }

    #[test]
    fn zero_padding_is_trimmed_from_the_first_block() {
        let data = [0, 0, 0, 0x50, b'a'];
        assert_eq!(
            strip_zero_padding(&data, 0, 8).expect("padding"),
            &[0x50, b'a']
        );
    }

    #[test]
    fn all_zero_pcluster_is_rejected() {
        let data = [0u8; 16];
        assert!(matches!(
            strip_zero_padding(&data, 0, 8),
            Err(Error::InvalidCompressedData(_))
        ));
    }

    #[test]
    fn zero_padding_search_stops_at_the_block_boundary() {
        // At offset 6 only two bytes of an 8-byte block remain to search.
        let data = [0, 0, 0x50, b'a'];
        assert!(matches!(
            strip_zero_padding(&data, 6, 8),
            Err(Error::InvalidCompressedData(_))
        ));
    }
}
