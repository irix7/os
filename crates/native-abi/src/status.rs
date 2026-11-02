// SPDX-License-Identifier: GPL-3.0-only
//! Native syscall status codes.
//!
//! These are the values the kernel returns in `$v0` from the native gate.  The
//! numbers are part of the kernel ABI; the C `errno` values they map to are a
//! libc decision and live in [`status_to_errno`].  The kernel never returns a
//! libc `errno`.

use crate::types::c_int;

pub const CELL_OK: u32 = 0;
pub const CELL_E_PERM: u32 = 1;
pub const CELL_E_NOENT: u32 = 2;
pub const CELL_E_INTR: u32 = 3;
pub const CELL_E_IO: u32 = 4;
pub const CELL_E_BADF: u32 = 5;
pub const CELL_E_AGAIN: u32 = 6;
pub const CELL_E_NOMEM: u32 = 7;
pub const CELL_E_ACCESS: u32 = 8;
pub const CELL_E_FAULT: u32 = 9;
pub const CELL_E_BUSY: u32 = 10;
pub const CELL_E_EXIST: u32 = 11;
pub const CELL_E_INVAL: u32 = 12;
pub const CELL_E_NOSPC: u32 = 13;
pub const CELL_E_NOTDIR: u32 = 14;
pub const CELL_E_ISDIR: u32 = 15;
pub const CELL_E_NOTEMPTY: u32 = 16;
pub const CELL_E_NOTSUP: u32 = 17;
pub const CELL_E_OVERFLOW: u32 = 18;
pub const CELL_E_TIMEDOUT: u32 = 19;
pub const CELL_E_CANCELED: u32 = 20;
pub const CELL_E_REVOKED: u32 = 21;
pub const CELL_E_MSGSIZE: u32 = 22;
pub const CELL_E_PROTO: u32 = 23;
pub const CELL_E_CONNREFUSED: u32 = 24;
pub const CELL_E_NOTCONN: u32 = 25;
pub const CELL_E_ADDRINUSE: u32 = 26;
pub const CELL_E_NETUNREACH: u32 = 27;
pub const CELL_E_HOSTUNREACH: u32 = 28;
pub const CELL_E_NAMETOOLONG: u32 = 29;
pub const CELL_E_LOOP: u32 = 30;
pub const CELL_E_CHILD: u32 = 31;
pub const CELL_E_SRCH: u32 = 32;
pub const CELL_E_NOSYS: u32 = 33;
pub const CELL_E_PIPE: u32 = 34;
pub const CELL_E_RANGE: u32 = 35;
pub const CELL_E_DOM: u32 = 36;
pub const CELL_E_DEADLK: u32 = 37;
pub const CELL_E_NOLCK: u32 = 38;
pub const CELL_E_NOMSG: u32 = 39;
pub const CELL_E_IDRM: u32 = 40;
pub const CELL_E_NOTTY: u32 = 41;
pub const CELL_E_SPIPE: u32 = 42;
pub const CELL_E_TXTBSY: u32 = 43;
pub const CELL_E_FBIG: u32 = 44;
pub const CELL_E_MLINK: u32 = 45;
pub const CELL_E_ROFS: u32 = 46;
pub const CELL_E_NODEV: u32 = 47;
pub const CELL_E_NXIO: u32 = 48;
pub const CELL_E_NOTBLK: u32 = 49;
pub const CELL_E_2BIG: u32 = 50;
pub const CELL_E_MFILE: u32 = 51;
pub const CELL_E_NFILE: u32 = 52;
pub const CELL_E_NOEXEC: u32 = 53;

/// Map a native status to the libc `errno` value.
///
/// This is the authoritative mapping for the native libc and the C probe.  It
/// is a pure function so it can be tested on the host and mirrored by the C
/// `status_errno[]` table.
pub const fn status_to_errno(status: u32) -> c_int {
    match status {
        CELL_OK => 0,
        CELL_E_PERM => 1,
        CELL_E_NOENT => 2,
        CELL_E_INTR => 4,
        CELL_E_IO => 5,
        CELL_E_BADF => 9,
        CELL_E_AGAIN => 11,
        CELL_E_NOMEM => 12,
        CELL_E_ACCESS => 13,
        CELL_E_FAULT => 14,
        CELL_E_BUSY => 16,
        CELL_E_EXIST => 17,
        CELL_E_INVAL => 22,
        CELL_E_NOSPC => 28,
        CELL_E_NOTDIR => 20,
        CELL_E_ISDIR => 21,
        CELL_E_NOTEMPTY => 39,
        CELL_E_NOTSUP => 95,
        CELL_E_OVERFLOW => 75,
        CELL_E_TIMEDOUT => 110,
        CELL_E_CANCELED => 125,
        CELL_E_REVOKED => 9,
        CELL_E_MSGSIZE => 90,
        CELL_E_PROTO => 71,
        CELL_E_CONNREFUSED => 111,
        CELL_E_NOTCONN => 107,
        CELL_E_ADDRINUSE => 98,
        CELL_E_NETUNREACH => 101,
        CELL_E_HOSTUNREACH => 113,
        CELL_E_NAMETOOLONG => 36,
        CELL_E_LOOP => 40,
        CELL_E_CHILD => 10,
        CELL_E_SRCH => 3,
        CELL_E_NOSYS => 38,
        CELL_E_PIPE => 32,
        CELL_E_RANGE => 34,
        CELL_E_DOM => 33,
        CELL_E_DEADLK => 35,
        CELL_E_NOLCK => 37,
        CELL_E_NOMSG => 42,
        CELL_E_IDRM => 43,
        CELL_E_NOTTY => 25,
        CELL_E_SPIPE => 29,
        CELL_E_TXTBSY => 26,
        CELL_E_FBIG => 27,
        CELL_E_MLINK => 31,
        CELL_E_ROFS => 30,
        CELL_E_NODEV => 19,
        CELL_E_NXIO => 6,
        CELL_E_NOTBLK => 15,
        CELL_E_2BIG => 7,
        CELL_E_MFILE => 24,
        CELL_E_NFILE => 23,
        CELL_E_NOEXEC => 8,
        _ => 5, // EIO for an unknown status; never guessed as success.
    }
}
