# Forking EROFS to big-endian for IRIX 7 (SGI Indy, IP24)

**Question.** Can EROFS be forked to a big-endian (BE) on-disk format for IRIX 7 — a
clean-room GPL-3.0 Rust OS on an R4400 (MIPS III, big-endian, 32–128 MB RAM, 10 Mbit
Ethernet) whose root is a read-only image fetched from Cloudflare R2? And *should* it be?

**Short answer.** Yes, it can — the format is small, fully specified, and re-encoding it is
mechanical. It should not be. The byte-order fork buys on the order of **tens of
milliseconds per boot** and costs the only independent oracle the project has (the Linux
driver, `fsck.erofs`, `dump.erofs`, `erofs-rs`, host mounting) for a filesystem being
written from scratch. The measurement in §5 shows metadata decoding is **0.01–0.05 %** of
the cold-boot time on this machine, because the 10 Mbit link and decompression dominate by
two to four orders of magnitude.

**Scope and assumptions.**

| Item | Value used in this report | Source |
|---|---|---|
| CPU | MIPS R4400, MIPS III, big-endian, 16 KB/16 KB L1, 1 instruction/cycle, no `wsbh`, no `lwle`/`lwre` | [R4000 UM] ch. 4; local `iris/R4000_um2.txt`; `indy-hardware.md` §2 |
| Clock | 75 MHz (Indy R4400SC); 100 MHz variants treated as 1.33× faster | `indy-hardware.md` §2 table |
| RAM | 32–128 MB (32 MB is the binding constraint) | project brief |
| Network | 10 Mbit Ethernet ⇒ 1.25 MB/s wire, ≈1.1 MB/s TCP goodput | §5.5 |
| Image source | Cloudflare R2 (HTTP range GET), cached on SCSI | project brief |
| EROFS reference | Linux master as of 2026-10 (`fs/erofs/erofs_fs.h`, 144-byte superblock, metabox/48-bit present) | [FSH] |
| erofs-utils | 1.9.4 (2026-08-19) | [UTILS] `VERSION` |

Where a number is an estimate rather than a measurement it is marked **(est.)** and the
basis is given.

