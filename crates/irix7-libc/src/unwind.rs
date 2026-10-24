// SPDX-License-Identifier: GPL-3.0-only
//! Minimal unwinder surface.
//!
//! `panic = "abort"` means `catch_unwind` never catches.  The vendored
//! backtrace code still references `_Unwind_Backtrace` and `_Unwind_GetIP`;
//! these stubs end the walk immediately.  A real MIPS unwinder is a later
//! milestone.

type UnwindTraceFn = unsafe extern "C" fn(*mut u8, *mut u8) -> i32;

#[no_mangle]
pub unsafe extern "C" fn _Unwind_Backtrace(_trace: UnwindTraceFn, _arg: *mut u8) -> i32 {
    5 // _URC_END_OF_STACK
}

#[no_mangle]
pub unsafe extern "C" fn _Unwind_GetIP(_context: *mut u8) -> usize {
    0
}

#[no_mangle]
pub unsafe extern "C" fn _Unwind_GetIPInfo(_context: *mut u8, before: *mut i32) -> usize {
    if !before.is_null() {
        *before = 0;
    }
    0
}

#[no_mangle]
pub unsafe extern "C" fn _Unwind_GetCFA(_context: *mut u8) -> usize {
    0
}

#[no_mangle]
pub unsafe extern "C" fn _Unwind_GetGR(_context: *mut u8, _index: i32) -> usize {
    0
}

#[no_mangle]
pub unsafe extern "C" fn _Unwind_SetGR(_context: *mut u8, _index: i32, _value: usize) {}

#[no_mangle]
pub unsafe extern "C" fn _Unwind_GetLanguageSpecificData(_context: *mut u8) -> *mut u8 {
    core::ptr::null_mut()
}

#[no_mangle]
pub unsafe extern "C" fn _Unwind_GetRegionStart(_context: *mut u8) -> usize {
    0
}

#[no_mangle]
pub unsafe extern "C" fn _Unwind_GetDataRelBase(_context: *mut u8) -> usize {
    0
}

#[no_mangle]
pub unsafe extern "C" fn _Unwind_GetTextRelBase(_context: *mut u8) -> usize {
    0
}

#[no_mangle]
pub unsafe extern "C" fn _Unwind_FindEnclosingFunction(_pc: *mut u8) -> *mut u8 {
    core::ptr::null_mut()
}

#[no_mangle]
pub unsafe extern "C" fn _Unwind_SetIP(_context: *mut u8, _value: usize) {}

#[no_mangle]
pub unsafe extern "C" fn _Unwind_DeleteException(_exception: *mut u8) {}

#[no_mangle]
pub unsafe extern "C" fn _Unwind_Resume(_exception: *mut u8) -> ! {
    loop {
        core::hint::spin_loop();
    }
}

#[no_mangle]
pub unsafe extern "C" fn _Unwind_RaiseException(_exception: *mut u8) -> i32 {
    5
}
