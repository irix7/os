// SPDX-License-Identifier: GPL-3.0-only
//! Native IRIX 7 n32 C types and public layouts.
//!
//! These are the provisional v0 layouts from `docs/spec/native-libc.md` §4.1.
//! The C oracle and the layout review will freeze them; they are not yet a
//! shipped ABI.

pub use core::ffi::c_void;

pub type c_char = i8;
pub type c_schar = i8;
pub type c_uchar = u8;
pub type c_short = i16;
pub type c_ushort = u16;
pub type c_int = i32;
pub type c_uint = u32;
pub type c_long = i32;
pub type c_ulong = u32;
pub type c_longlong = i64;
pub type c_ulonglong = u64;
pub type c_float = f32;
pub type c_double = f64;

pub type int8_t = i8;
pub type uint8_t = u8;
pub type int16_t = i16;
pub type uint16_t = u16;
pub type int32_t = i32;
pub type uint32_t = u32;
pub type int64_t = i64;
pub type uint64_t = u64;

pub type intptr_t = i32;
pub type uintptr_t = u32;
pub type ptrdiff_t = i32;
pub type size_t = u32;
pub type ssize_t = i32;

pub type off_t = i64;
pub type ino_t = u64;
pub type dev_t = u64;
pub type mode_t = u32;
pub type nlink_t = u32;
pub type blkcnt_t = i64;
pub type blksize_t = i32;
pub type pid_t = i32;
pub type uid_t = u32;
pub type gid_t = u32;
pub type id_t = u32;
pub type time_t = i64;
pub type clockid_t = i32;
pub type socklen_t = u32;
pub type sa_family_t = u16;
pub type rlim_t = u64;
pub type wchar_t = i32;
pub type pthread_t = u32;
pub type pthread_key_t = u32;

/// `struct timespec`: 16 bytes, 8-byte aligned.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct timespec {
    pub tv_sec: time_t,
    pub tv_nsec: c_long,
    pub _pad: u32,
}

/// `struct timeval`: 16 bytes, 8-byte aligned.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct timeval {
    pub tv_sec: time_t,
    pub tv_usec: c_long,
    pub _pad: u32,
}

/// `struct itimerspec`: two `timespec` values.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct itimerspec {
    pub it_interval: timespec,
    pub it_value: timespec,
}

/// `struct iovec`: 8 bytes in n32.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct iovec {
    pub iov_base: u32,
    pub iov_len: size_t,
}

/// `struct pollfd`: 8 bytes.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct pollfd {
    pub fd: c_int,
    pub events: c_short,
    pub revents: c_short,
}

/// `struct winsize`: 8 bytes.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct winsize {
    pub ws_row: u16,
    pub ws_col: u16,
    pub ws_xpixel: u16,
    pub ws_ypixel: u16,
}

/// `struct rlimit`: 16 bytes.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct rlimit {
    pub rlim_cur: rlim_t,
    pub rlim_max: rlim_t,
}

/// `struct stat`: provisional 128-byte layout.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct stat {
    pub st_dev: dev_t,
    pub st_ino: ino_t,
    pub st_mode: mode_t,
    pub st_nlink: nlink_t,
    pub st_uid: uid_t,
    pub st_gid: gid_t,
    pub st_rdev: dev_t,
    pub st_size: off_t,
    pub st_blksize: blksize_t,
    pub _pad0: u32,
    pub st_blocks: blkcnt_t,
    pub st_atim: timespec,
    pub st_mtim: timespec,
    pub st_ctim: timespec,
    pub _reserved: [u8; 16],
}

/// `struct dirent`: 280 bytes in this provisional layout.
///
/// `d_type` at offset 18 and `d_name` at offset 19 are the extension points
/// `std` and `readdir` use; they are not a claim about the final `NAME_MAX`.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct dirent {
    pub d_ino: ino_t,
    pub d_off: off_t,
    pub d_reclen: u16,
    pub d_type: u8,
    pub d_name: [u8; 256],
}

/// The POSIX.1-2024 directory record.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct posix_dent {
    pub d_ino: ino_t,
    pub d_off: off_t,
    pub d_reclen: u16,
    pub d_type: u8,
    pub d_name: [u8; 256],
}

/// `sigset_t`: 128 bytes, 1024 signals.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct sigset_t {
    pub bits: [u64; 16],
}

/// `stack_t`: 12 bytes.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct stack_t {
    pub ss_sp: u32,
    pub ss_size: size_t,
    pub ss_flags: c_int,
}

