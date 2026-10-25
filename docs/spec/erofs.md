# EROFS on-disk profile

**Status:** draft
**Version:** 0.1
**Owner:** storage
**Implementation:** [`crates/erofs`](../../crates/erofs)
**Oracle:** `mkfs.erofs` 1.9.4, `fsck.erofs`, `dump.erofs`

## 1. Scope

This document fixes the subset of the standard EROFS format that IRIX 7 reads in
v0.1.  The image is **standard little-endian EROFS**; the format is not forked for
the Indy's big-endian CPU (see the byte-order research note).  The reader is a
new `no_std` crate, not a port of `erofs-rs`.

The v0.1 profile covers the core read-only path needed by the root image:

- the 128-byte base superblock and its 48-bit extension fields;
- compact and extended inodes;
- flat plain and flat inline file data;
- compressed-full and compressed-compact file data with LZ4 pclusters,
  including big pclusters and inline (`ztailpacking`) tail pclusters;
- directories and symbolic links;
- one device (the primary image).

Fragments, encoded extents, interlaced pclusters, chunk-based files and metabox
images are recognised and rejected with typed errors.  They are the next layer.

## 2. Byte order

Every multi-byte field is little-endian.  The reader routes all field access
through `ByteOrder` (`LittleEndian` for EROFS, `BigEndian` for codec tests); there
are no native-endian loads.  The magic is checked as a value, not as a byte
pattern, so the codec can be tested with a big-endian copy of the same fields.

The standard format is identified by the magic `0xE0F5E1E2` at image offset 1024.
An image whose bytes there are not `E2 E1 F5 E0` is rejected with
`Error::BadMagic`.

## 3. Superblock

The base superblock starts at **image offset 1024** and is 128 bytes.  Extended
slots of 16 bytes follow it when `sb_extslots` is non-zero.

| Field | Offset | Width | Notes |
|---|---:|---:|---|
| `magic` | 0 | 4 | `0xE0F5E1E2` |
| `checksum` | 4 | 4 | CRC32C when `sb_csum` is set; not verified in v0.1 |
| `feature_compat` | 8 | 4 | Unknown bits are safe to ignore |
| `blkszbits` | 12 | 1 | 9..=16 accepted; 12 (4096 bytes) is the normal value |
| `sb_extslots` | 13 | 1 | Superblock size = 128 + 16 * slots |
| `rootnid_2b` / `blocks_hi` | 14 | 2 | Root nid, or the MSB of the block count when `rootnid_8b` is present |
| `inos` | 16 | 8 | Total inode count |
| `epoch` | 24 | 8 | Base for compact inode mtimes |
| `fixed_nsec` | 32 | 4 | Fixed nanoseconds for compact inodes |
| `blocks_lo` | 36 | 4 | Block count LSB |
| `meta_blkaddr` | 40 | 4 | First block of the shared metadata area |
| `xattr_blkaddr` | 44 | 4 | First block of the shared xattr area |
| `uuid` | 48 | 16 | Volume UUID |
| `volume_name` | 64 | 16 | NUL-padded label |
| `feature_incompat` | 80 | 4 | Unknown bits are a hard mount failure |
| `available_compr_algs` / `lz4_max_distance` | 84 | 2 | Compression configuration |
| `extra_devices` | 86 | 2 | Multi-device support is rejected |
| `devt_slotoff` | 88 | 2 | Device table offset |
| `dirblkbits` | 90 | 1 | Directory block size shift |
| `xattr_prefix_count` | 91 | 1 | Long xattr prefixes |
| `xattr_prefix_start` | 92 | 4 | Long xattr prefix table |
| `packed_nid` | 96 | 8 | Packed inode (compression/fragments) |
| `xattr_filter_reserved` | 104 | 1 | Xattr filter |
| `ishare_xattr_prefix_id` | 105 | 1 | Ishare xattrs |
| `reserved` | 106 | 2 | Zero |
| `build_time` | 108 | 4 | Seconds added to `epoch` for the build time |
| `rootnid_8b` | 112 | 8 | 48-bit root nid, present only when needed |
| `reserved2` | 120 | 8 | Zero |
| `metabox_nid` | 128 | 8 | Metabox; requires an extended slot and is rejected |

