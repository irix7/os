// SPDX-License-Identifier: GPL-3.0-only
//! Minimal per-process descriptor table for the bring-up userland.
//!
//! Console, `/dev/null`, two small mutable ramfs files, a directory table and
//! a small pipe ring. This is a bring-up namespace, not the capability-backed
//! VFS.

use crate::syserr::{
    SysResult, CELL_E_AGAIN, CELL_E_BADF, CELL_E_INVAL, CELL_E_ISDIR, CELL_E_MFILE,
    CELL_E_NAMETOOLONG, CELL_E_NODEV, CELL_E_NOENT, CELL_E_NOSPC, CELL_E_NOTDIR, CELL_E_NOTTY,
};

pub const MAX_FDS: usize = 16;
const FILE_CAPACITY: usize = 512;
const O_TRUNC: u32 = 0o1000;
const MAX_PIPES: usize = 8;
const PIPE_CAPACITY: usize = 4096;

pub struct FileEntry {
    pub path: &'static [u8],
}

static FILES: [FileEntry; 1] = [FileEntry {
    path: b"/hello.txt",
}];
const INITIAL: [&[u8]; 1] = [b"hello from the IRIX 7 ramfs\n"];
static mut FILE_DATA: [[u8; FILE_CAPACITY]; 1] = [[0; FILE_CAPACITY]; 1];
static mut FILE_LEN: [usize; 1] = [0; 1];
static mut PIPE_DATA: [[u8; PIPE_CAPACITY]; MAX_PIPES] = [[0; PIPE_CAPACITY]; MAX_PIPES];
static mut PIPE_HEAD: [usize; MAX_PIPES] = [0; MAX_PIPES];
static mut PIPE_LEN: [usize; MAX_PIPES] = [0; MAX_PIPES];
static mut PIPE_USED: [bool; MAX_PIPES] = [false; MAX_PIPES];
static mut CONSOLE_WINSIZE: [u16; 4] = [24, 80, 0, 0];

struct DirEntry {
    name: &'static [u8],
    kind: u8, // 4 = directory, 8 = regular file
}

static ROOT_ENTRIES: [DirEntry; 5] = [
    DirEntry {
        name: b".",
        kind: 4,
    },
    DirEntry {
        name: b"..",
        kind: 4,
    },
    DirEntry {
        name: b"hello.txt",
        kind: 8,
    },
    DirEntry {
        name: b"etc",
        kind: 4,
    },
    DirEntry {
        name: b"dev",
        kind: 4,
    },
];
static ETC_ENTRIES: [DirEntry; 3] = [
    DirEntry {
        name: b".",
        kind: 4,
    },
    DirEntry {
        name: b"..",
        kind: 4,
    },
    DirEntry {
        name: b"motd",
        kind: 8,
    },
];
static DEV_ENTRIES: [DirEntry; 4] = [
    DirEntry {
        name: b".",
        kind: 4,
    },
    DirEntry {
        name: b"..",
        kind: 4,
    },
    DirEntry {
        name: b"null",
        kind: 2,
    },
    DirEntry {
        name: b"tty",
        kind: 2,
    },
];
static DIRECTORIES: [&[DirEntry]; 3] = [&ROOT_ENTRIES, &ETC_ENTRIES, &DEV_ENTRIES];

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Closed,
    Console,
    Null,
    File(u32),
    Logical,
    Dir(u32),
    PipeRead(u32),
    PipeWrite(u32),
}

static mut TABLE: [Kind; MAX_FDS] = [
    Kind::Console,
    Kind::Console,
    Kind::Console,
    Kind::Closed,
    Kind::Closed,
    Kind::Closed,
    Kind::Closed,
    Kind::Closed,
    Kind::Closed,
    Kind::Closed,
    Kind::Closed,
    Kind::Closed,
    Kind::Closed,
    Kind::Closed,
    Kind::Closed,
    Kind::Closed,
];
static mut OFFSET: [usize; MAX_FDS] = [0; MAX_FDS];
static mut CWD: [u8; 64] = [0; 64];
static mut CWD_LEN: usize = 1;

