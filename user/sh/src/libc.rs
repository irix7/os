// SPDX-License-Identifier: GPL-3.0-only
//! The native syscall wrappers the shell calls.
//!
//! This bring-up build is self-contained: every call goes straight through
//! the `syscall 0x1F7` native gate in `crates/native-abi`.  The kernel
//! returns `CELL_E_*` status in `$v0` and the result in `$v1`.
//!
//! The wrappers are written in MIPS assembly via `core::arch::global_asm!` because this
//! nightly's inline `asm!` does not accept named MIPS registers.

#![allow(non_camel_case_types)]

use native_abi::dirent;
pub use native_abi::stat;
use native_abi::status::CELL_OK;

pub type c_int = i32;
pub type pid_t = i32;

pub const O_RDONLY: c_int = native_abi::O_RDONLY;
pub const O_WRONLY: c_int = native_abi::O_WRONLY;
pub const O_RDWR: c_int = native_abi::O_RDWR;
pub const O_CREAT: c_int = native_abi::O_CREAT;
pub const O_TRUNC: c_int = native_abi::O_TRUNC;
pub const O_APPEND: c_int = native_abi::O_APPEND;
pub const O_CLOEXEC: c_int = native_abi::O_CLOEXEC;
pub const O_DIRECTORY: c_int = native_abi::O_DIRECTORY;
pub const O_NONBLOCK: c_int = native_abi::O_NONBLOCK;
pub const SIGINT: c_int = native_abi::SIGINT;
pub const SIGQUIT: c_int = native_abi::SIGQUIT;
pub const SIGKILL: c_int = native_abi::SIGKILL;
pub const SIGTERM: c_int = native_abi::SIGTERM;
pub const SIGHUP: c_int = native_abi::SIGHUP;
pub const SIGPIPE: c_int = native_abi::SIGPIPE;
pub const SIG_DFL: usize = 0;
pub const SIG_IGN: usize = 1;
pub const S_IFMT: u32 = native_abi::S_IFMT;
pub const S_IFDIR: u32 = native_abi::S_IFDIR;
pub const S_IFCHR: u32 = native_abi::S_IFCHR;
pub const AT_FDCWD: c_int = native_abi::AT_FDCWD;

/// `struct utsname`: six 65-byte fields.
#[repr(C)]
pub struct utsname {
    pub sysname: [u8; 65],
    pub nodename: [u8; 65],
    pub release: [u8; 65],
    pub version: [u8; 65],
    pub machine: [u8; 65],
    pub domainname: [u8; 65],
}

/// Directory stream backed by the kernel's `getdents` cookie.
#[repr(C)]
#[derive(Copy, Clone)]
pub struct DIR {
    fd: c_int,
    buf: [u8; 4096],
    buf_len: usize,
    buf_pos: usize,
    current: dirent,
    current_valid: bool,
}

const MAX_DIRS: usize = 4;

static mut DIR_TABLE: [DIR; MAX_DIRS] = unsafe {
    [DIR {
        fd: -1,
        buf: [0; 4096],
        buf_len: 0,
        buf_pos: 0,
        current: dirent {
            d_ino: 0,
            d_off: 0,
            d_reclen: 0,
            d_type: 0,
            d_name: [0; 256],
        },
        current_valid: false,
    }; MAX_DIRS]
};

/// The process errno cell.
static mut ERRNO: c_int = 0;

/// `__errno_location()`: pointer to the thread's errno cell.
#[no_mangle]
pub unsafe extern "C" fn __errno_location() -> *mut c_int {
    core::ptr::addr_of_mut!(ERRNO)
}

/// Generic syscall entry: `$v0` = syscall number, `$a0`-`$a3` = arguments.
/// Returns `CELL_OK` in `$v0` and result in `$v1`; error status in `$v0`.
#[link_name = "syscall_gate"]
extern "C" {
    fn syscall_gate(num: u32, a0: u32, a1: u32, a2: u32, a3: u32) -> (c_int, c_int);
}

fn ok_or_errno(status: c_int, result: c_int) -> c_int {
    if status == CELL_OK as c_int {
        result
    } else {
        unsafe { *__errno_location() = status };
        -1
    }
}

