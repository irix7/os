// SPDX-License-Identifier: GPL-3.0-only
//! Provisional syscall ABI shared with the kernel.
//!
//! Numbers are local to the bring-up ABI and will be replaced by the native
//! capability ABI when relibc's `irix7` backend lands.

pub const SYS_WRITE: usize = 1;
pub const SYS_EXIT: usize = 2;
pub const SYS_MMAP: usize = 3;
pub const SYS_MUNMAP: usize = 4;
pub const SYS_MPROTECT: usize = 5;
pub const SYS_OPEN: usize = 6;
pub const SYS_CLOSE: usize = 7;
pub const SYS_FCNTL: usize = 8;
pub const SYS_DUP: usize = 9;
pub const SYS_GETCWD: usize = 10;
pub const SYS_POLL: usize = 11;
pub const SYS_SIGNAL: usize = 12;
pub const SYS_GETENTROPY: usize = 13;
pub const SYS_CLOCK_GETTIME: usize = 14;
pub const SYS_GETPID: usize = 15;
pub const SYS_READ: usize = 16;
pub const SYS_FSTAT: usize = 17;
pub const SYS_STAT: usize = 18;

#[inline(always)]
pub unsafe fn syscall0(n: usize) -> usize {
    let result: usize;
    core::arch::asm!(
        "syscall",
        inout("$2") n => result,
        out("$4") _,
        out("$5") _,
        out("$6") _,
        out("$7") _,
        options(nostack),
    );
    result
}

#[inline(always)]
pub unsafe fn syscall1(n: usize, a0: usize) -> usize {
    let result: usize;
    core::arch::asm!(
        "syscall",
        inout("$2") n => result,
        inout("$4") a0 => _,
        out("$5") _,
        out("$6") _,
        out("$7") _,
        options(nostack),
    );
    result
}

#[inline(always)]
pub unsafe fn syscall2(n: usize, a0: usize, a1: usize) -> usize {
    let result: usize;
    core::arch::asm!(
        "syscall",
        inout("$2") n => result,
        inout("$4") a0 => _,
        inout("$5") a1 => _,
        out("$6") _,
        out("$7") _,
        options(nostack),
    );
    result
}

#[inline(always)]
pub unsafe fn syscall3(n: usize, a0: usize, a1: usize, a2: usize) -> usize {
    let result: usize;
    core::arch::asm!(
        "syscall",
        inout("$2") n => result,
        inout("$4") a0 => _,
        inout("$5") a1 => _,
        inout("$6") a2 => _,
        out("$7") _,
        options(nostack),
    );
    result
}

#[inline(always)]
pub unsafe fn syscall4(n: usize, a0: usize, a1: usize, a2: usize, a3: usize) -> usize {
    let result: usize;
    core::arch::asm!(
        "syscall",
        inout("$2") n => result,
        inout("$4") a0 => _,
        inout("$5") a1 => _,
        inout("$6") a2 => _,
        inout("$7") a3 => _,
        options(nostack),
    );
    result
}

use crate::mem;

/// C `errno` storage.  A single-threaded bring-up layout; per-thread errno
/// arrives with the real pthread implementation.
static mut ERRNO: i32 = 0;

#[no_mangle]
pub unsafe extern "C" fn __errno_location() -> *mut i32 {
    core::ptr::addr_of_mut!(ERRNO)
}

#[repr(C)]
pub struct Timespec {
    pub tv_sec: i64,
    pub tv_nsec: i32,
    pub _pad: u32,
}

#[repr(C)]
pub struct Iovec {
    pub iov_base: *mut u8,
    pub iov_len: usize,
}

#[repr(C)]
pub struct PollFd {
    pub fd: i32,
    pub events: i16,
    pub revents: i16,
}

#[no_mangle]
pub unsafe extern "C" fn write(fd: i32, buf: *const u8, count: usize) -> isize {
    let result = syscall3(SYS_WRITE, fd as usize, buf as usize, count);
    if result == usize::MAX {
        *__errno_location() = 5;
        -1
    } else {
        result as isize
    }
}

