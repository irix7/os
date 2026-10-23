// SPDX-License-Identifier: GPL-3.0-only
use libc::{AT_FDCWD, c_int, openat};

pub fn read_it(fd: c_int) -> c_int {
    unsafe { libc::read(fd, core::ptr::null_mut(), 0) }
}

pub fn open_it() -> c_int {
    unsafe { openat(AT_FDCWD, core::ptr::null(), 0) }
}

cfg_select! {
    target_os = "linux" => {
        pub fn linux_extra() {
            unsafe { libc::statx(0, core::ptr::null(), 0, 0, core::ptr::null_mut()) }
        }
    }
    _ => {
        pub fn native_extra() {
            unsafe { libc::futimens(0, core::ptr::null()) }
        }
    }
}
