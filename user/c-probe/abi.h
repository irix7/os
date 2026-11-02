// SPDX-License-Identifier: GPL-3.0-only
//! Shared n32 ABI fixture for the freestanding C probe.
//!
//! These definitions are the C oracle for `crates/native-abi` and relibc's
//! `src/header/sys_stat/mod.rs`.  The static assertions compile only for the
//! n32 big-endian target; they are the fixture that pins every offset.

#ifndef IRIX7_C_PROBE_ABI_H
#define IRIX7_C_PROBE_ABI_H

typedef unsigned char u8;
typedef unsigned short u16;
typedef unsigned int u32;
typedef unsigned long long u64;
typedef signed int i32;
typedef signed long long i64;

/* Native gate and syscall numbers.  Keep in step with
 * crates/native-abi/src/syscall.rs. */
#define NATIVE_GATE_CODE 0x1F7

#define SYS_WRITE 1
#define SYS_EXIT 2
#define SYS_OPEN 6
#define SYS_CLOSE 7
#define SYS_GETPID 15
#define SYS_READ 16
#define SYS_FSTAT 17
#define SYS_STAT 18
#define SYS_READDIR 19
#define SYS_CLOCK_GETTIME 14

/* Native status codes.  Keep in step with
 * crates/native-abi/src/status.rs. */
#define CELL_OK 0
#define CELL_E_PERM 1
#define CELL_E_NOENT 2
#define CELL_E_INTR 3
#define CELL_E_IO 4
#define CELL_E_BADF 5
#define CELL_E_AGAIN 6
#define CELL_E_NOMEM 7
#define CELL_E_ACCESS 8
#define CELL_E_FAULT 9
#define CELL_E_BUSY 10
#define CELL_E_EXIST 11
#define CELL_E_INVAL 12
#define CELL_E_NOSPC 13
#define CELL_E_NOTDIR 14
#define CELL_E_ISDIR 15
#define CELL_E_NOTEMPTY 16
#define CELL_E_NOTSUP 17
#define CELL_E_OVERFLOW 18
#define CELL_E_TIMEDOUT 19
#define CELL_E_CANCELED 20
#define CELL_E_REVOKED 21
#define CELL_E_MSGSIZE 22
#define CELL_E_PROTO 23
#define CELL_E_CONNREFUSED 24
#define CELL_E_NOTCONN 25
#define CELL_E_ADDRINUSE 26
#define CELL_E_NETUNREACH 27
#define CELL_E_HOSTUNREACH 28
#define CELL_E_NAMETOOLONG 29
#define CELL_E_LOOP 30
#define CELL_E_CHILD 31
#define CELL_E_SRCH 32
#define CELL_E_NOSYS 33
#define CELL_E_PIPE 34
#define CELL_E_RANGE 35
#define CELL_E_DOM 36
#define CELL_E_DEADLK 37
#define CELL_E_NOLCK 38
#define CELL_E_NOMSG 39
#define CELL_E_IDRM 40
#define CELL_E_NOTTY 41
#define CELL_E_SPIPE 42
#define CELL_E_TXTBSY 43
#define CELL_E_FBIG 44
#define CELL_E_MLINK 45
#define CELL_E_ROFS 46
#define CELL_E_NODEV 47
#define CELL_E_NXIO 48
#define CELL_E_NOTBLK 49
#define CELL_E_2BIG 50
#define CELL_E_MFILE 51
#define CELL_E_NFILE 52

#define CELL_STATUS_COUNT 53

/* libc errno values (Linux/asm-generic mapping). */
#define EBADF 9
#define ENOENT 2
#define EFAULT 14

struct timespec {
    i64 tv_sec;
    i32 tv_nsec;
    u32 _pad;
};

/* 136 bytes.  Field order matches relibc's n32 `struct stat`. */
struct stat {
    u64 st_dev;
    u64 st_ino;
    u32 st_nlink;
    u32 st_mode;
    u32 st_uid;
    u32 st_gid;
    u64 st_rdev;
    i64 st_size;
    i32 st_blksize;
    u32 _pad0;
    i64 st_blocks;
    struct timespec st_atim;
    struct timespec st_mtim;
    struct timespec st_ctim;
    u8 _pad[24];
};