/// `sockaddr`: 16 bytes.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct sockaddr {
    pub sa_family: sa_family_t,
    pub sa_data: [u8; 14],
}

/// `sockaddr_in`: 16 bytes.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct sockaddr_in {
    pub sin_family: sa_family_t,
    pub sin_port: u16,
    pub sin_addr: [u8; 4],
    pub sin_zero: [u8; 8],
}

/// `sockaddr_in6`: 28 bytes.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct sockaddr_in6 {
    pub sin6_family: sa_family_t,
    pub sin6_port: u16,
    pub sin6_flowinfo: u32,
    pub sin6_addr: [u8; 16],
    pub sin6_scope_id: u32,
}

/// `sockaddr_storage`: at least 128 bytes, 8-byte aligned.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct sockaddr_storage {
    pub ss_family: sa_family_t,
    pub _pad: [u8; 6],
    pub _data: [u8; 120],
}

/// `sockaddr_un`: family plus a 108-byte path.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct sockaddr_un {
    pub sun_family: sa_family_t,
    pub sun_path: [u8; 108],
}

/// `struct msghdr`: provisional 32-byte layout.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct msghdr {
    pub msg_name: u32,
    pub msg_namelen: socklen_t,
    pub msg_iov: u32,
    pub msg_iovlen: size_t,
    pub msg_control: u32,
    pub msg_controllen: size_t,
    pub msg_flags: c_int,
    pub _pad: u32,
}

/// `struct cmsghdr`: 12 bytes.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct cmsghdr {
    pub cmsg_len: size_t,
    pub cmsg_level: c_int,
    pub cmsg_type: c_int,
}

/// `struct addrinfo`: provisional 32-byte layout.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct addrinfo {
    pub ai_flags: c_int,
    pub ai_family: c_int,
    pub ai_socktype: c_int,
    pub ai_protocol: c_int,
    pub ai_addrlen: socklen_t,
    pub ai_addr: u32,
    pub ai_canonname: u32,
    pub ai_next: u32,
}

/// `struct linger`: 8 bytes.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct linger {
    pub l_onoff: c_int,
    pub l_linger: c_int,
}

/// `fd_set`: 1024 bits.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct fd_set {
    pub bits: [u32; 32],
}

/// `struct termios`: provisional terminal state.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct termios {
    pub c_iflag: u32,
    pub c_oflag: u32,
    pub c_cflag: u32,
    pub c_lflag: u32,
    pub c_line: u8,
    pub c_cc: [u8; 32],
    pub _pad: [u8; 3],
    pub c_ispeed: u32,
    pub c_ospeed: u32,
}

/// `struct passwd`: 28 bytes of pointers and ids.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct passwd {
    pub pw_name: u32,
    pub pw_passwd: u32,
    pub pw_uid: uid_t,
    pub pw_gid: gid_t,
    pub pw_gecos: u32,
    pub pw_dir: u32,
    pub pw_shell: u32,
}

/// `struct utsname`: five 65-byte fields.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct utsname {
    pub sysname: [u8; 65],
    pub nodename: [u8; 65],
    pub release: [u8; 65],
    pub version: [u8; 65],
    pub machine: [u8; 65],
}

/// `Dl_info`: provisional 16-byte layout.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Dl_info {
    pub dli_fname: u32,
    pub dli_fbase: u32,
    pub dli_sname: u32,
    pub dli_saddr: u32,
}

/// Opaque pthread storage.  The sizes are provisional and 8-byte aligned.
macro_rules! opaque32 {
    ($name:ident) => {
        #[repr(C, align(8))]
        #[derive(Clone, Copy, Debug, PartialEq, Eq)]
        pub struct $name {
            pub _opaque: [u8; 32],
        }
    };
}

opaque32!(pthread_attr_t);
opaque32!(pthread_mutex_t);
opaque32!(pthread_mutexattr_t);
opaque32!(pthread_cond_t);
opaque32!(pthread_condattr_t);
opaque32!(pthread_rwlock_t);
opaque32!(pthread_rwlockattr_t);
opaque32!(pthread_barrier_t);
opaque32!(pthread_barrierattr_t);
opaque32!(pthread_spinlock_t);

/// Opaque `FILE` storage.  The native libc does not expose its internals.
#[repr(C, align(8))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FILE {
    pub _opaque: [u8; 128],
}

/// Opaque `DIR` storage.
#[repr(C, align(8))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DIR {
    pub _opaque: [u8; 128],
}

/// `sem_t` is optional; this provisional storage is 32 bytes.
#[repr(C, align(8))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct sem_t {
    pub _opaque: [u8; 32],
}
