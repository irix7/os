// SPDX-License-Identifier: GPL-3.0-only
//! Host layout tests for the provisional n32 ABI.
//!
//! These tests pin the recommended v0 layout.  They do not replace the C
//! oracle; they make every change to a public struct visible in review.

use core::mem::{align_of, offset_of, size_of};

use native_abi::*;

#[test]
fn fixed_width_types_are_n32() {
    assert_eq!((size_of::<c_char>(), align_of::<c_char>()), (1, 1));
    assert_eq!((size_of::<c_int>(), align_of::<c_int>()), (4, 4));
    assert_eq!((size_of::<c_long>(), align_of::<c_long>()), (4, 4));
    assert_eq!((size_of::<size_t>(), align_of::<size_t>()), (4, 4));
    assert_eq!((size_of::<ssize_t>(), align_of::<ssize_t>()), (4, 4));
    assert_eq!((size_of::<off_t>(), align_of::<off_t>()), (8, 8));
    assert_eq!((size_of::<time_t>(), align_of::<time_t>()), (8, 8));
    assert_eq!((size_of::<ino_t>(), align_of::<ino_t>()), (8, 8));
    assert_eq!((size_of::<pthread_t>(), align_of::<pthread_t>()), (4, 4));
}

#[test]
fn time_and_io_structs() {
    assert_eq!((size_of::<timespec>(), align_of::<timespec>()), (16, 8));
    assert_eq!(offset_of!(timespec, tv_nsec), 8);
    assert_eq!((size_of::<timeval>(), align_of::<timeval>()), (16, 8));
    assert_eq!(offset_of!(timeval, tv_usec), 8);
    assert_eq!((size_of::<itimerspec>(), align_of::<itimerspec>()), (32, 8));
    assert_eq!((size_of::<iovec>(), align_of::<iovec>()), (8, 4));
    assert_eq!((size_of::<pollfd>(), align_of::<pollfd>()), (8, 4));
    assert_eq!((size_of::<winsize>(), align_of::<winsize>()), (8, 2));
    assert_eq!((size_of::<rlimit>(), align_of::<rlimit>()), (16, 8));
}

#[test]
fn stat_is_the_recommended_128_byte_layout() {
    assert_eq!((size_of::<stat>(), align_of::<stat>()), (128, 8));
    assert_eq!(offset_of!(stat, st_ino), 8);
    assert_eq!(offset_of!(stat, st_size), 40);
    assert_eq!(offset_of!(stat, st_blksize), 48);
    assert_eq!(offset_of!(stat, st_blocks), 56);
    assert_eq!(offset_of!(stat, st_atim), 64);
    assert_eq!(offset_of!(stat, st_mtim), 80);
    assert_eq!(offset_of!(stat, st_ctim), 96);
    assert_eq!(offset_of!(stat, _reserved), 112);
}

#[test]
fn directory_records() {
    assert_eq!((size_of::<dirent>(), align_of::<dirent>()), (280, 8));
    assert_eq!(offset_of!(dirent, d_type), 18);
    assert_eq!(offset_of!(dirent, d_name), 19);
    assert_eq!(
        (size_of::<posix_dent>(), align_of::<posix_dent>()),
        (280, 8)
    );
    assert_eq!(offset_of!(posix_dent, d_type), 18);
    assert_eq!(offset_of!(posix_dent, d_name), 19);
}

#[test]
fn signal_and_socket_structs() {
    assert_eq!((size_of::<sigset_t>(), align_of::<sigset_t>()), (128, 8));
    assert_eq!((size_of::<stack_t>(), align_of::<stack_t>()), (12, 4));
    // POSIX order, and the order in native-libc.md §4.1.
    assert_eq!(offset_of!(stack_t, ss_sp), 0);
    assert_eq!(offset_of!(stack_t, ss_size), 4);
    assert_eq!(offset_of!(stack_t, ss_flags), 8);

    assert_eq!((size_of::<in_addr>(), align_of::<in_addr>()), (4, 4));
    assert_eq!((size_of::<in6_addr>(), align_of::<in6_addr>()), (16, 4));
    assert_eq!((size_of::<sockaddr>(), align_of::<sockaddr>()), (16, 2));
    assert_eq!(
        (size_of::<sockaddr_in>(), align_of::<sockaddr_in>()),
        (16, 4)
    );
    assert_eq!(offset_of!(sockaddr_in, sin_addr), 4);
    assert_eq!(
        (size_of::<sockaddr_in6>(), align_of::<sockaddr_in6>()),
        (28, 4)
    );
    assert_eq!(offset_of!(sockaddr_in6, sin6_addr), 8);
    assert_eq!(
        (
            size_of::<sockaddr_storage>(),
            align_of::<sockaddr_storage>()
        ),
        (128, 8)
    );
    assert_eq!(
        (size_of::<sockaddr_un>(), align_of::<sockaddr_un>()),
        (110, 2)
    );
    assert_eq!((size_of::<msghdr>(), align_of::<msghdr>()), (28, 4));
    assert_eq!(offset_of!(msghdr, msg_flags), 24);
    assert_eq!((size_of::<cmsghdr>(), align_of::<cmsghdr>()), (12, 4));
    assert_eq!((size_of::<addrinfo>(), align_of::<addrinfo>()), (32, 4));
    assert_eq!((size_of::<linger>(), align_of::<linger>()), (8, 4));
    assert_eq!((size_of::<fd_set>(), align_of::<fd_set>()), (128, 4));
}

#[test]
fn terminal_and_misc_structs() {
    assert_eq!((size_of::<termios>(), align_of::<termios>()), (60, 4));
    assert_eq!(offset_of!(termios, c_line), 16);
    assert_eq!(offset_of!(termios, c_cc), 17);
    assert_eq!(offset_of!(termios, c_ispeed), 52);
    assert_eq!(offset_of!(termios, c_ospeed), 56);
    assert_eq!((size_of::<passwd>(), align_of::<passwd>()), (28, 4));
    assert_eq!((size_of::<utsname>(), align_of::<utsname>()), (325, 1));
    assert_eq!(offset_of!(utsname, machine), 260);
    assert_eq!((size_of::<Dl_info>(), align_of::<Dl_info>()), (16, 4));
    assert_eq!((size_of::<FILE>(), align_of::<FILE>()), (128, 8));
    assert_eq!((size_of::<DIR>(), align_of::<DIR>()), (128, 8));
    assert_eq!((size_of::<sem_t>(), align_of::<sem_t>()), (32, 8));
    assert_eq!(
        (size_of::<pthread_mutex_t>(), align_of::<pthread_mutex_t>()),
        (32, 8)
    );
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
