// SPDX-License-Identifier: GPL-3.0-only
//! Mount an EROFS image from a SCSI disk.
//!
//! The image starts at LBA 0 on the selected target.  Reads go through the
//! shared LRU block cache, which turns an EROFS 4 KiB block into eight 512-byte
//! SCSI READ(6) commands.  This is the first bridge between the SCSI driver and
//! the filesystem reader; descriptor DMA and the full block cell come later.

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use core::cmp::min;

use block_cache::Lru;

/// A byte-addressed SCSI disk at 512-byte sector granularity.
struct ScsiDisk {
    target: u8,
}

impl block_cache::ReadAt for ScsiDisk {
    type Error = &'static str;

    fn read_at(&mut self, offset: u64, buf: &mut [u8]) -> Result<(), Self::Error> {
        let mut done = 0usize;
        while done < buf.len() {
            let position = offset
                .checked_add(done as u64)
                .ok_or("sector offset overflow")?;
            let lba = u32::try_from(position / 512).map_err(|_| "sector out of range")?;
            let within = (position % 512) as usize;
            let mut sector = [0u8; 512];
            crate::scsi::read_sector(self.target, lba, &mut sector)?;
            let count = min(512 - within, buf.len() - done);
            buf[done..done + count].copy_from_slice(&sector[within..within + count]);
            done += count;
        }
        Ok(())
    }
}

/// The SCSI disk wrapped in the block cache and exposed as an EROFS device.
struct CachedDisk {
    cache: Lru<ScsiDisk, 16>,
}

impl erofs::BlockDevice for CachedDisk {
    fn read_at(&mut self, offset: u64, buf: &mut [u8]) -> erofs::Result<()> {
        self.cache
            .read_at(offset, buf)
            .map_err(|_| erofs::Error::Io)
    }
}

/// Read `path` from the EROFS image at SCSI target `target`, LBA 0.
pub fn run(target: u8, path: &str) -> Result<Vec<u8>, String> {
    let disk = ScsiDisk { target };
    let cache =
        Lru::new(disk, 4096).map_err(|_| format!("block-cache: cannot allocate {} KiB", 64))?;
    let mut fs = erofs::Erofs::mount(CachedDisk { cache })
        .map_err(|error| format!("mount target {target}: {error}"))?;
    let inode = fs
        .lookup_path_follow(path)
        .map_err(|error| format!("lookup {path}: {error}"))?;

    let mut out = Vec::new();
    if inode.is_dir() {
        let entries = fs
            .read_dir(&inode)
            .map_err(|error| format!("readdir {path}: {error}"))?;
        out.extend_from_slice(
            format!(
                "erofsdisk: target {target} {path} ({} entries)\n",
                entries.len()
            )
            .as_bytes(),
        );
        for entry in entries {
            let name = core::str::from_utf8(&entry.name).unwrap_or("?");
            out.extend_from_slice(format!("  {name}\n").as_bytes());
        }
    } else {
        let data = fs
            .read_file(&inode)
            .map_err(|error| format!("read {path}: {error}"))?;
        out.extend_from_slice(
            format!("erofsdisk: target {target} {path} ({} bytes)\n", data.len()).as_bytes(),
        );
        let shown = data.len().min(4096);
        out.extend_from_slice(&data[..shown]);
        if shown < data.len() {
            out.extend_from_slice(b"\n[truncated]\n");
        } else if !data.ends_with(b"\n") {
            out.push(b'\n');
        }
    }
    Ok(out)
}

/// Parse `erofsdisk <target> [path]` and read the file.
pub fn command(arg: &[u8]) -> Result<Vec<u8>, String> {
    let text = core::str::from_utf8(arg).map_err(|_| "argument is not UTF-8".to_string())?;
    let mut parts = text.split_whitespace();
    let target_text = parts
        .next()
        .ok_or_else(|| "usage: erofsdisk <target> [path]".to_string())?;
    let target: u8 = target_text
        .parse()
        .map_err(|_| "target must be 0..7".to_string())?;
    if target > 7 {
        return Err("target must be 0..7".to_string());
    }
    let path = parts.next().unwrap_or("/etc/motd");
    if parts.next().is_some() {
        return Err("usage: erofsdisk <target> [path]".to_string());
    }
    run(target, path)
}