### 3.1 The 48-bit root nid

`rootnid_8b` is not simply "read this field when the 48-bit feature is set".
`mkfs.erofs` writes it only when the root nid does not fit in 16 bits; the same
condition populates `blocks_hi`, which shares offset 14 with `rootnid_2b`.
Therefore:

```text
if feature_incompat & 48BIT and rootnid_8b != 0:
    root_nid = rootnid_8b
    blocks   = blocks_lo | (blocks_hi << 32)
else:
    root_nid = rootnid_2b
    blocks   = blocks_lo
```

The 48-bit fixture exercises exactly this case: the feature bit is set and the
root nid (36) still fits in 16 bits, so `rootnid_8b` is zero.

### 3.2 Feature policy

`feature_compat` may contain unknown bits; they are ignored.  `feature_incompat`
must be a subset of the bits defined by the current format.  The reader rejects:

- unknown incompatible bits;
- `METABOX` (the inode location changes);
- `DEVICE_TABLE` and any non-zero `extra_devices` (only the primary device is
  supported).

Compression feature bits are accepted at mount time because an image may contain
both uncompressed and compressed inodes.  A compressed inode is decoded with the
LZ4 rules in §5.4; an inode that names another algorithm, fragments, encoded
extents or an interlaced pcluster is rejected with
`UnsupportedCompression`.  The SHA-256 of the whole image is the integrity check
in the boot path; the superblock CRC32C is optional and not verified by this
crate.

## 4. Inodes

Node IDs are byte offsets in units of **32 bytes** from the metadata area:
`iloc = meta_blkaddr * blocksize + nid * 32`.  An extended inode is 64 bytes and
occupies two slots; the following inode's nid therefore advances by two.

### 4.1 Compact inode (32 bytes)

| Field | Offset | Width |
|---|---:|---:|
| `i_format` | 0 | 2 |
| `i_xattr_icount` | 2 | 2 |
| `i_mode` | 4 | 2 |
| `i_nb` | 6 | 2 |
| `i_size` | 8 | 4 |
| `i_mtime` | 12 | 4 |
| `i_u` | 16 | 4 |
| `i_ino` | 20 | 4 |
| `i_uid` | 24 | 2 |
| `i_gid` | 26 | 2 |
| `i_reserved` | 28 | 4 |

### 4.2 Extended inode (64 bytes)

| Field | Offset | Width |
|---|---:|---:|
| `i_format` | 0 | 2 |
| `i_xattr_icount` | 2 | 2 |
| `i_mode` | 4 | 2 |
| `i_nb` | 6 | 2 |
| `i_size` | 8 | 8 |
| `i_u` | 16 | 4 |
| `i_ino` | 20 | 4 |
| `i_uid` | 24 | 4 |
| `i_gid` | 28 | 4 |
| `i_mtime` | 32 | 8 |
| `i_mtime_nsec` | 40 | 4 |
| `i_nlink` | 44 | 4 |
| `i_reserved2` | 48 | 16 |

### 4.3 Format bits

`i_format` bit 0 selects the extended layout.  Bits 1..3 are the data layout:

| Value | Layout | v0.1 |
|---:|---|---|
| 0 | flat plain | supported |
| 1 | compressed, full indexes | supported (§5.4) |
| 2 | flat inline | supported |
| 3 | compressed, compact indexes | supported (§5.4) |
| 4 | chunk-based | rejected with `UnsupportedLayout(4)` |
| 5..7 | reserved | rejected with `UnsupportedLayout(n)` |

Bit 4 has two meanings: for a non-directory compact inode it is `nlink_1`
(`i_nb` holds the high start-block bits and the link count is one); for a
directory it is `dot_omitted` (`i_nb` remains the link count).  Extended inodes
always carry the link count in `i_nlink`; `i_nb` is the high start-block word.

`i_u` is the start block for flat layouts, the block count for compressed
layouts, the encoded device for special files, and the chunk format for
chunk-based inodes.

### 4.4 Inline data and xattrs

