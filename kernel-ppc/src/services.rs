// SPDX-License-Identifier: GPL-3.0-only
//! The service table the PowerPC machine hands to loadable modules.
//!
//! The ABI is the same `module_abi::Services` the Indy and x86 machines use.
//! This slice implements the basics a module needs to start: console output,
//! the clock and machine information.  Allocation, network, block, screen and
//! module-loader services answer with an error, so a module that tries one
//! sees a refusal rather than a missing pointer.

use module_abi::{
    ModuleArena, ModuleContext, ModuleExportBinding, ModuleHandle, ModuleInfo, ModuleLoad,
    Services, ABI_VERSION,
};

use crate::{serial, timer};

/// Errors use the module ABI's negative convention.
const REFUSED: isize = -1;

macro_rules! stub {
    ($name:ident ($($ty:ty),*) -> $ret:ty = $value:expr) => {
        #[allow(clippy::missing_safety_doc)]
        unsafe extern "C" fn $name($(_: $ty),*) -> $ret {
            $value
        }
    };
}

macro_rules! safe_stub {
    ($name:ident ($($ty:ty),*) -> $ret:ty = $value:expr) => {
        extern "C" fn $name($(_: $ty),*) -> $ret {
            $value
        }
    };
}

extern "C" fn write(ptr: *const u8, len: usize) {
    if ptr.is_null() || len == 0 || len > 8192 {
        return;
    }
    // SAFETY: a module passes a buffer it owns for the duration of the call.
    let bytes = unsafe { core::slice::from_raw_parts(ptr, len) };
    serial::write(bytes);
}

/// The decrementer runs at 100 Hz, so one tick is 10 ms.
extern "C" fn clock_ms() -> u64 {
    u64::from(timer::ticks()) * 10
}

#[allow(clippy::missing_safety_doc)]
unsafe extern "C" fn alloc(_size: usize, _align: usize) -> *mut u8 {
    core::ptr::null_mut()
}

extern "C" fn poll() {}

/// Fixed information blocks.  Query 0 is the machine description the module
/// writes through `write`; the other documented kinds name the absent network
/// and reserved memory.
unsafe extern "C" fn info(which: u32, out: *mut u8, len: usize) -> isize {
    if out.is_null() || len == 0 {
        return REFUSED;
    }
    let text: &[u8] = match which {
        0 => b"ppc: g3beige\n",
        1 => b"net: none\n",
        2 => b"mem: reserved\n",
        3 => b"time: monotonic\n",
        _ => return REFUSED,
    };
    let count = text.len().min(len);
    // SAFETY: the module passes a buffer of `len` bytes it owns for the call.
    unsafe { core::ptr::copy_nonoverlapping(text.as_ptr(), out, count) };
    count as isize
}

stub!(console_read(u64, *mut u8, usize) -> isize = REFUSED);
safe_stub!(console_total() -> u64 = 0);
safe_stub!(reset() -> () = ());
safe_stub!(poweroff() -> () = ());
stub!(status(*mut u8, usize) -> isize = REFUSED);
stub!(
    config_get(*const u8, usize, *mut u8, usize) -> isize = REFUSED
);
stub!(screen_read(u32, *mut u8, usize) -> isize = REFUSED);
stub!(block_read(u8, u32, *mut u8) -> i32 = -1);
stub!(block_write(u8, u32, *const u8) -> i32 = -1);
stub!(module_arena(*mut ModuleArena) -> i32 = -1);
stub!(module_load(*const ModuleLoad, *mut ModuleHandle) -> i32 = -1);
stub!(module_execute(ModuleHandle, u32, *const ModuleContext, *mut i32) -> i32 = -1);
stub!(module_find(*const u8, u32, *const u8, u32, *mut ModuleHandle) -> i32 = -1);
stub!(module_find_export(*const u8, u32, u16, *mut ModuleExportBinding) -> i32 = -1);
stub!(module_query(ModuleHandle, *mut ModuleInfo) -> i32 = -1);
stub!(module_unload(ModuleHandle) -> i32 = -1);
stub!(tcp_listen(u32, u16) -> i32 = -1);
stub!(tcp_connect(u32, u32, u16) -> i32 = -1);
safe_stub!(tcp_state(u32) -> u32 = 0);
stub!(tcp_recv(u32, *mut u8, usize) -> isize = REFUSED);
stub!(tcp_send(u32, *const u8, usize) -> isize = REFUSED);
safe_stub!(tcp_close(u32) -> () = ());
safe_stub!(tcp_abort(u32) -> () = ());
safe_stub!(tcp_events(u32) -> u32 = 0);
safe_stub!(tcp_slots() -> u32 = 0);
stub!(udp_open(u32, u16) -> i32 = -1);
stub!(udp_send(u32, u32, u16, *const u8, usize) -> isize = REFUSED);
stub!(udp_recv(u32, *mut u8, usize, *mut u32, *mut u16) -> isize = REFUSED);
safe_stub!(udp_close(u32) -> () = ());
safe_stub!(udp_slots() -> u32 = 0);
stub!(udp_state(u32, *mut u32, *mut u16) -> i32 = -1);

/// The table itself.  Modules read `version` first and refuse an older table.
pub static SERVICES: Services = Services {
    version: ABI_VERSION,
    write,
    clock_ms,
    alloc,
    poll,
    tcp_listen,
    tcp_state,
    tcp_recv,
    tcp_send,
    tcp_close,
    tcp_abort,
    console_read,
    console_total,
    reset,
    poweroff,
    status,
    config_get,
    info,
    screen_read,
    block_read,
    block_write,
    module_arena,
    module_load,
    module_execute,
    module_find,
    module_find_export,
    module_query,
    module_unload,
    tcp_connect,
    tcp_events,
    tcp_slots,
    udp_open,
    udp_send,
    udp_recv,
    udp_close,
    udp_slots,
    udp_state,
};