/// `read(fd, buffer, count)`: bytes read, or -1 with errno set.
#[no_mangle]
pub unsafe extern "C" fn read(fd: c_int, buffer: *mut u8, count: usize) -> isize {
    let (status, result) = unsafe {
        syscall_gate(native_abi::SYS_READ, fd as u32, buffer as u32, count as u32, 0)
    };
    ok_or_errno(status, result) as isize
}

/// `write(fd, buffer, count)`: bytes written, or -1 with errno set.
#[no_mangle]
pub unsafe extern "C" fn write(fd: c_int, buffer: *const u8, count: usize) -> isize {
    let (status, result) = unsafe {
        syscall_gate(
            native_abi::SYS_WRITE,
            fd as u32,
            buffer as u32,
            count as u32,
            0,
        )
    };
    ok_or_errno(status, result) as isize
}

/// `open(path, flags, mode)`: open or create a file.
#[no_mangle]
pub unsafe extern "C" fn open(path: *const u8, flags: c_int, mode: c_int) -> c_int {
    let (status, result) = unsafe {
        syscall_gate(
            native_abi::SYS_OPEN,
            path as u32,
            flags as u32,
            mode as u32,
            0,
        )
    };
    ok_or_errno(status, result)
}

/// `close(fd)`: 0 on success, -1 with errno set.
#[no_mangle]
pub unsafe extern "C" fn close(fd: c_int) -> c_int {
    let (status, result) = unsafe { syscall_gate(native_abi::SYS_CLOSE, fd as u32, 0, 0, 0) };
    ok_or_errno(status, result)
}

/// `dup2(oldfd, newfd)`: new descriptor number, or -1 with errno set.
#[no_mangle]
pub unsafe extern "C" fn dup2(old_fd: c_int, new_fd: c_int) -> c_int {
    let (status, result) = unsafe {
        syscall_gate(native_abi::SYS_DUP2, old_fd as u32, new_fd as u32, 0, 0)
    };
    ok_or_errno(status, result)
}

/// `pipe2(fds, flags)`: 0 on success, -1 with errno set.
#[no_mangle]
pub unsafe extern "C" fn pipe2(fds: *mut c_int, flags: c_int) -> c_int {
    let (status, result) = unsafe {
        syscall_gate(native_abi::SYS_PIPE2, fds as u32, flags as u32, 0, 0)
    };
    ok_or_errno(status, result)
}

/// `fork()`: child pid in the parent, 0 in the child, -1 with errno set.
#[no_mangle]
pub unsafe extern "C" fn fork() -> pid_t {
    let (status, result) = unsafe { syscall_gate(native_abi::SYS_FORK, 0, 0, 0, 0) };
    ok_or_errno(status, result)
}

/// `execve(path, argv, envp)`: does not return on success; -1 with errno set.
#[no_mangle]
pub unsafe extern "C" fn execve(
    path: *const u8,
    argv: *const *const u8,
    envp: *const *mut u8,
) -> c_int {
    let (status, result) = unsafe {
        syscall_gate(
            native_abi::SYS_EXECVE,
            path as u32,
            argv as u32,
            envp as u32,
            0,
        )
    };
    ok_or_errno(status, result)
}

/// `waitpid(pid, status, options)`: reaped pid, or -1 with errno set.
#[no_mangle]
pub unsafe extern "C" fn waitpid(pid: pid_t, status: *mut c_int, options: c_int) -> pid_t {
    let (s, result) = unsafe {
        syscall_gate(
            native_abi::SYS_WAITPID,
            pid as u32,
            status as u32,
            options as u32,
            0,
        )
    };
    if s == CELL_OK as c_int {
        result as pid_t
    } else {
        unsafe { *__errno_location() = s };
        -1
    }
}

/// `getpid()`: the caller's pid.
#[no_mangle]
pub unsafe extern "C" fn getpid() -> pid_t {
    let (status, result) = unsafe { syscall_gate(native_abi::SYS_GETPID, 0, 0, 0, 0) };
    ok_or_errno(status, result)
}

