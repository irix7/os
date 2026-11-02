// SPDX-License-Identifier: GPL-3.0-only
//! Minimal per-process descriptor table for the bring-up userland.
//!
//! Console, `/dev/null`, two small mutable ramfs files, a directory table and
//! a small pipe ring. This is a bring-up namespace, not the capability-backed
//! VFS.
//!
//! A descriptor is a table slot that names an open file description.  `dup`
//! and `dup2` share that description, so a seek or read through one descriptor
//! moves the offset seen by the other, as POSIX requires.  Close-on-exec is
//! per descriptor, not per description.

use crate::syserr::{
    SysResult, CELL_E_AGAIN, CELL_E_BADF, CELL_E_INVAL, CELL_E_ISDIR, CELL_E_MFILE,
    CELL_E_NAMETOOLONG, CELL_E_NODEV, CELL_E_NOENT, CELL_E_NOSPC, CELL_E_NOTDIR, CELL_E_NOTTY,
    CELL_E_NXIO, CELL_E_OVERFLOW, CELL_E_PIPE, CELL_E_SPIPE,
};

pub const MAX_FDS: usize = 16;
const MAX_OPEN_FILES: usize = 16;
const FILE_CAPACITY: usize = 512;
const O_TRUNC: u32 = 0o1000;
const O_NONBLOCK: u32 = 0o4000;
const O_CLOEXEC: u32 = 0o2000000;
const FD_CLOEXEC: u32 = 1;
const F_GETFD: u32 = 1;
const F_SETFD: u32 = 2;
const SEEK_SET: u32 = 0;
const SEEK_CUR: u32 = 1;
const SEEK_END: u32 = 2;
const SEEK_DATA: u32 = 3;
const SEEK_HOLE: u32 = 4;
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

/// Open file descriptions.  `Kind::Closed` marks a free entry.
static mut OPEN_KIND: [Kind; MAX_OPEN_FILES] = [Kind::Closed; MAX_OPEN_FILES];
static mut OPEN_OFFSET: [usize; MAX_OPEN_FILES] = [0; MAX_OPEN_FILES];
static mut OPEN_REFS: [u8; MAX_OPEN_FILES] = [0; MAX_OPEN_FILES];

/// Descriptor slots: the description index, or -1 when the slot is free.
static mut FD_OPEN: [i32; MAX_FDS] = [-1; MAX_FDS];
static mut FD_FLAGS: [u32; MAX_FDS] = [0; MAX_FDS];
static mut CWD: [u8; 64] = [0; 64];
static mut CWD_LEN: usize = 1;