Inline xattrs, when present, occupy
`12 + 4 * (i_xattr_icount - 1)` bytes immediately after the inode.  The inline
file tail (for layout 2) starts after the inode and any xattrs:

```text
inline_offset = meta_blkaddr * blocksize + nid * 32
              + inode_size + xattr_size
```

The reader does not interpret xattrs, but it must skip them to find the inline
tail.  The `extended.erofs` fixture exercises 64-byte inodes; the `root.erofs`
fixture exercises compact inodes.

## 5. File data

### 5.1 Flat plain (0)

The file data is at `start_blk * blocksize`, `i_size` bytes long.  A size that is
not a block multiple is read only for the declared length.

### 5.2 Flat inline (2)

```text
full = size - (size % blocksize)
```

The first `full` bytes are at `start_blk * blocksize`.  The remaining
`size % blocksize` bytes are at `inline_offset`.  `start_blk` may be
`0xFFFFFFFF` (`EROFS_NULL_ADDR`) when `full` is zero; the reader rejects a null
start block only when it would actually read a full block.

`read_at(inode, offset, buf)` crosses the full/tail boundary without allocating
and checks `offset + buf.len() <= size`.  A buffered `read_file` is capped at
64 MiB; larger files should be streamed with `read_at`.

### 5.3 48-bit block addresses

For flat inodes, the high 16 bits of the start block are in `i_nb` when the
48-bit feature is on and the inode form can carry them.  The reader combines
them; if the feature is off, the high word is ignored.

### 5.4 Compressed file data (1 and 3)

A compressed inode stores one *logical cluster index* per filesystem block.
The index maps the logical cluster to a *physical cluster* (pcluster) that
holds the compressed bytes for one or more logical clusters.  The reader keeps
this mapping independent of the inode's own `i_u` block count: `i_u` is the
number of physical blocks used by the file, not a data address.

#### 5.4.1 Map header

The 8-byte `z_erofs_map_header` starts at
`ALIGN(iloc + inode_size + xattr_size, 8)`.  `inline_offset` in the decoded
inode already carries the unaligned end of the inode and xattrs.

| Field | Offset | Width | Notes |
|---|---:|---:|---|
| `h_fragmentoff` / `h_reserved1` | 0 | 4 | Fragments are rejected |
| `h_idata_size` | 2 | 2 | Encoded inline pcluster size |
| `h_advise` | 4 | 2 | See below |
| `h_algorithmtype` | 6 | 1 | Low nibble HEAD1, high nibble HEAD2 |
| `h_clusterbits` | 7 | 1 | Bits 0..3 add to `blkszbits`; bit 7 is the packed-inode flag |

`h_advise` bits:

| Bit | Name | Handling |
|---:|---|---|
| 0x0001 | `COMPACTED_2B` (layout 3) / `EXTENTS` (layout 1) | 2-byte packs; encoded extents are rejected |
| 0x0002 | `BIG_PCLUSTER_1` | Pcluster spans several blocks for HEAD1 |
| 0x0004 | `BIG_PCLUSTER_2` | The same for PLAIN and HEAD2 |
| 0x0008 | `INLINE_PCLUSTER` | The last pcluster is inline in metadata |
| 0x0010 | `INTERLACED_PCLUSTER` | Rejected |
| 0x0020 | `FRAGMENT_PCLUSTER` | Rejected |

The reader accepts only `h_clusterbits & 0x0f == 0`: logical clusters are the
same size as filesystem blocks.  `h_clusterbits` bit 7 (the whole file in the
packed inode) is also rejected.  The algorithm nibbles are `0` LZ4, `1` LZMA,
`2` DEFLATE and `3` Zstandard; only LZ4 is decoded.  A pcluster whose head
names another algorithm returns `UnsupportedCompression` and its bytes are
never returned as file data.

#### 5.4.2 Full indexes (layout 1)

`full_index_start = ALIGN(iloc + inode_size + xattr_size, 8) + 16`.  Entry
`lcn` is 8 bytes at `full_index_start + lcn * 8`:

| Field | Offset | Width |
|---|---:|---:|
| `di_advise` | 0 | 2 |
| `di_clusterofs` | 2 | 2 |
| `di_u.blkaddr` / `di_u.delta[2]` | 4 | 4 |

The low two bits of `di_advise` are the cluster type: 0 plain, 1 head 1,
2 non-head, 3 head 2.  For a head, `di_u.blkaddr` is the pcluster's first
block and `di_clusterofs` is the offset of the extent inside its first logical
cluster.  For a non-head, `di_u.delta[0]` is the lookback distance to the head,
`di_u.delta[1]` the distance to the next head, and bit 11 of `delta[0]`
(`CBLKCNT`) instead carries the pcluster's block count.  A head with bit 15 set
(`PARTIAL_REF`) is rejected.

#### 5.4.3 Compact indexes (layout 3)

Compact indexes pack several entries into a 32-byte or 8-byte group.  The
layout mirrors `z_erofs_load_compact_lcluster`:

```text
ebase = ALIGN(iloc + inode_size + xattr_size, 8) + 8
initial = ((32 - ebase % 32) / 4) & 7        # 4-byte entries at the start
compacted_2b = rounddown(totalidx - initial, 16) if COMPACTED_2B else 0
totalidx = ceil(size / blocksize)
```

The first `initial` entries use 8-byte groups of two 16-bit entries followed by
a 32-bit pcluster base.  The next `compacted_2b` entries use 32-byte groups of
sixteen 14-bit entries followed by the same 32-bit base.  Any remaining entries
use 8-byte groups again.  `encodebits` is 16 for a 4-byte pack and 14 for a
2-byte pack; the low `max(lclusterbits, 12)` bits are the value and the next two
bits the cluster type.

The 32-bit base in a group is the pcluster block before the group.  A head adds
the number of pcluster blocks that precede it, which is one per head entry plus
the `CBLKCNT` values of big-pcluster non-head entries.  The reader walks the
entries before the head inside the same group for this count, exactly as the
kernel does.  This is why the trailing base word is always present even when a
group is only partly used.

#### 5.4.4 Mapping a file offset

For a read at byte `offset`, the reader loads the index for
`lcn = offset / blocksize`.  A head entry with
`clusterofs <= offset % blocksize` owns the offset; otherwise the reader follows
`delta[0]` back through non-head entries to the head.  The extent starts at
`(head_lcn * blocksize) + head_clusterofs` and ends at the next head boundary,
or at `i_size` for the last inline extent.

The pcluster's encoded length is one block unless the head's advise enables big
pclusters.  In that case the first non-head entry after the head may carry
`CBLKCNT`; otherwise the length is one block.  The decoded length is bounded by
the extent length and by 12 MiB, and the encoded length by 1 MiB.  A corrupt
image that exceeds either limit returns `InvalidCompressedData`.

#### 5.4.5 Inline tail pclusters

With `ztailpacking`, the last pcluster is stored in the metadata block after
the inode's index area.  Its offset is the end of the index group that holds
the logical cluster containing the last byte of the file, and its length is
`h_idata_size`.  The reader computes this offset before the lookback, matching
`EROFS_GET_BLOCKS_FINDTAIL`, and rejects inline data that would cross a block
boundary.  The head logical cluster is still found through the normal lookback,
so an inline pcluster may cover several logical clusters ending mid-cluster.

#### 5.4.6 Decompression

A pcluster is decoded into a buffer covering the requested prefix of the
extent's logical data (at most the extent length):

- **PLAIN** heads store the bytes uncompressed ("shifted"); the reader copies
  the requested prefix, requiring it to fit in `plen`.  Interlaced plain
  pclusters are rejected.
- **HEAD1/HEAD2** pclusters are LZ4 blocks.  With `lz4_0padding`, the leading
  zero bytes of the first block are skipped to find the stream, which ends at
  the pcluster's end.  The decoder stops as soon as the requested prefix is
  full, like `LZ4_decompress_safe_partial`, so a pcluster that decodes to more
  data than the current extent asks for is still read correctly.

The LZ4 decoder is written in the crate with no unsafe code and no dependency.
It validates every input read, every match offset and every output bound; a
malformed stream returns `InvalidCompressedData`.

