// SPDX-License-Identifier: GPL-3.0-only
//! Native IRIX 7 syscall numbers and gate encoding.
//!
//! This is the v0 flat syscall surface used by the C probe and the relibc PAL.
//! It is deliberately not the IRIX 6.5 number space: the gate instruction
//! (`syscall 0x1F7`) selects this ABI, and the capability method encoding in
//! `docs/programming/native-abi.md` §4 will replace the flat calls without reusing
//! these numbers.
//!
//! The numbers are pinned by the v0 table in `docs/programming/native-abi.md` §5.8.
//! `SYS_GETDENTS` and `SYS_LSEEK` carry the directory cookie; `SYS_READDIR`
//! stays as the one-name wrapper the static `std` shim uses.

/// The 20-bit `syscall` code field for the native gate.
pub const NATIVE_GATE_CODE: u32 = 0x1F7;

/// The encoded `syscall 0x1F7` instruction word.
pub const NATIVE_GATE_INSTRUCTION: u32 = (NATIVE_GATE_CODE << 6) | 0x0C;

pub const SYS_WRITE: u32 = 1;
pub const SYS_EXIT: u32 = 2;
pub const SYS_MMAP: u32 = 3;
pub const SYS_MUNMAP: u32 = 4;
pub const SYS_MPROTECT: u32 = 5;
pub const SYS_OPEN: u32 = 6;
pub const SYS_CLOSE: u32 = 7;
pub const SYS_FCNTL: u32 = 8;
pub const SYS_DUP: u32 = 9;
pub const SYS_GETCWD: u32 = 10;
/// `poll(fds, count, timeout_ms)`; a negative timeout is bounded by the kernel.
pub const SYS_POLL: u32 = 11;
pub const SYS_SIGNAL: u32 = 12;
pub const SYS_GETENTROPY: u32 = 13;
pub const SYS_CLOCK_GETTIME: u32 = 14;
pub const SYS_GETPID: u32 = 15;
pub const SYS_READ: u32 = 16;
pub const SYS_FSTAT: u32 = 17;
pub const SYS_STAT: u32 = 18;
pub const SYS_READDIR: u32 = 19;
/// `nanosleep(timespec *)`; parks the caller until the interval expires, for
/// at most `SLEEP_LIMIT_MS` (60 s) per call.
pub const SYS_NANOSLEEP: u32 = 20;
pub const SYS_CHDIR: u32 = 21;
pub const SYS_PIPE: u32 = 22;
pub const SYS_TCGETWINSIZE: u32 = 23;
pub const SYS_TCSETWINSIZE: u32 = 24;
/// `lseek(fd, offset_lo, offset_hi, whence)`; returns the new offset.
pub const SYS_LSEEK: u32 = 25;
/// `pread(fd, io_at *)`; the block carries the buffer, length and 64-bit offset.
pub const SYS_PREAD: u32 = 26;
/// `pwrite(fd, io_at *)`; the block carries the buffer, length and 64-bit offset.
pub const SYS_PWRITE: u32 = 27;
pub const SYS_DUP2: u32 = 28;
pub const SYS_PIPE2: u32 = 29;
/// `getdents(fd, buf, len)`; fills whole `dirent` records and advances the
/// descriptor's directory cookie.
pub const SYS_GETDENTS: u32 = 30;
/// `unlink(path)`; removes a regular file.
pub const SYS_UNLINK: u32 = 31;
/// `rename(old, new)`; replaces an existing destination of the same type.
pub const SYS_RENAME: u32 = 32;
/// `mkdir(path, mode)`; creates a directory.
pub const SYS_MKDIR: u32 = 33;
/// `rmdir(path)`; removes an empty directory.
pub const SYS_RMDIR: u32 = 34;
/// `ftruncate(fd, length_lo, length_hi)`; sets a regular file's length.
pub const SYS_FTRUNCATE: u32 = 35;
/// `truncate(path, length_lo, length_hi)`; the path form of `ftruncate`.
pub const SYS_TRUNCATE: u32 = 36;
/// `thread_create(entry, arg, stack_top, tcb)` -> tid; the main thread is 1.
pub const SYS_THREAD_CREATE: u32 = 37;
/// `yield()`; save this thread's frame and resume the next runnable thread.
pub const SYS_YIELD: u32 = 38;
/// `thread_exit(status)`; retire this thread, or return to the console when
/// it was the last one.
pub const SYS_THREAD_EXIT: u32 = 39;
/// `futex_wait(uaddr, expected, timeout_ms)`; block while the 32-bit user word
/// at `uaddr` equals `expected`.  A negative timeout is bounded by the poll
/// rule and a positive one is clamped to the same bound; zero probes.
pub const SYS_FUTEX_WAIT: u32 = 40;
/// `futex_wake(uaddr, count)`; make up to `count` waiters on `uaddr` runnable
/// and return how many were woken.
pub const SYS_FUTEX_WAKE: u32 = 41;
/// `fork()`; clone the calling cell and return the child's local pid in the
/// parent and 0 in the child.
pub const SYS_FORK: u32 = 42;
/// `execve(path, argv, envp)`; replace the cell image from a ramfs path and
/// enter the new image.  Does not return on success.
pub const SYS_EXECVE: u32 = 43;
/// `waitpid(pid, status, options)`; reap a local child, blocking until it
/// exits unless `WNOHANG` is set.
pub const SYS_WAITPID: u32 = 44;
/// `wait4(pid, status, options, rusage)`; the `waitpid` fast path with the
/// `rusage` argument accepted and zero-filled in v0.
pub const SYS_WAIT4: u32 = 45;
/// `getppid()`; the local parent pid, or 0 when there is no live parent.
pub const SYS_GETPPID: u32 = 46;
/// `cell_id(struct cell_id *)`; copy the mesh-qualified process identity.
pub const SYS_CELL_ID: u32 = 47;
/// `fchdir(fd)`; make the directory named by an open descriptor the cwd.
/// `EBADF` for a closed descriptor, `ENOTDIR` for anything but a directory.
pub const SYS_FCHDIR: u32 = 48;
/// `ioctl(fd, request, arg)`; the terminal requests on a console descriptor:
/// `TCGETS`, `TCSETS`, `TCSETSW`, `TCSETSF`, `TIOCGWINSZ`, `TIOCSWINSZ`,
/// `TIOCGPGRP` and `TIOCSPGRP`.  Anything else is `ENOTTY`.
pub const SYS_IOCTL: u32 = 49;
/// `kill(pid, sig)`; `pid > 0` names a process, `0` the caller's group,
/// `-1` every process but the caller and `< -1` the group `-pid`.  Signal 0
/// only checks that a target exists.
pub const SYS_KILL: u32 = 50;
/// `sigaction(sig, const uint32_t *disposition, uint32_t *old)`; a disposition
/// is `SIG_DFL` (0) or `SIG_IGN` (1).  A handler address is `ENOSYS` in v0.
pub const SYS_SIGACTION: u32 = 51;
/// `setpgid(pid, pgid)`; zero names the caller and the target's own pid.
pub const SYS_SETPGID: u32 = 52;
/// `getpgid(pid)`; zero names the caller.
pub const SYS_GETPGID: u32 = 53;
