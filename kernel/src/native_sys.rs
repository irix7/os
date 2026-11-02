// SPDX-License-Identifier: GPL-3.0-only
//! Native POSIX syscall gate.
//!
//! The native gate is `syscall 0x1F7`.  The caller puts the native syscall
//! number in `$v0` and arguments in `$a0`–`$a3`.  On return `$v0` holds a
//! `CELL_E_*` status and `$v1` holds the result; the kernel never returns a
//! libc `errno`.  The `CellStart` capability ABI is separate (`native-abi.md`
//! §4) and does not reuse these numbers.
//!
//! Most calls leave the frame in place and the trap epilogue advances `EPC`
//! past the gate.  A call that switches threads replaces the whole frame, so
//! it returns [`Gate::Resume`] and the epilogue must not advance it again.

use native_abi::syscall::{
    SYS_CHDIR, SYS_CLOCK_GETTIME, SYS_CLOSE, SYS_DUP, SYS_DUP2, SYS_EXIT, SYS_FCNTL, SYS_FSTAT,
    SYS_FTRUNCATE, SYS_FUTEX_WAIT, SYS_FUTEX_WAKE, SYS_GETCWD, SYS_GETDENTS, SYS_GETENTROPY,
    SYS_GETPID, SYS_LSEEK, SYS_MKDIR, SYS_MMAP, SYS_MPROTECT, SYS_MUNMAP, SYS_NANOSLEEP, SYS_OPEN,
    SYS_PIPE, SYS_PIPE2, SYS_POLL, SYS_PREAD, SYS_PWRITE, SYS_READ, SYS_READDIR, SYS_RENAME,
    SYS_RMDIR, SYS_SIGNAL, SYS_STAT, SYS_TCGETWINSIZE, SYS_TCSETWINSIZE, SYS_THREAD_CREATE,
    SYS_THREAD_EXIT, SYS_TRUNCATE, SYS_UNLINK, SYS_WRITE, SYS_YIELD,
};

use crate::syserr::{SysResult, CELL_E_NOSYS};
use crate::thread::{Exit, Wait};
use crate::traps::TrapFrame;

/// What the trap epilogue must do with `EPC`.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Gate {
    /// The same thread continues; advance `EPC` past the gate instruction.
    Continue,
    /// The frame has already been replaced or prepared; do not touch `EPC`.
    Resume,
}

