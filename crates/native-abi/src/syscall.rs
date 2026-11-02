// SPDX-License-Identifier: GPL-3.0-only
//! Native IRIX 7 syscall numbers and gate encoding.
//!
//! This is the v0 flat syscall surface used by the C probe and the relibc PAL.
//! It is deliberately not the IRIX 6.5 number space: the gate instruction
//! (`syscall 0x1F7`) selects this ABI, and the capability method encoding in
//! `docs/spec/native-abi.md` §4 will replace the flat calls without reusing
//! these numbers.

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
pub const SYS_POLL: u32 = 11;
pub const SYS_SIGNAL: u32 = 12;
pub const SYS_GETENTROPY: u32 = 13;
pub const SYS_CLOCK_GETTIME: u32 = 14;
pub const SYS_GETPID: u32 = 15;
pub const SYS_READ: u32 = 16;
pub const SYS_FSTAT: u32 = 17;
pub const SYS_STAT: u32 = 18;
pub const SYS_READDIR: u32 = 19;
pub const SYS_NANOSLEEP: u32 = 20;
pub const SYS_CHDIR: u32 = 21;
pub const SYS_PIPE: u32 = 22;
pub const SYS_TCGETWINSIZE: u32 = 23;
pub const SYS_TCSETWINSIZE: u32 = 24;