/// `setpgid(pid, pgid)`: 0 on success, -1 with errno set.
#[no_mangle]
pub unsafe extern "C" fn setpgid(pid: pid_t, pgid: pid_t) -> c_int {
    let (status, result) = unsafe {
        syscall_gate(native_abi::SYS_SETPGID, pid as u32, pgid as u32, 0, 0)
    };
    ok_or_errno(status, result)
}

/// `kill(pid, sig)`: 0 on success, -1 with errno set.
#[no_mangle]
pub unsafe extern "C" fn kill(pid: pid_t, sig: c_int) -> c_int {
    let (status, result) = unsafe {
        syscall_gate(native_abi::SYS_KILL, pid as u32, sig as u32, 0, 0)
    };
    ok_or_errno(status, result)
}

/// `chdir(path)`: 0 on success, -1 with errno set.
#[no_mangle]
pub unsafe extern "C" fn chdir(path: *const u8) -> c_int {
    let (status, result) =
        unsafe { syscall_gate(native_abi::SYS_CHDIR, path as u32, 0, 0, 0) };
    ok_or_errno(status, result)
}

/// `getcwd(buffer, size)`: `buffer` on success, null with errno set.
#[no_mangle]
pub unsafe extern "C" fn getcwd(buffer: *mut u8, size: usize) -> *mut u8 {
    let (status, result) = unsafe {
        syscall_gate(native_abi::SYS_GETCWD, buffer as u32, size as u32, 0, 0)
    };
    if status == CELL_OK as c_int {
        buffer
    } else {
        unsafe { *__errno_location() = status };
        core::ptr::null_mut()
    }
}

/// `stat(path, out)`: 0 on success, -1 with errno set.
#[no_mangle]
pub unsafe extern "C" fn stat(path: *const u8, out: *mut native_abi::stat) -> c_int {
    let (status, result) = unsafe { syscall_gate(native_abi::SYS_STAT, path as u32, out as u32, 0, 0) };
    ok_or_errno(status, result)
}

/// `mkdir(path, mode)`: 0 on success, -1 with errno set.
#[no_mangle]
pub unsafe extern "C" fn mkdir(path: *const u8, mode: u32) -> c_int {
    let (status, result) =
        unsafe { syscall_gate(native_abi::SYS_MKDIR, path as u32, mode as u32, 0, 0) };
    ok_or_errno(status, result)
}

/// `rmdir(path)`: 0 on success, -1 with errno set.
#[no_mangle]
pub unsafe extern "C" fn rmdir(path: *const u8) -> c_int {
    let (status, result) =
        unsafe { syscall_gate(native_abi::SYS_RMDIR, path as u32, 0, 0, 0) };
    ok_or_errno(status, result)
}

/// `unlink(path)`: 0 on success, -1 with errno set.
#[no_mangle]
pub unsafe extern "C" fn unlink(path: *const u8) -> c_int {
    let (status, result) =
        unsafe { syscall_gate(native_abi::SYS_UNLINK, path as u32, 0, 0, 0) };
    ok_or_errno(status, result)
}

/// `rename(old, new)`: 0 on success, -1 with errno set.
#[no_mangle]
pub unsafe extern "C" fn rename(old: *const u8, new: *const u8) -> c_int {
    let (status, result) = unsafe {
        syscall_gate(native_abi::SYS_RENAME, old as u32, new as u32, 0, 0)
    };
    ok_or_errno(status, result)
}

/// `__errno_location()`: pointer to the thread's errno cell.
#[no_mangle]
pub unsafe extern "C" fn errno_loc() -> *mut c_int {
    __errno_location()
}

/// The calling thread's `errno`.
pub fn errno() -> c_int {
    unsafe { *__errno_location() }
}

/// `static mut environ`: the process environment pointer array.
pub static mut environ: *mut *mut u8 = core::ptr::null_mut();

/// `getenv(name)`: value pointer, or null when unset (no environment layer yet).
#[no_mangle]
pub unsafe extern "C" fn getenv(name: *const u8) -> *const u8 {
    let _ = name;
    core::ptr::null()
}

/// `setenv(name, value, overwrite)`: success no-op (no environment layer yet).
#[no_mangle]
pub unsafe extern "C" fn setenv(
    name: *const u8,
    value: *const u8,
    overwrite: c_int,
) -> c_int {
    let _ = (name, value, overwrite);
    0
}