/// Dispatch one native gate invocation.
pub fn dispatch(tf: &mut TrapFrame) -> Gate {
    let number = tf.arg(2);
    let a0 = tf.arg(4);
    let a1 = tf.arg(5);
    let a2 = tf.arg(6);
    let a3 = tf.arg(7);

    if number == SYS_EXIT {
        crate::traps::finish_user_exit(tf, a0);
        return Gate::Resume;
    }

    if number == SYS_YIELD {
        tf.regs[2] = native_abi::status::CELL_OK as u64;
        tf.regs[3] = 0;
        // `yield_current` advances the outgoing EPC itself when it switches;
        // without a peer the normal `Gate::Continue` path advances it.
        return if crate::thread::yield_current(tf) {
            Gate::Resume
        } else {
            Gate::Continue
        };
    }

    if number == SYS_THREAD_EXIT {
        return match crate::thread::exit_current(tf, a0) {
            Exit::Switched => Gate::Resume,
            Exit::Last => {
                crate::traps::finish_user_exit(tf, a0);
                Gate::Resume
            }
        };
    }

    if number == SYS_FUTEX_WAIT {
        // A wait may park this thread, so it returns through the same
        // frame-replacement path as `yield` and `thread_exit`.
        return match crate::thread::futex_wait(tf, a0, a1, a2 as i32) {
            Wait::Switched => Gate::Resume,
            Wait::Continue => Gate::Continue,
        };
    }

    if number == SYS_NANOSLEEP {
        // A sleep parks this thread on a Count deadline and lets a peer run;
        // with no peer the kernel parks the CPU until the timer.  Either way
        // the saved frame carries `CELL_OK`, so a switch must not advance EPC
        // again.
        let limit = match crate::user::sleep_ticks(a0) {
            Ok(limit) => limit,
            Err(status) => {
                tf.regs[2] = status as u64;
                tf.regs[3] = 0;
                return Gate::Continue;
            }
        };
        return match crate::thread::sleep_current(tf, limit) {
            Wait::Switched => Gate::Resume,
            Wait::Continue => Gate::Continue,
        };
    }

    let result: SysResult<u64> = match number {
        SYS_WRITE => crate::user::write_fd(a0, a1, a2).map(|count| count as u64),
        SYS_READ => crate::user::read_fd(a0, a1, a2).map(|count| count as u64),
        SYS_MMAP => crate::user::mmap(a1).map(|address| address as u64),
        SYS_MUNMAP | SYS_MPROTECT => Ok(0),
        SYS_OPEN => crate::user::open_path(a0, a1, a2).map(|fd| fd as u64),
        SYS_UNLINK => crate::user::unlink_user(a0, false).map(|()| 0),
        SYS_RENAME => crate::user::rename_user(a0, a1).map(|()| 0),
        SYS_MKDIR => crate::user::mkdir_user(a0, a1).map(|()| 0),
        SYS_RMDIR => crate::user::unlink_user(a0, true).map(|()| 0),
        SYS_FTRUNCATE => crate::user::ftruncate_user(a0, a1, a2).map(|()| 0),
        SYS_TRUNCATE => crate::user::truncate_user(a0, a1, a2).map(|()| 0),
        SYS_CLOSE => crate::fd::close(a0 as i32).map(|()| 0),
        SYS_DUP => crate::fd::dup(a0 as i32).map(|fd| fd as u64),
        SYS_GETCWD => crate::user::getcwd_text(a0, a1).map(|()| 0),
        SYS_POLL => crate::user::poll_user(a0, a1, a2 as i32).map(|ready| ready as u64),
        SYS_GETENTROPY => crate::user::entropy_user(a0, a1).map(|()| 0),
        SYS_CLOCK_GETTIME => crate::user::clock_user(a0, a1).map(|()| 0),
        SYS_GETPID => Ok(1),
        SYS_FSTAT => crate::user::fstat_user(a0, a1).map(|()| 0),
        SYS_STAT => crate::user::stat_user(a0, a1).map(|()| 0),
        SYS_READDIR => {
            crate::user::readdir_user(a0, a1, a2, a3).map(|entry| entry.unwrap_or(0) as u64)
        }
        SYS_GETDENTS => crate::user::getdents_user(a0, a1, a2).map(|count| count as u64),
        SYS_LSEEK => crate::user::lseek_user(a0, a1, a2, a3).map(|offset| offset as u64),
        SYS_PREAD => crate::user::pread_user(a0, a1).map(|count| count as u64),
        SYS_PWRITE => crate::user::pwrite_user(a0, a1).map(|count| count as u64),
        SYS_DUP2 => crate::user::dup2_user(a0, a1).map(|fd| fd as u64),
        SYS_PIPE2 => crate::user::pipe2_user(a0, a1).map(|()| 0),
        SYS_CHDIR => crate::user::chdir_user(a0).map(|()| 0),
        SYS_PIPE => crate::user::pipe_user(a0).map(|()| 0),
        SYS_TCGETWINSIZE => crate::user::tcgetwinsize_user(a0, a1).map(|()| 0),
        SYS_TCSETWINSIZE => crate::user::tcsetwinsize_user(a0, a1).map(|()| 0),
        // Signals are still absent.  std ignores SIGPIPE, and the kernel has no
        // signal machinery, so this is an explicit no-op success, not ENOSYS.
        SYS_FCNTL => crate::user::fcntl_user(a0, a1, a2).map(|value| value as u64),
        SYS_SIGNAL => Ok(0),
        SYS_THREAD_CREATE => crate::thread::create(tf.status, a0, a1, a2, a3).map(|tid| tid as u64),
        SYS_FUTEX_WAKE => crate::thread::futex_wake(a0, a1).map(|woken| woken as u64),
        _ => Err(CELL_E_NOSYS),
    };

    match result {
        Ok(value) => {
            tf.regs[2] = native_abi::status::CELL_OK as u64;
            tf.regs[3] = value;
        }
        Err(status) => {
            tf.regs[2] = status as u64;
            tf.regs[3] = 0;
        }
    }
    Gate::Continue
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gate_instruction_is_the_spec_value() {
        assert_eq!(native_abi::syscall::NATIVE_GATE_INSTRUCTION, 0x0000_7DCC);
    }
}
