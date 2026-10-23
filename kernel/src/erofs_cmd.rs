// SPDX-License-Identifier: GPL-3.0-only
//! Read an EROFS image over HTTP from the debug console.
//!
//! The image is treated as a byte-addressed block device.  A small one-block
//! cache keeps the metadata and inline data in the first block from being
//! fetched repeatedly; later this becomes the LRU in the block cell.

use core::cell::Cell;
use core::fmt::{self, Write};

use erofs::{BlockDevice, Erofs, Error, FileType};
use smoltcp::wire::Ipv4Address;

/// HTTP range cache size.  EROFS normally uses 4 KiB blocks.
const CACHE_SIZE: usize = 4096;
/// Console output ceiling.  The telnet transmit buffer is 16 KiB.
const OUTPUT_SIZE: usize = 4096;

/// An operation requested on the mounted image.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Op {
    /// Print superblock information.
    Info,
    /// List a directory.
    Ls,
    /// Print a file.
    Cat,
}

/// A parsed `erofs` console command.
pub struct Request {
    host: [u8; 64],
    host_len: usize,
    port: u16,
    path: [u8; 96],
    path_len: usize,
    op: Op,
    target: [u8; 96],
    target_len: usize,
}

impl Request {
    /// Parse `erofs <host>:<port><path> [info|ls|cat] [target]`.
    pub fn parse(arg: &[u8]) -> Option<Self> {
        let text = core::str::from_utf8(arg).ok()?;
        let mut parts = text.split_whitespace();
        let raw_url = parts.next()?;
        let op = match parts.next().unwrap_or("info") {
            "info" => Op::Info,
            "ls" => Op::Ls,
            "cat" => Op::Cat,
            _ => return None,
        };
        let target = parts.next().unwrap_or("/");
        if parts.next().is_some() {
            return None;
        }
        if raw_url.starts_with("https://") {
            return None;
        }
        let url = raw_url.strip_prefix("http://").unwrap_or(raw_url);
        let (host, rest) = url.split_once(':')?;
        let slash = rest.find('/')?;
        let (port_text, path) = rest.split_at(slash);
        let port: u16 = port_text.parse().ok()?;
        if host.is_empty()
            || host.len() > 64
            || path.is_empty()
            || path.len() > 96
            || target.len() > 96
        {
            return None;
        }

        let mut host_buf = [0u8; 64];
        host_buf[..host.len()].copy_from_slice(host.as_bytes());
        let mut path_buf = [0u8; 96];
        path_buf[..path.len()].copy_from_slice(path.as_bytes());
        let mut target_buf = [0u8; 96];
        target_buf[..target.len()].copy_from_slice(target.as_bytes());
        Some(Self {
            host: host_buf,
            host_len: host.len(),
            port,
            path: path_buf,
            path_len: path.len(),
            op,
            target: target_buf,
            target_len: target.len(),
        })
    }
}

/// Console output with a fixed ceiling.
pub struct Output {
    buf: [u8; OUTPUT_SIZE],
    len: usize,
    truncated: bool,
}

impl Output {
    fn new() -> Self {
        Self {
            buf: [0; OUTPUT_SIZE],
            len: 0,
            truncated: false,
        }
    }

    fn push(&mut self, bytes: &[u8]) {
        if self.truncated {
            return;
        }
        let room = self.buf.len().saturating_sub(self.len);
        let take = bytes.len().min(room);
        self.buf[self.len..self.len + take].copy_from_slice(&bytes[..take]);
        self.len += take;
        if take < bytes.len() {
            self.truncated = true;
        }
    }

    fn error(&mut self, error: Error, last_http: Option<&'static str>) {
        let _ = write!(self, "erofs: {error}\n");
        if matches!(error, Error::Io) {
            if let Some(text) = last_http {
                let _ = write!(self, "erofs: http: {text}\n");
            }
        }
    }

    /// The formatted bytes.  A truncated response ends with a marker.
    pub fn as_bytes(&self) -> &[u8] {
        &self.buf[..self.len]
    }
}

impl Write for Output {
    fn write_str(&mut self, text: &str) -> fmt::Result {
        self.push(text.as_bytes());
        Ok(())
    }
}