The decoder is behind the default `lz4` feature.  The kernel's HTTP EROFS
command depends on the crate with `default-features = false` to stay inside the
512 KiB PROM window; it rejects compressed inodes with `UnsupportedLayout`.
The loadable `modules/erofs` and `modules/mcp` images enable `lz4` and decode
the compressed fixtures.

A read that crosses a pcluster boundary maps and decodes each pcluster in turn.
The reader does not cache decompressed data; the caller's block cache still
serves the repeated metadata reads.

## 6. Directories

Directory data uses the same file-data rules (flat or compressed).  Each
directory block is a sequence of 12-byte entries followed by the names:

| Field | Offset | Width |
|---|---:|---:|
| `nid` | 0 | 8 |
| `nameoff` | 8 | 2 |
| `file_type` | 10 | 1 |
| `reserved` | 11 | 1 |

The first entry's `nameoff` is the byte offset at which names begin; it must be
at least 12 and a multiple of 12.  The number of entries in the block is
`first_nameoff / 12`.  Each name runs from its `nameoff` to the next entry's
`nameoff`, or to the end of the block for the last entry.  Names are trimmed at
the first NUL (the writer zero-pads the last block) and are limited to 255 bytes.
`.` and `..` are ordinary entries.  Resolution skips `.` and implements `..`
by walking back through the directories it descended into, because the format
has no parent pointer; `..` at the root stays at the root.  A `.` or `..`
component applied to a non-directory returns `NotDirectory`.

`file_type` uses the standard EROFS values: 1 regular, 2 directory, 3 character
device, 4 block device, 5 FIFO, 6 socket, 7 symlink.  A dirent nid with bit 63
set is a metabox inode and is rejected.

The reader reads the whole directory through `read_file` for now.  The metadata
cost is small; a future block-at-a-time iterator will avoid buffering large
directories.

## 7. Symbolic links

A symlink's target is its file data (normally flat inline).  `Inode::is_symlink`
is derived from `i_mode`; `read_link` rejects non-symlinks and otherwise returns
the target bytes.

There are two resolvers.  `lookup_path` never follows a symlink: a path that
ends at a link returns the link inode, which is what `read_link` expects.
`lookup_path_follow` follows links and returns the final target.
A relative target is resolved from the directory containing the link, an
absolute target from the root, and any remaining components are resolved
against the target: `/link/..` returns `NotDirectory`, because `link` resolves
to a regular file.

Following is bounded by `MAX_SYMLINK_DEPTH` (8).  A chain longer than that,
including a link that points at itself, returns `Error::SymlinkLoop` rather than
looping.  A target that is not valid UTF-8 returns
`InvalidInode("symlink target is not UTF-8")`, because the path API is textual.
`read_file_path` uses the following resolver, so `cat /link` prints the target
file.

## 8. Errors and limits

All on-disk offsets and lengths are checked with `checked_add`/`checked_mul`.
The reader returns:

- `Io` for a device read that cannot be satisfied;
- `Truncated` for a structure shorter than its definition;
- `BadMagic`, `InvalidSuperblock`, `InvalidInode`, `InvalidDirectory` for
  inconsistent fields;
- `UnsupportedFeature`, `UnsupportedLayout`, `Unsupported`,
  `UnsupportedCompression` for formats outside this profile;
- `InvalidCompressedData` for a malformed map header, index, pcluster length or
  LZ4 stream;
- `SymlinkLoop` for a chain of more than `MAX_SYMLINK_DEPTH` links;
- `OutOfBounds`, `TooLarge`, `OutOfMemory`, `NameTooLong` for read limits.

The crate contains no `unsafe` code.

## 9. API

`BlockDevice` is byte-addressed:

```rust
pub trait BlockDevice {
    fn read_at(&mut self, offset: u64, buf: &mut [u8]) -> Result<()>;
}
```

`&[u8]` implements it for host tests.  A SCSI disk driver or an HTTP Range
client implements the same trait in the kernel.

```rust
let mut fs = Erofs::mount(device)?;
let root = fs.root()?;
let inode = fs.lookup_path("/etc/motd")?;
let text = fs.read_file(&inode)?;
let target = fs.lookup_path_follow("/link")?;
for entry in fs.read_dir(&root)? { /* ... */ }
```