pub fn reset() {
    unsafe {
        for (index, slot) in TABLE.iter_mut().enumerate() {
            *slot = if index < 3 {
                Kind::Console
            } else {
                Kind::Closed
            };
            OFFSET[index] = 0;
        }
        for (index, data) in INITIAL.iter().enumerate() {
            FILE_DATA[index].fill(0);
            FILE_DATA[index][..data.len()].copy_from_slice(data);
            FILE_LEN[index] = data.len();
        }
        CWD[0] = b'/';
        CWD_LEN = 1;
        PIPE_HEAD = [0; MAX_PIPES];
        PIPE_LEN = [0; MAX_PIPES];
        PIPE_USED = [false; MAX_PIPES];
        CONSOLE_WINSIZE = [24, 80, 0, 0];
    }
}

pub fn kind(fd: i32) -> Kind {
    if fd < 0 || fd as usize >= MAX_FDS {
        return Kind::Closed;
    }
    unsafe { TABLE[fd as usize] }
}

fn install(kind: Kind) -> SysResult<i32> {
    unsafe {
        for (fd, slot) in TABLE.iter_mut().enumerate() {
            if *slot == Kind::Closed {
                *slot = kind;
                OFFSET[fd] = 0;
                return Ok(fd as i32);
            }
        }
    }
    Err(CELL_E_MFILE)
}

/// Create a pipe. Returns `(read_fd, write_fd)`.
pub fn pipe() -> SysResult<(i32, i32)> {
    unsafe {
        let Some(index) = (0..MAX_PIPES).find(|index| !PIPE_USED[*index]) else {
            return Err(CELL_E_MFILE);
        };
        PIPE_USED[index] = true;
        PIPE_HEAD[index] = 0;
        PIPE_LEN[index] = 0;
        let read = match install(Kind::PipeRead(index as u32)) {
            Ok(fd) => fd,
            Err(status) => {
                PIPE_USED[index] = false;
                return Err(status);
            }
        };
        match install(Kind::PipeWrite(index as u32)) {
            Ok(write) => Ok((read, write)),
            Err(status) => {
                TABLE[read as usize] = Kind::Closed;
                PIPE_USED[index] = false;
                Err(status)
            }
        }
    }
}

fn release_pipe(index: usize) {
    unsafe {
        let still_open = TABLE.iter().any(|kind| {
            matches!(kind, Kind::PipeRead(other) | Kind::PipeWrite(other) if *other as usize == index)
        });
        if !still_open && index < MAX_PIPES {
            PIPE_USED[index] = false;
            PIPE_HEAD[index] = 0;
            PIPE_LEN[index] = 0;
        }
    }
}

/// Resolve a path against the current working directory.
fn resolve(path: &[u8], out: &mut [u8; 64]) -> Option<usize> {
    let mut cursor = 0usize;
    if path.first() != Some(&b'/') {
        let cwd = unsafe { &CWD[..CWD_LEN] };
        for &byte in cwd {
            if cursor == out.len() {
                return None;
            }
            out[cursor] = byte;
            cursor += 1;
        }
        if cursor == 0 || out[cursor - 1] != b'/' {
            if cursor == out.len() {
                return None;
            }
            out[cursor] = b'/';
            cursor += 1;
        }
    }
    for &byte in path {
        if cursor == out.len() {
            return None;
        }
        out[cursor] = byte;
        cursor += 1;
    }
    let mut start = 0;
    let mut end = cursor;
    while start < end && out[start] == b'/' {
        start += 1;
    }
    while end > start && out[end - 1] == b'/' {
        end -= 1;
    }
    if start == end {
        out[0] = b'/';
        return Some(1);
    }
    let length = end - start;
    out.copy_within(start..end, 1);
    out[0] = b'/';
    Some(length + 1)
}

pub fn set_cwd(path: &[u8]) -> SysResult<()> {
    let mut resolved = [0u8; 64];
    let Some(len) = resolve(path, &mut resolved) else {
        return Err(CELL_E_NAMETOOLONG);
    };
    let path = &resolved[..len];
    if path != b"/" && path != b"/etc" && path != b"/dev" {
        return Err(CELL_E_NOENT);
    }
    unsafe {
        CWD.fill(0);
        CWD[..len].copy_from_slice(path);
        CWD_LEN = len;
    }
    Ok(())
}

pub fn copy_cwd(out: &mut [u8; 64]) -> usize {
    unsafe {
        out[..CWD_LEN].copy_from_slice(&CWD[..CWD_LEN]);
        CWD_LEN
    }
}

