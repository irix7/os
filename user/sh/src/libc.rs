// SPDX-License-Identifier: GPL-3.0-only
//! The relibc functions the shell calls.
//!
//! Layouts and constants come from `native-abi`, which is checked against the
//! `irix7-pal` branch.  Pointers to C strings are `*const u8`.

#![allow(non_camel_case_types)]

pub use native_abi::{dirent, stat};

pub type c_int = i32;
pub type pid_t = i32;

pub const O_RDONLY: c_int = native_abi::O_RDONLY;
pub const O_WRONLY: c_int = native_abi::O_WRONLY;
pub const O_CREAT: c_int = native_abi::O_CREAT;
pub const O_TRUNC: c_int = native_abi::O_TRUNC;
pub const O_APPEND: c_int = native_abi::O_APPEND;
pub const O_CLOEXEC: c_int = native_abi::O_CLOEXEC;
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

/// Opaque directory stream.
#[repr(C)]
pub struct DIR {
    _private: [u8; 0],
}

extern "C" {
    pub static mut environ: *mut *mut u8;

    pub fn read(fd: c_int, buffer: *mut u8, count: usize) -> isize;
    pub fn write(fd: c_int, buffer: *const u8, count: usize) -> isize;
    pub fn open(path: *const u8, flags: c_int, ...) -> c_int;
    pub fn close(fd: c_int) -> c_int;
    pub fn dup2(fd: c_int, new_fd: c_int) -> c_int;
    pub fn pipe2(fds: *mut c_int, flags: c_int) -> c_int;
    pub fn fork() -> pid_t;
    pub fn execve(path: *const u8, argv: *const *const u8, envp: *const *mut u8) -> c_int;
    pub fn waitpid(pid: pid_t, status: *mut c_int, options: c_int) -> pid_t;
    pub fn _exit(status: c_int) -> !;
    pub fn getpid() -> pid_t;
    pub fn setpgid(pid: pid_t, pgid: pid_t) -> c_int;
    pub fn tcsetpgrp(fd: c_int, pgid: pid_t) -> c_int;
    pub fn isatty(fd: c_int) -> c_int;
    pub fn kill(pid: pid_t, signal: c_int) -> c_int;
    pub fn signal(signal: c_int, handler: usize) -> usize;
    pub fn chdir(path: *const u8) -> c_int;
    pub fn getcwd(buffer: *mut u8, size: usize) -> *mut u8;
    pub fn stat(path: *const u8, out: *mut stat) -> c_int;
    pub fn mkdir(path: *const u8, mode: u32) -> c_int;
    pub fn rmdir(path: *const u8) -> c_int;
    pub fn unlink(path: *const u8) -> c_int;
    pub fn rename(old: *const u8, new: *const u8) -> c_int;
    pub fn opendir(path: *const u8) -> *mut DIR;
    pub fn readdir(dir: *mut DIR) -> *mut dirent;
    pub fn closedir(dir: *mut DIR) -> c_int;
    pub fn getenv(name: *const u8) -> *const u8;
    pub fn setenv(name: *const u8, value: *const u8, overwrite: c_int) -> c_int;
    pub fn unsetenv(name: *const u8) -> c_int;
    pub fn sleep(seconds: u32) -> u32;
    pub fn uname(out: *mut utsname) -> c_int;
    pub fn strerror(error: c_int) -> *const u8;
    pub fn __errno_location() -> *mut c_int;
}

/// The calling thread's `errno`.
pub fn errno() -> c_int {
    // SAFETY: relibc returns a valid pointer into the thread control block.
    unsafe { *__errno_location() }
}

/// The bytes of a NUL-terminated C string, without the terminator.
///
/// # Safety
/// `pointer` must be non-null and NUL-terminated, and must outlive `'a`.
pub unsafe fn cstr<'a>(pointer: *const u8) -> &'a [u8] {
    let mut length = 0;
    while unsafe { *pointer.add(length) } != 0 {
        length += 1;
    }
    unsafe { core::slice::from_raw_parts(pointer, length) }
}