#[no_mangle]
pub unsafe extern "C" fn read(fd: i32, buf: *mut u8, count: usize) -> isize {
    let result = syscall3(SYS_READ, fd as usize, buf as usize, count);
    if result == usize::MAX {
        *__errno_location() = 5;
        -1
    } else {
        result as isize
    }
}

#[no_mangle]
pub unsafe extern "C" fn writev(fd: i32, iov: *const Iovec, count: i32) -> isize {
    let mut total: isize = 0;
    for index in 0..count.max(0) as usize {
        let vector = &*iov.add(index);
        let written = write(fd, vector.iov_base, vector.iov_len);
        if written < 0 {
            return if total > 0 { total } else { written };
        }
        total += written;
        if written as usize != vector.iov_len {
            break;
        }
    }
    total
}

#[no_mangle]
pub unsafe extern "C" fn open(path: *const u8, flags: i32, _mode: i32) -> i32 {
    let result = syscall3(SYS_OPEN, path as usize, flags as usize, 0);
    if result == usize::MAX {
        *__errno_location() = 2;
        -1
    } else {
        result as i32
    }
}

#[no_mangle]
pub unsafe extern "C" fn close(fd: i32) -> i32 {
    let result = syscall1(SYS_CLOSE, fd as usize);
    if result == usize::MAX {
        *__errno_location() = 9;
        -1
    } else {
        0
    }
}

#[no_mangle]
pub unsafe extern "C" fn dup(fd: i32) -> i32 {
    let result = syscall1(SYS_DUP, fd as usize);
    if result == usize::MAX {
        *__errno_location() = 9;
        -1
    } else {
        result as i32
    }
}

#[no_mangle]
pub unsafe extern "C" fn fcntl(fd: i32, cmd: i32, arg: i32) -> i32 {
    let result = syscall3(SYS_FCNTL, fd as usize, cmd as usize, arg as usize);
    if result == usize::MAX {
        *__errno_location() = 9;
        -1
    } else {
        result as i32
    }
}

#[no_mangle]
pub unsafe extern "C" fn getcwd(buf: *mut u8, size: usize) -> *mut u8 {
    let result = syscall2(SYS_GETCWD, buf as usize, size);
    if result == usize::MAX {
        *__errno_location() = 34;
        core::ptr::null_mut()
    } else {
        buf
    }
}

#[no_mangle]
pub unsafe extern "C" fn poll(fds: *mut PollFd, nfds: usize, timeout: i32) -> i32 {
    let result = syscall3(SYS_POLL, fds as usize, nfds, timeout as usize);
    if result == usize::MAX {
        *__errno_location() = 22;
        -1
    } else {
        result as i32
    }
}

#[no_mangle]
pub unsafe extern "C" fn signal(_signal: i32, handler: usize) -> usize {
    syscall2(SYS_SIGNAL, _signal as usize, handler)
}

#[no_mangle]
pub unsafe extern "C" fn getentropy(buf: *mut u8, len: usize) -> i32 {
    let result = syscall2(SYS_GETENTROPY, buf as usize, len);
    if result == usize::MAX {
        *__errno_location() = 5;
        -1
    } else {
        0
    }
}

#[no_mangle]
pub unsafe extern "C" fn clock_gettime(_clock: i32, ts: *mut Timespec) -> i32 {
    let result = syscall2(SYS_CLOCK_GETTIME, _clock as usize, ts as usize);
    if result == usize::MAX {
        *__errno_location() = 22;
        -1
    } else {
        0
    }
}

#[no_mangle]
pub unsafe extern "C" fn getpid() -> i32 {
    syscall0(SYS_GETPID) as i32
}

#[no_mangle]
pub unsafe extern "C" fn _exit(status: i32) -> ! {
    let _ = syscall1(SYS_EXIT, status as usize);
    loop {
        core::hint::spin_loop();
    }
}

#[no_mangle]
pub unsafe extern "C" fn exit(status: i32) -> ! {
    _exit(status)
}

#[no_mangle]
pub unsafe extern "C" fn abort() -> ! {
    _exit(134)
}

#[no_mangle]
pub unsafe extern "C" fn strerror_r(_errnum: i32, buf: *mut u8, buflen: usize) -> i32 {
    let message = b"error\0";
    if buflen < message.len() {
        return 34;
    }
    core::ptr::copy_nonoverlapping(message.as_ptr(), buf, message.len());
    0
}

