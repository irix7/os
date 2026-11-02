// SPDX-License-Identifier: GPL-3.0-only
//! Minimal per-process descriptor table for the bring-up userland.
//!
//! Console, `/dev/null`, the mutable [`crate::ramfs`] tree and a small pipe
//! ring.  This is a bring-up namespace, not the capability-backed VFS.
//!
//! A descriptor is a table slot that names an open file description.  `dup`
//! and `dup2` share that description, so a seek or read through one descriptor
//! moves the offset seen by the other, as POSIX requires.  Close-on-exec is
//! per descriptor, not per description.

use crate::ramfs::{self, NodeType, Resolved};
use crate::syserr::{
    SysResult, CELL_E_AGAIN, CELL_E_BADF, CELL_E_BUSY, CELL_E_INVAL, CELL_E_ISDIR, CELL_E_MFILE,
    CELL_E_NAMETOOLONG, CELL_E_NOENT, CELL_E_NOTDIR, CELL_E_NOTEMPTY, CELL_E_NOTTY, CELL_E_NXIO,
    CELL_E_OVERFLOW, CELL_E_PIPE, CELL_E_ROFS, CELL_E_SPIPE,
};

pub const MAX_FDS: usize = 16;
const MAX_OPEN_FILES: usize = 16;
const O_RDONLY: u32 = native_abi::O_RDONLY as u32;
const O_WRONLY: u32 = native_abi::O_WRONLY as u32;
const O_RDWR: u32 = native_abi::O_RDWR as u32;
const O_ACCMODE: u32 = native_abi::O_ACCMODE as u32;
const O_CREAT: u32 = native_abi::O_CREAT as u32;
const O_EXCL: u32 = native_abi::O_EXCL as u32;
const O_TRUNC: u32 = native_abi::O_TRUNC as u32;
const O_APPEND: u32 = native_abi::O_APPEND as u32;
const O_NONBLOCK: u32 = native_abi::O_NONBLOCK as u32;
const O_DIRECTORY: u32 = native_abi::O_DIRECTORY as u32;
const O_CLOEXEC: u32 = native_abi::O_CLOEXEC as u32;
const FD_CLOEXEC: u32 = 1;
const F_GETFD: u32 = 1;
const F_SETFD: u32 = 2;
const F_GETFL: u32 = 3;
const F_SETFL: u32 = 4;
const SEEK_SET: u32 = 0;
const SEEK_CUR: u32 = 1;
const SEEK_END: u32 = 2;
const SEEK_DATA: u32 = 3;
const SEEK_HOLE: u32 = 4;
const MAX_PIPES: usize = 8;
const PIPE_CAPACITY: usize = 4096;

/// The longest a "blocking" read or write waits before returning `EAGAIN`.
///
/// The kernel is single-threaded: no other thread can run while a descriptor
/// wait is in progress, so an unbounded wait would hang the machine rather
/// than make progress.  The wait is a real readiness poll (console input and
/// pipe state are rechecked) with a fixed upper bound.  `poll` uses the same
/// bound for a negative timeout.
pub const WAIT_LIMIT_MS: u32 = 100;
/// CP0 Count ticks per millisecond at the Indy's 33 MHz rate.
pub const TICKS_PER_MS: u32 = 33_000;
const WAIT_LIMIT_TICKS: u32 = WAIT_LIMIT_MS * TICKS_PER_MS;

static mut PIPE_DATA: [[u8; PIPE_CAPACITY]; MAX_PIPES] = [[0; PIPE_CAPACITY]; MAX_PIPES];
static mut PIPE_HEAD: [usize; MAX_PIPES] = [0; MAX_PIPES];
static mut PIPE_LEN: [usize; MAX_PIPES] = [0; MAX_PIPES];
static mut PIPE_USED: [bool; MAX_PIPES] = [false; MAX_PIPES];
static mut CONSOLE_WINSIZE: [u16; 4] = [24, 80, 0, 0];

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Closed,
    Console,
    Null,
    File(u16),
    Logical(u16),
    Dir(u16),
    PipeRead(u32),
    PipeWrite(u32),
}