/// `unsetenv(name)`: success no-op (no environment layer yet).
#[no_mangle]
pub unsafe extern "C" fn unsetenv(name: *const u8) -> c_int {
    let _ = name;
    0
}

/// `sleep(seconds)`: no-op (no `SYS_NANOSLEEP` wrapper in this bring-up build).
#[no_mangle]
pub unsafe extern "C" fn sleep(seconds: u32) -> u32 {
    let _ = seconds;
    0
}

/// `uname(out)`: userspace stub — no `SYS_UNAME` exists in the v0 kernel.
#[no_mangle]
pub unsafe extern "C" fn uname(out: *mut utsname) -> c_int {
    if out.is_null() {
        unsafe { *__errno_location() = native_abi::CELL_E_BADF as c_int };
        return -1;
    }
    let fields: [(*mut u8, &[u8]); 6] = [
        ((*out).sysname.as_mut_ptr(), b"IRIX7\0"),
        ((*out).nodename.as_mut_ptr(), b"iris\0"),
        ((*out).release.as_mut_ptr(), b"7.0\0"),
        ((*out).version.as_mut_ptr(), b"native n32\0"),
        ((*out).machine.as_mut_ptr(), b"mips\0"),
        ((*out).domainname.as_mut_ptr(), b"\0"),
    ];
    for (dst, src) in fields {
        let n = src.len().min(64);
        for (i, &byte) in src.iter().enumerate().take(n) {
            unsafe { core::ptr::write_volatile(dst.add(i), byte) };
        }
    }
    0
}

/// `strerror(error)`: minimal messages.
#[no_mangle]
pub unsafe extern "C" fn strerror(error: c_int) -> *const u8 {
    static MSGS: &[(&[u8], c_int)] = &[
        (b"Success\0", 0),
        (b"No such file or directory\0", 2),
        (b"Interrupted system call\0", 4),
        (b"I/O error\0", 5),
        (b"Bad file number\0", 9),
        (b"No child process\0", 10),
        (b"Resource temporarily unavailable\0", 11),
        (b"Cannot allocate memory\0", 12),
        (b"Permission denied\0", 13),
        (b"Bad address\0", 14),
        (b"Device busy\0", 16),
        (b"File exists\0", 17),
        (b"Not a directory\0", 20),
        (b"Is a directory\0", 21),
        (b"Invalid argument\0", 22),
        (b"File table overflow\0", 23),
        (b"Too many open files\0", 24),
        (b"Not a typewriter\0", 25),
        (b"Text file busy\0", 26),
        (b"File too large\0", 27),
        (b"No space left on device\0", 28),
        (b"Illegal seek\0", 29),
        (b"Read-only file system\0", 30),
        (b"Broken pipe\0", 32),
        (b"Result too large\0", 35),
        (b"Directory not empty\0", 39),
        (b"No more processes\0", 31),
        (b"Function not implemented\0", 38),
    ];
    for (msg, code) in MSGS.iter() {
        if *code == error {
            return msg.as_ptr();
        }
    }
    b"Unknown error\0".as_ptr()
}

/// `tcsetpgrp(fd, pgid)`: stubbed success (no `SYS_IOCTL` in v0 gate).
#[no_mangle]
pub unsafe extern "C" fn tcsetpgrp(fd: c_int, pgid: pid_t) -> c_int {
    let _ = (fd, pgid);
    0
}

/// `isatty(fd)`: fds 0-2 are console devices.
#[no_mangle]
pub unsafe extern "C" fn isatty(fd: c_int) -> c_int {
    if fd >= 0 && fd <= 2 {
        1
    } else {
        0
    }
}

/// `signal(signal, handler)`: v0 has no signal delivery; success no-op.
#[no_mangle]
pub unsafe extern "C" fn signal(_signal: c_int, _handler: usize) -> usize {
    SIG_DFL
}

/// `memset(dst, byte, count)`: fill `count` bytes with `byte`.
#[no_mangle]
pub unsafe extern "C" fn memset(dst: *mut u8, byte: c_int, count: usize) -> *mut u8 {
    let mut i = 0;
    while i < count {
        unsafe { core::ptr::write_volatile(dst.add(i), byte as u8) };
        i += 1;
    }
    dst
}