#[no_mangle]
pub unsafe extern "C" fn strlen(text: *const i8) -> usize {
    let mut len = 0;
    while *text.add(len) != 0 {
        len += 1;
    }
    len
}

/// Allocate zeroed memory from the provisional bump heap.
#[no_mangle]
pub unsafe extern "C" fn calloc(count: usize, size: usize) -> *mut u8 {
    let Some(total) = count.checked_mul(size) else {
        return core::ptr::null_mut();
    };
    let ptr = malloc(total);
    if !ptr.is_null() {
        core::ptr::write_bytes(ptr, 0, total);
    }
    ptr
}

/// Minimal bump allocator.  `free` is intentionally a no-op in this shim.
#[no_mangle]
pub unsafe extern "C" fn malloc(size: usize) -> *mut u8 {
    mem::malloc(size)
}

#[no_mangle]
pub unsafe extern "C" fn free(ptr: *mut u8) {
    let _ = ptr;
}

#[no_mangle]
pub unsafe extern "C" fn realloc(ptr: *mut u8, size: usize) -> *mut u8 {
    mem::realloc(ptr, size)
}

#[no_mangle]
pub unsafe extern "C" fn posix_memalign(out: *mut *mut u8, align: usize, size: usize) -> i32 {
    let Some(ptr) = mem::aligned_alloc(align, size) else {
        return 12;
    };
    *out = ptr;
    0
}

#[no_mangle]
pub unsafe extern "C" fn aligned_alloc(align: usize, size: usize) -> *mut u8 {
    mem::aligned_alloc(align, size).unwrap_or(core::ptr::null_mut())
}

#[repr(C)]
pub struct Stat {
    pub st_dev: u64,
    pub st_ino: u64,
    pub st_mode: u32,
    pub st_nlink: u32,
    pub st_uid: u32,
    pub st_gid: u32,
    pub st_rdev: u64,
    pub st_size: i64,
    pub st_blksize: i32,
    pub _pad: u32,
    pub st_blocks: i64,
    pub st_atim: [i64; 2],
    pub st_mtim: [i64; 2],
    pub st_ctim: [i64; 2],
    pub spare: [u8; 16],
}

#[no_mangle]
pub unsafe extern "C" fn fstat(fd: i32, stat: *mut Stat) -> i32 {
    let result = syscall2(SYS_FSTAT, fd as usize, stat as usize);
    if result == usize::MAX {
        *__errno_location() = 9;
        -1
    } else {
        0
    }
}

#[no_mangle]
pub unsafe extern "C" fn stat(path: *const u8, stat: *mut Stat) -> i32 {
    let result = syscall2(SYS_STAT, path as usize, stat as usize);
    if result == usize::MAX {
        *__errno_location() = 2;
        -1
    } else {
        0
    }
}

#[no_mangle]
pub unsafe extern "C" fn lstat(path: *const u8, stat: *mut Stat) -> i32 {
    self::stat(path, stat)
}

#[no_mangle]
pub unsafe extern "C" fn fstatat(
    _dirfd: i32,
    path: *const u8,
    stat: *mut Stat,
    _flags: i32,
) -> i32 {
    self::stat(path, stat)
}

#[no_mangle]
pub unsafe extern "C" fn openat(_dirfd: i32, path: *const u8, flags: i32, mode: i32) -> i32 {
    self::open(path, flags, mode)
}

#[no_mangle]
pub unsafe extern "C" fn open64(path: *const u8, flags: i32, mode: i32) -> i32 {
    self::open(path, flags, mode)
}

#[no_mangle]
pub unsafe extern "C" fn stat64(path: *const u8, stat: *mut Stat) -> i32 {
    self::stat(path, stat)
}

#[no_mangle]
pub unsafe extern "C" fn fstat64(fd: i32, stat: *mut Stat) -> i32 {
    self::fstat(fd, stat)
}

#[no_mangle]
pub unsafe extern "C" fn lstat64(path: *const u8, stat: *mut Stat) -> i32 {
    self::stat(path, stat)
}

pub const SYS_READDIR: usize = 19;
pub const SYS_NANOSLEEP: usize = 20;
pub const SYS_CHDIR: usize = 21;

