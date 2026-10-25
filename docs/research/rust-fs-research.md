# IRIX 7 — Storage and Filesystem Architecture

**Rust filesystem implementations, XFS compatibility for IRIX 6.5, licensing, and a native format for new disks**

**Status:** research input to the proposed ADR-0021 (storage architecture) and ADR-0022 (native filesystem). This document does not itself decide; it assembles evidence and makes a recommendation.

**Date:** 2026-10-20.

**Scope.** IRIX 7 is a clean-room, GPL-3.0, Rust-native operating system for the SGI Indy (IP24, MIPS III R4400, big-endian, 32–128 MB RAM, narrow SCSI-2). It must read legacy IRIX 6.5 XFS volumes and eventually write them. All drivers, including SCSI and block, run in userspace cells; the kernel brokers MMIO, DMA buffers and interrupts. Native storage may use a different format. No SGI source and no GPL-2.0-only code may be incorporated.

**Method.** Claims are traced to primary sources: source repositories, crate manifests, on-disk-format documents, kernel documentation, hardware datasheets, and the licences themselves. Repository and crate metadata were read on 2026-10-20 and will drift. Where a project's README makes a capability claim that I did not independently exercise, I say so (for example, "claimed"). The 2026 generation of pure-Rust XFS crates is very new; their existence is a fact, their maturity is not yet a fact.

**A note on names.** "XFS v4" in this report means the superblock version number 4, the non-CRC on-disk format used by IRIX 6.5 and by Linux before v5. It is not the same thing as "XFS v1/v2 inodes" or "v1/v2 directories". IRIX 6.5 can contain either directory format; the distinction matters and is treated separately in §3.6.

---

## Table of contents