/// `memcpy(dst, src, count)`: copy `count` bytes from `src` to `dst`.
#[no_mangle]
pub unsafe extern "C" fn memcpy(dst: *mut u8, src: *const u8, count: usize) -> *mut u8 {
    let mut i = 0;
    while i < count {
        let b = unsafe { core::ptr::read_volatile(src.add(i)) };
        unsafe { core::ptr::write_volatile(dst.add(i), b) };
        i += 1;
    }
    dst
}

/// `memmove(dst, src, count)`: copy `count` bytes, handling overlap.
#[no_mangle]
pub unsafe extern "C" fn memmove(dst: *mut u8, src: *const u8, count: usize) -> *mut u8 {
    if dst as usize <= src as usize {
        return memcpy(dst, src, count);
    }
    let mut i = count;
    while i > 0 {
        i -= 1;
        let b = unsafe { core::ptr::read_volatile(src.add(i)) };
        unsafe { core::ptr::write_volatile(dst.add(i), b) };
    }
    dst
}

/// `memcmp(a, b, count)`: compare `count` bytes.
#[no_mangle]
pub unsafe extern "C" fn memcmp(a: *const u8, b: *const u8, count: usize) -> c_int {
    let mut i = 0;
    while i < count {
        let x = unsafe { core::ptr::read_volatile(a.add(i)) };
        let y = unsafe { core::ptr::read_volatile(b.add(i)) };
        if x != y {
            return x as c_int - y as c_int;
        }
        i += 1;
    }
    0
}

/// `strlen(s)`: length of a NUL-terminated string.
#[no_mangle]
pub unsafe extern "C" fn strlen(s: *const u8) -> usize {
    let mut len = 0;
    while unsafe { *s.add(len) } != 0 {
        len += 1;
    }
    len
}

/// `strcmp(a, b)`: compare two NUL-terminated strings.
#[no_mangle]
pub unsafe extern "C" fn strcmp(a: *const u8, b: *const u8) -> c_int {
    let mut i = 0;
    loop {
        let x = unsafe { *a.add(i) };
        let y = unsafe { *b.add(i) };
        if x != y || x == 0 {
            return x as c_int - y as c_int;
        }
        i += 1;
    }
}

/// `strncmp(a, b, n)`: compare at most `n` bytes of two strings.
#[no_mangle]
pub unsafe extern "C" fn strncmp(a: *const u8, b: *const u8, n: usize) -> c_int {
    let mut i = 0;
    while i < n {
        let x = unsafe { *a.add(i) };
        let y = unsafe { *b.add(i) };
        if x != y || x == 0 {
            return x as c_int - y as c_int;
        }
        if x == 0 {
            return 0;
        }
        i += 1;
    }
    0
}

/// `_exit(status)`: enters the kernel exit gate; does not return.
#[no_mangle]
pub unsafe extern "C" fn _exit(status: c_int) -> ! {
    unsafe {
        syscall_gate(native_abi::SYS_EXIT, status as u32, 0, 0, 0);
    }
    // The kernel's exit path never returns.  If it somehow does, park.
    loop {
        core::hint::spin_loop();
    }
}

/// The bytes of a NUL-terminated C string, without the terminator.
///
/// # Safety
/// `pointer` must be non-null and NUL-terminated, and must outlive `'a`.
pub unsafe fn cstr<'a>(pointer: *const u8) -> &'a [u8] {
    if pointer.is_null() {
        return &[];
    }
    let mut length = 0;
    while unsafe { *pointer.add(length) } != 0 {
        length += 1;
    }
    unsafe { core::slice::from_raw_parts(pointer, length) }
}

/// `opendir(path)`: open a directory stream.  Returns null on failure.
#[no_mangle]
pub unsafe extern "C" fn opendir(path: *const u8) -> *mut DIR {
    let fd = unsafe { open(path, O_RDONLY | O_DIRECTORY, 0) };
    if fd < 0 {
        return core::ptr::null_mut();
    }
    let slot = unsafe {
        (0..MAX_DIRS).find(|&i| DIR_TABLE[i].fd < 0).map(|i| &mut DIR_TABLE[i] as *mut DIR)
    };
    let Some(slot) = slot else {
        unsafe { close(fd) };
        return core::ptr::null_mut();
    };
    unsafe {
        (*slot).fd = fd;
        (*slot).buf_len = 0;
        (*slot).buf_pos = 0;
        (*slot).current_valid = false;
    };
    // Prime the buffer with getdents.
    unsafe { refill_dir(slot) };
    slot
}