#[repr(C)]
pub struct Dirent {
    pub d_ino: u64,
    pub d_off: i64,
    pub d_reclen: u16,
    pub d_type: u8,
    pub d_name: [i8; 256],
}

#[repr(C)]
pub struct DirStream {
    pub fd: i32,
    pub index: usize,
    pub entry: Dirent,
}

const DIR_STREAM_COUNT: usize = 8;
static mut DIR_STREAMS: [DirStream; DIR_STREAM_COUNT] = [const {
    DirStream {
        fd: -1,
        index: 0,
        entry: Dirent {
            d_ino: 0,
            d_off: 0,
            d_reclen: core::mem::size_of::<Dirent>() as u16,
            d_type: 0,
            d_name: [0; 256],
        },
    }
}; DIR_STREAM_COUNT];

fn dir_stream(fd: i32) -> *mut DirStream {
    unsafe {
        for stream in DIR_STREAMS.iter_mut() {
            if stream.fd < 0 {
                stream.fd = fd;
                stream.index = 0;
                return stream;
            }
        }
    }
    core::ptr::null_mut()
}

#[no_mangle]
pub unsafe extern "C" fn opendir(path: *const i8) -> *mut DirStream {
    let fd = open(path as *const u8, 0, 0);
    if fd < 0 {
        return core::ptr::null_mut();
    }
    let stream = dir_stream(fd);
    if stream.is_null() {
        close(fd);
    }
    stream
}

#[no_mangle]
pub unsafe extern "C" fn fdopendir(fd: i32) -> *mut DirStream {
    if fd < 0 {
        return core::ptr::null_mut();
    }
    dir_stream(fd)
}

#[no_mangle]
pub unsafe extern "C" fn readdir(stream: *mut DirStream) -> *mut Dirent {
    if stream.is_null() || (*stream).fd < 0 {
        return core::ptr::null_mut();
    }
    let mut name = [0u8; 256];
    let result = syscall4(
        SYS_READDIR,
        (*stream).fd as usize,
        (*stream).index,
        name.as_mut_ptr() as usize,
        name.len(),
    );
    if result == 0 || result == usize::MAX {
        return core::ptr::null_mut();
    }
    (*stream).index += 1;
    let entry = &mut (*stream).entry;
    entry.d_ino = (*stream).index as u64;
    entry.d_off = (*stream).index as i64;
    entry.d_reclen = core::mem::size_of::<Dirent>() as u16;
    entry.d_type = result as u8;
    for (index, slot) in entry.d_name.iter_mut().enumerate() {
        *slot = name[index] as i8;
        if name[index] == 0 {
            break;
        }
    }
    entry
}

#[no_mangle]
pub unsafe extern "C" fn readdir_r(
    stream: *mut DirStream,
    entry: *mut Dirent,
    result: *mut *mut Dirent,
) -> i32 {
    let found = readdir(stream);
    if found.is_null() {
        if !result.is_null() {
            *result = core::ptr::null_mut();
        }
        return 0;
    }
    core::ptr::copy_nonoverlapping(found, entry, 1);
    if !result.is_null() {
        *result = entry;
    }
    0
}

#[no_mangle]
pub unsafe extern "C" fn closedir(stream: *mut DirStream) -> i32 {
    if stream.is_null() || (*stream).fd < 0 {
        return -1;
    }
    let fd = (*stream).fd;
    (*stream).fd = -1;
    close(fd)
}

#[no_mangle]
pub unsafe extern "C" fn dirfd(stream: *mut DirStream) -> i32 {
    if stream.is_null() {
        -1
    } else {
        (*stream).fd
    }
}

#[no_mangle]
pub unsafe extern "C" fn nanosleep(request: *const Timespec, _remaining: *mut Timespec) -> i32 {
    let result = syscall1(SYS_NANOSLEEP, request as usize);
    if result == usize::MAX {
        *__errno_location() = 22;
        -1
    } else {
        0
    }
}

#[no_mangle]
pub unsafe extern "C" fn chdir(path: *const u8) -> i32 {
    let result = syscall1(SYS_CHDIR, path as usize);
    if result == usize::MAX {
        *__errno_location() = 2;
        -1
    } else {
        0
    }
}