/// Run a parsed request and return console output.
pub fn run(request: &Request) -> Output {
    let mut out = Output::new();
    let host = core::str::from_utf8(&request.host[..request.host_len]).unwrap_or("");
    let path = core::str::from_utf8(&request.path[..request.path_len]).unwrap_or("/");
    let target = core::str::from_utf8(&request.target[..request.target_len]).unwrap_or("/");

    let Some(ip) = crate::telnet::resolve_host(&request.host[..request.host_len]) else {
        out.push(b"erofs: unknown host\n");
        return out;
    };

    let last_http = Cell::new(None);
    let device = HttpRangeDevice {
        ip,
        port: request.port,
        host,
        path,
        cache: [0; CACHE_SIZE],
        cache_start: 0,
        cache_valid: false,
        last_http: &last_http,
    };

    let mut fs = match Erofs::mount(device) {
        Ok(fs) => fs,
        Err(error) => {
            out.error(error, last_http.get());
            return out;
        }
    };

    let result = (|| -> erofs::Result<()> {
        match request.op {
            Op::Info => {
                let sb = fs.superblock();
                let volume_end = sb
                    .volume_name
                    .iter()
                    .position(|&byte| byte == 0)
                    .unwrap_or(sb.volume_name.len());
                let volume = core::str::from_utf8(&sb.volume_name[..volume_end]).unwrap_or("");
                let _ = write!(out, "erofs: mounted\n");
                let _ = write!(out, "block size: {}\n", sb.block_size());
                let _ = write!(out, "blocks: {}\n", sb.blocks);
                let _ = write!(out, "inodes: {}\n", sb.inos);
                let _ = write!(out, "root nid: {}\n", sb.root_nid);
                let _ = write!(out, "meta block: {}\n", sb.meta_blkaddr);
                let _ = write!(
                    out,
                    "features: compat=0x{:08x} incompat=0x{:08x}\n",
                    sb.feature_compat, sb.feature_incompat
                );
                let _ = write!(
                    out,
                    "volume: {}\n",
                    if volume.is_empty() { "-" } else { volume }
                );
                let _ = write!(out, "uuid: ");
                for byte in sb.uuid {
                    let _ = write!(out, "{byte:02x}");
                }
                let _ = write!(out, "\n");
                Ok(())
            }
            Op::Ls => {
                let inode = fs.lookup_path(target)?;
                let entries = fs.read_dir(&inode)?;
                let _ = write!(out, "erofs: {} ({} entries)\n", target, entries.len());
                for entry in entries {
                    let name = core::str::from_utf8(&entry.name).unwrap_or("?");
                    let _ = writeln!(out, "  {} {}", type_char(entry.file_type), name);
                }
                Ok(())
            }
            Op::Cat => {
                let inode = fs.lookup_path(target)?;
                if inode.is_dir() {
                    return Err(Error::NotRegularFile);
                }
                let data = fs.read_file(&inode)?;
                let _ = write!(out, "erofs: cat {} ({} bytes)\n", target, data.len());
                const MARKER: &[u8] = b"\n[truncated]\n";
                let room = out.buf.len().saturating_sub(out.len + MARKER.len());
                let shown = data.len().min(room);
                out.push(&data[..shown]);
                if shown < data.len() {
                    out.truncated = true;
                } else if !data.ends_with(b"\n") {
                    out.push(b"\n");
                }
                Ok(())
            }
        }
    })();

    if let Err(error) = result {
        out.error(error, last_http.get());
    }
    if out.truncated {
        let marker = b"\n[truncated]\n";
        if out.len + marker.len() <= out.buf.len() {
            out.buf[out.len..out.len + marker.len()].copy_from_slice(marker);
            out.len += marker.len();
        }
    }
    out
}

fn type_char(file_type: FileType) -> char {
    match file_type {
        FileType::Directory => 'd',
        FileType::Regular => '-',
        FileType::Symlink => 'l',
        FileType::CharDevice => 'c',
        FileType::BlockDevice => 'b',
        FileType::Fifo => 'p',
        FileType::Socket => 's',
        FileType::Unknown => '?',
    }
}

/// A byte-addressed HTTP range block device with one cached 4 KiB block.
struct HttpRangeDevice<'a> {
    ip: Ipv4Address,
    port: u16,
    host: &'a str,
    path: &'a str,
    cache: [u8; CACHE_SIZE],
    cache_start: u64,
    cache_valid: bool,
    last_http: &'a Cell<Option<&'static str>>,
}

impl HttpRangeDevice<'_> {
    fn fill(&mut self, start: u64) -> erofs::Result<()> {
        let end = start.checked_add(CACHE_SIZE as u64 - 1).ok_or(Error::Io)?;
        let start = u32::try_from(start).map_err(|_| Error::Io)?;
        let end = u32::try_from(end).map_err(|_| Error::Io)?;
        let result = crate::http::get_range_to(
            self.ip,
            self.port,
            self.host,
            self.path,
            start,
            end,
            &mut self.cache,
        );
        match result {
            Ok((_summary, CACHE_SIZE)) => {
                self.cache_start = u64::from(start);
                self.cache_valid = true;
                Ok(())
            }
            Ok(_) => {
                self.last_http.set(Some("short range body"));
                Err(Error::Io)
            }
            Err(text) => {
                self.last_http.set(Some(text));
                Err(Error::Io)
            }
        }
    }
}

impl BlockDevice for HttpRangeDevice<'_> {
    fn read_at(&mut self, offset: u64, buf: &mut [u8]) -> erofs::Result<()> {
        let mut done = 0usize;
        while done < buf.len() {
            let position = offset.checked_add(done as u64).ok_or(Error::OutOfBounds)?;
            let start = position & !(CACHE_SIZE as u64 - 1);
            if !self.cache_valid || self.cache_start != start {
                self.fill(start)?;
            }
            let within = (position - start) as usize;
            let count = (CACHE_SIZE - within).min(buf.len() - done);
            buf[done..done + count].copy_from_slice(&self.cache[within..within + count]);
            done += count;
        }
        Ok(())
    }
}