/// Read the next block of directory entries into `dir.buf`.
unsafe fn refill_dir(dir: *mut DIR) -> bool {
    let fd = unsafe { (*dir).fd };
    let buf = unsafe { core::ptr::addr_of_mut!((*dir).buf) as *mut u8 };
    let (status, count) = unsafe {
        syscall_gate(native_abi::SYS_GETDENTS, fd as u32, buf as u32, 4096u32, 0)
    };
    if status != CELL_OK as c_int || count == 0 {
        return false;
    }
    unsafe {
        (*dir).buf_len = count as usize;
        (*dir).buf_pos = 0;
        (*dir).current_valid = false;
    }
    true
}

/// `readdir(dir)`: next entry, or null at end of directory.
#[no_mangle]
pub unsafe extern "C" fn readdir(dir: *mut DIR) -> *mut dirent {
    if dir.is_null() {
        return core::ptr::null_mut();
    }
    loop {
        if unsafe { (*dir).current_valid } {
            unsafe { (*dir).current_valid = false };
            return unsafe { core::ptr::addr_of_mut!((*dir).current) as *mut dirent };
        }
        // Try to parse the next record from the buffer.
        let pos = unsafe { (*dir).buf_pos };
        let len = unsafe { (*dir).buf_len };
        if pos + core::mem::size_of::<dirent>() > len {
            // Buffer exhausted; try to refill.
            if unsafe { refill_dir(dir) } {
                continue;
            }
            return core::ptr::null_mut();
        }
        let record_ptr = unsafe { (*dir).buf.as_ptr().add(pos) as *const dirent };
        unsafe {
            (*dir).buf_pos += core::mem::size_of::<dirent>();
            (*dir).current = core::ptr::read(record_ptr);
        }
        let d_reclen = unsafe { (*dir).current.d_reclen as usize };
        if d_reclen == 0 {
            // Zero-length record means end of directory.
            unsafe { (*dir).buf_len = 0 };
            return core::ptr::null_mut();
        }
        unsafe { (*dir).current_valid = true };
    }
}

/// `closedir(dir)`: close the directory stream.
#[no_mangle]
pub unsafe extern "C" fn closedir(dir: *mut DIR) -> c_int {
    if dir.is_null() {
        unsafe { *__errno_location() = native_abi::CELL_E_BADF as c_int };
        return -1;
    }
    let fd = unsafe { (*dir).fd };
    unsafe { (*dir).fd = -1 };
    if fd < 0 {
        return 0;
    }
    let (status, _result) = unsafe { syscall_gate(native_abi::SYS_CLOSE, fd as u32, 0, 0, 0) };
    if status == CELL_OK as c_int {
        0
    } else {
        unsafe { *__errno_location() = status };
        -1
    }
}

/// The syscall gate: `$v0` = syscall number, `$a0`-`$a3` = arguments.
///
/// Returns `CELL_OK` (0) in `$v0` and the result in `$v1`; on error `$v0`
/// holds a `CELL_E_*` status.  `$v1` is undefined on error.
core::arch::global_asm!(
    r#"
    .set noreorder
    .set mips3

    .globl syscall_gate
    .type syscall_gate, @function
    .align 2
syscall_gate:
    /* $a0 = syscall number -> $v0 */
    move    $v0, $a0
    /* syscall */
    syscall
    /* $v0 = status, $v1 = result */
    jr      $ra
    nop
    .size syscall_gate, .-syscall_gate

    .globl _start
    .type _start, @function
    .align 2
_start:
    /* $a0 = argc, $a1 = argv (already set by the kernel) */
    jal     main
    nop
    /* $v0 = return status -> $a0 for _exit */
    move    $a0, $v0
    jal     _exit
    nop
    .size _start, .- _start
"#
);