pub fn open_path(path: &[u8], flags: u32) -> SysResult<i32> {
    let mut resolved = [0u8; 64];
    let Some(len) = resolve(path, &mut resolved) else {
        return Err(CELL_E_NAMETOOLONG);
    };
    let path = &resolved[..len];
    if path == b"/dev/null" {
        return if crate::logical::device(path) {
            install(Kind::Null)
        } else {
            Err(CELL_E_NODEV)
        };
    }
    if path == b"/dev/tty" || path == b"/dev/console" {
        return Ok(0);
    }
    if path == b"/" || path == b"/." {
        return install(Kind::Dir(0));
    }
    if path == b"/etc" {
        return install(Kind::Dir(1));
    }
    if path == b"/etc/motd" {
        return if crate::logical::open_file(path) {
            install(Kind::Logical)
        } else {
            Err(CELL_E_NOENT)
        };
    }
    if path == b"/dev" {
        return install(Kind::Dir(2));
    }
    for (index, file) in FILES.iter().enumerate() {
        if file.path == path {
            if flags & O_TRUNC != 0 {
                unsafe {
                    FILE_LEN[index] = 0;
                }
            }
            return install(Kind::File(index as u32));
        }
    }
    Err(CELL_E_NOENT)
}

pub fn close(fd: i32) -> SysResult<()> {
    if fd < 0 || fd as usize >= MAX_FDS {
        return Err(CELL_E_BADF);
    }
    let old = kind(fd);
    if old == Kind::Closed {
        return Err(CELL_E_BADF);
    }
    unsafe {
        TABLE[fd as usize] = Kind::Closed;
        OFFSET[fd as usize] = 0;
    }
    if let Kind::PipeRead(index) | Kind::PipeWrite(index) = old {
        release_pipe(index as usize);
    }
    Ok(())
}

pub fn dup(fd: i32) -> SysResult<i32> {
    let source = kind(fd);
    if source == Kind::Closed {
        return Err(CELL_E_BADF);
    }
    let new = install(source)?;
    unsafe {
        OFFSET[new as usize] = OFFSET[fd as usize];
    }
    Ok(new)
}

pub fn read(fd: i32, buffer: &mut [u8]) -> SysResult<usize> {
    match kind(fd) {
        Kind::Logical => {
            let offset = unsafe { OFFSET[fd as usize] };
            let count = crate::logical::read_file(b"/etc/motd", offset, buffer).unwrap_or(0);
            unsafe { OFFSET[fd as usize] += count };
            Ok(count)
        }
        Kind::File(index) => {
            let index = index as usize;
            let offset = unsafe { OFFSET[fd as usize] };
            let len = unsafe { FILE_LEN[index] };
            if offset >= len {
                return Ok(0);
            }
            let count = buffer.len().min(len - offset);
            unsafe {
                buffer[..count].copy_from_slice(&FILE_DATA[index][offset..offset + count]);
                OFFSET[fd as usize] += count;
            }
            Ok(count)
        }
        Kind::PipeRead(index) => {
            let index = index as usize;
            let len = unsafe { PIPE_LEN[index] };
            if len == 0 {
                return Ok(0);
            }
            let count = buffer.len().min(len);
            let head = unsafe { PIPE_HEAD[index] };
            for (offset, slot) in buffer[..count].iter_mut().enumerate() {
                *slot = unsafe { PIPE_DATA[index][(head + offset) % PIPE_CAPACITY] };
            }
            unsafe {
                PIPE_HEAD[index] = (head + count) % PIPE_CAPACITY;
                PIPE_LEN[index] = len - count;
            }
            Ok(count)
        }
        Kind::Console | Kind::Null => Ok(0),
        Kind::Dir(_) => Err(CELL_E_ISDIR),
        Kind::PipeWrite(_) => Err(CELL_E_BADF),
        Kind::Closed => Err(CELL_E_BADF),
    }
}