/// Open file descriptions.  `Kind::Closed` marks a free entry.
static mut OPEN_KIND: [Kind; MAX_OPEN_FILES] = [Kind::Closed; MAX_OPEN_FILES];
static mut OPEN_OFFSET: [usize; MAX_OPEN_FILES] = [0; MAX_OPEN_FILES];
static mut OPEN_REFS: [u8; MAX_OPEN_FILES] = [0; MAX_OPEN_FILES];
/// Per-description status flags: the access mode and `O_NONBLOCK`.  They
/// belong to the description, so `dup` shares them, as POSIX requires.
static mut OPEN_STATUS: [u32; MAX_OPEN_FILES] = [0; MAX_OPEN_FILES];

/// Descriptor slots: the description index, or -1 when the slot is free.
static mut FD_OPEN: [i32; MAX_FDS] = [-1; MAX_FDS];
static mut FD_FLAGS: [u32; MAX_FDS] = [0; MAX_FDS];
/// The current working directory as an inode in the ramfs tree.
static mut CWD_NODE: u16 = ramfs::ROOT;

/// One process's descriptor slots and working directory.
///
/// The open file descriptions and pipes are system-wide objects; a descriptor
/// names one of them.  `fork` copies this table and takes a reference on every
/// description, so the child shares offsets and pipe state exactly as POSIX
/// requires while its own slots and cwd are independent.
#[derive(Clone, Copy)]
pub struct Snapshot {
    open: [i32; MAX_FDS],
    flags: [u32; MAX_FDS],
    cwd: u16,
}

impl Snapshot {
    pub const EMPTY: Self = Self {
        open: [-1; MAX_FDS],
        flags: [0; MAX_FDS],
        cwd: ramfs::ROOT,
    };
}

/// Reset the system-wide descriptor machinery and the ramfs tree.
///
/// This is the console's world reset, not a per-process operation.  It clears
/// every open description and pipe; call [`console_table`] to build the first
/// process's standard descriptors.
pub fn reset() {
    ramfs::reset();
    unsafe {
        FD_OPEN = [-1; MAX_FDS];
        FD_FLAGS = [0; MAX_FDS];
        OPEN_KIND = [Kind::Closed; MAX_OPEN_FILES];
        OPEN_OFFSET = [0; MAX_OPEN_FILES];
        OPEN_REFS = [0; MAX_OPEN_FILES];
        OPEN_STATUS = [0; MAX_OPEN_FILES];
        CWD_NODE = ramfs::ROOT;
        PIPE_HEAD = [0; MAX_PIPES];
        PIPE_LEN = [0; MAX_PIPES];
        PIPE_USED = [false; MAX_PIPES];
        CONSOLE_WINSIZE = [24, 80, 0, 0];
    }
}

/// Build the standard console table for a new root process and make it live.
pub fn console_table() -> Snapshot {
    unsafe {
        FD_OPEN = [-1; MAX_FDS];
        FD_FLAGS = [0; MAX_FDS];
        CWD_NODE = ramfs::ROOT;
        for fd in 0..3 {
            if let Some(description) = open_description(Kind::Console, O_RDWR) {
                FD_OPEN[fd] = description as i32;
            }
            FD_FLAGS[fd] = 0;
        }
    }
    snapshot()
}

/// Copy the live descriptor table without changing its reference counts.
pub fn snapshot() -> Snapshot {
    // SAFETY: single-threaded kernel; the live table is not modified during the
    // copy.
    unsafe {
        Snapshot {
            open: FD_OPEN,
            flags: FD_FLAGS,
            cwd: CWD_NODE,
        }
    }
}

/// Make `table` the live descriptor table.
pub fn restore(table: Snapshot) {
    // SAFETY: single-threaded kernel; the table belongs to the process this
    // address space and TLB set are being switched to.
    unsafe {
        FD_OPEN = table.open;
        FD_FLAGS = table.flags;
        CWD_NODE = table.cwd;
    }
}

/// Snapshot the live table for a fork and take a reference on every open
/// description.  The child shares descriptions; close-on-exec stays per slot.
pub fn fork_snapshot() -> Snapshot {
    let table = snapshot();
    for index in 0..MAX_FDS {
        if table.open[index] >= 0 {
            let _ = retain_description(table.open[index] as usize);
        }
    }
    table
}

