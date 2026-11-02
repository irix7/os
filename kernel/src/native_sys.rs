// SPDX-License-Identifier: GPL-3.0-only
//! Native POSIX syscall gate.
//!
//! The native gate is `syscall 0x1F7`.  The caller puts the native syscall
//! number in `$v0` and arguments in `$a0`–`$a3`.  On return `$v0` holds a
//! `CELL_E_*` status and `$v1` holds the result; the kernel never returns a
//! libc `errno`.  The `CellStart` capability ABI is separate (`native-abi.md`
//! §4) and does not reuse these numbers.

use native_abi::syscall::{
    SYS_CHDIR, SYS_CLOCK_GETTIME, SYS_CLOSE, SYS_DUP, SYS_DUP2, SYS_EXIT, SYS_FCNTL, SYS_FSTAT,
    SYS_GETCWD, SYS_GETDENTS, SYS_GETENTROPY, SYS_GETPID, SYS_LSEEK, SYS_MMAP, SYS_MPROTECT,
    SYS_MUNMAP, SYS_NANOSLEEP, SYS_OPEN, SYS_PIPE, SYS_PIPE2, SYS_POLL, SYS_PREAD, SYS_PWRITE,
    SYS_READ, SYS_READDIR, SYS_SIGNAL, SYS_STAT, SYS_TCGETWINSIZE, SYS_TCSETWINSIZE, SYS_WRITE,
};

use crate::syserr::{SysResult, CELL_E_NOSYS};
use crate::traps::TrapFrame;

/// Dispatch one native gate invocation.
///
/// Returns `true` if the call terminated the image (`exit`); the caller must
/// not advance EPC in that case.  Every other path writes `$v0`/`$v1` and
/// leaves EPC on the gate instruction.
pub fn dispatch(tf: &mut TrapFrame) -> bool {
    let number = tf.arg(2);
    let a0 = tf.arg(4);
    let a1 = tf.arg(5);
    let a2 = tf.arg(6);
    let a3 = tf.arg(7);

    if number == SYS_EXIT {
        crate::traps::finish_user_exit(tf, a0);
        return true;
    }

    let result: SysResult<u64> = match number {
        SYS_WRITE => crate::user::write_fd(a0, a1, a2).map(|count| count as u64),
        SYS_READ => crate::user::read_fd(a0, a1, a2).map(|count| count as u64),
        SYS_MMAP => crate::user::mmap(a1).map(|address| address as u64),
        SYS_MUNMAP | SYS_MPROTECT => Ok(0),
        SYS_OPEN => crate::user::open_path(a0, a1).map(|fd| fd as u64),
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
        SYS_NANOSLEEP => crate::user::sleep_user(a0).map(|()| 0),
        SYS_CHDIR => crate::user::chdir_user(a0).map(|()| 0),
        SYS_PIPE => crate::user::pipe_user(a0).map(|()| 0),
        SYS_TCGETWINSIZE => crate::user::tcgetwinsize_user(a0, a1).map(|()| 0),
        SYS_TCSETWINSIZE => crate::user::tcsetwinsize_user(a0, a1).map(|()| 0),
        // Signals are still absent.  std ignores SIGPIPE, and the kernel has no
        // signal machinery, so this is an explicit no-op success, not ENOSYS.
        SYS_FCNTL => crate::user::fcntl_user(a0, a1, a2).map(|value| value as u64),
        SYS_SIGNAL => Ok(0),
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
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gate_instruction_is_the_spec_value() {
        assert_eq!(native_abi::syscall::NATIVE_GATE_INSTRUCTION, 0x0000_7DCC);
    }
}