/* 280 bytes.  `d_type` is the documented extension; `d_name` is 256 bytes. */
struct dirent {
    u64 d_ino;
    i64 d_off;
    u16 d_reclen;
    u8 d_type;
    char d_name[256];
};

/* The main-thread TCB.  `$28` points at `self`; MIPS III has no `rdhwr`. */
struct tcb {
    u32 self;
    u32 dtv;
    i32 errno_value;
    u32 last_status;
};

_Static_assert(sizeof(struct timespec) == 16, "timespec size");
_Static_assert(sizeof(struct stat) == 136, "stat size");
_Static_assert(__builtin_offsetof(struct stat, st_dev) == 0, "st_dev");
_Static_assert(__builtin_offsetof(struct stat, st_ino) == 8, "st_ino");
_Static_assert(__builtin_offsetof(struct stat, st_nlink) == 16, "st_nlink");
_Static_assert(__builtin_offsetof(struct stat, st_mode) == 20, "st_mode");
_Static_assert(__builtin_offsetof(struct stat, st_uid) == 24, "st_uid");
_Static_assert(__builtin_offsetof(struct stat, st_gid) == 28, "st_gid");
_Static_assert(__builtin_offsetof(struct stat, st_rdev) == 32, "st_rdev");
_Static_assert(__builtin_offsetof(struct stat, st_size) == 40, "st_size");
_Static_assert(__builtin_offsetof(struct stat, st_blksize) == 48, "st_blksize");
_Static_assert(__builtin_offsetof(struct stat, st_blocks) == 56, "st_blocks");
_Static_assert(__builtin_offsetof(struct stat, st_atim) == 64, "st_atim");
_Static_assert(__builtin_offsetof(struct stat, st_mtim) == 80, "st_mtim");
_Static_assert(__builtin_offsetof(struct stat, st_ctim) == 96, "st_ctim");
_Static_assert(__builtin_offsetof(struct stat, _pad) == 112, "_pad");
_Static_assert(sizeof(struct dirent) == 280, "dirent size");
_Static_assert(__builtin_offsetof(struct dirent, d_ino) == 0, "d_ino");
_Static_assert(__builtin_offsetof(struct dirent, d_off) == 8, "d_off");
_Static_assert(__builtin_offsetof(struct dirent, d_reclen) == 16, "d_reclen");
_Static_assert(__builtin_offsetof(struct dirent, d_type) == 18, "d_type");
_Static_assert(__builtin_offsetof(struct dirent, d_name) == 19, "d_name");
_Static_assert(sizeof(struct tcb) == 16, "tcb size");
_Static_assert(__builtin_offsetof(struct tcb, self) == 0, "tcb self");
_Static_assert(__builtin_offsetof(struct tcb, dtv) == 4, "tcb dtv");
_Static_assert(__builtin_offsetof(struct tcb, errno_value) == 8, "tcb errno");

/* Auxiliary-vector keys. */
#define AT_NULL 0
#define AT_PAGESZ 1
#define AT_ENTRY 2
#define AT_PHDR 3
#define AT_PHNUM 4
#define AT_BASE 5
#define AT_RANDOM 6
#define AT_EXECFN 7
#define AT_UID 8
#define AT_EUID 9
#define AT_GID 10
#define AT_EGID 11
#define AT_HWCAP 12
#define AT_CLKTCK 13
#define AT_SECURE 14
#define AT_PLATFORM 15

#define S_IFMT 0170000
#define S_IFREG 0100000
#define S_ISREG(mode) (((mode) & S_IFMT) == S_IFREG)

extern const unsigned char status_errno[CELL_STATUS_COUNT];
extern struct tcb main_tcb;

long __irix7_syscall0(long number);
long __irix7_syscall1(long number, long a0);
long __irix7_syscall2(long number, long a0, long a1);
long __irix7_syscall3(long number, long a0, long a1, long a2);

#endif
