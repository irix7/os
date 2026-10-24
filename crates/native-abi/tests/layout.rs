// SPDX-License-Identifier: GPL-3.0-only
//! Host layout tests for the provisional n32 ABI.
//!
//! These tests pin the recommended v0 layout.  They do not replace the C
//! oracle; they make every change to a public struct visible in review.

use core::mem::{align_of, offset_of, size_of};

use native_abi::*;

#[test]
fn fixed_width_types_are_n32() {
    assert_eq!(size_of::<c_char>(), 1);
    assert_eq!(size_of::<c_int>(), 4);
    assert_eq!(size_of::<c_long>(), 4);
    assert_eq!(size_of::<size_t>(), 4);
    assert_eq!(size_of::<off_t>(), 8);
    assert_eq!(size_of::<time_t>(), 8);
    assert_eq!(size_of::<ino_t>(), 8);
}

#[test]
fn time_and_io_structs() {
    assert_eq!((size_of::<timespec>(), align_of::<timespec>()), (16, 8));
    assert_eq!(offset_of!(timespec, tv_nsec), 8);
    assert_eq!(size_of::<timeval>(), 16);
    assert_eq!(size_of::<itimerspec>(), 32);
    assert_eq!(size_of::<iovec>(), 8);
    assert_eq!(size_of::<pollfd>(), 8);
    assert_eq!(size_of::<winsize>(), 8);
    assert_eq!(size_of::<rlimit>(), 16);
}

#[test]
fn stat_is_the_recommended_128_byte_layout() {
    assert_eq!(size_of::<stat>(), 128);
    assert_eq!(align_of::<stat>(), 8);
    assert_eq!(offset_of!(stat, st_ino), 8);
    assert_eq!(offset_of!(stat, st_size), 40);
    assert_eq!(offset_of!(stat, st_blksize), 48);
    assert_eq!(offset_of!(stat, st_blocks), 56);
    assert_eq!(offset_of!(stat, st_atim), 64);
    assert_eq!(offset_of!(stat, st_mtim), 80);
    assert_eq!(offset_of!(stat, st_ctim), 96);
}

#[test]
fn directory_records() {
    assert_eq!(size_of::<dirent>(), 280);
    assert_eq!(offset_of!(dirent, d_type), 18);
    assert_eq!(offset_of!(dirent, d_name), 19);
    assert_eq!(size_of::<posix_dent>(), 280);
}

#[test]
fn signal_and_socket_structs() {
    assert_eq!(size_of::<sigset_t>(), 128);
    assert_eq!(size_of::<stack_t>(), 12);
    assert_eq!(size_of::<sockaddr>(), 16);
    assert_eq!(size_of::<sockaddr_in>(), 16);
    assert_eq!(size_of::<sockaddr_in6>(), 28);
    assert_eq!(size_of::<sockaddr_storage>(), 128);
    assert_eq!(size_of::<sockaddr_un>(), 110);
    assert_eq!(size_of::<msghdr>(), 32);
    assert_eq!(size_of::<cmsghdr>(), 12);
    assert_eq!(size_of::<addrinfo>(), 32);
    assert_eq!(size_of::<linger>(), 8);
    assert_eq!(size_of::<fd_set>(), 128);
}

#[test]
fn terminal_and_misc_structs() {
    assert_eq!(size_of::<termios>(), 60);
    assert_eq!(size_of::<passwd>(), 28);
    assert_eq!(size_of::<utsname>(), 325);
    assert_eq!(size_of::<Dl_info>(), 16);
    assert_eq!(size_of::<FILE>(), 128);
    assert_eq!(size_of::<DIR>(), 128);
    assert_eq!(size_of::<sem_t>(), 32);
    assert_eq!(size_of::<pthread_mutex_t>(), 32);
    assert_eq!(align_of::<pthread_mutex_t>(), 8);
}

#[test]
fn constant_sanity() {
    assert_eq!(EWOULDBLOCK, EAGAIN);
    assert_eq!(ENOTSUP, EOPNOTSUPP);
    assert_eq!(O_ACCMODE, 3);
    assert_eq!(O_RDWR, 2);
    assert_eq!(S_IFMT & S_IFREG, S_IFREG);
    assert_eq!(MAP_ANONYMOUS, MAP_ANON);
}