/// Drop every reference a dead process's table holds.
pub fn release_snapshot(table: &Snapshot) {
    for index in 0..MAX_FDS {
        if table.open[index] >= 0 {
            release_description(table.open[index] as usize);
        }
    }
}

/// Allocate a fresh open file description.  The caller owns its single ref.
fn open_description(kind: Kind, status: u32) -> Option<usize> {
    unsafe {
        for index in 0..MAX_OPEN_FILES {
            if OPEN_KIND[index] == Kind::Closed && OPEN_REFS[index] == 0 {
                OPEN_KIND[index] = kind;
                OPEN_OFFSET[index] = 0;
                OPEN_REFS[index] = 1;
                OPEN_STATUS[index] = status;
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
            OPEN_STATUS[index] = 0;
            if let Kind::PipeRead(pipe) | Kind::PipeWrite(pipe) = kind {
                release_pipe(pipe as usize);
            }
            if let Kind::File(node) | Kind::Dir(node) = kind {
                // POSIX keeps an unlinked inode alive until the last
                // descriptor to it closes.
                if !node_referenced(node) {
                    ramfs::release(node);
                }
            }
        }
    }
}

/// Whether any open file description still names this ramfs inode.
fn node_referenced(node: u16) -> bool {
    unsafe {
        (0..MAX_OPEN_FILES).any(|index| {
            OPEN_REFS[index] > 0
                && matches!(
                    OPEN_KIND[index],
                    Kind::File(other) | Kind::Dir(other) if other == node
                )
        })
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

fn install_flags(kind: Kind, descriptor_flags: u32, status_flags: u32) -> SysResult<i32> {
    let description = open_description(kind, status_flags).ok_or(CELL_E_MFILE)?;
    match free_descriptor() {
        Some(fd) => {
            unsafe {
                FD_OPEN[fd as usize] = description as i32;
                FD_FLAGS[fd as usize] = descriptor_flags;
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

/// Create a pipe with `O_CLOEXEC` and `O_NONBLOCK` applied to both descriptors.
pub fn pipe2(flags: u32) -> SysResult<(i32, i32)> {
    let close_on_exec = pipe_flags(flags)?;
    let nonblock = flags & O_NONBLOCK;
    unsafe {
        let Some(index) = (0..MAX_PIPES).find(|index| !PIPE_USED[*index]) else {
            return Err(CELL_E_MFILE);
        };
        PIPE_USED[index] = true;
        PIPE_HEAD[index] = 0;
        PIPE_LEN[index] = 0;
        let read = match install_flags(
            Kind::PipeRead(index as u32),
            close_on_exec,
            O_RDONLY | nonblock,
        ) {
            Ok(fd) => fd,
            Err(status) => {
                PIPE_USED[index] = false;
                return Err(status);
            }
        };
        match install_flags(
            Kind::PipeWrite(index as u32),
            close_on_exec,
            O_WRONLY | nonblock,
        ) {
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

pub(crate) fn cwd() -> u16 {
    unsafe { CWD_NODE }
}

fn is_directory(node: u16) -> bool {
    ramfs::is_dir(node)
}

fn file_mode(mode: u32) -> u32 {
    let permission = mode & 0o777;
    0o100000 | if permission == 0 { 0o644 } else { permission }
}

fn directory_mode(mode: u32) -> u32 {
    let permission = mode & 0o777;
    0o040000 | if permission == 0 { 0o755 } else { permission }
}

/// The `Kind` an inode opens as.
/// `/etc/motd` exists only while the live policy exposes the file.
fn logical_available() -> bool {
    crate::logical::open_file(b"/etc/motd")
}

fn kind_for(node: u16) -> Kind {
    match ramfs::node_type(node) {
        NodeType::File | NodeType::Image => Kind::File(node),
        NodeType::Dir => Kind::Dir(node),
        NodeType::Logical => Kind::Logical(node),
        NodeType::Null => Kind::Null,
        NodeType::Console => Kind::Console,
    }
}

pub fn set_cwd(path: &[u8]) -> SysResult<()> {
    let node = match ramfs::resolve(cwd(), path)? {
        Resolved::Node(node) => node,
        Resolved::Missing { .. } => return Err(CELL_E_NOENT),
    };
    if !is_directory(node) {
        return Err(CELL_E_NOTDIR);
    }
    unsafe {
        CWD_NODE = node;
    }
    Ok(())
}

/// The absolute path of the current directory, or `None` when it does not
/// fit the caller's buffer.
pub fn copy_cwd(out: &mut [u8; 64]) -> Option<usize> {
    ramfs::path_of(cwd(), out)
}

/// `open`/`openat` after the libc resolved `dirfd`.  `mode` is used only when
/// `O_CREAT` creates a regular file.
pub fn open_path(path: &[u8], flags: u32, mode: u32) -> SysResult<i32> {
    let access = flags & O_ACCMODE;
    if access == O_ACCMODE {
        return Err(CELL_E_INVAL);
    }
    let node = match ramfs::resolve(cwd(), path)? {
        Resolved::Node(node) => {
            if flags & O_CREAT != 0 && flags & O_EXCL != 0 {
                return Err(crate::syserr::CELL_E_EXIST);
            }
            node
        }
        Resolved::Missing { parent, name } => {
            if flags & O_CREAT == 0 {
                return Err(CELL_E_NOENT);
            }
            ramfs::create(parent, name, false, file_mode(mode))?
        }
    };
    if ramfs::node_type(node) == NodeType::Logical && !logical_available() {
        return Err(CELL_E_NOENT);
    }
    if flags & O_DIRECTORY != 0 && !is_directory(node) {
        return Err(CELL_E_NOTDIR);
    }
    if is_directory(node) {
        if access != O_RDONLY {
            return Err(CELL_E_ISDIR);
        }
        if flags & O_TRUNC != 0 {
            return Err(CELL_E_ISDIR);
        }
    } else if flags & O_TRUNC != 0 {
        // POSIX truncates regular files only; O_TRUNC on a device is a no-op.
        match ramfs::node_type(node) {
            NodeType::File => ramfs::truncate(node, 0)?,
            NodeType::Logical | NodeType::Image => return Err(CELL_E_ROFS),
            NodeType::Null | NodeType::Console => {}
            NodeType::Dir => return Err(CELL_E_ISDIR),
        }
    }
    let close_on_exec = if flags & O_CLOEXEC != 0 {
        FD_CLOEXEC
    } else {
        0
    };
    let status = access | (flags & (O_NONBLOCK | O_APPEND));
    install_flags(kind_for(node), close_on_exec, status)
}

/// `unlink` (or `rmdir` when `remove_dir` is set).  The inode is reclaimed
/// immediately unless an open descriptor still refers to it.
pub fn unlink(path: &[u8], remove_dir: bool) -> SysResult<()> {
    let node = match ramfs::resolve(cwd(), path)? {
        Resolved::Node(node) => node,
        Resolved::Missing { .. } => return Err(CELL_E_NOENT),
    };
    if node == ramfs::ROOT {
        return Err(CELL_E_BUSY);
    }
    let directory = is_directory(node);
    if remove_dir {
        if !directory {
            return Err(CELL_E_NOTDIR);
        }
        if node == cwd() {
            // Removing the working directory would leave getcwd with no path
            // to report.  POSIX allows EBUSY here.
            return Err(CELL_E_BUSY);
        }
        if !ramfs::is_empty_dir(node) {
            return Err(CELL_E_NOTEMPTY);
        }
    } else if directory {
        return Err(CELL_E_ISDIR);
    }
    let removed = ramfs::unlink(node);
    if !node_referenced(removed) {
        ramfs::release(removed);
    }
    Ok(())
}

/// `mkdir`.  The name must be new; the parent must be a directory.
pub fn mkdir(path: &[u8], mode: u32) -> SysResult<()> {
    match ramfs::resolve(cwd(), path)? {
        Resolved::Node(_) => Err(crate::syserr::CELL_E_EXIST),
        Resolved::Missing { parent, name } => {
            ramfs::create(parent, name, true, directory_mode(mode)).map(|_| ())
        }
    }
}

/// `rename`.  An existing destination is replaced when the types agree.
pub fn rename(old_path: &[u8], new_path: &[u8]) -> SysResult<()> {
    let old = match ramfs::resolve(cwd(), old_path)? {
        Resolved::Node(node) => node,
        Resolved::Missing { .. } => return Err(CELL_E_NOENT),
    };
    let mut name = [0u8; ramfs::NAME_MAX];
    let (parent, length) = match ramfs::resolve(cwd(), new_path)? {
        Resolved::Node(node) => {
            if node == ramfs::ROOT {
                return Err(CELL_E_INVAL);
            }
            (ramfs::parent(node), ramfs::name(node, &mut name))
        }
        Resolved::Missing {
            parent,
            name: component,
        } => {
            if component.len() > name.len() {
                return Err(CELL_E_NAMETOOLONG);
            }
            name[..component.len()].copy_from_slice(component);
            (parent, component.len())
        }
    };
    if length == 0 {
        return Err(CELL_E_INVAL);
    }
    if let Some(replaced) = ramfs::rename(old, parent, &name[..length])? {
        if !node_referenced(replaced) {
            ramfs::release(replaced);
        }
    }
    Ok(())
}

/// `truncate` on a descriptor.
pub fn ftruncate(fd: i32, length: i64) -> SysResult<()> {
    if length < 0 {
        return Err(CELL_E_INVAL);
    }
    match kind(fd) {
        Kind::Closed => Err(CELL_E_BADF),
        Kind::File(node) => ramfs::truncate(node, length as usize),
        Kind::Dir(_) => Err(CELL_E_ISDIR),
        Kind::Logical(_) => Err(CELL_E_ROFS),
        Kind::PipeRead(_) | Kind::PipeWrite(_) | Kind::Null | Kind::Console => Err(CELL_E_INVAL),
    }
}

/// `truncate` on a path.
pub fn truncate(path: &[u8], length: i64) -> SysResult<()> {
    if length < 0 {
        return Err(CELL_E_INVAL);
    }
    let node = match ramfs::resolve(cwd(), path)? {
        Resolved::Node(node) => node,
        Resolved::Missing { .. } => return Err(CELL_E_NOENT),
    };
    ramfs::truncate(node, length as usize)
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

/// Close every descriptor marked close-on-exec.  `execve` calls this before it
/// installs the new image; the open file descriptions are released when the
/// last reference goes.
pub fn close_cloexec() {
    for fd in 0..MAX_FDS {
        if unsafe { FD_FLAGS[fd] } & FD_CLOEXEC != 0 {
            close_slot(fd as i32);
        }
    }
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

/// `fcntl` for the descriptor and status flags this bring-up keeps.
///
/// `F_GETFD`/`F_SETFD` are per descriptor (close-on-exec).  `F_GETFL`/
/// `F_SETFL` are per open file description: they hold the access mode and
/// `O_NONBLOCK`, and `dup` shares them.  Unknown commands are `EINVAL`, not a
/// silent success: there is no lock or owner state to report, and claiming
/// otherwise would misrepresent the table.
pub fn fcntl(fd: i32, cmd: u32, arg: u32) -> SysResult<i32> {
    let Some(description) = description(fd) else {
        return Err(CELL_E_BADF);
    };
    match cmd {
        F_GETFD => Ok(unsafe { FD_FLAGS[fd as usize] } as i32),
        F_SETFD => {
            unsafe {
                FD_FLAGS[fd as usize] = arg & FD_CLOEXEC;
            }
            Ok(0)
        }
        F_GETFL => Ok(unsafe { OPEN_STATUS[description] } as i32),
        F_SETFL => {
            let access = unsafe { OPEN_STATUS[description] } & O_ACCMODE;
            unsafe {
                OPEN_STATUS[description] = access | (arg & (O_NONBLOCK | O_APPEND));
            }
            Ok(0)
        }
        _ => Err(CELL_E_INVAL),
    }
}

/// The recorded status flags of a descriptor's open file description.
fn status_flags(fd: i32) -> u32 {
    match description(fd) {
        Some(index) => unsafe { OPEN_STATUS[index] },
        None => 0,
    }
}

/// Whether the descriptor was opened (or set) with `O_NONBLOCK`.
pub fn nonblocking(fd: i32) -> bool {
    status_flags(fd) & O_NONBLOCK != 0
}

/// The size `SEEK_END`, `SEEK_DATA` and `SEEK_HOLE` measure against.  A
/// directory reports its `.` and `..` records plus its children.
fn length_of(kind: Kind) -> Option<usize> {
    match kind {
        Kind::File(node) => Some(ramfs::size(node)),
        Kind::Logical(_) => Some(crate::logical::file_size(b"/etc/motd").unwrap_or(0)),
        Kind::Dir(node) => Some(2 + ramfs::children(node)),
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
        if !matches!(file, Kind::File(_) | Kind::Logical(_)) {
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
        Kind::Logical(_) => Ok(crate::logical::read_file(b"/etc/motd", start, buffer).unwrap_or(0)),
        Kind::File(node) => ramfs::read(node, start, buffer),
        Kind::Console | Kind::Null => Ok(0),
        Kind::Dir(_) => Err(CELL_E_ISDIR),
        Kind::PipeRead(_) | Kind::PipeWrite(_) => Err(CELL_E_SPIPE),
        Kind::Closed => Err(CELL_E_BADF),
    }
}

fn write_at(kind: Kind, start: usize, buffer: &[u8]) -> SysResult<usize> {
    match kind {
        Kind::File(node) => ramfs::write(node, start, buffer),
        Kind::Null | Kind::Console => Ok(buffer.len()),
        Kind::Dir(_) => Err(CELL_E_ISDIR),
        Kind::Logical(_) => Err(CELL_E_ROFS),
        Kind::PipeRead(_) | Kind::PipeWrite(_) => Err(CELL_E_SPIPE),
        Kind::Closed => Err(CELL_E_BADF),
    }
}

/// Wait for `ready` until the fixed kernel bound.  Returns the final state.
///
/// The kernel has no scheduler, so this is the longest wait any descriptor
/// operation performs.  Interrupts are masked by EXL for the whole syscall,
/// but CP0 Count still advances.
fn wait_for(mut ready: impl FnMut() -> bool) -> bool {
    let start = crate::traps::count_read();
    loop {
        if ready() {
            return true;
        }
        if crate::traps::count_read().wrapping_sub(start) >= WAIT_LIMIT_TICKS {
            return ready();
        }
        core::hint::spin_loop();
    }
}

fn read_pipe(index: usize, buffer: &mut [u8], nonblocking: bool) -> SysResult<usize> {
    if buffer.is_empty() {
        return Ok(0);
    }
    let mut len = unsafe { PIPE_LEN[index] };
    if len == 0 {
        // With no writer left the pipe is at end-of-file.  Otherwise a
        // non-blocking read returns EAGAIN and a blocking one waits for the
        // bounded kernel interval before doing the same.
        if !pipe_has_writer(index) {
            return Ok(0);
        }
        if !nonblocking
            && !wait_for(|| (unsafe { PIPE_LEN[index] != 0 }) || !pipe_has_writer(index))
        {
            return Err(CELL_E_AGAIN);
        }
        len = unsafe { PIPE_LEN[index] };
        if len == 0 {
            return if pipe_has_writer(index) {
                Err(CELL_E_AGAIN)
            } else {
                Ok(0)
            };
        }
    }
    let count = buffer.len().min(len);
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

fn write_pipe(index: usize, buffer: &[u8], nonblocking: bool) -> SysResult<usize> {
    if buffer.is_empty() {
        return Ok(0);
    }
    if !pipe_has_reader(index) {
        return Err(CELL_E_PIPE);
    }
    let mut len = unsafe { PIPE_LEN[index] };
    let mut free = PIPE_CAPACITY.saturating_sub(len);
    if free == 0 {
        if !nonblocking
            && !wait_for(|| !pipe_has_reader(index) || unsafe { PIPE_LEN[index] } < PIPE_CAPACITY)
        {
            return Err(CELL_E_AGAIN);
        }
        if !pipe_has_reader(index) {
            return Err(CELL_E_PIPE);
        }
        len = unsafe { PIPE_LEN[index] };
        free = PIPE_CAPACITY.saturating_sub(len);
        if free == 0 {
            return Err(CELL_E_AGAIN);
        }
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

/// Read the console as a byte stream, honouring `O_NONBLOCK`.
fn read_console(buffer: &mut [u8], nonblocking: bool) -> SysResult<usize> {
    if buffer.is_empty() {
        return Ok(0);
    }
    let count = crate::kbd::read_input(buffer);
    if count != 0 {
        return Ok(count);
    }
    if nonblocking {
        return Err(CELL_E_AGAIN);
    }
    if !wait_for(crate::kbd::input_ready) {
        return Err(CELL_E_AGAIN);
    }
    let count = crate::kbd::read_input(buffer);
    if count == 0 {
        Err(CELL_E_AGAIN)
    } else {
        Ok(count)
    }
}

/// Readiness for `poll`.
///
/// `revents` carries only the requested events, plus `POLLERR`/`POLLHUP`/
/// `POLLNVAL`, which POSIX always reports.  A negative descriptor is ignored
/// and reports nothing.
pub fn poll_events(fd: i32, events: i16) -> i16 {
    use native_abi::{POLLERR, POLLHUP, POLLIN, POLLNVAL, POLLOUT};

    if fd < 0 {
        return 0;
    }
    let mut revents = 0i16;
    match kind(fd) {
        Kind::Closed => return POLLNVAL,
        Kind::PipeRead(index) => {
            let index = index as usize;
            let len = unsafe { PIPE_LEN[index] };
            if len != 0 && events & POLLIN != 0 {
                revents |= POLLIN;
            }
            if !pipe_has_writer(index) {
                revents |= POLLHUP;
            }
        }
        Kind::PipeWrite(index) => {
            let index = index as usize;
            if !pipe_has_reader(index) {
                revents |= POLLERR;
            }
            if unsafe { PIPE_LEN[index] } < PIPE_CAPACITY && events & POLLOUT != 0 {
                revents |= POLLOUT;
            }
        }
        Kind::Console => {
            if events & POLLIN != 0 && crate::kbd::input_ready() {
                revents |= POLLIN;
            }
            if events & POLLOUT != 0 {
                revents |= POLLOUT;
            }
        }
        // /dev/null and regular files are always ready: a read either returns
        // data or end-of-file without blocking.
        Kind::Null | Kind::File(_) | Kind::Logical(_) => {
            revents |= events & (POLLIN | POLLOUT);
        }
        Kind::Dir(_) => {
            revents |= events & POLLIN;
        }
    }
    revents
}

fn readable(fd: i32) -> bool {
    status_flags(fd) & O_ACCMODE != O_WRONLY
}

fn writable(fd: i32) -> bool {
    status_flags(fd) & O_ACCMODE != O_RDONLY
}

pub fn read(fd: i32, buffer: &mut [u8]) -> SysResult<usize> {
    let file = kind(fd);
    if file == Kind::Closed {
        return Err(CELL_E_BADF);
    }
    if !readable(fd) {
        return Err(CELL_E_BADF);
    }
    let nonblocking = nonblocking(fd);
    match file {
        Kind::PipeRead(index) => return read_pipe(index as usize, buffer, nonblocking),
        Kind::Console => return read_console(buffer, nonblocking),
        _ => {}
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
    if !writable(fd) {
        return Err(CELL_E_BADF);
    }
    if let Kind::PipeWrite(index) = file {
        return write_pipe(index as usize, buffer, nonblocking(fd));
    }
    // O_APPEND writes at the current end of file, not at the saved offset.
    let start = if status_flags(fd) & O_APPEND != 0 {
        length_of(file).unwrap_or(0)
    } else {
        offset(fd)
    };
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
        Kind::File(_) | Kind::Logical(_) | Kind::Console | Kind::Null => {}
        Kind::Closed => return Err(CELL_E_BADF),
        Kind::Dir(_) => return Err(CELL_E_ISDIR),
        Kind::PipeRead(_) | Kind::PipeWrite(_) => return Err(CELL_E_SPIPE),
    }
    if !readable(fd) {
        return Err(CELL_E_BADF);
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
        Kind::File(_) | Kind::Logical(_) | Kind::Console | Kind::Null => {}
        Kind::Closed => return Err(CELL_E_BADF),
        Kind::Dir(_) => return Err(CELL_E_ISDIR),
        Kind::PipeRead(_) | Kind::PipeWrite(_) => return Err(CELL_E_SPIPE),
    }
    if !writable(fd) {
        return Err(CELL_E_BADF);
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

/// `(mode, size, inode)` for a descriptor.
pub fn metadata(fd: i32) -> SysResult<(u32, i64, u64)> {
    match kind(fd) {
        Kind::File(node) => Ok((
            ramfs::mode(node),
            ramfs::size(node) as i64,
            ramfs::inode(node),
        )),
        Kind::Logical(node) => Ok((
            0o100644,
            crate::logical::file_size(b"/etc/motd").unwrap_or(0) as i64,
            ramfs::inode(node),
        )),
        Kind::Console | Kind::Null => Ok((0o020666, 0, 0)),
        Kind::Dir(node) => Ok((ramfs::mode(node), 0, ramfs::inode(node))),
        Kind::PipeRead(_) | Kind::PipeWrite(_) => Ok((0o010600, 0, 0)),
        Kind::Closed => Err(CELL_E_BADF),
    }
}

pub fn metadata_path(path: &[u8]) -> SysResult<(u32, i64, u64)> {
    let node = match ramfs::resolve(cwd(), path)? {
        Resolved::Node(node) => node,
        Resolved::Missing { .. } => return Err(CELL_E_NOENT),
    };
    if ramfs::node_type(node) == NodeType::Logical && !logical_available() {
        return Err(CELL_E_NOENT);
    }
    Ok((
        ramfs::mode(node),
        ramfs::size(node) as i64,
        ramfs::inode(node),
    ))
}

/// Fill `ordinal` of a directory into `out`, without a terminator.  Returns
/// `(inode, d_type, name length)`; ordinal 0 is `.` and 1 is `..`.
fn dir_entry(dir: u16, ordinal: usize, out: &mut [u8]) -> Option<(u64, u8, usize)> {
    match ordinal {
        0 => {
            if out.is_empty() {
                return None;
            }
            out[0] = b'.';
            Some((ramfs::inode(dir), ramfs::TYPE_DIR, 1))
        }
        1 => {
            if out.len() < 2 {
                return None;
            }
            out[0] = b'.';
            out[1] = b'.';
            Some((ramfs::inode(ramfs::parent(dir)), ramfs::TYPE_DIR, 2))
        }
        _ => {
            let node = ramfs::child_at(dir, ordinal - 2)?;
            Some((
                ramfs::inode(node),
                ramfs::type_of(node),
                ramfs::name(node, out),
            ))
        }
    }
}

pub fn readdir(fd: i32, index: usize, out: &mut [u8]) -> SysResult<Option<(u8, usize)>> {
    let dir = match kind(fd) {
        Kind::Dir(dir) => dir,
        Kind::Closed => return Err(CELL_E_BADF),
        _ => return Err(CELL_E_NOTDIR),
    };
    if out.is_empty() {
        return Err(CELL_E_INVAL);
    }
    let Some((_, kind, len)) = dir_entry(dir, index, out) else {
        return Ok(None);
    };
    let len = len.min(out.len() - 1);
    out[len] = 0;
    Ok(Some((kind, len)))
}

/// Fill one native `dirent` record.  Returns the record length in bytes.
fn fill_dirent(ino: u64, index: usize, kind: u8, name: &[u8], out: &mut [u8]) -> usize {
    let mut record = native_abi::dirent {
        d_ino: ino,
        d_off: index as i64 + 1,
        d_reclen: core::mem::size_of::<native_abi::dirent>() as u16,
        d_type: kind,
        d_name: [0; 256],
    };
    let len = name.len().min(record.d_name.len() - 1);
    record.d_name[..len].copy_from_slice(&name[..len]);
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
/// The walk sees the live tree: entries created or removed between calls are
/// reflected in the scan order.
pub fn getdents(fd: i32, out: &mut [u8]) -> SysResult<usize> {
    let dir = match kind(fd) {
        Kind::Dir(dir) => dir,
        Kind::Closed => return Err(CELL_E_BADF),
        _ => return Err(CELL_E_NOTDIR),
    };
    let total = 2 + ramfs::children(dir);
    let record = core::mem::size_of::<native_abi::dirent>();
    let mut index = offset(fd);
    let mut written = 0usize;
    let mut name = [0u8; ramfs::NAME_MAX];
    while index < total && written + record <= out.len() {
        let Some((ino, kind, len)) = dir_entry(dir, index, &mut name) else {
            break;
        };
        written += fill_dirent(ino, index, kind, &name[..len], &mut out[written..]);
        index += 1;
    }
    if written == 0 && index < total {
        return Err(CELL_E_INVAL);
    }
    set_offset(fd, index);
    Ok(written)
}
