// SPDX-License-Identifier: GPL-3.0-only
//! `sh`: the first interactive IRIX 7 userland shell.
//!
//! A `no_std` Rust program on the relibc C ABI.  It reads command lines from
//! the console terminal, runs builtins in the shell, runs the small utilities
//! in [`applets`] in a forked child and runs anything else with `execve`
//! after a `PATH` search.  Pipelines, `<`, `>`, `>>`, `;`, `&&`, `||`, quoting
//! and `$NAME` expansion are supported.  There is no globbing, no job control
//! and no scripting language.
//!
//! [`parse`] is pure and is tested on the host.  Everything that calls the
//! libc is compiled for the `irix7` target only.

#![no_std]

pub mod parse;

#[cfg(target_os = "irix7")]
mod applets;
#[cfg(target_os = "irix7")]
mod io;
#[cfg(target_os = "irix7")]
mod libc;
#[cfg(target_os = "irix7")]
mod shell;

#[cfg(target_os = "irix7")]
#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    io::write_all(2, b"sh: panic\n");
    // SAFETY: `_exit` does not return.
    unsafe { libc::_exit(101) }
}

/// C entry point; relibc's crt0 calls it.
#[cfg(target_os = "irix7")]
#[no_mangle]
pub extern "C" fn main(argc: i32, argv: *const *const u8) -> i32 {
    // SAFETY: crt0 passes `argc` valid NUL-terminated argument pointers.
    let args = unsafe { core::slice::from_raw_parts(argv, argc.max(0) as usize) };
    shell::run(args)
}