pub fn write(fd: i32, buffer: &[u8]) -> SysResult<usize> {
    match kind(fd) {
        Kind::File(index) => {
            let index = index as usize;
            let offset = unsafe { OFFSET[fd as usize] };
            if offset >= FILE_CAPACITY {
                return Err(CELL_E_NOSPC);
            }
            let count = buffer.len().min(FILE_CAPACITY - offset);
            unsafe {
                FILE_DATA[index][offset..offset + count].copy_from_slice(&buffer[..count]);
                OFFSET[fd as usize] += count;
                FILE_LEN[index] = FILE_LEN[index].max(offset + count);
            }
            Ok(count)
        }
        Kind::Null => Ok(buffer.len()),
        Kind::PipeWrite(index) => {
            let index = index as usize;
            let len = unsafe { PIPE_LEN[index] };
            let free = PIPE_CAPACITY.saturating_sub(len);
            let count = buffer.len().min(free);
            if count == 0 {
                return Err(CELL_E_AGAIN);
            }
            let head = unsafe { PIPE_HEAD[index] };
            for (offset, byte) in buffer[..count].iter().enumerate() {
                unsafe {
                    PIPE_DATA[index][(head + len + offset) % PIPE_CAPACITY] = *byte;
                }
            }
            unsafe {
                PIPE_LEN[index] = len + count;
            }
            Ok(count)
        }
        Kind::Console => Ok(buffer.len()),
        Kind::Dir(_) => Err(CELL_E_ISDIR),
        Kind::PipeRead(_) | Kind::Logical => Err(CELL_E_BADF),
        Kind::Closed => Err(CELL_E_BADF),
    }
}

/// Read the console window size into `out`.
pub fn get_winsize(fd: i32, out: &mut [u16; 4]) -> SysResult<()> {
    match kind(fd) {
        Kind::Console => {
            // SAFETY: single-threaded kernel; the value is copied out.
            *out = unsafe { CONSOLE_WINSIZE };
            Ok(())
        }
        Kind::Closed => Err(CELL_E_BADF),
        _ => Err(CELL_E_NOTTY),
    }
}

/// Set the console window size. Zero rows or columns are invalid.
pub fn set_winsize(fd: i32, value: &[u16; 4]) -> SysResult<()> {
    match kind(fd) {
        Kind::Console => {
            if value[0] == 0 || value[1] == 0 {
                return Err(CELL_E_INVAL);
            }
            unsafe {
                CONSOLE_WINSIZE = *value;
            }
            Ok(())
        }
        Kind::Closed => Err(CELL_E_BADF),
        _ => Err(CELL_E_NOTTY),
    }
}

pub fn metadata(fd: i32) -> SysResult<(u32, i64)> {
    match kind(fd) {
        Kind::File(index) => Ok((0o100644, unsafe { FILE_LEN[index as usize] } as i64)),
        Kind::Logical => Ok((
            0o100644,
            crate::logical::file_size(b"/etc/motd").unwrap_or(0) as i64,
        )),
        Kind::Console | Kind::Null => Ok((0o020666, 0)),
        Kind::Dir(_) => Ok((0o040755, 0)),
        Kind::PipeRead(_) | Kind::PipeWrite(_) => Ok((0o010600, 0)),
        Kind::Closed => Err(CELL_E_BADF),
    }
}

pub fn metadata_path(path: &[u8]) -> SysResult<(u32, i64)> {
    let mut resolved = [0u8; 64];
    let Some(len) = resolve(path, &mut resolved) else {
        return Err(CELL_E_NAMETOOLONG);
    };
    let path = &resolved[..len];
    if path == b"/dev/null" || path == b"/dev/tty" || path == b"/dev/console" {
        return Ok((0o020666, 0));
    }
    if path == b"/" || path == b"/etc" || path == b"/dev" {
        return Ok((0o040755, 0));
    }
    if path == b"/etc/motd" {
        return crate::logical::file_size(path)
            .map(|size| (0o100644, size as i64))
            .ok_or(CELL_E_NOENT);
    }
    FILES
        .iter()
        .enumerate()
        .find(|(_, file)| file.path == path)
        .map(|(index, _)| (0o100644, unsafe { FILE_LEN[index] } as i64))
        .ok_or(CELL_E_NOENT)
}

pub fn readdir(fd: i32, index: usize, out: &mut [u8]) -> SysResult<Option<(u8, usize)>> {
    let dir = match kind(fd) {
        Kind::Dir(dir) => dir,
        Kind::Closed => return Err(CELL_E_BADF),
        _ => return Err(CELL_E_NOTDIR),
    };
    let Some(entry) = DIRECTORIES[dir as usize].get(index) else {
        return Ok(None);
    };
    let len = entry.name.len().min(out.len().saturating_sub(1));
    out[..len].copy_from_slice(&entry.name[..len]);
    out[len] = 0;
    Ok(Some((entry.kind, len)))
}
