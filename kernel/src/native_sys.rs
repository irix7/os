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
    SYS_CELL_ID, SYS_CHDIR, SYS_CLOCK_GETTIME, SYS_CLOSE, SYS_DUP, SYS_DUP2, SYS_EXECVE, SYS_EXIT,
    SYS_FCHDIR, SYS_FCNTL, SYS_FORK, SYS_FSTAT, SYS_FTRUNCATE, SYS_FUTEX_WAIT, SYS_FUTEX_WAKE,
    SYS_GETCWD, SYS_GETDENTS, SYS_GETENTROPY, SYS_GETPGID, SYS_GETPID, SYS_GETPPID, SYS_IOCTL,
    SYS_KILL, SYS_LSEEK, SYS_MKDIR,
    SYS_MMAP, SYS_MPROTECT, SYS_MUNMAP, SYS_NANOSLEEP, SYS_OPEN, SYS_PIPE, SYS_PIPE2, SYS_POLL,
    SYS_PREAD, SYS_PWRITE, SYS_READ, SYS_READDIR, SYS_RENAME, SYS_RMDIR, SYS_SETPGID,
    SYS_SIGACTION, SYS_SIGNAL, SYS_STAT,
    SYS_TCGETWINSIZE, SYS_TCSETWINSIZE, SYS_THREAD_CREATE, SYS_THREAD_EXIT, SYS_TRUNCATE,
    SYS_UNLINK, SYS_WAIT4, SYS_WAITPID, SYS_WRITE, SYS_YIELD,
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
///
/// The console line discipline runs first, so a signal character typed while
/// a program computes is seen at its next call.  A fatal signal posted to the
/// running cell is applied after the call, when its frame can be replaced.
pub fn dispatch(tf: &mut TrapFrame) -> Gate {
    crate::tty::pump();
    let gate = call(tf);
    if crate::process::deliver_pending(tf) {
        Gate::Resume
    } else {
        gate
    }
}

/// Re-run the gate later for a blocking descriptor that is not ready.
///
/// `EPC` is left on the gate instruction, so the caller repeats the call when
/// it next runs.  Another thread or cell gets the CPU if one is runnable;
/// otherwise the CPU parks for one slice.  A pending signal ends the wait with
/// `EINTR` so the epilogue can apply it.
fn restart(tf: &mut TrapFrame) -> Gate {
    if crate::process::signal_pending() {
        tf.regs[2] = native_abi::status::CELL_E_INTR as u64;
        tf.regs[3] = 0;
        return Gate::Continue;
    }
    // Both yield paths advance the outgoing EPC by one instruction.  Step
    // back first so the saved frame resumes at the gate.
    tf.epc = tf.epc.wrapping_sub(4);
    if crate::thread::yield_current(tf) || crate::process::yield_current(tf) {
        return Gate::Resume;
    }
    tf.epc = tf.epc.wrapping_add(4);
    crate::traps::wait_until(crate::traps::count_read().wrapping_add(crate::fd::WAIT_SLICE_TICKS));
    Gate::Resume
}