`lookup_path` is the no-follow resolver; `lookup_path_follow` follows symbolic
links with the bound described in §7, and `read_file_path` uses it.  Paths are
always resolved from the root, with or without a leading slash; there is no
per-process current directory.

The reader keeps no cache; the caller owns the block cache.  The kernel's
`erofs_cmd` probes the superblock once to learn the image size, then uses
`http::get_range_to` or `tls::get_range_to` with a 256 KiB read-ahead window
clamped to the final block.  The first probe is 4 KiB; the mount then reads the
superblock, metadata and inline data from that window, and a file fetch replaces
it with the clamped read-ahead.  The kernel command is built only with the
`erofs-debug` feature (off by default to keep the compressed PROM small); the
loadable `erofs` module is the normal filesystem path.  With the feature enabled,
it is reached from the telnet console:

```text
erofs [http|https]://<host>:<port><path> [sha256=hex] [info|ls|cat] [path]
```

A bare `host:port/path` is HTTP, as before.  `info` prints the superblock, `ls`
lists a directory and `cat` prints a file.  Both `ls` and `cat` follow symbolic
links: `ls` works on a link to a directory and `cat /link` prints the contents
of `hello.txt`.  With `sha256=`, the command first performs a full GET, checks
the status, the length against the superblock's block count, and the
whole-image SHA-256; a mismatch aborts before mount.  The verification GET
currently transfers the image a second time; hashing while filling the range
cache is future work.

With `https://`, every range request uses the kernel TLS client and must return
`206` with exactly the requested length, so the server has to honour `Range`
over TLS.  Chains are verified against the single embedded root, SSL.com TLS
ECC Root CA 2022 (`kernel/src/ca/`), with the hostname checked; a host whose
chain ends elsewhere is rejected, and there is no fallback to HTTP.  Each range
is a fresh TLS 1.3 handshake because the client does not keep a session, and
the per-fetch record buffers still come from the bump heap, so a TLS mount costs
about 29 KiB of heap per range.  The digest check is a single full-body GET
with the TLS client's 30-second response deadline, so it is practical for
fixture-sized images; a large image over HTTPS needs the streaming digest
first.  Reusing the TLS record buffers across range requests and a persistent
session are the follow-ups; neither changes the on-disk path.

The disk filesystem path is a loadable module, not a kernel command.  The
`modules/erofs` module mounts an EROFS image at LBA 0 of SCSI target 2, reads
`/etc/motd` and prints it through the console service.  It uses the ABI 10+
`block_read` service, `block-cache`'s LRU (64 x 4 KiB, 256 KiB), its own global
allocator over the kernel `alloc` service, and the same EROFS reader.  The flat
module is 79,160 bytes and never enters the compressed PROM.  On iris it prints
the motd content and returns 0.  Descriptor DMA will replace the polled PIO
sector path.

## 10. Verification

`cargo test -p erofs` runs 30 integration tests (16 filesystem, 11 compressed,
3 codec) plus 11 unit tests for the LZ4 decoder and zero padding, against six
golden images:

- the full tree on compact inodes;
- the same tree on extended inodes;
- the same tree with the 48-bit feature;
- the same tree plus `many.bin` compressed with LZ4, compact indexes and
  `ztailpacking`;
- the same compressed tree with the full (legacy) index layout;
- the same compressed tree with 16 KiB big pclusters.

The uncompressed tests cover the superblock, root directory, nested
directories, an empty file, a symlink, symlink following (relative, absolute and
loop-bounded targets), `.` and `..` traversal, a 9000-byte flat-inline file
(including a read crossing the 8192-byte boundary), zero-length reads at EOF,
bad magic, unknown incompatibility bits, truncation, an invalid block size, and
an unknown data layout that must be recognised rather than misread.