**Contents.** [0. Executive summary](#0-executive-summary) ·
[1. On-disk byte order](#1-erofs-on-disk-format-what-is-little-endian-what-is-not) ·
[2. Checksums and compression](#2-checksums-and-compression) ·
[3. Host tooling](#3-host-tooling-mkfserofs-and-erofs-utils) ·
[4. Reader side](#4-reader-side-linux-erofs-rs-and-a-no_std-rust-reader) ·
[5. Cost of not forking](#5-the-actual-cost-of-not-forking) ·
[6. Alternatives](#6-alternatives) ·
[7. Recommendation](#7-recommendation) ·
[Appendices](#appendix-a--reproduced-mips-iii-codegen)

---

---

## 0. Executive summary

| Question | Answer |
|---|---|
| Is EROFS little-endian by specification? | Yes, explicitly: "Little endian on-disk design" ([RST] line 47) and "The magic number at offset 0x00 must be `0xE0F5E1E2` (little-endian)" ([ONDISK]). |
| Which fields are neutral? | Every `u8`, every byte array (UUID, volume name, device tag, names), inline file data, and the compressed payloads. Every multi-byte integer is `__le16/32/64`. |
| Would a byte-order change invalidate checksums? | Yes, necessarily — CRC32C is computed over on-disk bytes. It is a recompute, not a compatibility break. The kernel's seed `0x5045B54A` is itself the CRC of the first 8 superblock bytes (§2.1). |
| Is a BE variant technically feasible? | Yes. Incompatible feature bit `0x200` is free (`EROFS_ALL_FEATURE_INCOMPAT == 0x1FF`), and unknown incompat bits are a hard mount failure — exactly the semantics a format fork needs. |
| Cost of *not* forking | ~840 ns per inode decode, ~9.6 µs per 5-component path walk, ~45 ms for a full 1.5 MB metadata pass at 75 MHz. In context: one 4 KiB LZ4 block ≈ 0.3–0.5 ms; fetching a 300 MB image over 10 Mbit ≈ 273 s. |
| Recommendation | **Ship standard little-endian EROFS.** Write the Rust codec endian-parameterised so a BE variant is a compile-time flag if it is ever wanted, but do not change the wire format. |

---

## 1. EROFS on-disk format: what is little-endian, what is not

### 1.1 The specification says little-endian, in two places

The kernel documentation states it as a headline property:

> "Little endian on-disk design with 48-bit block addressing, supporting up to 1 EiB
> filesystem capacity with 4 KiB block size"
> — [RST], `Documentation/filesystems/erofs.rst`, lines 47–48.

The format specification hosted by the EROFS project is stronger — it makes the byte
pattern normative:

> "The magic number at offset 0x00 must be `0xE0F5E1E2` (little-endian). A reader must
> reject any image whose first four bytes at offset 1024 do not match this value."
> — [ONDISK], `src/ondisk/core_ondisk.md`, "Magic Number".

So little-endianness is not an implementation detail of the Linux driver; it is part of the
format's conformance definition. A BE image is, by that definition, not an EROFS image.

### 1.2 Field inventory

`fs/erofs/erofs_fs.h` is the authoritative field list. Every multi-byte field carries an
explicit `__le` type; the only fields without one are `u8`, `char[]`, or `u8[]`:

| Structure | Size | Multi-byte (must be byte-swapped on BE) | Neutral |
|---|---|---|---|
| `erofs_super_block` [FSH L54-93] | 144 B | `magic`, `checksum`, `feature_compat`, `rb.rootnid_2b`/`blocks_hi`, `inos`, `epoch`, `fixed_nsec`, `blocks_lo`, `meta_blkaddr`, `xattr_blkaddr`, `feature_incompat`, `u1`, `extra_devices`, `devt_slotoff`, `xattr_prefix_start`, `packed_nid`, `build_time`, `rootnid_8b`, `reserved2`, `metabox_nid`, `reserved3` — **21 fields** | `blkszbits`, `sb_extslots`, `dirblkbits`, `xattr_prefix_count`, `xattr_filter_reserved`, `ishare_xattr_prefix_id`, `reserved[2]`, `uuid[16]`, `volume_name[16]` |
| `erofs_inode_compact` [FSH L161-174] | 32 B | `i_format`, `i_xattr_icount`, `i_mode`, `i_nb`, `i_size`, `i_mtime`, `i_u`, `i_ino`, `i_uid`, `i_gid`, `i_reserved` — **11 fields** | — |
| `erofs_inode_extended` [FSH L177-192] | 64 B | `i_format`, `i_xattr_icount`, `i_mode`, `i_nb`, `i_size`, `i_u`, `i_ino`, `i_uid`, `i_gid`, `i_mtime`, `i_mtime_nsec`, `i_nlink` — **12 fields** | `i_reserved2[16]` |
| `erofs_dirent` [FSH L281-286] | 12 B | `nid` (8), `nameoff` (2) — **2 fields** | `file_type`, `reserved` |
| `erofs_xattr_entry` [FSH L232-238] | 4 B + name | `e_value_size` (2) | `e_name_len`, `e_name_index`, name bytes |
| `erofs_inode_chunk_index` [FSH L271-275] | 8 B | `startblk_hi`, `device_id`, `startblk_lo` | — |
| `z_erofs_map_header` [FSH L360-385] | 8 B | `h_fragmentoff`/`h_idata_size`/`h_extents_lo`, `h_advise`, `h_extents_hi` | `h_algorithmtype`, `h_clusterbits` |
| `z_erofs_lcluster_index` [FSH L405-418] | 8 B | `di_advise`, `di_clusterofs`, `blkaddr` / `delta[2]` | — |
| `z_erofs_extent` [FSH L427-434] | 32 B | `plen`, `pstart_lo`, `pstart_hi`, `lstart_lo`, `lstart_hi` | `reserved[12]` |
| `erofs_deviceslot` [FSH L43-50] | 128 B | `blocks_lo`, `uniaddr_lo`, `blocks_hi`, `uniaddr_hi` | `tag[64]`, `reserved[52]` |

The compile-time assertion at [FSH L443-467] shows how deeply the byte order is baked in:

```c
const __le64 fmh = *(__u64 *)&(struct z_erofs_map_header){ .h_clusterbits = 1 << 7 };
BUILD_BUG_ON(__builtin_constant_p(fmh) ? fmh != cpu_to_le64(1ULL << 63) : 0);
```

That is: the "this is a fragment inode" flag lives in **byte 7, bit 7** *because* the
8-byte header is little-endian. On a BE variant the same logical field is byte 0, bit 7;
every byte-level test of `h_clusterbits` changes meaning.

### 1.3 Magic and feature flags

* Magic value: `0xE0F5E1E2`, stored as `__le32` at file offset 1024 ([FSH L13, L55];
  [MAGIC] `#define EROFS_SUPER_MAGIC_V1 0xE0F5E1E2`). On disk the bytes are
  `E2 E1 F5 E0`.
* Superblock offset is 1024 bytes, "to allow for x86 boot sectors and other oddities"
  ([FSH L12-13]); base size 128 bytes, extended by `sb_extslots * 16` ([FSH L59],
  [ONDISK] "Superblock").
* `feature_compat` (offset 0x08): a reader **may ignore** unknown bits. Current bits:
  `SB_CHKSUM 0x1`, `MTIME 0x2`, `XATTR_FILTER 0x4`, `SHARED_EA_IN_METABOX 0x8`,
  `PLAIN_XATTR_PFX 0x10`, `ISHARE_XATTRS 0x20` ([FSH L15-20]).
* `feature_incompat` (offset 0x50): a reader **must refuse** the filesystem if any bit is
  unknown. Current bits run `LZ4_0PADDING 0x1` … `METABOX 0x100`, and
  `EROFS_ALL_FEATURE_INCOMPAT == (METABOX << 1) - 1 == 0x1FF` ([FSH L26-39]).

The refusal is explicit in the mount path:

```c
sbi->feature_incompat = le32_to_cpu(dsb->feature_incompat);
if (sbi->feature_incompat & ~EROFS_ALL_FEATURE_INCOMPAT) {
        erofs_err(sb, "unidentified incompatible feature %x, please upgrade kernel", ...);
        goto out;                       /* -EINVAL */
}
```

— [SUPER] `erofs_read_superblock()`, lines 289–295.

Two consequences matter for the design space:

1. **Bits 9–31 of `feature_incompat` are free.** `0x200` is an available, honest,
   forward-compatible marker for "big-endian layout".
2. Using a *compat* bit instead would be a bug: old readers would ignore it and then
   misparse every integer in the image.

---

## 2. Checksums and compression

### 2.1 Superblock CRC32C

EROFS has exactly one checksum: the optional superblock CRC32C, enabled by
`EROFS_FEATURE_COMPAT_SB_CHKSUM`. There are **no per-block or per-file data checksums**.

Specification ([ONDISK], "Superblock Checksum"): the digest covers
`[1024, 1024 + block_size)` with the four bytes of the `checksum` field treated as zero.

The kernel implements an equivalent, cheaper form by seeding the CRC with the digest of the
skipped prefix:

```c
/* skip .magic(pre-verified) and .checksum(0) fields */
crc = crc32c(0x5045B54A, (&dsb->checksum) + 1, len);
if (crc == le32_to_cpu(dsb->checksum))
        return 0;
```

— [SUPER] `erofs_superblock_csum_verify()`, lines 42–55.

`0x5045B54A` is not arbitrary. It is the raw CRC32C (reflected polynomial `0x82F63B78`,
init `~0`, **no** final inversion — see `erofs_crc32c()` in [UTILS] `include/erofs/internal.h`
L587-597, and [LIBCRC] `crc32c()` which returns the raw accumulator) of the first
eight bytes `E2 E1 F5 E0 00 00 00 00`. Verified:

```
$ python3 -c '...'          # reflected 0x82F63B78, init 0xffffffff, no final xor
crc(0xffffffff, b"\xe2\xe1\xf5\xe0\x00\x00\x00\x00") = 0x5045b54a
```

`mkfs.erofs` computes the plain form over the whole 1 KiB block with the field zeroed
([UTILS] `lib/super.c` `erofs_superblock_csum_set()`, lines ~350 and ~383); the two agree
because a CRC can be continued from the digest of a prefix.

### 2.2 Does a byte-order change invalidate checksums?

**Yes, by construction.** The CRC is over on-disk bytes; re-encoding any covered field
changes the digest and it must be recomputed. That is not a compatibility break, but two
details do bite:

1. **The seed is byte-pattern-dependent.** If a BE variant stored the magic big-endian
   (`E0 F5 E1 E2`), the kernel's `0x5045B54A` seed would be wrong and a new constant would be
   required. If the magic bytes and the checksum field position stay identical — the
   recommended design — the seed is unchanged and only the *rest* of the block is BE.
2. **The checksum field itself.** Storing it big-endian while the kernel's `crc32c()` seed
   trick assumes nothing about it is fine; the field is skipped on both sides.

### 2.3 Compressed layout

EROFS uses *fixed-sized output* compression: logical clusters (lclusters, one block each)
map to variable-length physical clusters (pclusters). The metadata is:

```
compressed inode:
  [ inode 32/64 B ][ z_erofs_map_header 8 B ][ lcluster index array ][ optional inline pcluster ]
                                                   |
                     compact (layout 3): 4 B per lcluster
                     full    (layout 1): 8 B per lcluster
```

`struct z_erofs_lcluster_index` ([FSH L405-418]) is 8 bytes:

| Field | Type | Meaning |
|---|---|---|
| `di_advise` | `__le16` | lcluster type (bits 0–1), `PARTIAL_REF` (15), `HOLE` (14), `D0_CBLKCNT` (11) |
| `di_clusterofs` | `__le16` | offset of the extent start inside the HEAD lcluster |
| `blkaddr` | `__le32` | physical block of the pcluster (HEAD lclusters) |
| `delta[2]` | `__le16[2]` | distance to previous/next HEAD (NONHEAD lclusters) |

`struct z_erofs_map_header` ([FSH L360-385]) adds `h_advise` (big pcluster, inline,
interlaced, fragment flags), `h_algorithmtype` (bits 0–3 = algorithm for HEAD1, bits 4–7 =
HEAD2) and `h_clusterbits` (logical cluster size, plus bit 7 = "whole file in packed inode").

Big pclusters record their compressed block count in the first NONHEAD lcluster's
`delta[0]`, flagged by `Z_EROFS_LI_D0_CBLKCNT` (bit 11) — [RST] "Data compression"; see the
diagram at [RST] lines 344–391.

All of this is integer metadata: **4–8 bytes per 4 KiB lcluster = 0.10–0.20 %** overhead
for compressed files, plus 8 bytes per file. A BE variant must swap it; there is no way around
that.

Physically, a pcluster is just the raw compressed stream occupying `pclusterblks`
consecutive blocks — EROFS puts **no per-block header** in front of it. Three variants
matter:

* the last block of a pcluster may be zero-padded up to the block size, and
  `EROFS_FEATURE_INCOMPAT_LZ4_0PADDING` records that the padding is guaranteed zero (which
  is what permits in-place decompression);
* `Z_EROFS_ADVISE_INLINE_PCLUSTER` stores the stream inside the inode's metadata area
  instead of in a data block;
* `Z_EROFS_ADVISE_INTERLACED_PCLUSTER` places a partial pcluster before the index, and the
  `fragments` feature moves file tails (or whole small files) into the packed inode.

None of these change the conclusion: the payload is a byte stream (§2.4); only the 8-byte
index entries are endian-sensitive.

### 2.4 Are the compressed payloads byte-order-neutral?

Yes — they are **self-describing byte streams**, not arrays of native integers. A decoder
reads bytes and assembles values regardless of host order:

| Algorithm | Format property | Host-endian dependence |
|---|---|---|
| LZ4 | "fixed byte-oriented encoding format"; match offsets are "2 bytes value, in little endian format (the 1st byte is the low byte…)" — i.e. serialised byte-by-byte, not a native `u16` | none |
| DEFLATE | RFC 1951: bits packed "starting with the least-significant bit of the byte" | none (bit order, not byte order) |
| MicroLZMA | byte-oriented range coder, LZMA2-style chunk header | none |
| Zstandard | RFC 8878: frame header fields "little-endian" **as byte sequences** (`Magic_Number: 4 bytes, little-endian format`) | none |

Citations: [LZ4] lines 11 and 78–79; [RFC1951] §3.1.1; [RFC8878] §3.1.1 and §3.1.1.1.

The practical rule: **payloads move verbatim; only the index structures around them are
endian-sensitive.** That is true for EROFS and for every alternative in §6.

### 2.5 What this means for a fork

The checksum and compression story is benign: a BE EROFS is a re-encode of ~200 bytes of
superblock, 32/64 bytes per inode, 12 bytes per dirent and 8 bytes per compressed lcluster.
No algorithm changes. The risk is entirely in *reader/writer correctness*, and the only
independent check on that is the little-endian reference implementation you would be
abandoning (§4, §7).

---

## 3. Host tooling: `mkfs.erofs` and erofs-utils

### 3.1 Architecture and dependencies

`erofs-utils` 1.9.4 [UTILS] builds five binaries from one library: `mkfs.erofs`,
`fsck.erofs`, `dump.erofs`, `mount.erofs`, `erofsfuse`. Declared dependencies
([UTILS] `docs/INSTALL.md`):

| Dependency | Role | Needed for a minimal root image? |
|---|---|---|
| `liblz4` ≥ 1.9.3 | LZ4/LZ4HC | yes, if LZ4 compression is used |
| `xz`/`liblzma` ≥ 5.3.2alpha | MicroLZMA | optional |
| `zlib` | DEFLATE and gzip | optional |
| `libzstd` | Zstandard | optional |
| `libuuid` | UUID generation | yes (or use `-U clear`) |
| `libselinux` | SELinux xattrs | no |
| `libfuse` 2/3 | `erofsfuse` | no (host debugging only) |
| `libxml2`, `json-c`, `libcurl`, `OpenSSL` | OCI / S3 remotes | no |

All compressors are ordinary C libraries whose output is a byte stream; none of them
changes behaviour with host byte order. The on-disk question is entirely in the container
structures.

### 3.2 Where erofs-utils handles endianness

It already has a complete abstraction, in `include/erofs/defs.h` L80–108:

```c
#if __BYTE_ORDER == __LITTLE_ENDIAN
#define cpu_to_le16(x) ((__u16)(x))
...
#else
#if __BYTE_ORDER == __BIG_ENDIAN
#define cpu_to_le16(x) (__builtin_bswap16(x))
...
```

So the tool is *intended* to be buildable on a big-endian host, and would then emit
**standard little-endian images** — the macros exist to make the host irrelevant, not to
offer a BE format. The discipline is consistent: **219 conversion sites** across `lib/`,
`mkfs/` and `include/` (109 `cpu_to_le*` writes, 110 `le*_to_cpu` reads).

That is the good news. The caveats:

1. **No big-endian CI.** The project is developed and tested on x86-64; s390x is the only
   mainstream BE architecture left and erofs-utils has no BE-specific test job.
2. **Unaligned typed stores.** The writer deliberately does things like
   `*(__le32 *)(out + destsize * vcnt - 4) = cpu_to_le32(...)` ([UTILS] `lib/compress.c`
   L910, also L1219, L1528; `lib/xattr.c` L1114). These are fine on x86 but trap on
   strict-alignment BE CPUs such as MIPS — so building the tool *on the Indy* is not
   viable even though the macros are correct.
3. **The checksum is taken over encoded bytes, not over fields.** `erofs_writesb()`
   assembles the struct with explicit conversions and `memcpy`s it ([UTILS] `lib/super.c`
   L219-275); `erofs_superblock_csum_set()` then CRCs the buffer. Flipping the format
   carries the checksum with it — no hidden little-endian assumption.

### 3.3 What a big-endian *writer* would actually require

Three routes, in increasing cost:

**Route A — patch the macros, build on x86.** Add `--enable-big-endian-format` that makes
`cpu_to_le16/32/64` unconditionally byte-swap (identity on BE hosts). Everything else in
the tool — in-memory `struct erofs_inode`, compressors, directory building — is already
host-native and unaffected. Effort: **~1 day of code, 1–2 weeks of testing** (there is no
existing BE corpus to diff against; each feature — xattrs, fragments, big pcluster,
ztailpacking — needs a round-trip test).
*Risk:* you now ship a tool whose output no other implementation can read, and `fsck.erofs`
built the same way is the only thing that can check it.

**Route B — a Rust writer in the project's own tooling.** The image is built in CI on
x86-64, so the writer must encode BE fields explicitly anyway (`to_be_bytes()`), and the
reader is new Rust code regardless. Feature subset needed for a root image: superblock,
compact+extended inodes, flat/inline/chunk layouts, dirents, xattrs, LZ4 pclusters,
fragments. Effort: **3–6 weeks** for that subset with tests, and it is code the project
needs under *either* decision if it wants reproducible image builds.

**Route C — standard EROFS, no writer work.** Use `mkfs.erofs` unchanged (or `erofs-rs`'s
builder). Zero fork cost.

The decisive observation: **Route C's writer is a solved problem and Route B's writer is
needed only because of the fork.** The fork does not save writer work — it creates it.

---

## 4. Reader side: Linux, `erofs-rs`, and a `no_std` Rust reader

### 4.1 The Linux driver is little-endian only

There is no endianness configuration in `fs/erofs`. `grep -r 'BIG_ENDIAN\|bswap' fs/erofs/`
returns **nothing**, while the driver performs **83 little-endian reads** (`le16_to_cpu` 35,
`le32_to_cpu` 37, `le64_to_cpu` 11) plus one `cpu_to_le64`. The mount path is unambiguous:

```c
dsb = (struct erofs_super_block *)(data + EROFS_SUPER_OFFSET);
if (le32_to_cpu(dsb->magic) != EROFS_SUPER_MAGIC_V1) { ... }
```

— [SUPER] L264–268.

A BE image is rejected at the first four bytes by every Linux kernel in existence. That is
also the property that makes a fork safe: it cannot be silently misread.

### 4.2 `erofs-rs`

`Dreamacro/erofs-rs` 0.3.0 (MIT OR Apache-2.0) [ERS] is 13,031 lines across 37 files. It
parses with `binrw` and annotates **all nine on-disk structures it parses** with `#[br(little)]`
([ERS] `erofs/src/types.rs` L25, 285, 310, 382, 399, 408, 417, 425, 432). That is a clean
single point of control — but it is a *compile-time* attribute, not a switch.

It is also not `no_std` today:

* `types.rs` uses `rustix::fs::FileType` unconditionally (L10), and `dirent.rs` uses
  `typed_path::UnixPath` (L5).
* `error.rs` is already `cfg`-gated for `std` (L43-46), and the crate does `extern crate
  alloc` — so the intent exists.
* Compression features: `lz4_flex`, `miniz_oxide`, `ruzstd` are `no_std`-capable;
  `lzma-rust2` is pulled in with `features = ["std", "encoder"]` and the `lzma` feature
  depends on `std` ([ERS] `erofs/Cargo.toml`).

A `no_std` port is therefore a **real fork of a 13k-line crate**, not a feature flag, and
it must happen under either decision (§7).

### 4.3 Can an EROFS-BE variant coexist with standard EROFS?

Yes, cleanly, with three rules:

1. **Use `feature_incompat` bit 9 (`0x200`), named `EROFS_FEATURE_INCOMPAT_BIG_ENDIAN`.**
   Unknown incompat bits are a hard `-EINVAL` in every existing reader ([SUPER] L289-295),
   so a BE image can never be mistaken for an LE one. Do **not** use `feature_compat`.
2. **Decide the magic convention.** Two options:
   * *Keep the bytes identical* (`E2 E1 F5 E0`, i.e. LE-encoded `0xE0F5E1E2`). Then
     `blkid`, `file`, and the format's own conformance sentence still identify the image,
     and the kernel's CRC seed `0x5045B54A` remains valid (§2.2). A BE reader compares the
     four bytes, not a `u32`.
   * *Store it big-endian* (`E0 F5 E1 E2`). Numerically identical when read BE, but the
     documented byte pattern changes and the CRC seed must be recomputed.
   Recommendation, if a fork ever happens: keep the bytes. The feature flag, not the magic,
   should carry the meaning.
3. **Recompute the superblock CRC** with the same algorithm and range, since every covered
   field after offset 8 is re-encoded.

With those rules the two formats are distinguishable, mutually unreadable by accident, and
self-describing. The cost is that every reader must branch on the flag before parsing —
which is the same endian-generic codec described next.

### 4.4 What a `no_std` Rust reader needs (either decision)

A single `ByteOrder` abstraction, resolved at compile time, covers both worlds:

```rust
trait ErofsOrder {
    fn u16(b: [u8; 2]) -> u16;
    fn u32(b: [u8; 4]) -> u32;
    fn u64(b: [u8; 8]) -> u64;
}
struct Le;   // from_le_bytes: 3 / 9 / 24 instructions on MIPS III
struct Be;   // from_be_bytes: a plain load
```

On MIPS III `from_le_bytes` is 3–9 instructions (§5.1); `from_be_bytes` is a plain load.
Decoding into native structs — which any safe Rust reader does anyway — makes the byte
order a property of the *parser*, not of every later access.

What the reader must **not** do is cast on-disk bytes directly to `#[repr(C)]` structs: the
format is full of unaligned, packed fields (`erofs_dirent` is 12 bytes and packed;
`z_erofs_lcluster_index` is 8 bytes and packed), and MIPS III faults on unaligned loads.
That constraint is identical for LE and BE.

---

## 5. The actual cost of *not* forking

This is the section that decides the question. If byte-swapping metadata on an R4400 were
expensive, a BE format would be worth the ecosystem loss. It is not expensive, and the
reason is structural: **metadata is a tiny fraction of the image, and it is decoded once
per access into native structs, not swapped in a hot loop.**

### 5.1 MIPS III byte-swap sequences (no `wsbh`)

The R4400 implements MIPS III. `wsbh` is MIPS32r2/MIPS64r2, and `lwle`/`lwre` are
MIPS32r2; neither exists on this CPU. The available primitives are shifts, masks, `or`,
and the unaligned `lwl`/`lwr` pair.

Compiling with LLVM 21.1.8 for `--target=mips-unknown-linux-gnu -march=mips3 -mabi=32 -EB
-O2` ([ASM], reproduced in Appendix A) gives the canonical sequences:

**16-bit — 3 instructions** (4 if the caller needs the upper half cleared):

```asm
srl     $1, $4, 8
sll     $2, $4, 8
or      $1, $2, $1
andi    $2, $1, 65535      # zero-extension, often free in context
```

**32-bit — 9 instructions** (one 64-bit shift is used because MIPS III registers are 64
bits wide):

```asm
srl     $1, $4, 24         # byte 3 -> bits 0-7
dsrl    $2, $4, 8
andi    $2, $2, 65280      # byte 2 -> bits 8-15
or      $1, $2, $1
sll     $2, $4, 24         # byte 0 -> bits 24-31
andi    $3, $4, 65280
sll     $3, $3, 8          # byte 1 -> bits 16-23
or      $2, $2, $3
or      $2, $2, $1
```

A hand-scheduled version keeping `0x00FF00FF` in a register reaches **8**; the compiler's 9
is the honest number to budget.

**64-bit — 24 instructions** (LLVM's sequence, excluding the `jr`/`sd` return convention).
For comparison: a 64-bit swap is 3× a 32-bit swap.

**`lwl`/`lwr` do not byte-swap.** This is a persistent myth worth killing with the primary
source. The R4000 manual defines `LWL` as: "it starts at the specified byte in memory and
loads that byte into the high-order (left-most) byte of the register; then it loads bytes
from memory until it reaches the low-order byte of the word in memory" ([R4000 UM] A-103).
`LWR` is the mirror. The pair `lwl rt,0(base)` + `lwr rt,3(base)` is therefore a **normal
unaligned word load**, not a swap — which is exactly how LLVM uses it, followed by the
shift/mask swap above when a byte-swapped value is required. QEMU's implementation
(`gen_lxl`/`gen_lxr`, `target/mips/tcg/translate.c` L1987-2035) confirms the same merge
semantics. There is no 2-instruction swap on MIPS III.

**Cost model.** The R4400 issues one instruction per cycle and the sequences are pure ALU
with no load stalls, so instructions ≈ cycles. At 75 MHz (13.33 ns/cycle):

| Operation | Instructions | Cycles | Time @75 MHz |
|---|---|---|---|
| `u16` swap | 3–4 | 3–4 | 40–53 ns |
| `u32` swap | 9 | 9 | 120 ns |
| `u64` swap | 24 | 24 | 320 ns |

### 5.2 What one EROFS operation costs

Applying the table to the actual structures (§1.2):

| Operation | Swaps | Instructions | Time @75 MHz |
|---|---|---|---|
| Decode one compact inode (32 B) | 6×u16 + 5×u32 | 6·3 + 5·9 = **63** | **0.84 µs** |
| Decode one extended inode (64 B) | 4×u16 + 6×u32 + 2×u64 | 12 + 54 + 48 = **114** | 1.5 µs |
| Decode one dirent (12 B) | 1×u64 + 1×u16 | 24 + 3 = **27** | 0.36 µs |
| Decode one 8 B lcluster index | 2×u16 + 1×u32 | 6 + 9 = **15** | 0.20 µs |
| Path walk, 5 components, 3 dirents/block | 5×(3·27 + 63) | **720** | **9.6 µs** |
| Full metadata pass, 1.5 MB image | ~375k words × 9 | **~3.4 M** | **~45 ms** |

For scale: one 4 KiB block read from the Indy's SCSI disk is on the order of **5–10 ms**
(seek plus transfer). A cold path walk that touches five directory blocks is ~30 ms of I/O
and **9.6 µs** of swapping — 0.03 %. Even a *warm* path walk, where everything is in the
page cache, adds 9.6 µs to a syscall that already costs tens of microseconds.

### 5.3 Metadata-to-data ratio (measured)

Estimated by walking a real Unix root tree (`/run/current-system/sw`, a NixOS system
closure) and applying the EROFS core layout rules — 32-byte compact inodes, 12-byte dirents
plus name, directory blocks padded to 4 KiB:

| Quantity | Value |
|---|---|
| Files / directories | 10,098 / 158 |
| File data (4 KiB-rounded) | 40.0 MiB |
| EROFS metadata (inodes + dirents, block-padded) | 1.14 MiB |
| **Metadata share** | **2.77 %** |
| Metadata per file | 118 B |

Scaling to a plausible IRIX 6.5 root — 300 MB of file data, ~25,000 files:

| Scenario | Data | Metadata | Metadata share |
|---|---|---|---|
| Uncompressed | 300 MB | 2.95 MB | **0.97 %** |
| LZ4, ~2.5:1 | 120 MB | 2.95 MB | **2.4 %** |
| Pathological (many tiny files, 100k) | 300 MB | 11.8 MB | 3.8 % |

So the byte-swapping surface is **1–4 % of the bytes on the medium**, and it is touched
once per access rather than streamed.

If a reader wanted to swap metadata *in place* into a RAM copy at mount (to make later
accesses free), it would need ~1.5–3 MB of RAM — **5–9 % of a 32 MB machine**, which is a
much stronger argument against a mount-time copy than the 45 ms it would cost. Decode on
demand with a small cache is the right design, and it is byte-order-agnostic.

### 5.4 Decompression dominates metadata by two orders of magnitude

LZ4 decode on an in-order, 1-IPC, 16 KB-cache machine is roughly **4–8 cycles/byte**
(est. — the decoder copies matches in 8/4/2/1-byte wild copies with a branch per
sequence; an in-order core cannot hide those dependencies, while modern OoO cores reach
~0.7 c/B). At 75 MHz that is **9–19 MB/s**.

| Work | Rate (est.) | 4 KiB block | 120 MB image |
|---|---|---|---|
| LZ4 decode | 9–19 MB/s | 0.2–0.45 ms | 6–13 s |
| DEFLATE inflate | 6–12 MB/s | 0.35–0.7 ms | 10–20 s |
| MicroLZMA decode | 2.5–5 MB/s | 0.8–1.6 ms | 24–48 s |
| Zstd decode | 12–25 MB/s | 0.16–0.35 ms | 5–10 s |
| **Metadata swap** | — | **0.84 µs/inode** | **~45 ms** |

Decoding one 4 KiB pcluster costs **250–500×** more than decoding the inode that points at
it. Over a whole image, decompression is **130–1000×** more expensive than swapping every
metadata field once. **Decompression dominates; byte order is noise.**

### 5.5 The 10 Mbit link dominates everything

An Indy has 10BASE-T. Wire rate is 1.25 MB/s; TCP goodput with a 1500-byte MTU is ~1.1 MB/s.

| Transfer | Bytes | Time at 1.1 MB/s |
|---|---|---|
| 300 MB uncompressed image | 300 MB | **273 s** (4.5 min) |
| LZ4 image, 2.5:1 | 120 MB | **109 s** (1.8 min) |
| Single 4 KiB R2 range GET | 4 KiB | 3.6 ms of wire time + HTTP/TLS round trips |

Cold-boot budget, therefore:

```
network        109–273 s   ████████████████████████████████████████████  94.8–95.4 %
decompress       6–13 s    ██                                             4.5–5.2 %
metadata swap    0.045 s   ▏                                             0.02–0.04 %
```

A BE format changes the last line only. If the image is fetched once and cached on SCSI,
the link cost is paid once per image update; even then, the metadata term never becomes
visible.

### 5.6 What the fork would actually buy

* **~45 ms per cold boot** (one full metadata pass), or ~0 if metadata is decoded lazily.
* **~10 µs per warm path walk**.
* No reduction in image size, no reduction in decompression, no reduction in network time.

Against that: the loss of the only independent implementation to test against (§7).

---

## 6. Alternatives

### 6.1 The candidates

| # | Option | On-disk order | Host tooling | Reader effort (est.) | Interoperability | Main risk |
|---|---|---|---|---|---|---|
| A | **Standard EROFS, swap in reader** | LE | `mkfs.erofs`, `erofs-rs` builder | 2–4 weeks for the core+LZ4 subset | Linux/Android mount, `fsck.erofs`, `dump.erofs`, `erofsfuse`, `erofs-rs` | none material |
| B | **EROFS-BE fork** (`incompat 0x200`) | BE | patched erofs-utils **or** new Rust writer (3–6 weeks) | 2–4 weeks (same codec, BE constants) | **none** — no external reader can check it | silent spec drift; no oracle |
| C | **SquashFS 4** | LE | `mksquashfs` (writes LE even on BE hosts via its swap macros) | 3–5 weeks: bit-packed inodes, 8 KiB compressed metadata blocks, fragments | Linux mount, `unsquashfs` | worse random access; metadata must be decompressed before parsing |
| D | **SquashFS-BE** | BE | patch `mksquashfs` | as C + swap | **none** | same as B, plus a more intricate format |
| E | **I7FS** (project's own) | BE native | own `mkfs`/`fsck` | 4–6 person-months for a writable FS [FSR §7] | none | schedule; but needed later for the writable root anyway |
| F | **Custom BE read-only image** | BE | own builder | 2–4 weeks | none | reinvents EROFS with fewer features and no fsck |
| G | **Byte-order-neutral container** (CBOR/varint metadata + raw data blocks) | neutral | own builder | 3–5 weeks | none | parse cost, larger metadata, no compression interop |

Effort figures are estimates for a production-quality implementation with tests, not for a
prototype.

### 6.2 Notes on the non-EROFS options

**SquashFS (C, D).** SquashFS 4 is little-endian on disk: `squashfs_fs.h` carries 136
`__le` fields and `super.c` does `le32_to_cpu(sblk->s_magic)` ([SQSH] L242-245, [SQSH]
`super.c` L233). The current kernel driver has **no byte-swap path** (`grep -i swap
fs/squashfs/` returns only a comment). `squashfs-tools` *does* carry `squashfs_swap.h`
whose header says "macros to convert each structure from big endian to little endian", used
by `mksquashfs` so a BE build host still produces an LE image — the same design as
erofs-utils, and the same conclusion: the tools are host-portable, the format is not.

SquashFS's disadvantage for this project is structural, not endian: inodes and directory
tables live inside **8 KiB compressed metadata blocks** (`SQUASHFS_METADATA_SIZE`), so
every metadata access is a decompression, and its inode numbers are not O(1) addresses. On
a 75 MHz R4400 with 32 MB, that is a worse fit than EROFS's directly addressable
`meta_blkaddr + 32*nid` inode space. EROFS is also the format with active upstream
investment (Android, Fedora live media, composefs) and a documented core-format subset
([ONDISK]) that is small enough to implement cleanly.

**I7FS (E).** I7FS is the project's *proposed* native format — big-endian, 4 KiB blocks,
256-byte inodes, extents, B+tree directories, journal, CRC32C — specified in
`docs/spec/i7fs.md` and costed at 4–6 person-months including `fsck` ([FSR] §7). It is the
right answer for the **writable** root in phase 2, and the wrong answer for the read-only
boot image: it has no compression, no tooling, no reference implementation, and building it
now would delay the boot path by months for a format that will exist anyway. Note also that
a read-only *variant* of I7FS is not free: you still need `mkfs`, a reader, and a verifier,
which is most of the work.

**Custom BE image (F).** A superblock + extent table + dirents is easy to write in a week.
It is also EROFS with fewer features, no compression story unless you borrow LZ4 block
format, no fsck, and no ability to inspect the image on the host. It only makes sense if
the project deliberately wants a *simpler-than-EROFS* format; in that case I7FS (which has
a spec and a roadmap) is the better vehicle.

**Neutral container (G).** Encoding metadata as CBOR/MessagePack/varints removes the byte
order question entirely, at the cost of a parser, larger metadata (often 1.5–2×), and no
compression interop. It is a reasonable choice for a *new* format, not for a derivative of
EROFS.

### 6.3 Licence note (relevant to ADR-0012)

`fs/erofs/erofs_fs.h` is `SPDX-License-Identifier: MIT` — the on-disk definitions may be
used directly in a GPL-3.0 codebase with attribution. The rest of `fs/erofs/` is
`GPL-2.0-only`, which is **not** compatible with GPL-3.0 for code copying; it can be read
as a reference for behaviour (clean-room), which is what ADR-0009/0014 already require.
`erofs-rs` is `MIT OR Apache-2.0` and clears the licence gate ([FSR] §10). A BE fork would
be derived from the MIT header plus the project's own code — no new licence problem, but
also no new benefit.

---

## 7. Recommendation

### 7.1 Verdict

**Keep standard, little-endian EROFS. Do not fork the on-disk format.** Implement the
reader and (if needed) the writer with a compile-time `ByteOrder` parameter so that a BE
variant remains a one-line change, but ship LE images.

### 7.2 Why — the numbers

| Consideration | Standard EROFS + swap | EROFS-BE fork |
|---|---|---|
| Metadata bytes to re-encode | 1–4 % of the image | same |
| Added CPU per cold boot | **~45 ms** (full pass) or ~0 (lazy) | 0 |
| Added CPU per path walk | **9.6 µs** (warm) | 0 |
| Added CPU vs decompression | 1/250 of LZ4 decode | — |
| Added CPU vs network | 1/2400–1/6000 of a 10 Mbit fetch | — |
| Independent readers available | Linux, `fsck.erofs`, `dump.erofs`, `erofsfuse`, `erofs-rs` | **none** |
| Host verification of built images | `mount -t erofs` on the CI host | impossible without the fork |
| Ongoing maintenance | zero; new features are gated by incompat bits | permanent divergence |
| Writer work | none (`mkfs.erofs`) | 3–6 weeks (Rust) or a patched tool with no external check |

The last four rows are decisive. This is a **from-scratch filesystem implementation** for a
clean-room OS. The single most valuable engineering asset is an independent implementation
to diff against: mount the image on the CI host, run `fsck.erofs`, list it with
`dump.erofs`, read it with `erofs-rs`, and compare file trees byte for byte. A BE fork
throws all of that away to save 45 ms per boot on a machine whose boot is dominated by a
10 Mbit link.

### 7.3 Implementation plan (standard EROFS)

1. **Reader**: a new `no_std` crate in the OS (do not port `erofs-rs` — 13k lines, `std`
   and `rustix` coupled). Implement the *core format* first: superblock, compact inodes,
   flat and inline layouts, dirents; then LZ4 pclusters, then fragments/ztailpacking.
   Make every field decode go through `trait ErofsOrder` (`LE` / `BE`), defaulting to LE.
2. **Writer**: use `mkfs.erofs` in CI (hermetic, pinned version), with `-T` and `-U clear`
   for reproducible images, or `erofs-rs`'s builder if a Rust-only toolchain is required.
3. **Oracle in CI**: `fsck.erofs` + `mount -t erofs -o loop` + a tree diff against the
   source directory; keep a small corpus of images (plain, LZ4, fragments, xattrs) as
   golden files.
4. **On target**: decode on demand into native structs with a small LRU over inode/dirent
   decodes. Do **not** swap a whole metadata region in place: EROFS interleaves inline file
   data and names with numeric fields, so a blind block-wide swap corrupts data.
5. **Distribution**: name the R2 object by the SHA-256 of the image, serve it with
   immutable cache headers, and verify the digest before mount. EROFS has no data
   checksums of its own (§2.1), so the digest is the integrity boundary; content-addressed
   names also make range GETs safe to cache at the edge.
6. **Measure**: if profiling ever shows metadata decode > 5 % of boot or > 10 % of a
   metadata-heavy workload, revisit §7.4.

### 7.4 Revisit triggers (when a BE fork would become rational)

* The image becomes **uncompressed** *and* metadata-heavy (≥ 10 % of bytes) *and* boot
  becomes CPU-bound rather than network-bound. None of these is true today.
* The link is upgraded (100 Mbit+) **and** compression is dropped, so the 45 ms becomes
  visible relative to a ~3 s boot.
* A second target with a much slower clock or no caches makes per-access decode the
  bottleneck (it still would not be, at 63 instructions per inode).

Even then, the right move is to flip the codec's `ByteOrder` parameter, not to fork the
format: the same source produces both, and the LE build remains the testable one.

### 7.5 If the project forks anyway — minimum design checklist

1. Keep offsets and struct sizes identical; change only byte order.
2. Set `feature_incompat` bit 9 (`0x200`), name it `EROFS_FEATURE_INCOMPAT_BIG_ENDIAN`.
3. Keep the magic **bytes** `E2 E1 F5 E0` so `blkid`/`file` still identify the image and
   the CRC seed `0x5045B54A` stays valid; compare bytes, not a `u32`.
4. Recompute the superblock CRC over the re-encoded block with the checksum field zeroed.
5. Leave names, inline data and compressed payloads untouched.
6. Write a conformance document and a `fsck` that only your implementation can run — and
   accept that it is self-referential.
7. Keep an LE encoder in CI for differential testing of the shared codec.

---

## Appendix A — Reproduced MIPS III codegen

Host: LLVM 21.1.8 (`clang --target=mips-unknown-linux-gnu -march=mips3 -mabi=32 -EB -O2 -S`).
The full listings are in `/tmp/opencode/mipsasm/sw_mips3.s`; the 32-bit case is:

```asm
bswap32:
	srl	$1, $4, 24
	dsrl	$2, $4, 8
	andi	$2, $2, 65280
	or	$1, $2, $1
	sll	$2, $4, 24
	andi	$3, $4, 65280
	sll	$3, $3, 8
	or	$2, $2, $3
	jr	$ra
	or	$2, $2, $1        # 9 ALU instructions; the last `or` fills the branch delay slot
```

Note the `dsrl`: MIPS III registers are 64 bits wide, so LLVM uses a doubleword shift
where MIPS32 would use `srl`. The sequence is 9 instructions, all single-cycle ALU, no
loads, no stalls.

Reproduce:

```sh
cat > sw.c <<'EOF'
#include <stdint.h>
uint16_t bswap16(uint16_t v) { return __builtin_bswap16(v); }
uint32_t bswap32(uint32_t v) { return __builtin_bswap32(v); }
uint64_t bswap64(uint64_t v) { return __builtin_bswap64(v); }
EOF
clang --target=mips-unknown-linux-gnu -march=mips3 -mabi=32 -EB -O2 -S -o - sw.c
```

The same source compiled with `-march=mips32r2` replaces each sequence with a single
`wsbh` (plus `rotr` for 32-bit), which is why the "no `wsbh`" constraint matters and why
numbers from modern MIPS cannot be reused here.

---

## Appendix B — Metadata-ratio measurement

```python
import os, math
root = "/run/current-system/sw"
files = dirs = datablocks = 0
meta_inodes = meta_dirents = 0

def walk(d):
    global files, dirs, datablocks, meta_inodes, meta_dirents
    try: entries = list(os.scandir(d))
    except OSError: return
    dirs += 1; meta_inodes += 32          # compact inode
    de = 0
    for e in entries:
        de += 12 + len(e.name)            # erofs_dirent + name
        if e.is_dir(follow_symlinks=False):
            walk(e.path)
        else:
            files += 1; meta_inodes += 32
            datablocks += math.ceil(e.stat(follow_symlinks=False).st_size / 4096) * 4096
    meta_dirents += math.ceil(de / 4096) * 4096 if de else 4096

walk(root)
meta = meta_inodes + meta_dirents
print(files, dirs, datablocks, meta, 100*meta/(meta+datablocks))
```

Output: `10098 158 41943040 1196032 2.77`.

Caveats: this models the *core* format (no xattrs, no compression, compact inodes only).
Real images add xattr bodies, shared-xattr blocks and compressed-index arrays; that pushes
the share up, not down, by a few tenths of a percent.

---

## Appendix C — Sources

Primary sources are marked P; project-local evidence is marked L.

| Tag | Source | Used for |
|---|---|---|
| [FSH] P | `fs/erofs/erofs_fs.h`, Linux master — <https://github.com/torvalds/linux/blob/master/fs/erofs/erofs_fs.h> (`SPDX-License-Identifier: MIT`) | every struct, every `__le` field, flag values, `BUILD_BUG_ON` layout assertions |
| [SUPER] P | `fs/erofs/super.c`, Linux master — <https://github.com/torvalds/linux/blob/master/fs/erofs/super.c> | magic check (L264-268), incompat rejection (L289-295), CRC32C seed (L41-57) |
| [RST] P | `Documentation/filesystems/erofs.rst` — <https://docs.kernel.org/filesystems/erofs.html> | "Little endian on-disk design" (L47-48); compression layout (L335-391) |
| [ONDISK] P | EROFS format specification, `src/ondisk/core_ondisk.md` — <https://github.com/erofs/docs/blob/main/src/ondisk/core_ondisk.md> | normative magic statement, field table, checksum range, feature-flag semantics |
| [MAGIC] P | `include/uapi/linux/magic.h` — <https://github.com/torvalds/linux/blob/master/include/uapi/linux/magic.h> | `EROFS_SUPER_MAGIC_V1 0xE0F5E1E2` |
| [UTILS] P | erofs-utils 1.9.4, commit `1c44a9ce` — <https://git.kernel.org/pub/scm/linux/kernel/git/xiang/erofs-utils.git> (mirror <https://github.com/erofs/erofs-utils>) | `defs.h` endian macros, `lib/super.c` CRC, `include/erofs/internal.h` `erofs_crc32c`, dependencies |
| [ERS] P | `Dreamacro/erofs-rs` 0.3.0, commit `c32542a` — <https://github.com/Dreamacro/erofs-rs> | `#[br(little)]` structs, `no_std` status, licence |
| [LZ4] P | LZ4 block format — <https://github.com/lz4/lz4/blob/dev/doc/lz4_Block_format.md> | byte-oriented encoding; 2-byte LE offsets as serialised bytes |
| [LIBCRC] P | `lib/libcrc32c.c`, `crypto/crc32c_generic.c`, Linux master — <https://github.com/torvalds/linux/blob/master/lib/libcrc32c.c> | `crc32c()` returns the raw accumulator without final inversion |
| [RFC1951] P | RFC 1951 (DEFLATE) — <https://www.rfc-editor.org/rfc/rfc1951> | LSB-first bit packing (§3.1.1) |
| [RFC8878] P | RFC 8878 (Zstandard) — <https://www.rfc-editor.org/rfc/rfc8878> | little-endian fields as byte sequences (§3.1.1) |
| [SQSH] P | `fs/squashfs/squashfs_fs.h`, `super.c`; `plougher/squashfs-tools` | SquashFS v4 `__le` fields, no kernel swap path, `squashfs_swap.h` |
| [R4000 UM] P | MIPS Technologies, *R4000 Microprocessor User's Manual*, 2nd ed., Appendix A (LWL/LWR, pp. A-103–A-104) — local `iris/R4000_um2.txt` | `lwl`/`lwr` semantics; 1-IPC pipeline |
| [QEMU] P | `target/mips/tcg/translate.c`, `gen_lxl`/`gen_lxr` — <https://github.com/qemu/qemu> | independent confirmation of `lwl`/`lwr` merge semantics |
| [ASM] P | LLVM 21.1.8 codegen for `-march=mips3 -EB` (Appendix A) | instruction counts for 16/32/64-bit swaps |
| [FSR] L | `/tmp/opencode/rust-fs-research.md` §6–§7, §10 | I7FS effort estimate (4–6 person-months), licence gate, filesystem comparison |
| [INDY] L | `/tmp/opencode/indy-hardware.md` §2 | R4400 cache/TLB and Indy clock table |

Pins: Linux `master` fetched 2026-10-20; erofs-utils 1.9.4 (2026-08-19); erofs-rs 0.3.0
(2026-10-05); clang/LLVM 21.1.8.

### Limitations

* No EROFS image was built on the research host (no C toolchain, no `mkfs.erofs`); the
  metadata ratio is a **model over a real file tree**, not a statistic from a real image.
* LZ4/DEFLATE/LZMA/Zstd decode rates are **estimates** extrapolated from cycles-per-byte
  on in-order cores; they were not measured on an R4400.
* The swap sequences are **static codegen**, not hardware measurements. The R4400's
  single-issue pipeline makes instructions ≈ cycles for pure ALU sequences, but cache
  misses on the metadata stream are not modelled.
* The 75 MHz R4400 figure comes from the project's hardware notes (`indy-hardware.md` §2);
  100 MHz Indy variants exist and would scale the times by 0.75.