fn call(tf: &mut TrapFrame) -> Gate {
    let number = tf.arg(2);
    let a0 = tf.arg(4);
    let a1 = tf.arg(5);
    let a2 = tf.arg(6);
    let a3 = tf.arg(7);

    if number == SYS_EXIT {
        return match crate::process::exit_current(tf, a0) {
            crate::process::Exit::Switched => Gate::Resume,
            crate::process::Exit::Last => {
                crate::traps::finish_user_exit(tf, a0);
                Gate::Resume
            }
        };
    }

    if number == SYS_FORK {
        return match crate::process::fork(tf) {
            Ok(pid) => {
                tf.regs[2] = native_abi::status::CELL_OK as u64;
                tf.regs[3] = pid as u64;
                Gate::Continue
            }
            Err(status) => {
                tf.regs[2] = status as u64;
                tf.regs[3] = 0;
                Gate::Continue
            }
        };
    }

    if number == SYS_EXECVE {
        return match crate::process::exec(tf, a0, a1, a2) {
            Ok(()) => Gate::Resume,
            Err(status) => {
                tf.regs[2] = status as u64;
                tf.regs[3] = 0;
                Gate::Continue
            }
        };
    }

    if number == SYS_WAITPID || number == SYS_WAIT4 {
        let rusage = if number == SYS_WAIT4 { a3 } else { 0 };
        return match crate::process::wait(tf, a0 as i32, a1, a2 as i32, rusage) {
            Ok(crate::process::Wait::Switched) => Gate::Resume,
            Ok(crate::process::Wait::Continue) => Gate::Continue,
            Err(status) => {
                tf.regs[2] = status as u64;
                tf.regs[3] = 0;
                Gate::Continue
            }
        };
    }

    if number == SYS_YIELD {
        tf.regs[2] = native_abi::status::CELL_OK as u64;
        tf.regs[3] = 0;
        // A thread yield first tries another thread in this cell.  If none is
        // runnable, hand the CPU to another local cell; both switch paths
        // advance the outgoing EPC themselves.  Without either peer the normal
        // `Gate::Continue` path advances it.
        if crate::thread::yield_current(tf) {
            return Gate::Resume;
        }
        return if crate::process::yield_current(tf) {
            Gate::Resume
        } else {
            Gate::Continue
        };
    }

    if number == SYS_THREAD_EXIT {
        return match crate::thread::exit_current(tf, a0) {
            Exit::Switched => Gate::Resume,
            Exit::Last => match crate::process::exit_current(tf, a0) {
                crate::process::Exit::Switched => Gate::Resume,
                crate::process::Exit::Last => {
                    crate::traps::finish_user_exit(tf, a0);
                    Gate::Resume
                }
            },
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

    if number == SYS_READ || number == SYS_WRITE {
        let result = if number == SYS_READ {
            crate::user::read_fd_once(a0, a1, a2)
        } else {
            crate::user::write_fd(a0, a1, a2)
        };
        if result == Err(crate::syserr::CELL_E_AGAIN) && !crate::fd::nonblocking(a0 as i32) {
            return restart(tf);
        }
        match result {
            Ok(count) => {
                tf.regs[2] = native_abi::status::CELL_OK as u64;
                tf.regs[3] = count as u64;
            }
            Err(status) => {
                tf.regs[2] = status as u64;
                tf.regs[3] = 0;
            }
        }
        return Gate::Continue;
    }

    let result: SysResult<u64> = match number {
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
        SYS_GETPID => Ok(crate::process::current_pid() as u64),
        SYS_GETPPID => Ok(crate::process::current_ppid() as u64),
        SYS_CELL_ID => crate::user::cell_id_user(a0).map(|()| 0),
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
        SYS_FCHDIR => crate::fd::fchdir(a0 as i32).map(|()| 0),
        SYS_PIPE => crate::user::pipe_user(a0).map(|()| 0),
        SYS_TCGETWINSIZE => crate::user::tcgetwinsize_user(a0, a1).map(|()| 0),
        SYS_TCSETWINSIZE => crate::user::tcsetwinsize_user(a0, a1).map(|()| 0),
        // Signals are still absent.  std ignores SIGPIPE, and the kernel has no
        // signal machinery, so this is an explicit no-op success, not ENOSYS.
        SYS_FCNTL => crate::user::fcntl_user(a0, a1, a2).map(|value| value as u64),
        SYS_SIGNAL => Ok(0),
        SYS_THREAD_CREATE => crate::thread::create(tf.status, a0, a1, a2, a3).map(|tid| tid as u64),
        SYS_FUTEX_WAKE => crate::thread::futex_wake(a0, a1).map(|woken| woken as u64),
        SYS_IOCTL => crate::user::ioctl_user(a0, a1, a2).map(|()| 0),
        SYS_KILL => crate::process::kill(a0 as i32, a1).map(|()| 0),
        SYS_SIGACTION => crate::user::sigaction_user(a0, a1, a2).map(|()| 0),
        SYS_SETPGID => crate::process::setpgid(a0, a1).map(|()| 0),
        SYS_GETPGID => crate::process::getpgid(a0).map(|pgid| pgid as u64),
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