pub fn reset() {
    unsafe {
        FD_OPEN = [-1; MAX_FDS];
        FD_FLAGS = [0; MAX_FDS];
        OPEN_KIND = [Kind::Closed; MAX_OPEN_FILES];
        OPEN_OFFSET = [0; MAX_OPEN_FILES];
        OPEN_REFS = [0; MAX_OPEN_FILES];
        for fd in 0..3 {
            if let Some(description) = open_description(Kind::Console) {
                FD_OPEN[fd] = description as i32;
            }
            FD_FLAGS[fd] = 0;
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

/// Allocate a fresh open file description.  The caller owns its single ref.
fn open_description(kind: Kind) -> Option<usize> {
    unsafe {
        for index in 0..MAX_OPEN_FILES {
            if OPEN_KIND[index] == Kind::Closed && OPEN_REFS[index] == 0 {
                OPEN_KIND[index] = kind;
                OPEN_OFFSET[index] = 0;
                OPEN_REFS[index] = 1;
                return Some(index);
            }
        }
    }
    None
}

/// Drop one reference to a description, releasing it when the last goes.
fn release_description(index: usize) {
    unsafe {
        if index >= MAX_OPEN_FILES || OPEN_REFS[index] == 0 {
            return;
        }
        OPEN_REFS[index] -= 1;
        if OPEN_REFS[index] == 0 {
            let kind = OPEN_KIND[index];
            OPEN_KIND[index] = Kind::Closed;
            OPEN_OFFSET[index] = 0;
            if let Kind::PipeRead(pipe) | Kind::PipeWrite(pipe) = kind {
                release_pipe(pipe as usize);
            }
        }
    }
}

fn retain_description(index: usize) -> bool {
    unsafe {
        if index >= MAX_OPEN_FILES || OPEN_REFS[index] == 0 {
            return false;
        }
        OPEN_REFS[index] = OPEN_REFS[index].saturating_add(1);
        true
    }
}

fn description(fd: i32) -> Option<usize> {
    if fd < 0 || fd as usize >= MAX_FDS {
        return None;
    }
    let index = unsafe { FD_OPEN[fd as usize] };
    if index < 0 {
        None
    } else {
        Some(index as usize)
    }
}

pub fn kind(fd: i32) -> Kind {
    match description(fd) {
        Some(index) => unsafe { OPEN_KIND[index] },
        None => Kind::Closed,
    }
}

fn offset(fd: i32) -> usize {
    match description(fd) {
        Some(index) => unsafe { OPEN_OFFSET[index] },
        None => 0,
    }
}

fn set_offset(fd: i32, value: usize) {
    if let Some(index) = description(fd) {
        unsafe {
            OPEN_OFFSET[index] = value;
        }
    }
}

fn install_flags(kind: Kind, flags: u32) -> SysResult<i32> {
    let description = open_description(kind).ok_or(CELL_E_MFILE)?;
    match free_descriptor() {
        Some(fd) => {
            unsafe {
                FD_OPEN[fd as usize] = description as i32;
                FD_FLAGS[fd as usize] = flags;
            }
            Ok(fd)
        }
        None => {
            release_description(description);
            Err(CELL_E_MFILE)
        }
    }
}

fn free_descriptor() -> Option<i32> {
    (0..MAX_FDS)
        .find(|fd| unsafe { FD_OPEN[*fd] < 0 })
        .map(|fd| fd as i32)
}

fn close_slot(fd: i32) {
    if let Some(index) = description(fd) {
        unsafe {
            FD_OPEN[fd as usize] = -1;
            FD_FLAGS[fd as usize] = 0;
        }
        release_description(index);
    }
}

fn pipe_flags(flags: u32) -> SysResult<u32> {
    // Pipes are always non-blocking on this single-threaded kernel; O_NONBLOCK
    // is accepted because it describes the behaviour that already holds.
    if flags & !(O_CLOEXEC | O_NONBLOCK) != 0 {
        return Err(CELL_E_INVAL);
    }
    Ok(if flags & O_CLOEXEC != 0 {
        FD_CLOEXEC
    } else {
        0
    })
}

/// Create a pipe. Returns `(read_fd, write_fd)`.
pub fn pipe() -> SysResult<(i32, i32)> {
    pipe2(0)
}

/// Create a pipe with `O_CLOEXEC` applied to both descriptors.
pub fn pipe2(flags: u32) -> SysResult<(i32, i32)> {
    let close_on_exec = pipe_flags(flags)?;
    unsafe {
        let Some(index) = (0..MAX_PIPES).find(|index| !PIPE_USED[*index]) else {
            return Err(CELL_E_MFILE);
        };
        PIPE_USED[index] = true;
        PIPE_HEAD[index] = 0;
        PIPE_LEN[index] = 0;
        let read = match install_flags(Kind::PipeRead(index as u32), close_on_exec) {
            Ok(fd) => fd,
            Err(status) => {
                PIPE_USED[index] = false;
                return Err(status);
            }
        };
        match install_flags(Kind::PipeWrite(index as u32), close_on_exec) {
            Ok(write) => Ok((read, write)),
            Err(status) => {
                close_slot(read);
                Err(status)
            }
        }
    }
}

fn pipe_in_use(index: usize) -> bool {
    unsafe {
        (0..MAX_OPEN_FILES).any(|description| {
            OPEN_REFS[description] > 0
                && matches!(
                    OPEN_KIND[description],
                    Kind::PipeRead(other) | Kind::PipeWrite(other) if other as usize == index
                )
        })
    }
}

fn pipe_has_reader(index: usize) -> bool {
    unsafe {
        (0..MAX_OPEN_FILES).any(|description| {
            OPEN_REFS[description] > 0
                && matches!(
                    OPEN_KIND[description],
                    Kind::PipeRead(other) if other as usize == index
                )
        })
    }
}

fn pipe_has_writer(index: usize) -> bool {
    unsafe {
        (0..MAX_OPEN_FILES).any(|description| {
            OPEN_REFS[description] > 0
                && matches!(
                    OPEN_KIND[description],
                    Kind::PipeWrite(other) if other as usize == index
                )
        })
    }
}

fn release_pipe(index: usize) {
    unsafe {
        if !pipe_in_use(index) && index < MAX_PIPES {
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
    let close_on_exec = if flags & O_CLOEXEC != 0 {
        FD_CLOEXEC
    } else {
        0
    };
    if path == b"/dev/null" {
        return if crate::logical::device(path) {
            install_flags(Kind::Null, close_on_exec)
        } else {
            Err(CELL_E_NODEV)
        };
    }
    if path == b"/dev/tty" || path == b"/dev/console" {
        return Ok(0);
    }
    if path == b"/" || path == b"/." {
        return install_flags(Kind::Dir(0), close_on_exec);
    }
    if path == b"/etc" {
        return install_flags(Kind::Dir(1), close_on_exec);
    }
    if path == b"/etc/motd" {
        return if crate::logical::open_file(path) {
            install_flags(Kind::Logical, close_on_exec)
        } else {
            Err(CELL_E_NOENT)
        };
    }
    if path == b"/dev" {
        return install_flags(Kind::Dir(2), close_on_exec);
    }
    for (index, file) in FILES.iter().enumerate() {
        if file.path == path {
            if flags & O_TRUNC != 0 {
                unsafe {
                    FILE_LEN[index] = 0;
                }
            }
            return install_flags(Kind::File(index as u32), close_on_exec);
        }
    }
    Err(CELL_E_NOENT)
}

pub fn close(fd: i32) -> SysResult<()> {
    if description(fd).is_none() {
        return Err(CELL_E_BADF);
    }
    close_slot(fd);
    Ok(())
}

pub fn dup(fd: i32) -> SysResult<i32> {
    let Some(source) = description(fd) else {
        return Err(CELL_E_BADF);
    };
    let new = free_descriptor().ok_or(CELL_E_MFILE)?;
    if !retain_description(source) {
        return Err(CELL_E_BADF);
    }
    unsafe {
        FD_OPEN[new as usize] = source as i32;
        FD_FLAGS[new as usize] = 0;
    }
    Ok(new)
}

/// Duplicate `fd` onto `new_fd`, closing any descriptor already there.
pub fn dup2(fd: i32, new_fd: i32) -> SysResult<i32> {
    let Some(source) = description(fd) else {
        return Err(CELL_E_BADF);
    };
    if new_fd < 0 || new_fd as usize >= MAX_FDS {
        return Err(CELL_E_BADF);
    }
    if fd == new_fd {
        return Ok(new_fd);
    }
    close_slot(new_fd);
    if !retain_description(source) {
        return Err(CELL_E_BADF);
    }
    unsafe {
        FD_OPEN[new_fd as usize] = source as i32;
        FD_FLAGS[new_fd as usize] = 0;
    }
    Ok(new_fd)
}

/// `fcntl` for the two descriptor flags this bring-up keeps.
///
/// Unknown commands are `EINVAL`, not a silent success: there is no open-file
/// status word to report, and claiming otherwise would misrepresent the table.
pub fn fcntl(fd: i32, cmd: u32, arg: u32) -> SysResult<i32> {
    if description(fd).is_none() {
        return Err(CELL_E_BADF);
    }
    match cmd {
        F_GETFD => Ok(unsafe { FD_FLAGS[fd as usize] } as i32),
        F_SETFD => {
            unsafe {
                FD_FLAGS[fd as usize] = arg & FD_CLOEXEC;
            }
            Ok(0)
        }
        _ => Err(CELL_E_INVAL),
    }
}

/// The size `SEEK_END`, `SEEK_DATA` and `SEEK_HOLE` measure against.
fn length_of(kind: Kind) -> Option<usize> {
    match kind {
        Kind::File(index) => Some(unsafe { FILE_LEN[index as usize] }),
        Kind::Logical => Some(crate::logical::file_size(b"/etc/motd").unwrap_or(0)),
        Kind::Dir(dir) => Some(DIRECTORIES[dir as usize].len()),
        Kind::Null => Some(0),
        _ => None,
    }
}

/// Reposition a descriptor.  Directories use the same 32-bit cookie that
/// `getdents` writes in `d_off`; pipes and the console are not seekable.
pub fn lseek(fd: i32, offset_value: i64, whence: u32) -> SysResult<i64> {
    let file = kind(fd);
    let size = match file {
        Kind::Closed => return Err(CELL_E_BADF),
        Kind::PipeRead(_) | Kind::PipeWrite(_) | Kind::Console => return Err(CELL_E_SPIPE),
        _ => length_of(file).unwrap_or(0) as i64,
    };
    let current = offset(fd) as i64;

    if whence == SEEK_DATA || whence == SEEK_HOLE {
        if !matches!(file, Kind::File(_) | Kind::Logical) {
            return Err(CELL_E_INVAL);
        }
        if offset_value < 0 {
            return Err(CELL_E_INVAL);
        }
        // The bring-up files are dense: data runs to EOF and the only hole
        // starts there.
        if whence == SEEK_DATA {
            if offset_value >= size {
                return Err(CELL_E_NXIO);
            }
        } else if offset_value > size {
            return Err(CELL_E_NXIO);
        }
        let result = if whence == SEEK_DATA {
            offset_value
        } else {
            size
        };
        set_offset(fd, result as usize);
        return Ok(result);
    }

    let base = match whence {
        SEEK_SET => 0,
        SEEK_CUR => current,
        SEEK_END => size,
        _ => return Err(CELL_E_INVAL),
    };
    let Some(result) = base.checked_add(offset_value) else {
        return Err(CELL_E_OVERFLOW);
    };
    if result < 0 {
        return Err(CELL_E_INVAL);
    }
    if result > u32::MAX as i64 {
        return Err(CELL_E_OVERFLOW);
    }
    set_offset(fd, result as usize);
    Ok(result)
}

fn read_at(kind: Kind, start: usize, buffer: &mut [u8]) -> SysResult<usize> {
    match kind {
        Kind::Logical => Ok(crate::logical::read_file(b"/etc/motd", start, buffer).unwrap_or(0)),
        Kind::File(index) => {
            let index = index as usize;
            let len = unsafe { FILE_LEN[index] };
            if start >= len {
                return Ok(0);
            }
            let count = buffer.len().min(len - start);
            unsafe {
                buffer[..count].copy_from_slice(&FILE_DATA[index][start..start + count]);
            }
            Ok(count)
        }
        Kind::Console | Kind::Null => Ok(0),
        Kind::Dir(_) => Err(CELL_E_ISDIR),
        Kind::PipeRead(_) | Kind::PipeWrite(_) => Err(CELL_E_SPIPE),
        Kind::Closed => Err(CELL_E_BADF),
    }
}

fn write_at(kind: Kind, start: usize, buffer: &[u8]) -> SysResult<usize> {
    match kind {
        Kind::File(index) => {
            let index = index as usize;
            if buffer.is_empty() {
                return Ok(0);
            }
            if start >= FILE_CAPACITY {
                return Err(CELL_E_NOSPC);
            }
            let count = buffer.len().min(FILE_CAPACITY - start);
            unsafe {
                FILE_DATA[index][start..start + count].copy_from_slice(&buffer[..count]);
                FILE_LEN[index] = FILE_LEN[index].max(start + count);
            }
            Ok(count)
        }
        Kind::Null | Kind::Console => Ok(buffer.len()),
        Kind::Dir(_) => Err(CELL_E_ISDIR),
        Kind::PipeRead(_) | Kind::PipeWrite(_) | Kind::Logical => Err(CELL_E_SPIPE),
        Kind::Closed => Err(CELL_E_BADF),
    }
}

fn read_pipe(index: usize, buffer: &mut [u8]) -> SysResult<usize> {
    let len = unsafe { PIPE_LEN[index] };
    if len == 0 {
        // With no writer left the pipe is at end-of-file; otherwise this is a
        // non-blocking read with nothing to return.
        return if pipe_has_writer(index) {
            Err(CELL_E_AGAIN)
        } else {
            Ok(0)
        };
    }
    let count = buffer.len().min(len);
    if count == 0 {
        return Ok(0);
    }
    let head = unsafe { PIPE_HEAD[index] };
    for (position, slot) in buffer[..count].iter_mut().enumerate() {
        *slot = unsafe { PIPE_DATA[index][(head + position) % PIPE_CAPACITY] };
    }
    unsafe {
        PIPE_HEAD[index] = (head + count) % PIPE_CAPACITY;
        PIPE_LEN[index] = len - count;
    }
    Ok(count)
}

fn write_pipe(index: usize, buffer: &[u8]) -> SysResult<usize> {
    if buffer.is_empty() {
        return Ok(0);
    }
    if !pipe_has_reader(index) {
        return Err(CELL_E_PIPE);
    }
    let len = unsafe { PIPE_LEN[index] };
    let free = PIPE_CAPACITY.saturating_sub(len);
    if free == 0 {
        return Err(CELL_E_AGAIN);
    }
    let count = buffer.len().min(free);
    let head = unsafe { PIPE_HEAD[index] };
    for (position, byte) in buffer[..count].iter().enumerate() {
        unsafe {
            PIPE_DATA[index][(head + len + position) % PIPE_CAPACITY] = *byte;
        }
    }
    unsafe {
        PIPE_LEN[index] = len + count;
    }
    Ok(count)
}

pub fn read(fd: i32, buffer: &mut [u8]) -> SysResult<usize> {
    let file = kind(fd);
    if file == Kind::Closed {
        return Err(CELL_E_BADF);
    }
    if let Kind::PipeRead(index) = file {
        return read_pipe(index as usize, buffer);
    }
    let start = offset(fd);
    let count = read_at(file, start, buffer)?;
    set_offset(fd, start + count);
    Ok(count)
}

pub fn write(fd: i32, buffer: &[u8]) -> SysResult<usize> {
    let file = kind(fd);
    if file == Kind::Closed {
        return Err(CELL_E_BADF);
    }
    if let Kind::PipeWrite(index) = file {
        return write_pipe(index as usize, buffer);
    }
    let start = offset(fd);
    let count = write_at(file, start, buffer)?;
    set_offset(fd, start + count);
    Ok(count)
}

/// Read at an explicit offset without moving the descriptor's position.
pub fn pread(fd: i32, position: i64, buffer: &mut [u8]) -> SysResult<usize> {
    if position < 0 {
        return Err(CELL_E_INVAL);
    }
    let file = kind(fd);
    match file {
        Kind::File(_) | Kind::Logical | Kind::Console | Kind::Null => {}
        Kind::Closed => return Err(CELL_E_BADF),
        Kind::Dir(_) => return Err(CELL_E_ISDIR),
        Kind::PipeRead(_) | Kind::PipeWrite(_) => return Err(CELL_E_SPIPE),
    }
    if position > u32::MAX as i64 {
        return Err(CELL_E_OVERFLOW);
    }
    read_at(file, position as usize, buffer)
}

/// Write at an explicit offset without moving the descriptor's position.
pub fn pwrite(fd: i32, position: i64, buffer: &[u8]) -> SysResult<usize> {
    if position < 0 {
        return Err(CELL_E_INVAL);
    }
    let file = kind(fd);
    match file {
        Kind::File(_) | Kind::Logical | Kind::Console | Kind::Null => {}
        Kind::Closed => return Err(CELL_E_BADF),
        Kind::Dir(_) => return Err(CELL_E_ISDIR),
        Kind::PipeRead(_) | Kind::PipeWrite(_) => return Err(CELL_E_SPIPE),
    }
    if position > u32::MAX as i64 {
        return Err(CELL_E_OVERFLOW);
    }
    write_at(file, position as usize, buffer)
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

/// The inode number reported for directory entry `index` of directory `dir`.
///
/// The bring-up namespace has no on-disk inodes; this is a stable identity per
/// directory and slot so `d_ino` does not change between calls.
fn dir_ino(dir: u32, index: usize) -> u64 {
    dir as u64 * 64 + index as u64 + 1
}

/// Fill one native `dirent` record.  Returns the record length in bytes.
fn fill_dirent(dir: u32, index: usize, entry: &DirEntry, out: &mut [u8]) -> usize {
    let mut record = native_abi::dirent {
        d_ino: dir_ino(dir, index),
        d_off: index as i64 + 1,
        d_reclen: core::mem::size_of::<native_abi::dirent>() as u16,
        d_type: entry.kind,
        d_name: [0; 256],
    };
    let len = entry.name.len().min(record.d_name.len() - 1);
    record.d_name[..len].copy_from_slice(&entry.name[..len]);
    let size = core::mem::size_of::<native_abi::dirent>();
    let source = &record as *const native_abi::dirent as *const u8;
    for (position, slot) in out[..size].iter_mut().enumerate() {
        *slot = unsafe { core::ptr::read_volatile(source.add(position)) };
    }
    size
}

/// `getdents`: fill whole `dirent` records and advance the directory cookie.
///
/// `d_off` is the cookie of the next entry (`index + 1`), so the last record
/// leaves the descriptor at end-of-directory.  `lseek` writes the same value
/// back, which is what makes `telldir`/`seekdir` work.  A buffer too small for
/// one record is `EINVAL`, which is the signal relibc's `DIR` uses to grow.
pub fn getdents(fd: i32, out: &mut [u8]) -> SysResult<usize> {
    let dir = match kind(fd) {
        Kind::Dir(dir) => dir,
        Kind::Closed => return Err(CELL_E_BADF),
        _ => return Err(CELL_E_NOTDIR),
    };
    let entries = DIRECTORIES[dir as usize];
    let record = core::mem::size_of::<native_abi::dirent>();
    let mut index = offset(fd);
    let mut written = 0usize;
    while index < entries.len() && written + record <= out.len() {
        written += fill_dirent(dir, index, &entries[index], &mut out[written..]);
        index += 1;
    }
    if written == 0 && index < entries.len() {
        return Err(CELL_E_INVAL);
    }
    set_offset(fd, index);
    Ok(written)
}