The compressed tests mount all three compressed images and compare every file
with the uncompressed fixture and a generated expected pattern.  They read
`/big.bin` (one inline pcluster) and `/many.bin` (three block pclusters plus an
inline tail, crossing cluster and extent boundaries), and assert that the map
header really carries the inline-pcluster advise.  Error tests corrupt the
algorithm nibble, the logical cluster bits, the advise bits, a whole pcluster
and a truncated LZ4 stream, and expect the typed errors above.  The codec tests
decode both byte orders.

The absolute-target and loop tests do not add fixtures: each copies the compact
image in memory and rewrites the target of `/link`, which is a compact inline
symlink, keeping the metadata layout and its size unchanged.  The loop test
points `/link` at itself and expects `SymlinkLoop`.

The images are generated by `crates/erofs/tests/fixtures/build.sh` with
`mkfs.erofs 1.9.4`, checked with `fsck.erofs`, and committed so CI does not need
the tool.  Their SHA-256 digests are recorded in the fixture README.  The three
uncompressed images are unchanged by the compressed-fixture addition; the
compressed tree is a copy plus `many.bin`.

The kernel command was verified on the iris guest against
[`rom/tools/range-server.py`](../../rom/tools/range-server.py): `info`, `ls /`
(eight entries including `.`, `..` and the symlink), `cat /etc/motd` and
`cat /big.bin` (the 9000-byte file, read through two data blocks and an inline
tail).  Each command mounts the image afresh; an `info` or `ls` issues one 4 KiB
probe range, while `cat /big.bin` issues the probe plus one clamped read-ahead
request covering the whole 12 KiB image.  A correct `sha256=` prints
`erofs: sha256 ok`; an all-zero digest prints `erofs: sha256: digest mismatch`
and does not mount.  The window is 256 KiB, not the fixture's 12 KiB: against a
1.25 MiB test image, `cat /big.bin` issues five full 256 KiB ranges and a 4 KiB
tail, and the serial diagnostic reports each as `body=00040000` with a matching
`206` `Content-Length`.

### 10.1 HTTPS range check

A public host for the TLS range path must both honour `Range` over TLS and
chain to the embedded SSL.com root.  `example.com` does both (Cloudflare
answers `Range: bytes=0-99` with `206`), so the guest check is:

```text
https example.com:443/ range=0-99
https example.com:443/ <sha256 of the first 100 bytes> range=0-99
https example.com:443/
```

The first command prints `https: 206 range=0-99 100 bytes` and the digest of
those 100 bytes, which `curl -s https://example.com/ | head -c 100 | sha256sum`
reproduces on the host; the second prints `verify: ok`.  The full-body command
keeps the old output and checks that the range path did not change it.
`raw.githubusercontent.com` honours `Range` too, but its chain ends at
USERTrust/Sectigo, so the kernel rejects it until another root is embedded.

## 11. Future work

- Fragments, the packed inode and `all-fragments`.
- Encoded extents (the 48-bit compressed layout) and interlaced pclusters.
- Non-LZ4 algorithms (LZMA, DEFLATE, Zstandard).
- Chunk-based files and multi-device images.
- Metabox metadata.
- Superblock CRC32C verification.
- A block-at-a-time directory iterator.
- One shared LRU in the block cell, so the kernel console and the modules stop
  keeping separate 256 KiB caches.
- Reuse the TLS record buffers across HTTPS range requests and keep the session
  open, so a TLS mount costs one handshake rather than one per range.
- Hash the image while filling that cache, instead of a second full GET, and
  take the expected digest from the boot environment.  This also removes the
  full-body GET's single response deadline, which currently caps an HTTPS
  image at roughly what fits in 30 seconds of transfer.
- A SCSI `BlockDevice` and the block cell capability.
- An overlay for `/tmp`, `/etc` and `/home`.
- An independent tree diff against `erofs-rs` and a host mount in CI.

## 12. References

- Linux `fs/erofs/erofs_fs.h` (MIT), the authoritative field definitions.
- EROFS on-disk format specification, `erofs-utils` `src/ondisk/`.
- `erofs-utils` 1.9.4: `mkfs.erofs`, `fsck.erofs`, `dump.erofs`.
- [Architecture §11.2](architecture.md#112-root-image-erofs) and
  [byte-order research](../research/erofs-big-endian-research.md).