1. [Executive summary](#1-executive-summary)
2. [The Rust filesystem landscape](#2-the-rust-filesystem-landscape)
3. [Pure-Rust XFS and the IRIX 6.5 on-disk format](#3-pure-rust-xfs-and-the-irix-65-on-disk-format)
4. [Licensing](#4-licensing)
5. [Architecture options](#5-architecture-options)
6. [Recommended storage architecture](#6-recommended-storage-architecture)
7. [The native filesystem: I7FS](#7-the-native-filesystem-i7fs)
8. [Roadmap, effort and risk](#8-roadmap-effort-and-risk)
9. [Open questions and decisions needed](#9-open-questions-and-decisions-needed)
10. [Appendix A — Sources](#appendix-a--sources)
11. [Appendix B — XFS on-disk cheat sheet](#appendix-b--xfs-on-disk-cheat-sheet)
12. [Appendix C — Licence compatibility matrix](#appendix-c--licence-compatibility-matrix)
13. [Appendix D — A short reading list](#appendix-d--a-short-reading-list)

---

## 1. Executive summary

### 1.1 The headline findings

**1. "There is no pure-Rust XFS" is out of date.** As of October 2026 there are at least four independent pure-Rust XFS readers and one claimed writer:

- **`xfuse` / `xfs-fuse`** — a 2021 Google Summer of Code read-only FUSE server, BSD-2-Clause, 14k downloads, still maintained in 2026. Its directory code is `dir3`-centric (the v5 format); IRIX v4 directory support is not demonstrated.
- **`xfs-core`** (from `xfs-forensic`) — Apache-2.0, from-scratch reader for v4 and v5 superblocks, AG headers, inodes, extents, bmap B+trees, all five directory forms, and v5 CRC32C. It has no log replay and no write path.
- **`lamxfs`** — Apache-2.0, `no_std`, read-only, explicitly supports v4 and v5, shortform/block/leaf/node directories, sparse inodes, reflink read-through, and **v5** in-memory log replay. It **refuses a v4 log that needs replay**.
- **`rust-fs-xfs`** — MIT, very new (created 2026-08-24), claims v4/v5 reading including in-memory dirty-log replay, v5 write through the log, `mkfs` and `fsck`. It has no independent users, no stars, and a two-week commit history at the time of writing. Treat it as a research prototype, not a dependency.

There is also **`mkfs-xfs`** (Apache-2.0), which formats an empty v5 volume, and **`korp-mkfs-xfs`** (MIT/Apache-2.0), which formats an empty v5 volume with CRC-stamped metadata. There is **no** mature pure-Rust XFS writer, and **no** pure-Rust implementation that targets IRIX's v4 log format or IRIX ACLs.

**2. XFS v4 is a tractable read-only target, but not a small one.** The metadata is big-endian, which is a gift on MIPS: no byte swapping on the hot path. The v4 format has no CRCs, no reverse-mapping B+tree, no reflink, no sparse inodes, no free-inode B+tree, no big timestamps and no 64-bit extent counters. A read-only implementation needs the superblock, AG headers, inode core, extents and bmap B+tree, the four directory forms, symlinks and a clean-log check. That is roughly 4–6 person-months for a careful implementation with tests. Dirty IRIX log replay is another 3–5 person-months because IRIX writes `h_fmt = XLOG_FMT_UNKNOWN` and a big-endian log, and Linux explicitly refuses to recover it. Full write support is 18–30 person-months and should not be on the critical path.

**3. The licensing traps are real and specific.** Linux XFS and xfsprogs `libxfs` are **GPL-2.0-only**, which is incompatible with GPL-3.0; they may be read for facts but not copied or linked. OpenZFS is **CDDL-1.0**, also incompatible with the GPL. The `btrfsutils` workspace is a subtler trap: its default is **GPL-2.0-only**, `btrfs-disk`/`btrfs-fuse` override to MIT/Apache-2.0, and `btrfs-cli`/`btrfs-mkfs` stay GPL-2.0-only. Read each crate's manifest, not just the repository licence. MIT, Apache-2.0, BSD-2/3-Clause and MPL-2.0 are compatible with GPL-3.0, with the usual notice and file-level obligations.

**4. The hard architectural problem is DMA, not IPC.** The Indy has no IOMMU. The HPC3 SCSI DMA engine consumes 32-bit physical addresses from a descriptor chain in main memory and transfers at most 8 KiB per descriptor in practice. A userspace driver that can write the HPC3's MMIO registers can program DMA to any physical address. "Userspace driver" therefore gives fault isolation and restartability, but not DMA confidentiality or integrity, unless the kernel owns the descriptor ring and validates every buffer, or the driver is treated as part of the TCB. The report recommends the former and states the cost.

**5. The recommended shape is a native filesystem plus an XFS compatibility server, both in userspace cells.** The first **compatibility** milestone is a clean-room XFS v4 read-only server, mounted on a read-only image/overlay root. The first **product** root is a read-only image, then a new, small, big-endian, journalled extent filesystem — working name **I7FS** — for new disks, because XFS write is the long pole and XFS v4 is a deprecated format with no CRCs. Keep RedoxFS as the fallback if the native filesystem schedule slips. Do not put XFS in the kernel.

### 1.2 The recommendation in one page

| Decision | Recommendation | Why |
|---|---|---|
| Where filesystems run | Userspace cells, one server per volume, over a kernel-brokered block-device capability | Matches the all-drivers-in-userspace decision; restartable; testable on the host via FUSE; no XFS in the TCB |
| Legacy compatibility | Clean-room **XFS v4 read-only** first; dirty-log replay second; write last | Read-only covers booting, executing and copying IRIX data; write is 4–5× the effort and risks corrupting the only copy |
| New disks | **I7FS v1**: 4 KiB blocks, 64-bit, big-endian, CRC32C, extents, B+tree directories, internal journal | Fits the Indy; big-endian-native; crash-consistent; ~5k–8k lines, auditable; no legacy constraints |
| Alternative native FS | Fork **RedoxFS** (MIT) if I7FS slips | Mature-ish, CoW, checksums, no_std-capable, tooling; costs a little-endian port and Redox shims |
| XFS write | Defer; mount read-only by default; require explicit `rw` and a backup | 18–30 person-months; no pure-Rust production writer exists; v4 is deprecated upstream |
| Boot/root early | Read-only image (SquashFS/EROFS or a read-only I7FS image) plus RAM overlay | Gets the system to userland before any writable FS is trustworthy |
| Block cache | In the FS server, 4 KiB blocks, 4–8 MiB on 32 MB machines, 16 MiB on 128 MB | Avoids double caching; keeps the kernel out of the data path |
| DMA | Kernel-owned descriptor rings and pinned bounce buffers; driver submits buffer handles | No IOMMU; the only way to keep a buggy driver from DMA-ing the kernel |
| Licence gate | MIT/Apache/BSD/MPL-2.0 only; GPL-2.0-only and CDDL excluded | GPL-3.0 compatibility |

### 1.3 What would change this recommendation

- If the project decides that **XFS identity** requires new disks to be XFS, then the native filesystem becomes "XFS v4 subset" and the schedule moves by 18–30 person-months. The report argues against this.
- If the target IRIX 6.5 population is dominated by **pre-6.5.14 volumes with v1 directories**, add 2–4 person-months for v1 directory support, or require a one-time `xfsdump`/`xfsrestore` conversion on IRIX.
- If the Indy's SCSI driver is kept in the kernel TCB despite the "all drivers userspace" decision, the DMA problem becomes much easier and the FS server can be fully untrusted. That is a legitimate fallback.
- If a production-grade pure-Rust XFS writer appears and passes `xfstests`/`xfs_repair`, revisit the native-format decision. None exists today.

---

## 2. The Rust filesystem landscape

### 2.1 Inclusion criteria and how to read the table

I included every Rust filesystem implementation I could find that either (a) implements an on-disk format, (b) implements a filesystem protocol that IRIX 7 might use (9p, FUSE/virtiofs, NFS), or (c) is a widely used building block that a storage stack would consider (`fatfs`, `littlefs2`, `embedded-sdmmc`). I excluded pure userspace libraries that merely wrap `std::fs` (for example the `simple-fs` crate, which is a convenience API, not an on-disk filesystem), and I excluded proprietary or unmaintained one-off projects that do not build.

The columns are:

- **Licence** — the licence declared by the repository or crate manifest, not GitHub's heuristic where they disagree.
- **Maturity** — a judgement from stars, downloads, age, tests and whether the project is used as a default filesystem. "Research" means an OS project's in-tree implementation, not a library with external users.
- **R/W** — read/write support, including whether write is complete or partial.
- **no_std** — whether the core can build without `std`; "alloc" means it needs an allocator.
- **BE** — whether the on-disk format is big-endian (native for the Indy) or little-endian (requires swaps), and whether the code is known to handle a big-endian host.
- **Notes** — the limitation that matters for IRIX 7.

### 2.2 The table

| Project / crate | Licence | Maturity | R/W | no_std | On-disk byte order | Notes |
|---|---|---|---|---|---|---|
| **RedoxFS** (`redoxfs` 0.9.1) | MIT | Medium: default Redox FS; 155 stars; 139k downloads; active 2026 | R/W | Yes (alloc; `std` feature for tools) | Little-endian (`endian_num::Le`) | CoW, checksums, encryption, LZ4 compression, 4 KiB blocks, 128 KiB records, 256-block header ring; no journal (CoW), no fsck; depends on `redox_syscall` error types |
| **Redox TFS** (`redox-os/tfs`) | MIT | Abandoned; 2.9k stars; last push 2024 | R/W (historical) | Yes | Little-endian | Content-addressed, snapshottable; replaced by RedoxFS; not the Theseus TFS |
| **Theseus** (`theseus-os/Theseus`) | MIT | Research; 3.2k stars; last push 2024-09 | In-memory only | Yes | N/A | Current tree has `memfs`, `fs_node`, `block_cache`, `storage_manager`; **no persistent on-disk filesystem**; FAT32 was an experiment via `rust-fatfs` |
| **Asterinas** (`asterinas/asterinas`) | MPL-2.0 | Research/production-ish; 5.0k stars; active | R/W for ext2; exFAT; overlayfs; virtiofs | Kernel `no_std` (OSTD) | Little-endian formats | Ext2, exFAT, OverlayFS, RamFS, ProcFS, SysFS, virtiofs; x86-64/RISC-V only; ext2 driver tested with xfstests; extracting it from OSTD is work |
| **rcore-fs** (`rcore-os/rcore-fs`) | MIT | Teaching/research; 52 stars; last push 2024-06 | R/W for SFS/ext2; ramfs | Yes | Little-endian | `rcore-fs-sfs` (simple FS), `rcore-fs-ext2`, `rcore-fs-ramfs`, mountfs, devfs, hostfs, block cache; small and readable; no journal |
| **`fatfs`** (`rafalh/rust-fatfs`) | MIT | High: 363 stars; 1.7M downloads; active | R/W | Yes | Little-endian | FAT12/16/32, LFN, format; no journal, no permissions, 4 GiB file limit; good for exchange media, not a root FS |
| **`littlefs2`** (`trussed-dev/littlefs2`) | Apache-2.0/MIT wrapper; littlefs C is BSD-3 | Medium: 103 stars; 335k downloads | R/W | Yes | Endian-aware, flash-oriented | Rust wrapper over the C littlefs; power-loss resilient, wear levelling; designed for NOR flash, not a 10 GB SCSI disk; small files, no UNIX permissions |
| **`embedded-sdmmc`** | MIT/Apache-2.0 | High: 490 stars; 384k downloads | R/W | Yes | Little-endian | SD/MMC card driver plus FAT16/32; not a general block FS |
| **`ext4-view`** | MIT/Apache-2.0 | Medium-high: 145k downloads; active | Read-only | Yes (alloc) | Little-endian | ext2/3/4 read-only; no journal replay, no write; good for reading Linux images |
| **`ext4_rs`** | MIT | Medium: 72 stars; 11.6k downloads | R/W | Yes (nightly) | Little-endian | OS-independent ext4; **requires nightly** (`#![feature(error_in_core)]`); no journal support found; useful as a reference, not a production root FS |
| **`ntfs`** (`ColinFinck/ntfs`) | MIT/Apache-2.0 | High: 603 stars; 448k downloads | Read-only | Yes | Little-endian | NTFS 3.x, compressed files, ADS, no_std; excellent for reading Windows media; not relevant to IRIX |
| **`exfat` / `exfat-fs`** | MIT | Low: small crates, few downloads | R/W (varies) | Some | Little-endian | exFAT for removable media; low maturity; no crash consistency |
| **`backhand`** (SquashFS) | MIT/Apache-2.0 | High: 172 stars; 293k downloads | R/W/create | No (`std`) | Little-endian | SquashFS 4.0, gzip/xz/zstd/lz4/lzo; excellent for read-only rootfs images; image builder, not a general writable FS |
| **`erofs-rs`** | MIT/Apache-2.0 | Low-medium: 19 stars; 0.3.0; active | Read + build | Yes (alloc) | Little-endian | EROFS reader/builder; good for read-only images; newer than backhand |
| **`p9` / `ninep`** | BSD-3 / MIT | Medium-high | Protocol | No | N/A | 9p2000 server/client; useful for host-guest sharing and exporting, not a disk FS |
| **`fuser`** | MIT | High: 1.3k stars; 5.9M downloads | Protocol | No | N/A | FUSE in Rust; Linux/macOS; invaluable as a **host test harness** for the IRIX 7 FS server |
| **`fuse-backend-rs` / `virtiofsd`** | Apache-2.0 AND BSD-3 / Apache-2.0 AND BSD-3 | High | Protocol | No | N/A | virtio-fs; only relevant if IRIX 7 runs under a hypervisor, not on an Indy |
| **`nfsserve` / `nfs3_server`** | BSD-3 | High | Protocol | No | N/A | NFSv3 server; could export an IRIX 7 volume to legacy clients; not a local FS |
| **`btrfs-disk` / `btrfs-fuse`** | MIT/Apache-2.0 (per-crate override); `btrfs-cli`/`btrfs-mkfs` are GPL-2.0-only | Medium: 15 stars; 0.13.0 | Metadata R/W; FUSE read-only | No | Little-endian | Full Btrfs is complex; the permissive crates are usable, the GPL-2.0-only tools are not; see §4 |
| **`zfs-forensic-core`** | Apache-2.0 | Low: 5.4k downloads; created 2026 | Read-only | No | Little-endian (ZFS) | From-scratch ZFS reader; OpenZFS itself is CDDL-1.0; ZFS is overkill for an Indy |
| **`xfs-core`** | Apache-2.0 | Low-medium: 5.7k downloads; created 2026-07 | Read-only | No | **Big-endian** | v4/v5 superblock, AG headers, inodes, extents, bmap B+tree, dirs, CRC; **no log replay** |
| **`xfuse` / `xfs-fuse`** | BSD-2-Clause | Low-medium: 14k downloads; GSoC 2021; maintained 2026 | Read-only | No | **Big-endian** | FUSE server; `dir3`-centric; v5 tested; v4/IRIX unproven |
| **`lamxfs`** | Apache-2.0 | Low: 493 downloads; created 2026-06; 10 commits | Read-only | **Yes** | **Big-endian** | v4/v5, all dir forms, v5 log replay, v4 log refused; clean, small, well-documented |
| **`rust-fs-xfs`** | MIT | Very low: created 2026-08; 874 downloads; 0 stars | v4/v5 read; v5 write (claimed) | No | **Big-endian** | Claims dirty-log replay, v5 journalled write, fsck, mkfs; two-week history; treat as prototype |
| **`mkfs-xfs` / `korp-mkfs-xfs`** | Apache-2.0 / MIT OR Apache-2.0 | Very low | Format empty v5 | No | **Big-endian** | Useful for test images; not a driver |
| **`xfs-forensic`** | Apache-2.0 | Low | Read-only | No | **Big-endian** | Auditor built on `xfs-core`; deleted-inode recovery, CRC checks |
| **`squashfs` / `squashfs_reader`** | MIT | Low/abandoned | Read-only | Varies | Little-endian | Older SquashFS readers; prefer `backhand` |
| **`tarfs`** | MIT | Low | Read-only | Likely | N/A | Tar archive as a filesystem; initramfs use |
| **`iso9660`** | MIT/Apache-2.0 | Low | Read-only | No | Both (ISO 9660 is LE) | Incomplete; CD-ROM only |

### 2.3 What the table means for IRIX 7

**A note on `simplefs`.** The crates named `simplefs`/`simple-fs` are not on-disk filesystem implementations; `simple-fs` is a convenience wrapper over `std::fs`. The canonical teaching "simple FS" in the Rust OS world is `rcore-fs-sfs`, which is a real on-disk format but too limited for a system disk.

**There is no drop-in native filesystem.** The candidates split into four groups:

1. **Mature general-purpose Rust filesystems:** RedoxFS and `fatfs`. RedoxFS is the only one with CoW, checksums and a realistic UNIX feature set; `fatfs` is simple and robust but has no permissions, no journal and a 4 GiB file limit.
2. **Mature read-only or image filesystems:** `ext4-view`, `ntfs`, `backhand`, `erofs-rs`. None can be a writable root.
3. **Research kernels with in-tree filesystems:** Theseus (in-memory only), Asterinas (ext2/exFAT/overlayfs/virtiofs), rcore-fs (SFS/ext2). Their code is useful as a reference and, where the licence allows, as a porting source, but it is coupled to their kernel abstractions.
4. **Protocols:** 9p, FUSE/virtiofs, NFS. These are interoperability and testing tools, not storage formats.

**Theseus is not a source of a persistent filesystem.** The current Theseus tree contains `kernel/memfs`, `kernel/fs_node`, `kernel/block_cache`, `kernel/storage_manager` and `kernel/storage_device`; `kernel/memfs` is explicitly an in-memory filesystem backed by `MappedPages`. There is no `tfs` crate in the current tree. The `redox-os/tfs` project is a different, abandoned content-addressed filesystem, and the `tfs` crate on crates.io is a third, unrelated 2016 project. Anyone who remembers "Theseus TFS" should check the tree before assuming it exists. Source: <https://github.com/theseus-os/Theseus/tree/master/kernel>, <https://www.theseus-os.com/Theseus/doc/memfs/>, <https://github.com/redox-os/tfs>, <https://crates.io/crates/tfs>.

**Asterinas is the most interesting research implementation.** It has a reworked ext2 driver with sparse files, delayed allocation removed in favour of Linux-like allocation, rollback on error and a sync path; it is tested against xfstests, LTP and gVisor. It also has exFAT, overlayfs, ramfs, procfs, sysfs and virtiofs. But it is MPL-2.0 (compatible with GPL-3.0), built on OSTD, and little-endian x86-64/RISC-V. Porting its ext2 driver means extracting it from OSTD's page cache, block layer and error model. It is a good reference for "what a production Rust ext2 looks like", not a drop-in. Sources: <https://github.com/asterinas/asterinas>, <https://arxiv.org/abs/2506.03876>, <https://asterinas.github.io/2026/10/13/announcing-asterinas-0.19.0.html>.

**RedoxFS is the only realistic "port an existing writable FS" candidate.** It is MIT, has a `no_std` core, 4 KiB blocks, CoW, checksums, encryption, compression, a FUSE driver for host testing, and `mkfs`/`resize`/`clone` tools. Its weaknesses for IRIX 7 are equally clear: the on-disk format is explicitly little-endian (`endian_num::Le`), the core depends on `redox_syscall` error types, the transaction engine is ~2,000 lines with 63 `unsafe` blocks, the header ring is 256 blocks, records are 128 KiB, and there is no fsck. A port is a bounded but real project; §5.5 estimates it.

**`fatfs`, `littlefs2` and `embedded-sdmmc` are not root-filesystem candidates.** FAT has no permissions, no hard links, no journal and a 4 GiB file limit. littlefs is designed for NOR flash and tiny files. embedded-sdmmc is an SD card driver. They are useful for exchange media (floppy, ZIP, CF) and for bootstrapping, not for the Indy's system disk.

**SquashFS and EROFS are the right phase-0 rootfs.** Both are read-only image formats with host-side builders (`mksquashfs`, `mkfs.erofs`) and Rust readers (`backhand`, `erofs-rs`). A read-only image plus a RAM overlay gets IRIX 7 to userland before any writable filesystem is trustworthy. They are little-endian, but reads are bulk decompression, so the byte-swap cost is negligible compared with the CPU cost of decompression on an R4400.

**FUSE is the secret weapon for development.** The FS server can be compiled twice: once for IRIX 7, once as a Linux FUSE server using `fuser`. That gives a fast edit/run/test loop on the host, with the same on-disk code, before booting the Indy. `fuser` is MIT and mature (5.9M downloads). The IRIX 7 server should expose a small `BlockDevice` trait so the host build can use an image file and the target build can use the SCSI block-device capability.

---

## 3. Pure-Rust XFS and the IRIX 6.5 on-disk format

### 3.1 The state of the art, precisely

The crates.io search for "xfs" returns a confusing mix. The crate literally named **`xfs` 0.2.0** is a parser for XFS performance data (`xfs_info` output), not a filesystem. The filesystem work is in the crates below.

| Crate / repo | Version / date | Licence | v4 | v5 | Dirs | Log replay | Write | no_std | Assessment |
|---|---|---|---|---|---|---|---|---|---|
| `xfs-core` (<https://github.com/SecurityRonin/xfs-forensic>) | 0.1.6, 2026-08 | Apache-2.0 | Yes | Yes | shortform, block, leaf, node, btree | **No** | No | No | Best-documented v4/v5 **reader**; forensic focus; no log, no write; 146 files, 81 commits |
| `lamxfs` (<https://github.com/lamco-admin/lamxfs>) | 0.1.3, 2026-10 | Apache-2.0 | Yes | Yes | all forms | **v5 only**; v4 refused | No | **Yes** | Clean, small, no_std; designed for UEFI bootloaders; v4 log refusal is explicit |
| `xfuse` / `xfs-fuse` (<https://github.com/KhaledEmaraDev/xfuse>) | 0.7.1, 2026-09 | BSD-2-Clause | Superblock accepted | Yes | `dir3` code | No | No | No | GSoC 2021; FUSE; v5-tested; v4 directory support unproven |
| `rust-fs-xfs` (<https://github.com/antimatter-studios/rust-fs-xfs>) | 0.14.2, 2026-10 | MIT | Yes (read-only) | Yes | all forms | **Claimed** v4/v5 in-memory | **Claimed** v5 journalled; v4 overwrite only | No | Created 2026-08; 100+ commits in a two-week burst; 428 files; claims `fsck`/`mkfs`; no independent users |
| `mkfs-xfs` (<https://github.com/justincormack/mkfs-xfs>) | 0.1.0, 2026-05 | Apache-2.0 | No | Empty v5 | n/a | n/a | Format only | No | Byte-compares against pinned `mkfs.xfs`; useful for fixtures |
| `korp-mkfs-xfs` (crates.io) | 0.1.2, 2026-09 | MIT/Apache-2.0 | No | Empty v5 | n/a | n/a | Format only | No | CRC-stamped empty v5; appliance-oriented |
| `xfs-forensic` | 0.1.4, 2026-08 | Apache-2.0 | Yes | Yes | all forms | No | No | No | Auditor on top of `xfs-core`; deleted-inode recovery |

**Direct answer.** Yes: pure-Rust XFS **readers** exist (at least `xfs-core`, `lamxfs`, `xfuse`, and the reader half of `rust-fs-xfs`). A pure-Rust XFS **writer** is claimed by `rust-fs-xfs`, but there is no production-grade writer, no independent validation, and no IRIX v4 writer. A clean-room read-only implementation is still required for IRIX 6.5 because none of the existing crates supports v1 directories, IRIX ACLs, or IRIX's dirty-log format.

The important conclusions:

- **A read-only XFS reader is no longer a research gap.** There are four independent implementations, two of which (`xfs-core`, `lamxfs`) explicitly handle v4. `lamxfs` is the most interesting for IRIX 7 because it is `no_std`, small, and honest about what it refuses.
- **IRIX-specific support is still a gap.** None of these crates mentions IRIX in its source or documentation (I searched the READMEs and `src/` for "IRIX"). None implements IRIX ACLs, IRIX `XLOG_FMT_IRIX_BE` replay, IRIX v1 directories, or IRIX's `sb_features2` variants. `lamxfs` supports v4 *Linux* filesystems and refuses a dirty v4 log; IRIX 6.5 volumes are v4 with a big-endian log that Linux calls `XLOG_FMT_UNKNOWN`.
- **The claimed writer is not a dependency.** `rust-fs-xfs` may turn out to be excellent, but a project whose entire commit history is two weeks old, with no users, cannot be the foundation of a compatibility story for irreplaceable IRIX disks. It is worth watching, fuzzing and comparing against `xfs_db`; it is not worth trusting yet.
- **The permissive licences are compatible with GPL-3.0**, so these crates can be used as references, test oracles, or (after provenance review) as a starting point. Apache-2.0 is compatible with GPL-3.0; BSD-2 and MIT are compatible. The provenance question is separate: if a crate copied GPL-2.0-only Linux code, its Apache-2.0 declaration is not curative. `lamxfs` claims a clean-room implementation from the public format document and the Linux on-disk headers; `xfs-core` claims "from-scratch". Audit before reuse.

### 3.2 What "XFS v4" means, and what IRIX actually wrote

XFS's superblock has a version number in the low four bits of `sb_versionnum`:

| Value | Meaning | Era |
|---|---|---|
| 1 | Original XFS | IRIX 5.3, 6.0.1, 6.1 |
| 2 | Attributes | IRIX 6.2 |
| 3 | New inode version | IRIX 6.2 |
| 4 | Bitmask version (the v4 format) | IRIX 6.2+ and Linux until v5 |
| 5 | CRC/self-describing metadata (v5) | Linux 3.15+ (2014) |

Source: `xfs_format.h`, lines 29–48, <https://github.com/torvalds/linux/blob/master/fs/xfs/libxfs/xfs_format.h>. The SGI on-disk-format book describes the v4 layout and says "the value must be 4 plus the following flags" (<http://dubeyko.com/development/FileSystems/XFS/xfs_filesystem_structure.pdf>, p. 9). That book is the best vendor-authored description of the format IRIX actually shipped; its journal chapter is a TODO, which is why the log section below relies on Linux's `xfs_log_format.h` and the recovery code.

IRIX's own `mkfs_xfs` man page is equally important:

- inode size default **256 bytes**, minimum 256 in IRIX 6.5 (128 in IRIX 5.3), maximum 2048;
- directory format **version 1 or 2**, defaulting to 2 in the man page's later revisions; IRIX 6.5.5 introduced v2 directories, and IRIX 6.5.14 made them the default;
- v2 directories allow a directory block size from the filesystem block size up to 64 KiB;
- filesystems created with v2 directories cannot be mounted by IRIX releases older than 6.5.5.

Sources: <https://nixdoc.net/man-pages/IRIX/man1/mkfs_xfs.1.html>, <https://techpubs.jurassic.nl/library/manuals/2000/007-2825-013/sgi_html/ch06.html>, IRIX 6.5.14 release notes (<https://wiki.preterhuman.net/IRIX_6.5.14>), IRIX 6.5.5 update guide (<https://pixelbart.net/SGI/IRIX/pdf/irix.6.5.5.pdf>).

So an IRIX 6.5 volume can be:

- **v2 inode + v2 directory** — the common modern case, and the one Linux's remaining v4 support handles;
- **v2 inode + v1 directory** — a 6.5.5–6.5.13 volume with v1 directories; individual inodes may be v1 or v2, because inodes convert on demand (link-count overflow or project ID), not filesystem-wide;
- **v1 inode + v1 directory** — older volumes whose inodes never needed conversion, with v1 directories. A v1 inode can appear in an otherwise v2-directory volume, so accept both inode versions per inode.

The v1 directory is the compatibility trap. The SGI book says "Only v2 directories covered here. v1 directories are obsolete" (p. 38). Linux's XFS maintainers removed v1 directory support in 2015 ("we don't support filesystems older than dir v2 support", commit `5f6f366`, <https://github.com/chandanr/xfsprogs-dev/commit/5f6f36600d1acb8c89386e5efd654c9b84f57c75>), and xfsprogs 4.10.0-rc1 removed "old Irix support" in 2017 (<https://git.kernel.org/pub/scm/fs/xfs/xfsprogs-dev.git/plain/doc/CHANGES>). A v1-directory IRIX volume is therefore unreadable by current Linux, and I found no pure-Rust XFS crate that advertises v1-directory support. IRIX 7 has a unique opportunity: it can support the format that everyone else abandoned.

### 3.3 The on-disk format a reader must implement

This section is a working map, not a replacement for the SGI book. Offsets are in bytes unless stated. All multi-byte fields are big-endian except where noted.

#### 3.3.1 Superblock and geometry

The superblock is one sector at offset 0 of each allocation group (AG). The primary is AG 0; secondaries are used by repair. The v4 fields a reader needs:

| Field | Type | Use |
|---|---|---|
| `sb_magicnum` | u32 | `0x58465342` ("XFSB") |
| `sb_blocksize` | u32 | Filesystem block size, 512–65536 |
| `sb_dblocks` | u64 | Total data blocks |
| `sb_rblocks`, `sb_rextents` | u64 | Realtime device; normally zero |
| `sb_uuid` | 16 bytes | Filesystem identity |
| `sb_logstart` | u64 | First filesystem block of the internal log; 0 for external |
| `sb_rootino` | u64 | Root inode number; often 128 on a 4 KiB-block filesystem |
| `sb_rbmino`, `sb_rsumino` | u64 | Realtime bitmap/summary inodes; normally unused |
| `sb_rextsize` | u32 | Realtime extent size |
| `sb_agblocks` | u32 | Blocks per AG; the last AG uses `agf_length` |
| `sb_agcount` | u32 | Number of AGs |
| `sb_logblocks` | u32 | Log size in filesystem blocks |
| `sb_versionnum` | u16 | Version + feature bits |
| `sb_sectsize` | u16 | Sector size, usually 512; can be 4096 |
| `sb_inodesize` | u16 | Inode size, usually 256 |
| `sb_inopblock` | u16 | Inodes per block = blocksize / inodesize |
| `sb_fname[12]` | bytes | Label |
| `sb_blocklog`, `sb_sectlog`, `sb_inodelog`, `sb_inopblog`, `sb_agblklog` | u8 | Logarithms used for address arithmetic |
| `sb_inprogress` | u8 | Non-zero means `mkfs` was interrupted; refuse |
| `sb_icount`, `sb_ifree`, `sb_fdblocks`, `sb_frextents` | u64 | Counters; not needed for read-only |
| `sb_uquotino`, `sb_gquotino`, `sb_qflags` | mixed | Quotas; optional |
| `sb_inoalignmt`, `sb_unit`, `sb_width` | u32 | Alignment; not needed for read-only |
| `sb_dirblklog` | u8 | Directory block size = `blocksize << dirblklog` |
| `sb_logsectlog`, `sb_logsectsize`, `sb_logsunit` | mixed | External log geometry |
| `sb_features2` | u32 | Extra feature bits if `MOREBITSBIT` is set |

The canonical description is SGI's book, pp. 7–10, and `xfs_format.h`'s `struct xfs_dsb`. The v5 superblock adds `sb_features_compat`, `sb_features_ro_compat`, `sb_features_incompat`, `sb_features_log_incompat`, `sb_crc`, `sb_spino_align`, `sb_pquotino`, `sb_lsn`, `sb_meta_uuid` and more. For IRIX, those fields do not exist; do not read past the v4 structure.

#### 3.3.2 Allocation groups and their headers

An AG is a self-contained region of `sb_agblocks` blocks (the last AG may be shorter). The first four sectors of each AG are:

- block 0 / sector 0: superblock;
- sector 1: AGF (`XAGF`, `0x58414746`), free-space B+tree roots, free block count, `agf_length`;
- sector 2: AGI (`XAGI`, `0x58414749`), inode B+tree root, inode count, free inode count, 64 unlinked buckets;
- sector 3: AGFL (`XAFL`, `0x5841464c`), the AG free list.

The AG block layout after the headers is: free-space B+tree root(s), inode B+tree root, free list blocks, inode chunks, then data/metadata blocks. The SGI book's AG chapter (pp. 6–22) has the diagram. `xfs_format.h` has the exact structures (`struct xfs_agf`, `xfs_agi`, `xfs_agfl`).

For a read-only IRIX reader, the AGF/AGI are mostly optional: you can translate an inode number to a disk address directly (next section) and read the root inode. You need `agf_length` only to validate the last AG and to iterate all blocks for fsck. You need the AGI/inobt only to enumerate inodes or to handle sparse inodes (v5 only). That is a useful simplification: **v4 read-only does not need the free-space B+trees at all.**

#### 3.3.3 Inode numbers and inode chunks

XFS inode numbers are not arbitrary. An absolute inode number is:

```
agno   = ino >> (sb_agblklog + sb_inopblog)
agino  = ino & ((1 << (sb_agblklog + sb_inopblog)) - 1)
agbno  = agino >> sb_inopblog
offset = agino & (sb_inopblock - 1)
fsb    = agno * sb_agblocks + agbno
byte   = fsb * sb_blocksize + offset * sb_inodesize
```

The AG-relative inode number uses `sb_agblklog + sb_inopblog` bits; the absolute number puts the AG number above those bits. Directory entries store absolute 64-bit inode numbers (except shortform directories, which may store 32-bit numbers). The SGI book, pp. 18–19, has the bit diagram. The root inode is typically 128 on a 4 KiB-block filesystem: AG 0, block 8, slot 0 with 256-byte inodes.

Inodes are allocated in **chunks of 64**, tracked by the inode B+tree (`inobt`), whose leaf records are `{ir_startino, ir_freecount, ir_free}` with a 64-bit free bitmap. In v4 the chunk is always a full, aligned 64-inode block (plus the inode block's own layout), so the direct mapping above is valid. In v5, **sparse inodes** allow a chunk to be partially populated; `ir_free` gains a hole mask and the direct mapping is no longer sufficient. This is one of the reasons IRIX v4 is materially easier than current Linux XFS.

#### 3.3.4 Inode core and forks

A v1/v2 inode has a **100-byte** header: 96 bytes of core plus `di_next_unlinked`. The literal area (data fork + attribute fork) starts at offset **100** (`0x64`). A v3 inode (v5 only) has a 176-byte header and the literal area starts at **176** (`0xb0`).

The v2 core, in order:

| Field | Type | Notes |
|---|---|---|
| `di_magic` | u16 | `0x494e` ("IN") |
| `di_mode` | u16 | Standard `S_IF*` + permissions |
| `di_version` | u8 | 1 or 2 (3 for v5) |
| `di_format` | u8 | 0 = dev, 1 = local, 2 = extents, 3 = btree, 4 = uuid |
| `di_onlink` | u16 | v1 link count; unused in v2 |
| `di_uid` | u32 | Owner |
| `di_gid` | u32 | Group |
| `di_nlink` | u32 | Link count (v2) |
| `di_projid_lo` | u16 | Project ID low |
| `di_projid_hi` | u16 | Project ID high (if `PROJID32BIT`) |
| `di_pad[6]` | bytes | Zero |
| `di_flushiter` | u16 | Incremented on flush |
| `di_atime`, `di_mtime`, `di_ctime` | 8 bytes each | `{i32 sec, i32 nsec}` |
| `di_size` | u64 | File size in bytes |
| `di_nblocks` | u64 | Blocks used, including btree metadata |
| `di_extsize` | u32 | Extent size hint |
| `di_nextents` | u32 | Data fork extent count |
| `di_anextents` | u16 | Attribute fork extent count |
| `di_forkoff` | u8 | Attribute fork offset in 8-byte units; 0 = no attributes |
| `di_aformat` | i8 | Attribute fork format |
| `di_dmevmask` | u32 | DMAPI |
| `di_dmstate` | u16 | DMAPI |
| `di_flags` | u16 | `XFS_DIFLAG_*` |
| `di_gen` | u32 | Generation number |
| `di_next_unlinked` | u32 | AG-relative next inode on the unlinked list |

Sources: SGI book, pp. 23–29; `xfs_format.h` `struct xfs_dinode`; the current XFS documentation's inode chapter (<https://kernel.googlesource.com/pub/scm/linux/kernel/git/djwong/xfs-documentation/+/refs/heads/master/design/XFS_Filesystem_Structure/ondisk_inode.asciidoc>).

The **data fork** is at offset 100 (or 176 for v3) and extends to `di_forkoff * 8` if attributes exist, otherwise to the end of the inode. The **attribute fork** runs from `di_forkoff * 8` to the end. With `attr1`, `di_forkoff` is fixed in the middle; with `attr2` (the `XFS_SB_VERSION2_ATTR2BIT` feature), it grows down from the end as attributes are added. For a read-only reader, `di_forkoff == 0` means "no attributes" and the whole literal area is data.

#### 3.3.5 Extents and the bmap B+tree

An extent is a **128-bit packed record**, 16 bytes, big-endian:

| Bits | Width | Meaning |
|---|---|---|
| 127 | 1 | Unwritten flag (1 = allocated but not written) |
| 126–73 | 54 | Logical file block offset |
| 72–21 | 52 | Absolute filesystem block number |
| 20–0 | 21 | Block count |

The layout is documented in the SGI book, p. 31, and in `xfs_format.h` as `BMBT_EXNTFLAG_BITLEN`, `BMBT_STARTOFF_BITLEN`, `BMBT_STARTBLOCK_BITLEN`, `BMBT_BLOCKCOUNT_BITLEN`. The 21-bit block count means a single extent is at most 2 MiB blocks (8 GiB at 4 KiB blocks), which is ample for IRIX-era files.

The 52-bit start block is an XFS `xfs_fsblock_t`: the AG number is in the high bits and the AG-relative block number is in the low `sb_agblklog` bits. Convert it to a physical block with `agno = startblock >> sb_agblklog; agbno = startblock & ((1 << sb_agblklog) - 1); physical = agno * sb_agblocks + agbno`. Do not treat the field as a flat byte or block address.

If `di_format == XFS_DINODE_FMT_EXTENTS`, the data fork is an array of `di_nextents` extents. With a 256-byte inode and no attributes, about 19 extents fit (SGI book, p. 32). If the file needs more, `di_format == XFS_DINODE_FMT_BTREE` and the data fork holds a `xfs_bmdr_block` root:

```
u16 level; u16 numrecs; key[1..numrecs] (u64 startoff); ptr[1..numrecs] (u64 fsblock)
```

The root points to **long-format** B+tree blocks with magic `BMAP` (v4) or `BMA3` (v5), a `xfs_btree_lblock` header (magic, level, numrecs, leftsib, rightsib, blkno, lsn, uuid, owner, crc in v5), and either keys/pointers (internal nodes) or 16-byte extents (leaves). The B+tree is keyed by logical start offset; leaves are linked left/right; a lookup descends by comparing the file offset. The SGI book, pp. 34–37, has the diagrams; `xfs_format.h` has `struct xfs_bmbt_rec`, `xfs_bmbt_key`, `xfs_bmbt_ptr`, `xfs_bmdr_block`.

The reader algorithm is conventional: for a logical block, binary-search the inline extent array or descend the bmbt; if the offset falls in a hole, return zeros; if the extent is unwritten, return zeros; if the extent is normal, read `blockcount` blocks from `startblock`. Handle extents that begin before the requested offset and end after it.

#### 3.3.6 Directories

XFS v4 has four directory forms, selected by `di_format` and size:

1. **Shortform** (`di_format = LOCAL`): the directory is inside the inode. Header:
   ```
   u8 count; u8 i8count; inode_number parent;
   entry[] { u8 namelen; u16 offset; u8 name[namelen]; inode_number ino; }
   ```
   `count` is the number of entries when all inode numbers fit in 32 bits; `i8count` is the number when 64-bit numbers are used. `.` is implicit; `..` is the `parent` field. Entries are packed, with the inode number 4 or 8 bytes depending on `i8count`. The `offset` is the directory offset used for `readdir` cookies.
2. **Block** (`di_format = EXTENTS`, one directory block): a single directory block (size `blocksize << dirblklog`, up to 64 KiB) containing a `xfs_dir2_data_hdr` with magic `XD2B` (`0x58443242`), data entries, then embedded `xfs_dir2_leaf_entry` records and a `xfs_dir2_block_tail {count, stale}`. There is no separate leaf magic in this form. It is used when the directory fits in one block but not in the inode.
3. **Leaf** (`di_format = EXTENTS`, multiple data blocks plus a leaf block): data blocks have magic `XD2D` (`0x58443244`); the leaf block has magic `XD2F1` (`0xd2f1`, single-leaf) or `XD2FF` (`0xd2ff`, multi-leaf), a `xfs_dir2_leaf_hdr` with `forw`/`back`/`magic`/`count`/`stale`, an array of `{hash, address}` entries sorted by hash, and a tail of best-free values per data block. The leaf entries are the hash index; the data blocks hold the names and inodes.
4. **Node** (`di_format = BTREE`): a B+tree of directory data blocks. Internal nodes use `xfs_da_node_hdr` magic `0xfebe` and `xfs_da_node_entry` `{hash, before}`; leaves are the leaf blocks above. This is used when the leaf index itself outgrows one block.

The current Linux magic numbers are in `xfs_da_format.h`: `XFS_DIR2_BLOCK_MAGIC 0x58443242`, `XFS_DIR2_DATA_MAGIC 0x58443244`, `XFS_DIR2_FREE_MAGIC 0x58443246`, `XFS_DIR2_LEAF1_MAGIC 0xd2f1`, `XFS_DIR2_LEAFN_MAGIC 0xd2ff`, `XFS_DA_NODE_MAGIC 0xfebe`. The v5 equivalents are `XDB3`, `XDD3`, `XDF3`, `0x3df1`, `0x3dff`, `0x3ebe`; the low bits are deliberately similar so a reader can distinguish v2 from v3 by magic. Source: <https://github.com/torvalds/linux/blob/master/fs/xfs/libxfs/xfs_da_format.h>.

A data entry is `{u64 inumber; u8 namelen; u8 name[]; u8 ftype (optional); padding}`. In v4, `ftype` is present only if `sb_features2 & XFS_SB_VERSION2_FTYPE`; in v5 it is always present. The free-space tag is `0xffff`. Directory lookup uses `xfs_da_hashname` (a 32-bit hash) to find the leaf entry, then compares the full name to resolve collisions. A simple reader can linear-scan every data block for small directories and use the hash index for large ones.

#### 3.3.7 Symlinks, devices and extended attributes

- **Symlinks** are either `di_format = LOCAL` (the target is the data fork bytes, up to the inode's literal area) or `EXTENTS` (the target is stored in data blocks). The length is `di_size`; there is no NUL terminator on disk.
- **Device nodes** store the device number in the data fork (`xfs_dev_t`), format `DEV`.
- **Extended attributes** live in the attribute fork. `di_aformat = LOCAL` uses `xfs_attr_shortform` (name/value pairs packed after a 2-byte count); `EXTENTS` uses an extent list of attribute blocks; `BTREE` uses a bmap-style B+tree. Attribute blocks have `XFS_ATTR_LEAF_MAGIC 0xfbee` for leaves and `XFS_DA_NODE_MAGIC 0xfebe` for nodes. Names are namespace-prefixed (`user.`, `trusted.`, `security.`, `system.`); IRIX ACLs are stored as `SGI_ACL_FILE` and `SGI_ACL_DEFAULT`, not as Linux's `system.posix_acl_*` names. Source: Linux `fs/xfs/xfs_acl.c`, <https://github.com/torvalds/linux/blob/master/fs/xfs/xfs_acl.c>.

For a first read-only mount, attributes can be ignored; `stat` still works, but ACL enforcement and `getfattr` do not. For IRIX compatibility, ACLs matter: an IRIX file's mode bits are only part of its permission model.

#### 3.3.8 The journal (log)

The log is a circular region of `sb_logblocks` filesystem blocks, either internal at `sb_logstart` or external (a separate device). It is addressed in **512-byte basic blocks**. Each record starts with a 512-byte `xlog_rec_header`:

| Field | Type | Notes |
|---|---|---|
| `h_magicno` | be32 | `0xFEEDBABE` |
| `h_cycle` | be32 | Write cycle |
| `h_version` | be32 | 1 or 2 |
| `h_len` | be32 | Record length in bytes, 64-bit aligned |
| `h_lsn` | be64 | Log sequence number of this record |
| `h_tail_lsn` | be64 | First record with uncommitted buffers |
| `h_crc` | **le32** | v5 only; note the little-endian field in big-endian metadata |
| `h_prev_block` | be32 | Previous record block |
| `h_num_logops` | be32 | Number of operations |
| `h_cycle_data[]` | be32 | Cycle stamping for data blocks |
| `h_fmt` | be32 | `XLOG_FMT_*`; **IRIX leaves this zero** |
| `h_fs_uuid` | 16 bytes | Linux-added; **IRIX leaves this zero** |
| `h_size` | be32 | Log v2 record size |

Source: `xfs_log_format.h`, <https://github.com/torvalds/linux/blob/master/fs/xfs/libxfs/xfs_log_format.h>. The format constants are the key to IRIX compatibility:

```
XLOG_FMT_UNKNOWN  0
XLOG_FMT_LINUX_LE 1
XLOG_FMT_LINUX_BE 2
XLOG_FMT_IRIX_BE  3
```

Linux's recovery explicitly refuses a dirty IRIX log:

> "IRIX doesn't write the `h_fmt` field and leaves it zeroed (`XLOG_FMT_UNKNOWN`). This stops us from trying to recover a dirty log created in IRIX."
>
> — `xlog_header_check_recover`, `fs/xfs/xfs_log_recover.c`, <https://github.com/torvalds/linux/blob/master/fs/xfs/xfs_log_recover.c>

It *does* mount a clean IRIX log: `xlog_header_check_mount` tolerates a null `h_fs_uuid` and prints "null uuid in log - IRIX style log". This is why a clean IRIX XFS volume can still be mounted on Linux (with v4 support enabled), while a crashed one cannot. It is also the strongest argument for implementing XFS on IRIX 7 itself: IRIX 7 is big-endian, so it can read and write the IRIX log format natively, and it can recover volumes that Linux refuses.

Each log record contains `xlog_op_header` operations (`oh_tid`, `oh_len`, `oh_clientid`, `oh_flags`, `oh_res2`) followed by payloads. The flags are `XLOG_START_TRANS`, `XLOG_COMMIT_TRANS`, `XLOG_CONTINUE_TRANS`, `XLOG_WAS_CONT_TRANS`, `XLOG_END_TRANS`, `XLOG_UNMOUNT_TRANS`. The payloads are typed log items: buffer items (`xfs_buf_log_format`), inode items (`xfs_inode_log_format` plus `xfs_log_dinode`), inode-create items, dquot items, EFI/EFD, and (v5) rmap/refcount/bmap/exchange intent items. A v4 log is simpler than v5, but still requires a real transaction redo engine.

**The clean-log check.** A filesystem is cleanly unmounted if the last record before the head is an unmount record (`XLOG_UNMOUNT_TRANS`) and the head/tail accounting says there is nothing to replay. Linux's `xlog_find_tail` and `xfs_log_clean` implement this. For a first IRIX 7 mount, implement the check and refuse anything dirty; that is a few hundred lines and avoids the entire recovery engine.

#### 3.3.9 B+tree conventions across the filesystem

XFS uses several B+tree families, and they share conventions but not headers:

| Family | Header | Pointer width | Key | Used by |
|---|---|---|---|---|
| Free space by block (`bnobt`) | `xfs_btree_sblock` | 32-bit AG block | start block | AGF, allocation/fsck |
| Free space by count (`cntbt`) | `xfs_btree_sblock` | 32-bit AG block | block count | AGF, allocation/fsck |
| Inode allocation (`inobt`) | `xfs_btree_sblock` | 32-bit AG block | AG-relative inode number | AGI, inode lookup/fsck |
| Free inode (`finobt`, v5) | `xfs_btree_sblock` | 32-bit AG block | AG-relative inode number | AGI, allocation |
| Bmap (`bmbt`) | `xfs_btree_lblock` | 64-bit fsblock | logical file offset | inode data/attr fork |
| Reverse map (`rmapbt`, v5) | `xfs_btree_lblock` | 64-bit fsblock | physical block/owner | fsck/repair |
| Refcount (`refcountbt`, v5) | `xfs_btree_lblock` | 64-bit fsblock | physical block | reflink |
| Directory/attribute | `xfs_da_node_hdr` | 32-bit directory block | name hash | dir/attr data |

The **short** header (`xfs_btree_sblock`) contains magic, level, numrecs, left sibling, right sibling; pointers are 32-bit AG-relative block numbers. The **long** header (`xfs_btree_lblock`) contains magic, level, numrecs, left/right siblings, block number, LSN, UUID, owner and (v5) CRC; pointers are 64-bit `xfs_fsblock_t` values. In both, level 0 is a leaf; internal nodes hold `numrecs` keys and `numrecs` pointers; keys are sorted; sibling links allow ordered iteration. The root level comes from the inode (`di_bmbt.level`), the AGF (`agf_levels[]`) or the AGI (`agi_level`). Cap the level at a small geometry-derived maximum and detect cycles; a corrupt image can otherwise loop forever.

For a read-only IRIX mount, the **only** trees on the hot path are the bmap tree (for files with many extents) and the directory/attribute trees. The free-space and inode trees are needed for `fsck`, allocation and write. That is another reason read-only is a much smaller project than write.

#### 3.3.10 v5 CRC and self-describing metadata

Everything in this subsection is **v5 only**; IRIX 6.5 does not have it, but a reader that will survive contact with modern Linux disks should know it.

- **Every metadata block gets a header** with magic, CRC32C, UUID, owner, block number and LSN. The magic numbers change (`BMAP` → `BMA3`, `XD2B` → `XDB3`, `IABT` → `IAB3`, `FIBT` → `FIB3`), so a reader can tell v4 from v5 by the magic before parsing the body. Source: <https://docs.kernel.org/filesystems/xfs/xfs-self-describing-metadata.html>.
- **The CRC field is little-endian** (`__le32`), even though the surrounding metadata is big-endian. This is deliberate (the checksum is computed over the byte stream and stored in a fixed-width field); it is also a classic source of endian bugs. The inode `di_crc` and the log record `h_crc` are both `__le32`.
- **Inodes are v3**: 176-byte header, `di_crc`, `di_changecount`, `di_lsn`, `di_flags2`, `di_cowextsize`, `di_crtime`, `di_ino`, `di_uuid`. The literal area starts at 176, not 100.
- **Sparse inodes**: an inode chunk can be partially populated. The inobt record's free bitmap is replaced/augmented by a hole mask, and the direct inode-number-to-block mapping is no longer valid for a sparse chunk. You must consult the inobt. This is why IRIX v4 is easier.
- **Reverse mapping (`rmapbt`)**: records every block's owner, used by online repair and reflink. Not needed for reads, but a v5 reader that wants to validate ownership must parse it.
- **Reflink**: two inodes can share extents; `di_flags2 & XFS_DIFLAG2_REFLINK` marks the file, and the refcount B+tree tracks shared blocks. Read pass-through is easy (read the shared extent normally); write requires copy-on-write and refcount updates.
- **Free inode btree (`finobt`)**, **inode btree block counts (`inobtcount`)**, **bigtime** (64-bit nanosecond timestamps with an epoch offset), **nrext64** (64-bit extent counters), **parent pointers**, **metadir**, **exchangerange**, **zoned**: all optional v5 features. Refuse unknown bits rather than misreading.
- **`meta_uuid`**: if set, metadata headers use `sb_meta_uuid` rather than `sb_uuid`; a reader must check the right one.

### 3.4 What a clean-room read-only XFS implementation actually requires

The minimum for mounting an IRIX 6.5 XFS volume read-only and reading files is smaller than "implement XFS" suggests. A staged plan:

#### Stage 0 — Device and label (≈0.5 person-month)

- Parse the SGI volume header at LBA 0: magic `0x0BE5A941`, 16 partition entries at offset 312, each `{u32 sectors; u32 start; u32 type}`; type 10 = XFS, 7 = EFS, 11 = XFS log, 8/12/13 = logical volume managers; type 6 = whole volume. Verify the label checksum. Source: <https://keramics.github.io/sgilabel.html>, IRIX `vh(7M)` <https://help.graphica.com.au/irix-6.5.30/man/7M/vh>.
- Translate the partition start (in 512-byte sectors) to a byte offset; the filesystem's block 0 is at that offset.
- Reject XLV/XVM, EFS, swap, and unknown types with a clear error. EFS is a separate, simpler reader; it is out of scope for this phase but see §3.6.

#### Stage 1 — Superblock and root inode (≈0.5 person-month)

- Read the 512-byte superblock at offset 0; validate `XFSB`, version 4, `sb_inprogress == 0`, `sb_blocksize` power of two, `sb_sectsize`, `sb_inodesize`, `sb_inopblock`, `sb_agblklog`, `sb_agcount`, `sb_logstart`, `sb_rootino`.
- Reject unknown feature bits in `sb_versionnum`/`sb_features2`, except the ones you implement. The SGI rule is explicit: "Any disk checking tools or drivers that do not recognize any set bits must not operate upon the filesystem" (book, p. 9). For read-only, you can be more permissive for features that cannot affect reads (e.g., quotas), but be explicit.
- Compute the root inode's byte offset via the formula in §3.3.3; read the inode; validate `di_magic`, `di_version` 1 or 2, `di_format`, `di_forkoff`, `di_nextents`, `di_size`.

#### Stage 2 — Directories and path resolution (≈1.5–2 person-months)

- Shortform parser: `count`/`i8count`, `parent`, variable-width inode numbers, bounds checks.
- Block directory parser: `XD2B`, data entries, leaf entries, best-free table.
- Leaf directory parser: `XD2D` data blocks, `XD2F1`/`XD2FF` leaf, hash entries, `forw`/`back`, best-free.
- Node directory parser: `0xfebe` nodes, `hash`/`before` entries, descend to leaves.
- Path resolution: start at `sb_rootino`, split the path, look up each component, follow symlinks with a depth limit, handle `.`/`..` (including the shortform parent field), reject NUL and over-long names.
- `readdir` cookies: use the on-disk `offset` / `address` as the cookie so that a restartable readdir does not duplicate or skip entries.

#### Stage 3 — Files, extents and the bmap B+tree (≈1.5 person-months)

- Inline extent array; sort/validate; map logical to physical.
- Bmbt root and long-format blocks; validate magic, level, numrecs, sibling pointers, key order; cap tree depth to a small constant derived from the geometry (XFS stores the maximum in `m_bm_maxlevels`; do not accept an arbitrary depth); detect cycles.
- Holes and unwritten extents; partial first/last blocks.
- `stat`, `read`, `readlink`, `access`, `mmap` page supply.
- Sparse files: a hole reads as zeros; `di_size` can exceed allocated blocks.

#### Stage 4 — Attributes, ACLs, quotas (≈1–2 person-months)

- Shortform and leaf attributes; remote values; namespace handling.
- IRIX ACLs: `SGI_ACL_FILE`/`SGI_ACL_DEFAULT` binary layout, mapping to the compat container's `acl` model. This is a compatibility feature, not a POSIX formality.
- Quotas: read-only display; enforcement is a write-path concern.

#### Stage 5 — Log replay (≈3–5 person-months)

- Find head and tail; parse record headers; handle torn last record; validate CRC for v5 only.
- Replay buffer items, inode items, inode-create, dquot, EFI/EFD; for v4, no rmap/refcount/bmap intents.
- IRIX format: accept `h_fmt == 0` (IRIX) and `XLOG_FMT_IRIX_BE` (3); treat the payload as big-endian; do not require `h_fs_uuid`.
- Intent recovery: for v4, the unlinked list and EFI; for v5, the intent items.
- This is the part that no existing pure-Rust crate implements for v4. `lamxfs` explicitly refuses it; `rust-fs-xfs` claims it but is unproven.

#### Stage 6 — Write (≈18–30 person-months)

- Allocation: free-space B+trees, AGFL, inode chunks, sparse inode handling (v5), realtime (optional).
- Directory insertion/removal in all four forms; hash index maintenance; shortform↔block↔leaf↔node transitions.
- Extent allocation, bmbt insertion/removal/rebalance, unwritten extent conversion, truncate, delayed allocation (not required for correctness).
- Transactions and log formatting: reserve, commit, checkpoint, unmount; log force; tail updates.
- Unlinked list, orphan recovery, quotas, ACLs, project IDs, growfs, repair.
- CRC (v5) and self-describing headers, if v5 write is ever needed.
- This is a multi-person-year project if done to `xfs_repair` standards. Do not schedule it before the read path is trusted.

### 3.5 The dirty-log problem, and why it matters more on IRIX

A crash leaves committed metadata changes in the log. Linux's recovery engine can replay a Linux log; it refuses an IRIX log because `h_fmt` is zero. The practical consequences:

- A **cleanly unmounted** IRIX 6.5 XFS volume is readable by Linux (with v4 support) and by a reader that implements the format. The log is empty; no replay needed.
- A **crashed** IRIX volume is not recoverable by Linux. The user must boot IRIX, mount it (which replays the log), and unmount cleanly; or run IRIX `xfs_repair`. Using an old xfsprogs (pre-4.10) is not a reliable workaround: the log's big-endian payload and the `h_fmt`/native-format check are independent obstacles, and old tools are increasingly hard to run. The reliable recovery path is IRIX itself.
- IRIX 7, being big-endian, is the natural long-term recovery tool. It can accept `h_fmt == 0` / 3, parse the big-endian payload, and replay. This is a genuine differentiator, not a nice-to-have: without it, every crashed legacy volume needs a working IRIX installation.

**Recommendation:** for the first release, mount clean v4 volumes read-only; detect a dirty log and return `EUCLEAN` with a message that names the volume and says "mount and unmount under IRIX, or run xfs_repair". Offer an explicit `ro,norecovery` mount for forensics, with a loud warning. Implement replay in phase 3, after the read path and the on-disk tests are solid.

### 3.6 IRIX-specific traps

**1. v1 directories.** As discussed, pre-6.5.14 volumes may use v1 directories; Linux removed support in 2015 and xfsprogs in 2017. If the target population includes these, IRIX 7 must implement a fifth directory form. The v1 format is documented in older SGI books and in the pre-removal Linux code; the on-disk layout is simpler than v2 (no hash leaf, linear directory blocks). Budget 2–4 person-months including tests. Alternatively, require a one-time `xfsdump`/`xfsrestore` conversion under IRIX.

**2. v1 inodes.** The v2 inode is the baseline Linux supported. v1 inodes have `di_onlink` (16-bit link count) and no project ID; the kernel converts them to v2 in memory. A reader can accept v1 and treat `di_onlink` as `di_nlink`. The 2024 xfstests case `xfs/1890` exercises a v1 inode with both `di_onlink` and `di_nlink` set to 1; do not reject that combination. Source: <https://www.spinics.net/lists/linux-xfs/msg88768.html>.

**3. 256-byte inodes and the 19-extent limit.** IRIX's default inode is 256 bytes; a file with more than ~19 extents switches to a bmbt. Do not assume large inline extent arrays. A 512-byte inode gives ~37 extents; 2 KiB gives more. Read `di_forkoff` rather than hard-coding.

**4. Sector size.** IRIX disks are usually 512-byte sectors, but XFS supports 4 KiB sectors and the AG headers move accordingly. The superblock is one sector; `sb_sectsize` tells you where AGF/AGI/AGFL start. Do not hard-code 512.

**5. Directory block size.** `sb_dirblklog` can make directory blocks larger than filesystem blocks (up to 64 KiB). A directory "block" is not a filesystem block. Read `sb_dirblklog`.

**6. ftype.** v4 can have `ftype` in directory entries if `sb_features2 & XFS_SB_VERSION2_FTYPE`; IRIX volumes may not. Do not assume the byte is there. In v5, `ftype` is always present.

**7. ACLs.** IRIX ACLs live in `SGI_ACL_FILE`/`SGI_ACL_DEFAULT`, not Linux's `system.posix_acl_*`. If IRIX 7 ignores them, files that rely on ACLs will be mis-permissioned. This is a security-relevant compatibility gap.

**8. Timestamps.** v4 timestamps are signed 32-bit seconds plus 32-bit nanoseconds; the 2038 problem is real. IRIX 6.5 has no bigtime. Do not overflow; clamp or reject dates beyond 2038 on write. Read them as `i64` and present a 64-bit `timespec`.

**9. Realtime subvolume and external log.** IRIX can put file data on a realtime device and the log on a separate partition (`sb_logstart == 0`). The Indy usually has one disk; still, detect and either support or refuse explicitly. Do not silently read the wrong device.

**10. XLV/XVM.** IRIX logical volumes can span disks; the partition type is 8/12/13. Indy configurations are usually single-disk, but a volume could be on XLV. Refuse with a clear error rather than treating the XLV partition as XFS.

**11. EFS.** IRIX 6.5 can still mount EFS, and a 6.5 disk may have EFS partitions (type 7), especially `/stand` or old volumes. The brief says legacy IRIX 6.5 disks are XFS; verify that assumption against the actual media. If EFS is in scope, it is a separate, much simpler reader (512-byte blocks, extent-based inodes, inode bitmap, no journal); budget 2–4 person-months. Do not conflate EFS with XFS.

**12. The SGI label is not a partition table you can ignore.** IRIX device names (`dks0d1s0`) encode controller/drive/slice; the slice offset is in the volume header, not in an MBR/GPT. A reader that assumes the filesystem starts at LBA 0 will fail on every real IRIX disk.

**13. v4 has no metadata checksums.** A bit flip can produce a plausible inode, extent or directory entry. Magic, range and cross-field checks catch some corruption, not all. A read-only v4 implementation should fail closed on inconsistency, never "repair" silently, and should recommend a backup before any write support is enabled. This is a fundamental limitation of the format, not of the implementation.

### 3.7 Assessment of the existing crates as a starting point

If the project decides to reuse rather than write from scratch, the ranking is:

1. **`lamxfs`** — best fit for a clean, small, `no_std` reader with v4/v5 and honest refusal semantics. It has 10 commits and 75 files; it is readable in a day. Its gaps are exactly IRIX's gaps: v4 log replay, IRIX ACLs, v1 directories. Apache-2.0 is compatible. Use it as a **reference implementation and test oracle**, and consider forking it for the v4 read path if its structure matches IRIX 7's block layer.
2. **`xfs-core`** — best v4/v5 metadata coverage and tests, including v4 AG headers and directories. It is `std`-oriented and has no log. Apache-2.0. Use it to cross-check structure parsing and to generate test expectations.
3. **`xfuse`** — useful as a FUSE harness and for v5 directory code, but its `dir3` focus makes it a poor IRIX v4 starting point.
4. **`rust-fs-xfs`** — watch, fuzz, compare; do not depend. If it survives six months of independent testing, it could become the write path. Its MIT licence is compatible.
5. **`mkfs-xfs` / `korp-mkfs-xfs`** — use to generate v5 test images on the host; not relevant to IRIX v4.

The clean-room process still applies. A permissive licence does not prove provenance. The safest path is: use the SGI format book and `xfs_db` output as the specification; use the permissive crates as **behavioural oracles** (run them on the same images and compare results), not as copy-paste sources; keep the dirty-reader/clean-writer split from ADR-0014.

---

## 4. Licensing

### 4.1 The rules that matter

IRIX 7 is GPL-3.0. The relevant compatibility rules are not symmetric:

- **GPL-3.0 can incorporate MIT, BSD-2-Clause, BSD-3-Clause, ISC, Apache-2.0 and MPL-2.0 code.** The combined work is conveyed under GPL-3.0; the original notices must be kept. Apache-2.0 is compatible with GPLv3 but not with GPLv2; MPL-2.0 is compatible with GPLv3 through its §3.3 "Larger Work" clause, with file-level obligations.
- **GPL-2.0-only cannot be incorporated into GPL-3.0.** The FSF's GPL FAQ is explicit: GPLv2-only code "must remain under GPLv2 only", and while your project depends on it "you won't be able to upgrade the license of your own code to GPLv3-or-later, and the work as a whole ... can only be conveyed under the terms of GPLv2." A GPL-2.0-**or-later** work is compatible, because the recipient may choose GPLv3.
- **CDDL-1.0 is incompatible with the GPL.** The FSF licence list says: "This is a free software license. It has a weak per-file copyleft (like version 1 of the Mozilla Public License) which makes it incompatible with the GNU GPL. This means a module covered by the GPL and a module covered by the CDDL cannot legally be linked together." That is the OpenZFS problem.
- **LGPL-2.1 is compatible with GPL-3.0** (you may link or convert), but it is not relevant to the XFS core; only xfsprogs' `libhandle` is LGPL-2.1.
- **Public domain / CC0** code is compatible, but provenance and warranty disclaimers need care.

Sources: FSF GPL FAQ, "How are the various GNU licenses compatible with each other?" <https://www.gnu.org/licenses/gpl-faq.html>; FSF licence list, Apache-2.0, MPL-2.0 and CDDL entries <https://www.gnu.org/licenses/license-list.html>; GNU GPLv3 text <https://www.gnu.org/licenses/gpl-3.0.html>.

### 4.2 Compatibility table for the projects in this report

| Project / component | Licence (authoritative source) | GPL-3.0 compatible? | What may be reused |
|---|---|---|---|
| Linux kernel XFS (`fs/xfs`) | **GPL-2.0-only**; every file starts `// SPDX-License-Identifier: GPL-2.0`; Linux `COPYING` says GPL-2.0 only | **No** | Facts and behaviour only; no code, no copied struct definitions/comments |
| xfsprogs `libxfs` | **GPL-2.0-only** (`libxfs/xfs_format.h` SPDX GPL-2.0) | **No** | Facts only |
| xfsprogs `libhandle` | **LGPL-2.1** | Yes | Could link, but not needed |
| SGI XFS on-disk-format book | © 2006 Silicon Graphics; published document | Format facts are functional; the prose and diagrams are copyrighted | Use the facts; do not copy prose, diagrams or code |
| IRIX 6.5 source (leaked or otherwise) | Proprietary | **No** | Nothing; clean-room ADR-0009/0014 |
| RedoxFS | **MIT** | Yes | Code, with notice |
| Theseus | **MIT** | Yes | Code, with notice |
| Asterinas | **MPL-2.0** | Yes (MPL §3.3) | File-level; keep MPL notices and make MPL files available |
| rcore-fs | **MIT** | Yes | Code, with notice |
| `fatfs` | **MIT** | Yes | Code, with notice |
| `littlefs2` wrapper | **Apache-2.0 OR MIT** | Yes | Code, with notice |
| littlefs C | **BSD-3-Clause** | Yes | Code, with notice |
| `embedded-sdmmc` | **MIT OR Apache-2.0** | Yes | Code, with notice |
| `ext4-view` | **MIT OR Apache-2.0** | Yes | Code, with notice |
| `ext4_rs` | **MIT** | Yes | Code, with notice; nightly-only |
| `ntfs` | **MIT OR Apache-2.0** | Yes | Code, with notice |
| `exfat-fs` / `exfat` | **MIT** | Yes | Code, with notice |
| `backhand` | **MIT OR Apache-2.0** | Yes | Code, with notice |
| `erofs-rs` | **MIT OR Apache-2.0** | Yes | Code, with notice |
| `p9` | **BSD-3-Clause** | Yes | Code, with notice |
| `fuser` | **MIT** | Yes | Code, with notice |
| `fuse-backend-rs` | **Apache-2.0 AND BSD-3-Clause** | Yes | Code, with both notices |
| `nfsserve`, `nfs3_server` | **BSD-3-Clause** | Yes | Code, with notice |
| **`btrfsutils` workspace default** | **GPL-2.0-only** in the workspace `Cargo.toml` | **No** | Nothing from crates that inherit the default |
| **`btrfs-disk` / `btrfs-fuse`** | **MIT OR Apache-2.0** (explicit per-crate override) | Yes | Code, with notice |
| **`btrfs-cli` / `btrfs-mkfs`** | **GPL-2.0-only** (explicit or inherited) | **No** | Nothing |
| OpenZFS | **CDDL-1.0** | **No** | Nothing |
| `zfs-forensic-core` | **Apache-2.0** | Yes | Code, with notice; independent clean-room reader |
| `xfs-core` | **Apache-2.0** | Yes | Code, with notice; provenance audit |
| `xfuse` / `xfs-fuse` | **BSD-2-Clause** | Yes | Code, with notice |
| `lamxfs` | **Apache-2.0** | Yes | Code, with notice; provenance audit |
| `rust-fs-xfs` | **MIT** | Yes | Code, with notice; prototype maturity |
| `mkfs-xfs` | **Apache-2.0** | Yes | Code, with notice |
| `korp-mkfs-xfs` | **MIT OR Apache-2.0** | Yes | Code, with notice |

The authoritative sources for the XFS entries are: Linux `COPYING` (<https://github.com/torvalds/linux/blob/master/COPYING>), `fs/xfs/xfs_super.c` and `fs/xfs/libxfs/xfs_format.h` SPDX headers, xfsprogs `libxfs/xfs_format.h` and `libhandle/handle.c` (<https://git.kernel.org/pub/scm/fs/xfs/xfsprogs-dev.git/plain/libxfs/xfs_format.h>), and OpenZFS `LICENSE` (<https://github.com/openzfs/zfs/blob/master/LICENSE>). The btrfs trap is a good lesson in per-crate manifests: the workspace default is `license = "GPL-2.0-only"` (<https://github.com/rustutils/btrfsutils/blob/main/Cargo.toml>), `disk/Cargo.toml` overrides to `MIT OR Apache-2.0`, `fuse/Cargo.toml` and `fs/Cargo.toml` also override, and `cli/Cargo.toml` is GPL-2.0-only. Do not infer a crate's licence from the repository's top-level manifest.

### 4.3 The XFS position in detail

**Linux XFS is GPL-2.0-only.** It is not "GPL-2.0-or-later". The SPDX identifier is `GPL-2.0` (the deprecated spelling of `GPL-2.0-only`), and `COPYING` says "GNU General Public License version 2 only". There is no practical way to relicense it: it has hundreds of contributors and corporate copyright holders, and SGI's original code is only one part. Even if the project were willing to ship GPL-2.0-only, IRIX 7 is GPL-3.0; the combination is not permitted.

**xfsprogs is mostly GPL-2.0-only, with an LGPL-2.1 library.** `libxfs` (the on-disk format code) is GPL-2.0-only; `libhandle` is LGPL-2.1. The command-line tools are GPL-2.0. Using `xfs_db`, `xfs_repair`, `xfs_logprint` or `xfsdump` as **separate processes** to create test images and compare behaviour is fine; linking their code into IRIX 7 is not.

**SGI's XFS documentation is published, but the IRIX source is not.** The on-disk format book was published by SGI in 2006 and is the right specification. The IRIX kernel source was never released under a free licence; the leaked source is proprietary and is excluded by ADR-0009/0014. The clean-room process must therefore be: dirty readers read the published format and the IRIX binaries/behaviour; clean writers implement from a behavioural specification; the public repo contains only the clean implementation.

**On-disk format facts are not code.** Structure layouts, field offsets and magic numbers are functional facts; they are not protected by copyright in the way expression is (the idea/expression distinction, and in many jurisdictions merger). But a copied C struct with its comments, field names and formatting is expression. Do not copy `xfs_format.h` into Rust; write independent Rust types from the specification, with your own names and comments, and verify against `xfs_db`.

**Patents.** XFS was open-sourced by SGI in 2000; no current patent claims are known against the format. This is not legal advice; a formal freedom-to-operate review is outside the scope of this report.

### 4.4 Provenance is a separate gate from licence

A permissive licence does not cure copied GPL code. Several of the 2026 XFS crates are very new and may have been generated with reference to Linux XFS. Before reusing any of them:

1. **Ask for the provenance statement.** Does the author claim clean-room? What sources did they read? `lamxfs` says it was written from the public SGI/Red Hat format document and the Linux on-disk headers, treating format facts as uncopyrightable; that is a plausible clean-room position, but it needs review.
2. **Diff against Linux XFS.** Look for identical comments, identifier names, unusual constants, and struct layouts that match Linux's code rather than the published format. Identical comments are a red flag.
3. **Run the similarity screen.** ADR-0015's structural split and similarity screen should be applied to any imported crate, not only to SGI-derived code.
4. **Prefer behavioural reuse.** Running `xfs-core`/`lamxfs` on the same images and comparing results is safe; copying their parser internals is not, unless provenance is clear.

### 4.5 The licence gate for IRIX 7

Proposed rule, to be recorded in ADR-0012's annex:

- **Allowed:** MIT, BSD-2-Clause, BSD-3-Clause, ISC, Apache-2.0, MPL-2.0, LGPL-2.1/3.0 (with relinking), public domain/CC0.
- **Allowed with care:** MPL-2.0 (file-level copyleft; keep notices; make MPL files available), LGPL (dynamic link or relink), Apache-2.0 (patent termination clause; keep NOTICE).
- **Not allowed:** GPL-2.0-only, CDDL-1.0, proprietary, "source-available" licences with field-of-use restrictions, and any crate whose manifest and repository disagree until the discrepancy is resolved.
- **Always required:** a `LICENSE`/`NOTICE` audit, a provenance statement for filesystem code, and a record in `docs/map.md`'s crate dependency table.

---

## 5. Architecture options

### 5.1 The forces

The storage design has to satisfy six forces at once:

1. **Compatibility.** IRIX 6.5 disks are XFS v4; the system must read them, and eventually write them. Legacy binaries must be able to `open`, `read`, `mmap` and `exec` from those volumes without knowing they are on a foreign filesystem.
2. **Isolation.** Drivers, including SCSI and block, run in userspace. The kernel should broker MMIO, DMA and interrupts, not contain a filesystem.
3. **The Indy's hardware.** One 200 MHz R4400, 32–128 MB RAM, a 10 MB/s narrow SCSI-2 bus, no IOMMU, 4 KiB pages, a small TLB. Every copy and every context switch is visible.
4. **Licence and provenance.** No GPL-2.0-only, no CDDL, no SGI source.
5. **Crash consistency.** IRIX disks contain the only copy of user data. A read-only mount must not write; a writable mount must journal and must be recoverable.
6. **Schedule.** The project needs a bootable system with a usable root before XFS write is done, and it needs to be able to say "your IRIX files are safe" early.

### 5.2 Option A — userspace FS server, clean-room XFS read-only first

**Shape.** A `fs_server` cell per mounted volume. It owns the filesystem's in-memory state (inode cache, block cache, journal state) and exposes a capability-based file API to the VFS. It talks to a `block_device` capability, which is backed by the SCSI driver cell. The kernel provides:
- MMIO mapping for the SCSI controller (HPC3 + WD33C93B) to the driver;
- a DMA buffer allocator/pinner and descriptor-ring broker;
- interrupt delivery and completion ports;
- capability creation/revocation and IPC.

The XFS server starts read-only. It parses the SGI label, superblock, AG headers, inodes, extents, bmbt and directories. It refuses a dirty log (or replays it in a later phase). A separate native FS server handles new disks. The VFS picks the server by filesystem type.

**Pros.**
- Matches the "all drivers in userspace" decision and the Theseus cell model: a filesystem is a cell with a narrow public interface, restartable after a fault.
- The FS server can be tested on Linux via FUSE (`fuser`) against the same on-disk code, before the Indy boots.
- A bug in XFS parsing cannot corrupt the kernel; it can only return an error or crash the server.
- Read-only by construction for phase 1: no write path, no risk to legacy disks.
- XFS write can be added behind the same interface without changing the VFS or the compat container.
- The native FS and XFS share the block cache, page cache and IPC machinery.

**Cons.**
- IPC and capability checks add latency to metadata-heavy workloads; on a 200 MHz CPU, a per-block round trip would be fatal. The design must batch, use shared memory and keep the page cache in the server or in a shared mapping.
- The FS server needs a page-cache/mmap story: legacy binaries `mmap` files and execute from them; the server must supply pages, handle `mmap` faults and `msync`.
- Two servers (XFS and native) mean two code paths, two sets of tests, two recovery stories.
- The DMA broker is real work and has no IOMMU safety net.

**Effort.** Block device + SCSI + DMA broker: 2–4 person-months. FS server framework + VFS/page cache: 2–3. XFS v4 read-only (clean log): 4–6. Dirty-log replay: 3–5. Native FS: 4–6 (or RedoxFS port 2–4). Total to a bootable native + XFS-read-only system: **12–19 person-months**, with a usable read-only image earlier.

**Risk.** Medium. The main risks are DMA correctness, page-cache/mmap integration, and the long tail of XFS directory/attribute cases.

### 5.3 Option B — XFS in the kernel

**Shape.** Link a clean-room XFS implementation (or a port) into the monolithic kernel, with the block layer in-kernel. The VFS calls it directly.

**Pros.**
- No IPC; the fastest path on a slow CPU.
- Simpler page cache and `mmap` integration: the kernel already owns the page cache.
- One address space, one lock model, no capability plumbing for every read.

**Cons.**
- Contradicts the decision that all drivers, including block/SCSI, run in userspace. If the block layer is in the kernel but the SCSI driver is not, the FS still crosses a boundary; if both are in the kernel, the driver decision is reversed.
- Puts a large, untrusted parser (XFS metadata from arbitrary legacy disks) in the TCB. That is exactly the kind of code that should be isolated.
- A kernel XFS bug is a panic, not a restartable server.
- The kernel grows by tens of thousands of lines, against the small-TCB goal.
- It does not avoid the hard work: the parser and writer are the same size.

**Effort.** XFS v4 read-only: 4–6 person-months; dirty log: 3–5; write: 18–30. Plus kernel integration and page-cache work (1–2). Total **8–13 person-months** for read-only, but with a much worse TCB and a direct conflict with the driver decision.

**Verdict.** Rejected. It optimises a cost (IPC) that the design can largely avoid with shared memory, and it pays with the project's main safety property.

### 5.4 Option C — native FS plus an XFS compatibility server

**Shape.** New disks use a native filesystem designed for the Indy. IRIX 6.5 volumes are mounted by a separate XFS server, read-only at first. The VFS presents both through the same file API. The native FS is the root; XFS volumes are mounted at `/legacy` or wherever the user chooses.

**Pros.**
- The system becomes usable before XFS write exists: the root is native, the legacy data is read-only.
- The native FS can be designed for the hardware: 4 KiB blocks, big-endian, small metadata, journal, no reflink/rmap/quota baggage.
- XFS write is optional, not on the boot path. Users who need to modify legacy data can copy to native or boot IRIX.
- The XFS server can be as paranoid as it needs to be: read-only, no allocation, no journal writes.
- Clear failure domain: a native FS bug does not endanger legacy disks; an XFS parser bug does not corrupt the root.

**Cons.**
- Two filesystems to implement, test, document and maintain.
- The native FS is not readable by IRIX 6.5; data exchange requires XFS or a network protocol.
- The native FS needs its own `mkfs`, `fsck`, image builder and host tools.
- Users may expect to write to their old XFS volumes; "copy to native" is a workflow change.

**Effort.** Native FS (I7FS): 4–6 person-months including fsck. XFS read-only: 4–6. Shared framework: 2–3. Block/SCSI: 2–4. Total **12–19 person-months** to a usable system, with a bootable native root earlier than Option A's XFS path. XFS write adds 18–30 later.

**Verdict.** Recommended, with I7FS as the native format. This is the only option that gives a usable system and a credible compatibility story without betting the schedule on XFS write.

### 5.5 Option D — port an existing filesystem

**Shape.** Take RedoxFS, ext4_rs, rcore-fs-ext2, or Asterinas' ext2 and adapt it to IRIX 7's block layer and VFS. Use it as the native FS; add an XFS read-only server separately.

**Pros.**
- Much less design work; a working on-disk format and tested code.
- RedoxFS brings CoW, checksums, encryption, compression, `mkfs`/`resize`/`clone` and a FUSE driver.
- ext4_rs / Asterinas ext2 bring a familiar format and Linux host tooling.
- A port can be validated against the original implementation on the host.

**Cons.**
- **Endianness.** RedoxFS, ext4 and ext2 are little-endian on disk. The Indy will byte-swap every metadata field. That is survivable (disk I/O dominates), but it is a constant tax and a source of bugs.
- **Kernel coupling.** RedoxFS depends on `redox_syscall` errors and a `Disk` trait; Asterinas ext2 depends on OSTD's page cache, block layer and error model; rcore-fs depends on its own VFS traits.
- **Format mismatch.** RedoxFS is CoW with 128 KiB records and a 256-block header ring; on a 32 MB machine, that is a lot of metadata per update. ext4 is a feature-rich format with a journal that the Rust ports do not fully implement.
- **No IRIX identity.** A ported ext4 root is a Linux filesystem on an SGI workstation; it does not carry the XFS/IRIX story.
- **Maintenance.** You inherit upstream's format evolution and bugs, but not its test suite.

**Effort.** RedoxFS bring-up port: 2–4 person-months; add 2–3 for fsck, crash testing and endian fixes, so 4–7 for a product (excluding XFS). ext4_rs port: 3–5 (nightly, no journal). rcore-fs-ext2: 2–3 (simple, but limited). Asterinas ext2 extraction: 2–4. Add XFS read-only 4–6 and framework 2–3; total **8–15 person-months** for bring-up, 12–19 for a hardened system.

**Verdict.** RedoxFS is the best fallback if I7FS slips. It is not the primary recommendation because the endianness and coupling costs erode the schedule advantage, and the format is not designed for the Indy's memory and disk geometry.

### 5.6 Option E — adopt an existing pure-Rust XFS crate

**Shape.** Use `xfs-core`, `lamxfs`, `xfuse` or `rust-fs-xfs` as the XFS server's core, with a thin IRIX adaptation layer.

**Pros.**
- Fastest path to a read-only XFS mount: weeks, not months.
- `lamxfs` is `no_std`, small and v4-aware; `xfs-core` has broad v4/v5 metadata coverage; `xfuse` has a working FUSE front end.
- Permissive licences (Apache-2.0/BSD-2/MIT) are compatible with GPL-3.0.

**Cons.**
- **IRIX gaps.** No IRIX ACLs, no v1 directories, no IRIX log replay (lamxfs refuses v4 logs; xfs-core has no log at all).
- **Maturity.** `lamxfs` has 10 commits; `xfs-core` was created in 2026-07; `rust-fs-xfs` has a two-week commit history. None has a production track record.
- **Provenance.** A permissive licence does not prove clean-room; generated code may have copied Linux structures/comments.
- **Fit.** `xfs-core` is `std`-oriented; `lamxfs` is UEFI-oriented and may not expose the page-cache/mmap hooks IRIX 7 needs; `xfuse` is v5-dir3-centric.
- **Write.** Only `rust-fs-xfs` claims v5 write; no v4 write. You would still build the write path.

**Effort.** Integrate a reader: 1–3 person-months; adapt to IRIX v4 and fill gaps: 2–4; total 3–7 for read-only, but with high uncertainty. XFS write remains 18–30.

**Verdict.** Use as a **reference/oracle and possibly a fork for the v4 read path**, not as the architecture. The project should not depend on a crate with a two-week commit history for irreplaceable disks.

### 5.7 Comparison

| Criterion | A: userspace XFS RO first | B: in-kernel XFS | C: native + XFS compat | D: port existing FS | E: adopt XFS crate |
|---|---|---|---|---|---|
| IRIX 6.5 compatibility | Excellent (read-only first) | Excellent | Excellent via compat server | None for IRIX; native only | Partial (v4 gaps) |
| Fits all-drivers-userspace | Yes | **No** | Yes | Yes | Yes |
| TCB size | Small | Large | Small | Medium | Small |
| Effort to first usable root (cumulative, storage stack) | 10–16 PM (XFS RO root) | 8–13 PM (XFS RO root) | 8–13 PM (native root); 12–19 PM with XFS RO | 8–13 PM (native root); 12–19 PM with XFS RO | 4–8 PM (XFS RO; risky) |
| Effort to XFS read-only (cumulative) | 10–16 PM | 8–13 PM | 12–19 PM | 12–19 PM | 4–8 PM |
| Effort to XFS write (additional) | 18–30 PM | 18–30 PM | 18–30 PM (optional) | 18–30 PM (optional) | 18–30 PM |
| Crash consistency | XFS read-only; native journal only if a native FS is added | XFS journal (if implemented correctly) | Native journal + XFS read-only | Depends on port | Read-only; write unproven |
| Maintenance burden | Medium (two FS) | High (kernel) | Medium (two FS) | Low-medium (upstream) | Low if upstream active |
| Licence | GPL-3.0 clean | GPL-2.0 trap if ported | GPL-3.0 clean | Permissive | Permissive |
| Overall risk | Medium | High (TCB + licence) | **Medium-low** | Medium | High (maturity) |

*PM = person-months, assuming one experienced systems engineer with filesystem knowledge, a working Rust toolchain and a test harness; ±50%. Ranges are cumulative for the storage stack (SCSI/block layer, FS framework, tests and the named filesystem) and overlap because phases share work; they are planning estimates, not commitments.*

---

## 6. Recommended storage architecture

### 6.1 The shape in one diagram

```
  legacy IRIX binary                  native Rust program
          │                                   │
          ▼                                   ▼
  compat container (o32/n32)           native capability API
          │                                   │
          └──────────────┬────────────────────┘
                         ▼
                 VFS / mount namespace
                         │  file capabilities, page grants
          ┌──────────────┼───────────────────┐
          ▼              ▼                   ▼
   xfs_server      i7fs_server         ramfs / procfs / ...
   (read-only)     (read-write)         (in-memory)
          │              │
          └──────┬───────┘
                 ▼
        block_device capability
                 │  submit_io / completion port
                 ▼
          scsi_driver cell
                 │  WD33C93B registers (MMIO)
                 ▼
     kernel DMA broker / HPC3 registers / IRQ
                 │
                 ▼
            SCSI disk
```

The VFS does not know about XFS or I7FS. It knows about **file objects**, **page objects** and **mounts**. Each filesystem server registers a mount with a set of operations: `lookup`, `getattr`, `read`, `write`, `readdir`, `readlink`, `create`, `unlink`, `rename`, `fsync`, `get_page`, `put_page`. The operations are capability invocations; the arguments are handles, not paths, after lookup.

### 6.2 Objects and capabilities

The native object model should expose:

| Object | Rights | Notes |
|---|---|---|
| `BlockDevice` | `READ`, `WRITE`, `FLUSH`, `GET_GEOMETRY`, `SUBMIT_IO` | Represents a partition, not the whole disk; revocation on hot-unplug |
| `DmaBuffer` | `READ`, `WRITE`, `MAP` | Pinned, physically contiguous (or scatter-gather list), owned by the kernel; mapped into the driver/server as needed |
| `File` | `READ`, `WRITE`, `SEEK`, `MMAP`, `FSYNC`, `GETATTR`, `SETATTR` | Returned by `open`/`create`; revocable |
| `Dir` | `READ`, `LOOKUP`, `CREATE`, `REMOVE`, `RENAME` | Returned by `open` on a directory |
| `Mount` | `MOUNT`, `UNMOUNT`, `STATFS`, `SYNC` | Held by the VFS, not by user programs |
| `CompletionPort` | `WAIT`, `CANCEL` | For asynchronous block I/O and FS server completions |

A file handle is `(object id, rights, generation)`. The generation is bumped on unlink/reuse, so a stale handle cannot access a new file that reused the inode. Revocation is explicit: closing the last handle, unmounting, or restarting the server invalidates all handles and wakes waiters with `EIO`/`ENODEV`.

The block device capability is the **only** way to reach the disk. User programs never get it; the FS server gets it at mount time, with rights chosen by the mount policy (read-only XFS gets `READ`/`GET_GEOMETRY` only). The SCSI driver gets a `BlockDevice`-backed request queue, not raw disk access.

### 6.3 The SCSI driver cell

The Indy's storage path is:

- **WD33C93B** SCSI controller, narrow SCSI-2, up to 10 MB/s, on the HPC3 peripheral controller;
- **HPC3** provides the SCSI DMA channel and the interrupt;
- the system disk is typically SCSI ID 1, the controller ID 0; the CD-ROM is ID 6.

Sources: Linux/MIPS IP22 page <https://linux-mips.org/wiki/IP22>; HPC3 datasheet <https://erikarn.github.io/sgi/indy/datasheets/sgi_indy_hpc3.pdf>; Indy technical notes <https://tech-pubs.net/reputable-archive/indytech.html>.

The driver cell should own:

- the WD33C93B register window (SASR/SCMD and the extended registers);
- the SCSI command queue, timeouts, abort/reset, tagged queueing (the Linux driver advertises `can_queue = 16`);
- the translation of block I/O requests into SCSI READ(10)/WRITE(10)/SYNCHRONIZE CACHE(10) commands;
- error handling: retry, sense data, medium errors, bus reset.

The kernel should own:

- the HPC3 SCSI DMA registers (`ndptr`, `ctrl`), or expose only safe operations (`dma_start`, `dma_stop`, `dma_flush`) rather than the raw MMIO page;
- the DMA descriptor chain and the physical-address validation;
- the interrupt handler that acknowledges the HPC3 and signals the driver;
- the DMA buffer pool and pinning.

This split is the key to making "userspace SCSI" meaningful without an IOMMU. The WD33C93B is programmed by the driver; the HPC3 DMA engine is programmed by the kernel on the driver's behalf. The driver never learns a physical address it can write into `ndptr`, and the kernel checks that every descriptor points into a buffer the driver owns.

### 6.4 DMA on the HPC3, and why it is the hard part

The HPC3 SCSI DMA engine walks a chain of descriptors in main memory. Each descriptor is three 32-bit words:

```c
struct hpc_dma_desc {
    u32 pbuf;     /* physical address of data buffer */
    u32 cntinfo;  /* length + flags; HPCDMA_EOX marks the last descriptor */
    u32 pnext;    /* physical address of the next descriptor */
};
```

Source: Linux `arch/mips/include/asm/sgi/hpc3.h`, <https://github.com/torvalds/linux/blob/master/arch/mips/include/asm/sgi/hpc3.h>. The Linux driver caps each descriptor at **8192 bytes** because "even cntinfo could be up to 16383, without magic only 8192 works correctly" (`drivers/scsi/sgiwd93.c`, <https://github.com/torvalds/linux/blob/master/drivers/scsi/sgiwd93.c>). It allocates a page of descriptors, chains them, terminates with `HPCDMA_EOX`, and calls `dma_cache_sync` on the descriptor page before starting the channel.

There is **no IOMMU**. The HPC3 uses the 32-bit physical address in `pbuf` directly. A driver that can write `ndptr` can make the device DMA to or from any physical address. Therefore:

- **If the driver owns the HPC3 DMA registers**, the driver is effectively in the TCB for memory integrity. Language safety does not help; a bug or a malicious driver can overwrite the kernel.
- **If the kernel owns the DMA registers and descriptor chain**, the driver can still program the WD33C93B, but it cannot choose arbitrary physical addresses. It submits `(buffer handle, offset, length, direction)`; the kernel validates the handle against the driver's grant, builds descriptors, flushes caches, starts the channel, and completes the request. This is the recommended design.
- **Bounce buffers** are the simple implementation: the kernel copies between the FS server's page-cache page and a pinned DMA buffer. That costs a copy per I/O (a 128 KiB copy is roughly 100–200 µs on a 200 MHz R4400, against a ~13 ms disk transfer at 10 MB/s; acceptable), and it avoids scatter-gather complexity. A later optimisation can pin the page-cache pages directly if they are physically contiguous or use a scatter-gather list.
- **Cache maintenance** is mandatory. The R4400 has a write-back data cache; before an outbound transfer (the device reads memory), flush the buffer; after an inbound transfer (the device writes memory), invalidate it. The Linux driver's `dma_cache_sync` is the model. Do not assume the HPC3 snoops the CPU cache.
- **Address limit.** The HPC3 descriptor `pbuf` is 32 bits. The Indy has at most 128 MB, so all RAM is below 4 GiB; still, the kernel should allocate DMA buffers from a dedicated pool and validate the physical range.
- **Ordering and barriers.** The descriptor chain and the WD33C93B program must be visible before `ch_active` is set; use a memory barrier and cache flush. On completion, invalidate the data buffer before the driver reads it.

This is the single most important place where the "all drivers userspace" decision meets hardware reality. The report recommends being explicit in the ADR: **the SCSI driver is userspace, but the HPC3 DMA engine is kernel-owned; the driver is not trusted with physical addresses.** If the project instead decides to map the HPC3 registers into the driver, record that the driver is in the TCB and adjust the threat model.

### 6.5 The block cache

The block cache lives in the FS server, not in the kernel and not in the SCSI driver. It caches **filesystem blocks** (4 KiB by default) and is unified with the page cache where possible.

Design:

- **Key:** `(volume_id, fsblock)` for metadata; `(volume_id, inode, logical_page)` for file data. Metadata and data can share the same `Page` type but have separate indexes.
- **Size:** 4–6 MiB on a 32 MB machine, 12–16 MiB on a 128 MB machine. Metadata is pinned preferentially; data is evictable. A 4 KiB block cache of 6 MiB holds 1,536 blocks.
- **Replacement:** CLOCK/second-chance with a dirty bit; metadata gets a higher weight. Do not use a pure LRU list with a lock per access; on a single CPU, a simple clock is enough.
- **Read-ahead:** 32–128 KiB sequential, detected per file handle; cap the read-ahead so it does not evict metadata. For `readdir`, read the directory data blocks in one batch.
- **Writeback:** dirty pages are written in fsblock order, with a generation number; `fsync` waits for the write and the journal commit; `sync` writes all dirty pages and checkpoints the journal.
- **No double caching:** if the VFS page cache holds a file page, the FS server should not also hold a copy. Either the page cache is in the server (and clients map its pages), or the server is a pass-through for file data and only caches metadata. For simplicity and zero-copy, put the page cache in the FS server and let clients map its pages via a `VmObject` capability.
- **Coherency:** one server per volume owns the cache; no other cell writes the block device. If a second mount of the same volume is attempted, refuse or share the server. XFS read-only can allow multiple readers, but the cache must be immutable.
- **Memory pressure:** the cache must be shrinkable. Theseus's `block_cache` is a cautionary tale: its documentation admits it only reads/writes single blocks, cannot drop cached blocks under memory pressure, and gives stale results if another crate writes the device (<https://www.theseus-os.com/Theseus/doc/block_cache/>). Do not copy that design.

### 6.6 Page cache, `mmap` and `exec`

Legacy IRIX binaries do not just `read`; they `mmap` executables, shared libraries and data files, and the loader may execute directly from the mapping. The FS server must supply pages, not just byte streams.

- A `File` object exposes `get_page(file, page_index) -> PageCapability`. The page is a 4 KiB page in the FS server's page cache, with a state (`clean`, `dirty`, `writeback`, `error`).
- The VFS maps the page into the client's address space (read-only for XFS, read-write for native). For a write fault, the client gets a writable mapping only if the file was opened writable; the server is notified on `msync`/`munmap`/`fsync`.
- `exec` maps the text/rodata pages read-only and the data pages copy-on-write or read-write. The compat container's ELF loader uses the same page API as a native program.
- `mmap` of a file larger than RAM works because pages are demand-fetched; the server does not read the whole file.
- `msync(MS_SYNC)` flushes dirty pages and waits for the journal; `msync(MS_ASYNC)` marks them for writeback.
- `truncate`/`unlink` must invalidate mappings or keep the inode alive until the last mapping is gone. Use a generation number and a `File` object refcount; do not reuse the inode while mappings exist.

### 6.7 IPC and zero-copy

The FS server is a cell. On IRIX 7's single-address-space design, a "call" can be a direct function call through a capability-checked dispatch table; on an MMU-isolated variant, it is a syscall-like transition. Either way, the design should assume the expensive case and batch.

- **Metadata:** `lookup` returns a `File` handle + `stat` in one call; `readdir` returns a batch of entries (e.g. 32) per call; `statfs` returns a struct.
- **Data:** `read` returns a list of page capabilities, not a byte copy. The client maps them; the server retains ownership. For small reads (< 4 KiB), inline the bytes in the reply to avoid a mapping.
- **Writes:** the client grants a page to the server, or the server copies into its page cache. For large writes, use a shared ring of pages with a credit count.
- **Asynchronous I/O:** the block device uses a completion port. The FS server can have multiple outstanding SCSI commands (queue depth 16); a single worker thread is enough on one CPU, but the I/O path should not block on a single request.
- **Timeouts and cancellation:** every IPC call has a deadline; a hung FS server is restartable. `EINTR`/`ETIMEDOUT` semantics must be defined for legacy syscalls.

### 6.8 Crash consistency

The native FS uses **metadata journalling**; XFS read-only does not write at all.

Journal protocol (I7FS):

1. Reserve log space for the transaction.
2. Write the transaction's metadata changes to the log (redo records), with a checksum and a monotonically increasing LSN.
3. Write a commit record; optionally issue `SYNCHRONIZE CACHE` to make the log durable.
4. Apply the changes to their home locations (checkpoint), in any order, but not before the commit is durable.
5. Update the superblock's checkpoint LSN after the home writes are durable; keep two superblock copies with generation numbers.

For `fsync`:
- write the file's dirty data pages to their home locations;
- issue `SYNCHRONIZE CACHE` (or use FUA writes if the disk supports them);
- commit the metadata transaction that records the new size/blocks;
- return only after the commit is durable.

For `rename`/`unlink`:
- log the directory entry change and the inode link count in one transaction;
- commit; the operation is atomic after replay;
- for `unlink`, add the inode to the AG unlinked list before committing, so a crash does not leak it; replay completes the unlink.

Torn writes:
- every metadata block has a CRC32C and a generation/LSN; a torn block is rejected and the journal is replayed;
- the journal record has a length and CRC; a torn last record is discarded (the classic "last record may be incomplete" rule);
- the superblock has two copies and a generation; mount chooses the newest valid one.

Testing:
- fault injection at every write (drop, duplicate, reorder, truncate) using a host-side `dm-flakey`/`blkdebug` or a QEMU snapshot;
- crash the IRIX 7 guest during `fsync`, `rename`, `unlink`, `mkdir`, `truncate`;
- replay and compare the directory tree/file contents with a known-good copy;
- run `fsck` after every crash; it must report clean after replay.

XFS read-only needs only the clean-log check. XFS write (later) must use XFS's own log format and ordering; do not reuse I7FS's journal for XFS.

### 6.9 Failure and restart

- **FS server crash:** the supervisor restarts it. All open file handles are invalidated; clients get `EIO` on the next operation. The mount is re-established; the native FS replays its journal; XFS read-only re-reads the superblock and log state.
- **SCSI driver crash:** outstanding I/O is failed; the block device capability is revoked; the FS server remounts read-only or goes to a degraded state. A bus reset may be needed.
- **Disk error:** the block device returns a typed error (`EIO`, `EMEDIUMTYPE`, `ENOSPC`); the FS server maps it to the file operation; it must not panic or retry forever.
- **Unmount:** flush dirty data, checkpoint the journal, revoke handles, release the block device. For XFS read-only, just drop the cache.
- **Hot unplug:** not relevant for the internal Indy disk, but the capability model should support revocation for external SCSI devices.

### 6.10 Security and trust

- **XFS metadata is untrusted input.** Every field is bounds-checked; every block number is validated against the volume; every count is capped; every tree traversal has a depth limit and cycle detection. Fuzz with `cargo-fuzz`; a malformed image must return an error, not panic or hang.
- **Path resolution** must reject `..` escapes above the mount root, NUL bytes, over-long components, and symlink loops. The compat container's `chroot`/`fchdir` semantics must be preserved.
- **No raw block device to unprivileged programs.** The block device capability is held by the FS server and the SCSI driver only.
- **DMA** is the exception to the isolation story; see §6.4. Record the trust assumption in the ADR.
- **Setuid binaries** on legacy XFS need the compat container's credential model; the FS server must return the on-disk uid/gid/mode faithfully, including ACLs, or privilege escalation follows.
- **Mount options:** `ro`, `nosuid`, `nodev`, `noexec` for legacy volumes by default where possible; the compat container may need `suid` for IRIX system binaries, so make it explicit.

### 6.11 Boot path

The PROM loads the IRIX 7 kernel. The kernel initialises the SCSI driver cell, the block device, the root FS server and the VFS, in that order. The root can be:

1. a read-only image on disk or CD (SquashFS/EROFS/I7FS-read-only), with a RAM overlay for `/tmp` and `/var`;
2. a native I7FS volume (phase 2);
3. an XFS volume mounted read-only (phase 1, for a "live CD" that reads IRIX data).

The compat container starts after the root is mounted; it uses the same VFS and page cache. There is no separate "legacy block driver" and no second path to the disk.

---

## 7. The native filesystem: I7FS

### 7.1 Why a new format at all?

The project has three plausible native-format strategies:

1. **Use XFS v4 for new disks too.** One format, maximum identity, and the XFS reader/writer can be shared. But XFS write is 18–30 person-months, v4 has no CRCs and a 2038 timestamp limit, and new disks would inherit a format that upstream Linux is removing. It makes the native volume hostage to the hardest part of the project.
2. **Port RedoxFS (or ext4).** Less design work, but a little-endian format on a big-endian machine, a foreign on-disk layout, kernel/OS coupling, and a format that was not designed for 32 MB and a 10 MB/s disk. RedoxFS is a credible fallback, not an obvious first choice.
3. **Design a small native format.** More design work up front, but the format fits the hardware, the code is auditable, and the project owns its recovery story. The effort is comparable to a careful RedoxFS port once the endianness and shim work is counted.

This report recommends (3): **I7FS v1**, a small, big-endian, journalled, extent-based filesystem. It borrows XFS's good ideas (allocation groups, inodes, extents, B+ trees, a metadata log) without copying its on-disk layout. XFS remains the compatibility format, not the native one.

The 2–4 person-month RedoxFS estimate in §5.5 is for a **bring-up port**, not a product: it excludes fsck, crash testing and the endian fixes that a little-endian format needs on a big-endian CPU. Add 2–3 person-months for those, and the RedoxFS path is 4–7 person-months, comparable to I7FS's 4–6. I7FS wins on fit and auditability, not on a large schedule margin.

### 7.2 Goals and non-goals

**Goals.**

- Boot and run from a 32 MB Indy with a 100 MB–2 GB SCSI disk.
- 4 KiB blocks, 64-bit block numbers, 64-bit file sizes, 64-bit inode numbers.
- Big-endian on disk, with an explicit byte-order field for a future little-endian port.
- Metadata CRCs, a journal, and a fast `fsck`.
- UNIX semantics: permissions, uid/gid, hard links, symlinks, sparse files, timestamps, `rename` atomicity.
- A small, auditable implementation: target 5,000–8,000 lines of safe Rust plus tests.
- Host-side tools: `mkfs`, `fsck`, image builder, FUSE driver, so images can be built and tested on Linux.

**Non-goals for v1.**

- Reflink, snapshots, compression, encryption, quotas, project IDs, realtime subvolumes, online grow/shrink, RAID, zoned storage.
- XFS on-disk compatibility.
- Multi-device volumes.
- Crash consistency for user data beyond `fsync` (metadata is journalled; data is ordered, not checksummed).

These can be added with feature flags and format versions later; v1 must be complete and stable before any of them.

### 7.3 On-disk layout (provisional)

**Geometry.**

| Parameter | Value | Rationale |
|---|---|---|
| Sector | 512 bytes | SCSI disk; the block device layer hides 4K-native disks |
| Filesystem block | 4096 bytes | Matches the MIPS page size; 4K is a good balance for 32 MB |
| Block number | 64-bit | Future-proof; 2^64 blocks is far beyond the Indy |
| Inode size | 256 bytes | Same as IRIX XFS; 16 inodes per 4K block |
| Inode number | 64-bit | `(agno << 32) | agino` or a flat 64-bit counter |
| Max file size | 2^63 - 1 bytes | 64-bit `size` |
| Max volume | 2^64 blocks in the format; practical 2–16 TiB | 64-bit block numbers; the Indy's disks are far smaller |
| Timestamps | 64-bit seconds + 32-bit nanoseconds | No 2038 problem |
| Byte order | Big-endian, `magic` + `byte_order` field | Native on MIPS; explicit for portability |

**Superblock** at block 0 and a backup at the last block:

```
magic[8]          "I7FS\0\0\0\0"
version           u32 (1)
byte_order        u32 (0x01020304; detects endianness)
block_size        u32 (4096)
total_blocks      u64
free_blocks       u64
root_ino          u64
journal_start     u64
journal_blocks    u32
inode_size        u32 (256)
inode_blocks      u64
ag_count          u32
features          u64
uuid[16]
label[32]
generation        u64
checkpoint_lsn    u64
crc32c            u32
```

The superblock is checksummed; the backup is written after the primary at each checkpoint. Mount chooses the newest valid copy. `features` is a bitmask; unknown mandatory bits are refused.

**Allocation groups.** For volumes up to ~8 GiB, one AG is enough. For larger volumes, 4 AGs (or one per 8 GiB) reduce free-space tree depth and improve locality. Each AG has:

- an AG header (magic, AG number, length, free block count, free inode count, roots of the free-space and inode B+ trees);
- a free-space B+tree keyed by start block (and optionally a second keyed by length, XFS-style, for best-fit);
- an inode B+tree keyed by inode number;
- inode chunks of 64 inodes (16 blocks of 4 KiB at 256-byte inodes);
- data blocks.

A single AG is simpler and adequate for the Indy's typical 1–4 GB disk. Start with one AG; make `ag_count` a format field so a future version can add more.

**Inode** (256 bytes):

```
magic             u16  0x4937 ("I7")
version           u8   1
format            u8   0=dev, 1=inline, 2=extents, 3=btree
mode              u16
nlink             u32
uid               u32
gid               u32
flags             u32
generation        u32
size              u64
blocks            u64
atime             (i64 sec, u32 nsec)
mtime             (i64 sec, u32 nsec)
ctime             (i64 sec, u32 nsec)
crtime            (i64 sec, u32 nsec)
nextents          u32
forkoff           u16   (attribute fork offset in bytes; 0 = none)
ino               u64
parent_ino        u64   (optional; for fsck/orphan recovery)
crc32c            u32
data[]            extents or btree root or inline bytes
attr[]            optional attribute fork
```

The data fork starts at a fixed offset (e.g. 128) and grows to `forkoff` or the end. The attribute fork grows down from the end. Inline data (small files, short symlinks, small directories) lives in the data fork. Extents are 24 bytes:

```
logical_offset    u64
physical_block    u64
length_blocks     u32
flags             u32  (unwritten, prealloc; other bits reserved)
```

This is deliberately simpler than XFS's 128-bit packed extent. It is 8 bytes larger, but the parsing is trivial and the alignment is natural on a 32-bit MIPS CPU. A 256-byte inode with a 128-byte header and 24-byte extents holds about 5 extents inline; files with more switch to a B+tree. That is fewer than XFS's 19 because the extent is larger; if that matters, shrink the header or use a 16-byte packed extent later. For v1, simplicity wins.

**Directories.**

- **Inline** for small directories: a header `{count, parent_ino, hash_seed}` followed by entries `{name_len, type, ino, name[]}`. No hash index; linear scan is fine for < 20 entries.
- **B+tree** for larger directories: leaves store `{name_hash, ino, type, name[]}` sorted by hash; internal nodes store `{hash, block}`. Lookup hashes the name, descends, then compares the full name to resolve collisions. A 4 KiB leaf holds ~100 entries; a 1,000-entry directory is 10 leaves and one root.
- `.` and `..` are real entries (unlike XFS shortform); this simplifies path resolution and fsck.
- `rename` is atomic in the journal: log the removal from the old directory and the insertion into the new one in one transaction.

**Journal.**

- Internal, 2–4 MiB (default 2 MiB), circular, 4 KiB log blocks.
- Record header: magic `I7JL`, version, sequence, length, transaction id, crc32c, flags (commit/unmount).
- Operations: `InodeUpdate`, `BlockFree`, `BlockAlloc`, `DirInsert`, `DirRemove`, `SuperUpdate`, `OrphanAdd`, `OrphanRemove`.
- Each operation is idempotent: replay applies the new value or checks the generation; it does not "undo".
- A commit record makes the transaction durable; home-location writes happen after commit; the superblock checkpoint LSN advances after the home writes are durable.
- On mount, replay from `checkpoint_lsn` to the head; discard a torn last record; then run `fsck` if the log was dirty and the superblock was not checkpointed.

**Free space and inodes.** The free-space B+tree stores extents `{start, length}`; allocation is best-fit or first-fit with a cursor. The inode B+tree stores allocated chunks; a bitmap per chunk marks free inodes. The inode allocator prefers the AG of the parent directory. This is close to XFS's model but much smaller: no AGFL, no rmap, no reflink, no finobt.

**Checksums.** CRC32C (Castagnoli) on the superblock, AG headers, inode, directory blocks, journal records and B+tree blocks. The CRC field is stored **big-endian**, consistently, and documented; XFS's little-endian CRC field is a trap, not a model to copy. A CRC32C implementation is available in the `crc32c` crate (Apache-2.0/MIT) or can be written in a few hundred lines.

### 7.4 Why not XFS, RedoxFS or ext4?

| Candidate | Why not as the native format |
|---|---|
| **XFS v4** | Write support is 18–30 person-months; no CRCs; 2038 timestamps; upstream Linux is removing v4; new disks would inherit a deprecated format. Keep XFS for compatibility, not for new volumes. |
| **XFS v5** | IRIX 6.5 cannot read it; it adds rmap/reflink/sparse/CRC complexity; write support is even larger. No IRIX identity benefit. |
| **RedoxFS** | Little-endian; 128 KiB records and a 256-block header ring are heavy for 32 MB; depends on `redox_syscall`; ~2,000-line transaction engine with 63 `unsafe`; no fsck. Good fallback, not the first choice. |
| **ext4** | Little-endian; journal and feature creep; the Rust ports do not implement the journal; ext2 is too limited (indirect blocks, no extents); ext4_rs is nightly-only. |
| **FAT / littlefs** | No permissions, no hard links, no journal, file-size limits; flash-oriented. |

I7FS is not trying to beat XFS on scalability. It is trying to be **correct, small, crash-consistent and native** on a 1990s workstation. That is a different optimisation target, and it is the right one for the boot/root volume.

### 7.5 Implementation plan

**Milestone 1 — format and host tools (1–1.5 PM).**
- Write the format specification (this section, expanded).
- Implement `mkfs.i7fs` on the host: superblock, AG header, root inode, root directory, journal.
- Implement a read-only host tool that lists the root and reads a file.
- Add a `fuser`-based mount for host testing.

**Milestone 2 — read/write core (1.5–2 PM).**
- Block device trait, block cache, inode cache.
- Inline/extent/btree file data; directory lookup/insert/remove; symlinks; `stat`.
- Allocation: free-space B+tree, inode chunks, orphan list.
- Journal: transaction reserve/commit/checkpoint; replay.

**Milestone 3 — fsck and crash tests (1–1.5 PM).**
- `fsck.i7fs`: replay, check superblocks, AG headers, free-space tree, inode tree, link counts, duplicate blocks, orphan list, lost+found.
- Fault injection: drop/duplicate/reorder/truncate writes; crash during `fsync`, `rename`, `unlink`, `truncate`.
- Compare against a known-good tree after every replay.

**Milestone 4 — IRIX 7 integration (1 PM).**
- FS server cell, VFS operations, page cache/mmap, capabilities.
- Boot from an I7FS image; run the compat container; run IRIX binaries from an XFS volume.

Total: **4.5–6 person-months** for a solid v1, including tools and tests. That is comparable to a RedoxFS port once the endianness and shim work is counted, and the result is a format the project can audit and evolve.

### 7.6 What "done" looks like

- `mkfs.i7fs` creates a 100 MB image; `fsck.i7fs` reports clean.
- A host FUSE mount can create, read, write, rename, unlink, truncate, symlink and hard-link files; `fsck` stays clean.
- A crash during any metadata operation replays to a consistent tree; `fsck` reports clean or repairs to a documented state.
- IRIX 7 boots from the image, runs a native Rust shell and an IRIX 6.5 binary from an XFS volume.
- A 32 MB Indy can build a kernel and run a small workload without thrashing the cache.
- The format is documented in `docs/spec/i7fs.md`, with a version byte and a compatibility policy.

---

## 8. Roadmap, effort and risk

### 8.1 Phases and decision gates

| Phase | Deliverable | Effort (PM) | Gate |
|---|---|---|---|
| **0. Bring-up** | SCSI driver cell, block device capability, DMA broker, read-only root image (SquashFS/EROFS or I7FS-read-only), VFS skeleton, FUSE host harness | 3–4 | Boots to a shell on the Indy; reads a file from a read-only image; host FUSE mount works |
| **1. XFS read-only** | SGI label, superblock, inode, extent/bmbt, dir2 (shortform/block/leaf/node), symlinks, clean-log check, `stat`/`read`/`readdir`/`readlink` | 4–6 | Reads a corpus of IRIX 6.5 images; matches `xfs_db`/`ls -lR`/checksums; no panics on malformed images |
| **2. Native root** | I7FS v1: format, `mkfs`, `fsck`, journal, read/write, page cache/mmap, FS server | 4–6 | Boots from I7FS; crash tests pass; host FUSE + fsck clean |
| **3. XFS dirty log** | IRIX_BE log replay: head/tail, records, buffer/inode/EFI items, torn-write handling | 3–5 | Replays a crashed IRIX image to the same tree as an IRIX mount; `xfs_repair -n` agrees |
| **4. XFS write** | Allocation, directory mutation, bmbt updates, transactions, unlinked list, fsck | 18–30 | `xfs_repair -n` clean after create/unlink/rename/truncate; xfstests subset; backup/restore round-trip |
| **5. Hardening** | ACLs, quotas, v1 directories, EFS, XLV detection, performance, docs | 4–8 | Compatibility matrix complete; documented limits |

**Critical path to a usable system:** Phase 0 → Phase 2 (native root) → Phase 1 (XFS read-only) → Phase 3 (dirty log). If compatibility is the priority, run Phase 1 first on the read-only image root, then Phase 2; the two can proceed in parallel once the block device and VFS exist. XFS write (Phase 4) is a separate, optional programme.

**Decision gates.**

- **G0 (end of Phase 0):** Does the DMA broker work without corrupting memory under stress? Does the FS server survive a crash/restart? If not, stop and fix the trust model before adding filesystems.
- **G1 (end of Phase 1):** Does the XFS reader match the oracle on the full corpus? If v1 directories are in the corpus and unsupported, decide: implement or scope out.
- **G2 (end of Phase 2):** Does I7FS survive 10,000 random crash points with `fsck` clean? If not, do not make it the root.
- **G3 (end of Phase 3):** Does replay produce the same tree as IRIX? If not, do not enable write.
- **G4 (before Phase 4):** Is there a verified backup/restore path and a `xfs_repair`-compatible fsck? If not, do not write to legacy disks.

### 8.2 Staffing scenarios

| Team | To usable native + XFS RO (12–19 PM) | To XFS write (+18–30 PM) |
|---|---|---|
| 1 engineer | 12–19 months | 30–49 months |
| 2 engineers | 7–11 months | 18–30 months |
| 3–4 engineers | 5–7 months | 12–20 months |

These assume experienced systems engineers, a working Rust toolchain and test rig, and no major kernel/ABI surprises. The estimates are ±50%; the XFS write estimate is the least certain because it depends on how much of `xfs_repair`'s checking you replicate.

### 8.3 Risk register

| Risk | Likelihood | Impact | Mitigation |
|---|---|---|---|
| DMA broker bug corrupts memory (no IOMMU) | Medium | Critical | Kernel-owned descriptors; bounce buffers; stress tests; treat SCSI driver as TCB until proven |
| XFS v4 parser mishandles a valid IRIX image | Medium | High (data inaccessible) | Corpus of real images; differential testing against `xfs_db`; fuzzing; fail closed |
| Dirty IRIX log cannot be replayed | High | High (volume unusable until IRIX mount) | Refuse by default with clear message; implement Phase 3; document IRIX recovery |
| v1 directories in the target corpus | Medium | High | Survey the actual media; implement v1 or require conversion; budget 2–4 PM |
| I7FS schedule slips | Medium | Medium | RedoxFS fallback; keep root on read-only image longer |
| Native FS data loss on crash | Low–medium | Critical | Journal + CRC + fsck + 10k crash-point tests; mount `ro` after unclean shutdown until fsck |
| XFS write corrupts legacy disk | Low (if gated) | Critical | Read-only default; explicit `rw`; backup; `xfs_repair -n`; never write without fsck |
| Licence/provenance contamination | Medium | High (project blocked) | Licence gate; provenance statements; similarity screen; prefer behavioural reuse |
| Memory pressure on 32 MB | Medium | High | 4–6 MiB cache; metadata priority; no double caching; measure on hardware |
| SCSI driver bugs (timeouts, aborts, tagged queue) | Medium | High | Start with queue depth 1; add tagged queueing later; error injection; bus reset tests |
| Performance below IRIX 6.5 | Medium | Medium | Batch IPC; readahead; avoid copies; measure against `dd`/`tar` on IRIX |
| Toolchain/ABI surprises | Medium | Medium | Prototype the FS server as a host FUSE binary first; keep the block trait small |
| Upstream XFS v4 removal | High (already happening) | Medium | IRIX 7 is the long-term reader; keep an old xfsprogs VM as an oracle; do not depend on Linux for production |

### 8.4 Test corpus

The single most valuable asset is a **corpus of real IRIX 6.5 XFS images** with known contents. Build it before writing the parser:

- IRIX 6.5.22, 6.5.14, 6.5.5 and (if available) 6.2/6.5.4;
- filesystem block sizes 512, 1 KiB, 2 KiB, 4 KiB, 8 KiB;
- inode sizes 256 and 512 (and 128 if a v1 volume);
- directory versions 1 and 2;
- internal and external logs; log v1 and v2;
- files: empty, small inline, 1 extent, many extents (bmbt), sparse, large (> 2 GiB if the disk allows), hard links, symlinks (short and long), device nodes;
- directories: shortform, block, leaf, node; many entries; long names; deleted/reused slots;
- attributes: user/trusted/security, ACLs, default ACLs;
- quotas if used;
- a crashed volume with a dirty log, and the same volume after IRIX recovery;
- a volume with a v1 inode (if you can create one).

Capture for each: `xfs_db` metadata dumps, `ls -lR`/`find` output, file checksums, `xfs_repair -n` output, `xfs_logprint` output. Store the images (or compressed metadata dumps) with a manifest. This corpus is the oracle for every phase.

### 8.5 Testing layers

1. **Unit tests** for every parser: bounds, counts, offsets, magic, endianness, malformed input.
2. **Property tests** for extent mapping, directory hash lookup, B+tree traversal.
3. **Differential tests** against `xfs_db`/`xfs_repair`/`xfs_logprint` and against `xfs-core`/`lamxfs` where they support the feature.
4. **Fuzzing** with `cargo-fuzz`: superblock, AGF/AGI/AGFL, inode, extent, bmbt, each directory form, log record. No panics, no OOM, no unbounded loops.
5. **Host FUSE tests**: same FS server code on Linux; `fsx`/`pjdfstest`-style workloads; `fsck` after every run.
6. **Crash tests**: QEMU snapshots, `dm-flakey`/`blkdebug`, simulated power-cut; replay; compare.
7. **Hardware tests**: real Indy, real SCSI disk, 32 MB and 128 MB; boot time, `tar`/`dd` throughput, memory usage.
8. **Compatibility tests**: run IRIX 6.5 binaries from an XFS volume; `ls`, `cat`, `cp`, `tar`, `find`, a small GUI app if possible; compare outputs.

---

## 9. Open questions and decisions needed

These are the questions the ADR must answer explicitly. The "default" column is the report's recommendation if no other information is available.

| # | Question | Default recommendation |
|---|---|---|
| 1 | Which IRIX 6.5 minor versions and disk populations must be supported? | 6.5.14+ (v2 dirs) as the baseline; survey the actual media; v1 dirs as a later compatibility feature |
| 2 | Is EFS in scope? | No for v1; add a small EFS reader later if the media contains EFS partitions |
| 3 | Are XLV/XVM volumes in scope? | No; detect and refuse with a clear error |
| 4 | Must the first release replay dirty IRIX logs? | No; refuse dirty logs with a clear message; Phase 3 adds replay |
| 5 | Is XFS write required for 1.0? | No; read-only + copy to native is the compatibility story; XFS write is a later programme |
| 6 | Who owns the HPC3 DMA registers? | The kernel; the SCSI driver owns only the WD33C93B protocol registers and submits buffer handles |
| 7 | Is the SCSI driver in the TCB? | Only if the kernel does not own DMA; the recommended design keeps it out of the TCB for memory integrity |
| 8 | Native FS: I7FS or RedoxFS? | I7FS; RedoxFS is the fallback if I7FS slips past 6 PM or fails crash tests |
| 9 | Root filesystem for bring-up? | Read-only SquashFS/EROFS or I7FS image + RAM overlay; native I7FS root in Phase 2 |
| 10 | ACLs in the first XFS read-only release? | No; `stat`/mode works, ACL enforcement does not; document the gap and add in Phase 5 |
| 11 | Quotas? | Read-only display later; enforcement only with XFS write |
| 12 | Realtime subvolume? | Out of scope; detect and refuse |
| 13 | Performance target? | Boot to shell < 60 s on a 200 MHz/32 MB Indy; sequential read ≥ 5 MB/s; metadata ops ≥ 200/s |
| 14 | Team and schedule? | 2 engineers, 7–11 months to usable native + XFS RO; 3–4 engineers, 5–7 months |
| 15 | ADR numbers? | ADR-0021 storage architecture; ADR-0022 native filesystem format; ADR-0023 DMA/IOMMU trust model |

The most consequential question is **#5**. If the project declares XFS write as a 1.0 requirement, the schedule and risk profile change completely: XFS write is larger than the rest of the storage stack combined, and it must be correct enough not to destroy the only copy of a user's data. The report recommends keeping it out of 1.0.

The second most consequential is **#6/#7**. If the SCSI driver owns the HPC3 DMA registers, the "userspace driver" story is a fault-isolation story, not a security story. That may be acceptable on a single-user 1990s workstation, but it should be a recorded decision, not an accident.

### 9.1 What to do next

1. **Adopt Option C**: userspace FS cells, a kernel-brokered block device, a native I7FS root and a separate XFS compatibility server. Do not put XFS in the kernel.
2. **Build the XFS v4 read-only corpus and parser first** as the compatibility milestone: SGI label, superblock, inodes, extents/bmbt, dir2, symlinks, clean-log check. Refuse dirty logs until Phase 3.
3. **Build I7FS v1 for the product root**: 4 KiB blocks, big-endian, CRC32C, extents, B+tree directories, internal journal, `mkfs`, `fsck`, host FUSE tests. Keep RedoxFS as the fallback if it slips.
4. **Make the kernel own the HPC3 DMA descriptor ring** and validate every buffer; keep the SCSI driver's MMIO to the WD33C93B protocol registers. Record the trust model in an ADR.
5. **Defer XFS write** until there is a `xfs_repair`-compatible fsck, a verified backup/restore path, and a corpus that proves create/unlink/rename/truncate. Mount legacy volumes read-only by default.

---

## Appendix A — Sources

### A.1 XFS on-disk format and design

- SGI, *XFS Filesystem Structure*, 2nd edition rev. 2 (2006): <http://dubeyko.com/development/FileSystems/XFS/xfs_filesystem_structure.pdf>; original xfs.org book: <https://xfs.org/docs/xfsdocs-xml-dev/XFS_Filesystem_Structure/tmp/en-US/html/index.html> (certificate expired at the time of writing; the PDF mirror is usable).
- Current XFS documentation source (inode, directories, AGs, log): <https://kernel.googlesource.com/pub/scm/linux/kernel/git/djwong/xfs-documentation/+/refs/heads/master/design/XFS_Filesystem_Structure/>.
- Linux `xfs_format.h`: <https://github.com/torvalds/linux/blob/master/fs/xfs/libxfs/xfs_format.h>.
- Linux `xfs_log_format.h`: <https://github.com/torvalds/linux/blob/master/fs/xfs/libxfs/xfs_log_format.h>.
- Linux `xfs_da_format.h`: <https://github.com/torvalds/linux/blob/master/fs/xfs/libxfs/xfs_da_format.h>.
- Linux XFS log recovery (IRIX format check): <https://github.com/torvalds/linux/blob/master/fs/xfs/xfs_log_recover.c>.
- Linux XFS ACLs (`SGI_ACL_FILE`): <https://github.com/torvalds/linux/blob/master/fs/xfs/xfs_acl.c>.
- Linux XFS self-describing metadata: <https://docs.kernel.org/filesystems/xfs/xfs-self-describing-metadata.html>.
- Linux XFS delayed logging design: <https://docs.kernel.org/filesystems/xfs/xfs-delayed-logging-design.html>.
- Linux XFS online fsck design: <https://docs.kernel.org/filesystems/xfs/xfs-online-fsck-design.html>.
- Linux XFS admin guide (v4 deprecation, mount options): <https://docs.kernel.org/admin-guide/xfs.html>.
- Sweeney et al., "Scalability in the XFS File System", USENIX ATC 1996: <https://www.usenix.org/legacy/publications/library/proceedings/sd96/sweeney.html>; PDF <http://www.cs.columbia.edu/~nieh/teaching/e6118_s00/papers/sweeney_xfs96.pdf>.
- SGI, "XFS: A Next Generation Journalled 64-Bit Filesystem With Guaranteed Rate I/O" (white paper): <http://www.sgistuff.net/software/irixintro/documents/xfs-whitepaper.html>.

### A.2 IRIX specifics

- IRIX `mkfs_xfs(1M)`: <https://nixdoc.net/man-pages/IRIX/man1/mkfs_xfs.1.html>.
- IRIX 6.5 `vh(7M)` volume header: <https://help.graphica.com.au/irix-6.5.30/man/7M/vh>.
- IRIX Admin: Disks and Filesystems (007-2825-009): <https://irix7.com/techpubs/007-2825-009.pdf>.
- IRIX 6.5.5 update guide (v2 directories introduced): <https://pixelbart.net/SGI/IRIX/pdf/irix.6.5.5.pdf>.
- IRIX 6.5.14 release notes (v2 directories default): <https://wiki.preterhuman.net/IRIX_6.5.14>.
- IRIX 6.5.5/6.5.14 "what's new" summary: <http://www.bcbp.gu.se/WhatsNew/cool/cool.html>.
- SGI disklabel format: <https://keramics.github.io/sgilabel.html>.
- IRIX `xfs_repair(1M)`: <https://nixdoc.net/man-pages/irix/man1/xfs_repair.1.html>.

### A.3 Linux XFS v1/v2 history

- xfsprogs commit "xfs: kill unsupported superblock versions" (2015): <https://github.com/chandanr/xfsprogs-dev/commit/5f6f36600d1acb8c89386e5efd654c9b84f57c75>.
- xfsprogs `doc/CHANGES`, 4.10.0-rc1 "remove old Irix support": <https://git.kernel.org/pub/scm/fs/xfs/xfsprogs-dev.git/plain/doc/CHANGES>.
- Linux commit "xfs: remove the di_version field from struct icdinode" (v1 inode conversion): <https://www.spinics.net/lists/linux-xfs/msg70719.html>.
- xfstests `xfs/1890` (v1 inode nlink/onlink): <https://www.spinics.net/lists/linux-xfs/msg88768.html>.
- XFS log endianness discussion: <https://www.spinics.net/lists/xfs/msg28486.html>.

### A.4 Hardware

- Linux/MIPS IP22 page (WD33C93B, SCSI): <https://linux-mips.org/wiki/IP22>.
- HPC3 datasheet (SCSI DMA, descriptor chain, 8 KiB practical limit): <https://erikarn.github.io/sgi/indy/datasheets/sgi_indy_hpc3.pdf>.
- Linux `sgiwd93.c` (descriptor chain, `dma_cache_sync`, queue depth 16): <https://github.com/torvalds/linux/blob/master/drivers/scsi/sgiwd93.c>.
- Linux `hpc3.h` (`struct hpc_dma_desc`): <https://github.com/torvalds/linux/blob/master/arch/mips/include/asm/sgi/hpc3.h>.
- Indy technical notes (SCSI IDs, narrow SCSI-2): <https://tech-pubs.net/reputable-archive/indytech.html>.

### A.5 Rust implementations

- RedoxFS: <https://github.com/redox-os/redoxfs>, <https://crates.io/crates/redoxfs>.
- Redox TFS: <https://github.com/redox-os/tfs>.
- Theseus: <https://github.com/theseus-os/Theseus>, book <https://www.theseus-os.com/Theseus/book/>, OSDI 2020 paper <https://www.usenix.org/conference/osdi20/presentation/boos>.
- Theseus `memfs`: <https://www.theseus-os.com/Theseus/doc/memfs/>.
- Theseus `block_cache` limitations: <https://www.theseus-os.com/Theseus/doc/block_cache/>.
- Asterinas: <https://github.com/asterinas/asterinas>, paper <https://arxiv.org/abs/2506.03876>, 0.19 release <https://asterinas.github.io/2026/10/13/announcing-asterinas-0.19.0.html>.
- rcore-fs: <https://github.com/rcore-os/rcore-fs>.
- `fatfs`: <https://github.com/rafalh/rust-fatfs>, <https://crates.io/crates/fatfs>.
- `littlefs2`: <https://github.com/trussed-dev/littlefs2>, littlefs <https://github.com/littlefs-project/littlefs>.
- `embedded-sdmmc`: <https://github.com/rust-embedded-community/embedded-sdmmc-rs>.
- `ext4-view`: <https://github.com/nicholasbishop/ext4-view-rs>.
- `ext4_rs`: <https://github.com/yuoo655/ext4_rs>.
- `ntfs`: <https://github.com/ColinFinck/ntfs>.
- `exfat-fs`: <https://github.com/hannahfluch/exfat-fs>; `exfat`: <https://github.com/obhq/exfat>.
- `backhand`: <https://github.com/wcampbell0x2a/backhand>.
- `erofs-rs`: <https://github.com/Dreamacro/erofs-rs>.
- `p9`: <https://github.com/google/rust-p9>; `ninep`: <https://github.com/sminez/ninep>.
- `fuser`: <https://github.com/cberner/fuser>.
- `fuse-backend-rs`: <https://github.com/cloud-hypervisor/fuse-backend-rs>.
- `nfsserve`: <https://github.com/huggingface/nfsserve>; `nfs3_server`: <https://github.com/Vaiz/nfs3>.
- `btrfsutils` / `btrfs-disk`: <https://github.com/rustutils/btrfsutils>.
- `zfs-forensic`: <https://github.com/SecurityRonin/zfs-forensic>.
- OpenZFS: <https://github.com/openzfs/zfs>.
- `xfs-forensic` / `xfs-core`: <https://github.com/SecurityRonin/xfs-forensic>.
- `xfuse`: <https://github.com/KhaledEmaraDev/xfuse>.
- `lamxfs`: <https://github.com/lamco-admin/lamxfs>.
- `rust-fs-xfs`: <https://github.com/antimatter-studios/rust-fs-xfs>.
- `mkfs-xfs`: <https://github.com/justincormack/mkfs-xfs>.
- `korp-mkfs-xfs`: <https://crates.io/crates/korp-mkfs-xfs>.

### A.6 Licensing

- GNU GPLv3: <https://www.gnu.org/licenses/gpl-3.0.html>.
- GNU GPL FAQ: <https://www.gnu.org/licenses/gpl-faq.html>.
- GNU licence list (Apache-2.0, MPL-2.0, CDDL): <https://www.gnu.org/licenses/license-list.html>.
- Linux `COPYING` (GPL-2.0 only): <https://github.com/torvalds/linux/blob/master/COPYING>.
- Linux XFS SPDX headers: <https://github.com/torvalds/linux/blob/master/fs/xfs/xfs_super.c>, <https://github.com/torvalds/linux/blob/master/fs/xfs/libxfs/xfs_format.h>.
- xfsprogs `libxfs/xfs_format.h` (GPL-2.0): <https://git.kernel.org/pub/scm/fs/xfs/xfsprogs-dev.git/plain/libxfs/xfs_format.h>.
- xfsprogs `libhandle/handle.c` (LGPL-2.1): <https://git.kernel.org/pub/scm/fs/xfs/xfsprogs-dev.git/plain/libhandle/handle.c>.
- OpenZFS `LICENSE` (CDDL-1.0): <https://github.com/openzfs/zfs/blob/master/LICENSE>.
- `btrfsutils` workspace `Cargo.toml` (default GPL-2.0-only; per-crate overrides in `disk/`, `fuse/`, `fs/`, `cli/`): <https://github.com/rustutils/btrfsutils/blob/main/Cargo.toml>.

### A.7 General filesystem and OS design

- Theseus OSDI 2020 paper (cells, SAS/SPL): <https://www.usenix.org/system/files/osdi20-boos.pdf>.
- Asterinas ATC 2025 paper (framekernel, safe Rust): <https://arxiv.org/abs/2506.03876>.
- FUSE protocol: <https://github.com/libfuse/libfuse>.
- 9P2000: <https://9p.io/sys/man/5/INDEX.html>.
- NFSv3 RFC 1813: <https://www.rfc-editor.org/rfc/rfc1813>.
- SquashFS: <https://docs.kernel.org/filesystems/squashfs.html>.
- EROFS: <https://docs.kernel.org/filesystems/erofs.html>.

---

## Appendix B — XFS on-disk cheat sheet

This is a checklist for the implementer, not a substitute for the specification. All values are big-endian unless marked.

### B.1 Address arithmetic

```
agno   = ino >> (sb_agblklog + sb_inopblog)
agino  = ino & ((1 << (sb_agblklog + sb_inopblog)) - 1)
agbno  = agino >> sb_inopblog
offset = agino & (sb_inopblock - 1)          // inode index within block
fsb    = agno * sb_agblocks + agbno
byte   = fsb * sb_blocksize + offset * sb_inodesize
```

For the last AG, use `agf_length` instead of `sb_agblocks` when validating `agbno`.

### B.2 Magic numbers

| Object | Magic | Notes |
|---|---|---|
| Superblock | `0x58465342` "XFSB" | At offset 0 of each AG |
| AGF | `0x58414746` "XAGF" | Sector 1 |
| AGI | `0x58414749` "XAGI" | Sector 2 |
| AGFL | `0x5841464c` "XAFL" | Sector 3 |
| Inode | `0x494e` "IN" | `di_magic` |
| Bmap btree (v4) | `0x424d4150` "BMAP" | Long-format block |
| Bmap btree (v5) | `0x424d4133` "BMA3" | CRC header |
| Free-space btree by block | `0x41425442` "ABTB" | Short-format |
| Free-space btree by count | `0x41425443` "ABTC" | Short-format |
| Inode btree | `0x49414254` "IABT" | Short-format |
| Inode btree CRC header (v5) | `0x49414233` "IAB3" | v5 only |
| Free inode btree (v5) | `0x46494254` "FIBT" / `0x46494233` "FIB3" | v5 only |
| Directory block (v2) | `0x58443242` "XD2B" | Single block |
| Directory data (v2) | `0x58443244` "XD2D" | Multi-block |
| Directory free (v2) | `0x58443246` "XD2F" | Free index |
| Directory leaf1 (v2) | `0xd2f1` | Single leaf |
| Directory leafN (v2) | `0xd2ff` | Multi leaf |
| Directory node (v2) | `0xfebe` | Internal node |
| Directory block (v3) | `0x58444233` "XDB3" | v5 |
| Directory data (v3) | `0x58444433` "XDD3" | v5 |
| Directory free (v3) | `0x58444633` "XDF3" | v5 |
| Directory leaf1 (v3) | `0x3df1` | v5 |
| Directory leafN (v3) | `0x3dff` | v5 |
| Directory node (v3) | `0x3ebe` | v5 |
| Attribute leaf | `0xfbee` | v4 |
| Attribute leaf (v3) | `0x3bee` | v5 |
| Log record | `0xFEEDBABE` | 512-byte header |
| SGI volume header | `0x0BE5A941` | LBA 0 |

Sources: `xfs_format.h`, `xfs_da_format.h`, SGI book. Verify against `xfs_db` output rather than trusting this table for a new format version.

### B.3 v4 superblock feature bits (`sb_versionnum`)

| Bit | Name | Meaning |
|---|---|---|
| `0x000f` | `NUMBITS` | Version number (must be 4) |
| `0x0010` | `ATTRBIT` | Extended attributes exist |
| `0x0020` | `NLINKBIT` | 32-bit `di_nlink` (v2 inodes) |
| `0x0040` | `QUOTABIT` | Quotas enabled |
| `0x0080` | `ALIGNBIT` | `sb_inoalignmt` used |
| `0x0100` | `DALIGNBIT` | `sb_unit`/`sb_width` used |
| `0x0200` | `SHAREDBIT` | Shared filesystem (obsolete; refuse) |
| `0x0400` | `LOGV2BIT` | v2 log |
| `0x0800` | `SECTORBIT` | `sb_sectsize != 512` |
| `0x1000` | `EXTFLGBIT` | Unwritten extents (always set in practice) |
| `0x2000` | `DIRV2BIT` | v2 directories |
| `0x4000` | `BORGBIT` | ASCII case-insensitive names |
| `0x8000` | `MOREBITSBIT` | `sb_features2` is valid |

Source: `xfs_format.h`, lines 35–48. The `OKBITS` mask excludes `SHAREDBIT`; unknown bits must be refused.

### B.4 `sb_features2` bits

| Bit | Name | Meaning |
|---|---|---|
| `0x00000002` | `LAZYSBCOUNTBIT` | Global counters updated at unmount |
| `0x00000008` | `ATTR2BIT` | attr2 inline layout |
| `0x00000010` | `PARENTBIT` | Parent pointers |
| `0x00000080` | `PROJID32BIT` | 32-bit project IDs |
| `0x00000100` | `CRCBIT` | v5 CRC metadata |
| `0x00000200` | `FTYPE` | File type in directory entries |

`CRCBIT` implies v5 in practice; a v4 filesystem with CRC is not a thing. For IRIX, `FTYPE` may or may not be set; check before reading the byte.

### B.5 Inode formats

| `di_format` | Value | Used by |
|---|---|---|
| `DEV` | 0 | Device nodes |
| `LOCAL` | 1 | Small dirs, short symlinks, inline data |
| `EXTENTS` | 2 | Extent array in the inode |
| `BTREE` | 3 | Bmap B+tree root in the inode |
| `UUID` | 4 | Obsolete |

`di_version`: 1 (v1 inode, `di_onlink`), 2 (v2 inode, `di_nlink`/project ID), 3 (v5, 176-byte header).

### B.6 Extent record

```
bit 127      : unwritten flag
bits 126..73 : logical start offset (54 bits, filesystem blocks)
bits 72..21  : absolute start block (52 bits, filesystem blocks)
bits 20..0   : block count (21 bits)
```

The record is 16 bytes, stored as two big-endian u64 words. A common decoding is:

```
u64 w0 = be64(rec[0..8]);
u64 w1 = be64(rec[8..16]);

flag       = w0 >> 63;
startoff   = (w0 >> 9) & 0x003f_ffff_ffff_ffff;  // 54 bits
startblock = ((w0 & 0x1ff) << 43) | (w1 >> 21);  // 52 bits, xfs_fsblock_t
blockcount = w1 & 0x1f_ffff;                     // 21 bits

// xfs_fsblock_t -> physical block:
agno   = startblock >> sb_agblklog;
agbno  = startblock & ((1 << sb_agblklog) - 1);
fsb    = agno * sb_agblocks + agbno;
```

Use the named bit widths from `xfs_format.h` and test against `xfs_db`; do not trust a hand-written shift without a fixture. The `startblock` field is not a flat block number: it encodes the AG number and AG-relative block, exactly like the inode-number arithmetic.

### B.7 Log record header (512 bytes)

| Offset | Size | Field |
|---|---|---|
| 0 | 4 | `h_magicno` (be32 `0xFEEDBABE`) |
| 4 | 4 | `h_cycle` |
| 8 | 4 | `h_version` (1 or 2) |
| 12 | 4 | `h_len` |
| 16 | 8 | `h_lsn` |
| 24 | 8 | `h_tail_lsn` |
| 32 | 4 | `h_crc` (**le32**, v5 only) |
| 36 | 4 | `h_prev_block` |
| 40 | 4 | `h_num_logops` |
| 44 | 128 | `h_cycle_data[32]` |
| 172 | 4 | `h_fmt` (`0` IRIX, `1` Linux LE, `2` Linux BE, `3` IRIX BE) |
| 176 | 16 | `h_fs_uuid` (zero on IRIX) |
| 192 | 4 | `h_size` (log v2) |
| 196 | 4 | `h_pad0` |
| 200 | 184 | `h_reserved` |
| 384 | 128 | `h_ext[]` (extended header) |

The exact offsets depend on packing and on the `h_ext` flexible array; use `xfs_log_format.h` and verify with `xfs_logprint`. The key facts are: 512-byte header, magic at 0, `h_fmt` at 172, UUID at 176, `h_crc` is little-endian.

### B.8 SGI volume header

| Offset | Size | Field |
|---|---|---|
| 0 | 4 | Magic `0x0BE5A941` (BE) |
| 4 | 2 | Root partition number |
| 6 | 2 | Swap partition number |
| 8 | 16 | Boot file name |
| 24 | 48 | Device parameters |
| 72 | 240 | 15 volume descriptors (16 bytes each) |
| 312 | 192 | 16 partition entries (12 bytes each) |
| 504 | 4 | Checksum |
| 508 | 4 | Padding |

Partition entry: `{u32 sectors; u32 start; u32 type}`. Types: 0 volhdr, 3 swap, 6 volume, 7 EFS, 8 LVOL, 9 RVOL, 10 XFS, 11 XFS log, 12 XLV, 13 XVM. Source: <https://keramics.github.io/sgilabel.html>.

### B.9 XFS v4 vs v5

| Feature | v4 (IRIX 6.5) | v5 (Linux 2014+) |
|---|---|---|
| Metadata CRC | No | CRC32C on every block |
| Self-describing header | No | UUID, owner, blkno, LSN |
| Inode version | 1 or 2 (100-byte header) | 3 (176-byte header) |
| Directory format | dir2 | dir3 |
| Bmap magic | `BMAP` | `BMA3` |
| Free inode btree | No | Optional (`finobt`) |
| Inode btree block counts | No | Optional (`inobtcount`) |
| Reverse mapping | No | Optional (`rmapbt`) |
| Reflink | No | Optional (`reflink`) |
| Sparse inodes | No | Optional (`spinodes`) |
| Big timestamps | No | Optional (`bigtime`) |
| 64-bit extent counts | No | Optional (`nrext64`) |
| Parent pointers | No | Optional (`parent`) |
| Metadata directory | No | Optional (`metadir`) |
| Log format | IRIX BE / unknown | Linux LE/BE + CRC |
| Timestamp range | 1901–2038 | ~1901–2486 with bigtime |
| IRIX 6.5 can read | Yes | No |
| Linux can read | Deprecated; removal scheduled | Yes |

The v5 feature bits are in `sb_features_incompat` and `sb_features_ro_compat`; the v4 equivalent is `sb_features2`. Do not confuse the two.

### B.10 IRIX 6.5 mkfs defaults

| Parameter | Default | Notes |
|---|---|---|
| Block size | 4096 | 512–65536; 512 recommended for small root, 4096 generally |
| Inode size | 256 | 256–2048; 256 typical |
| Directory format | v2 after 6.5.14; v1 before | `-n version=1` for old volumes |
| Directory block size | 4096 | Up to 64 KiB for v2 |
| Log | internal, v2 | external optional |
| Allocation groups | computed | 8 in the IRIX admin example for a large disk |
| Sector size | 512 | 4 KiB possible |

Sources: IRIX `mkfs_xfs(1M)`, IRIX Admin: Disks and Filesystems, 6.5.5/6.5.14 release notes.

---

## Appendix C — Licence compatibility matrix (quick gate)

| Licence | GPL-3.0 compatible? | Obligations | Examples in this report |
|---|---|---|---|
| MIT | Yes | Keep copyright + permission notice | RedoxFS, Theseus, rcore-fs, fatfs, fuser, rust-fs-xfs |
| BSD-2-Clause | Yes | Keep copyright + disclaimer | xfuse |
| BSD-3-Clause | Yes | Keep copyright + disclaimer; no endorsement | p9, nfsserve, littlefs C |
| Apache-2.0 | Yes | Keep NOTICE; patent termination | xfs-core, lamxfs, mkfs-xfs, erofs-rs, zfs-forensic-core |
| MPL-2.0 | Yes (MPL §3.3) | File-level copyleft; keep MPL files available | Asterinas |
| LGPL-2.1/3.0 | Yes | Relink / reverse-engineer for debugging | xfsprogs `libhandle` |
| GPL-2.0-only | **No** | Cannot combine with GPL-3.0 | Linux XFS, xfsprogs `libxfs`, btrfsutils |
| GPL-2.0-or-later | Yes | Choose GPLv3 for the combined work | Some older code |
| CDDL-1.0 | **No** | Cannot link with GPL | OpenZFS |
| Proprietary | No | — | IRIX source |

**Rule of thumb:** if the repository `LICENSE`/`COPYING` and the crate manifest disagree, stop and resolve it before use. `btrfsutils` is the cautionary example: the workspace default is GPL-2.0-only, while `btrfs-disk`, `btrfs-fuse` and `btrfs-fs` override to MIT/Apache-2.0 and `btrfs-cli`/`btrfs-mkfs` do not. Check the exact crate you are using, and its dependencies.

---

## Appendix D — A short reading list

If you read only five things:

1. SGI, *XFS Filesystem Structure* (the v4 book): <http://dubeyko.com/development/FileSystems/XFS/xfs_filesystem_structure.pdf>.
2. Linux `xfs_format.h`, `xfs_da_format.h`, `xfs_log_format.h` (the authoritative field definitions).
3. Linux `xfs_log_recover.c` (the IRIX `h_fmt` check and the recovery model).
4. `lamxfs` (a small, readable, no_std v4/v5 reader): <https://github.com/lamco-admin/lamxfs>.
5. Theseus's block-cache documentation (what not to do): <https://www.theseus-os.com/Theseus/doc/block_cache/>.

---

*End of report.*

