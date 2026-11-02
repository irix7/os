// SPDX-License-Identifier: GPL-3.0-only
//! The service table the x86_64 machine hands to loadable modules.
//!
//! The ABI is the same `module_abi::Services` the Indy kernel uses.  This slice
//! implements the basics a module needs to start: console output, the clock,
//! allocation and polling.  Network, block-through-module, module-to-module and
//! screen services answer with an error, so a module that tries them sees a
//! refusal rather than a missing pointer.  Real TCP and UDP services arrive with
//! the x86 socket layer.

use core::alloc::Layout;

use module_abi::{
    ModuleArena, ModuleContext, ModuleExportBinding, ModuleHandle, ModuleInfo, ModuleLoad,
    Services, ABI_VERSION,
};

use crate::{serial, tcp, timer};

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
    // SAFETY: a module passes a buffer it owns for the duration of the call.
    let bytes = unsafe { core::slice::from_raw_parts(ptr, len) };
    serial::write(bytes);
}

extern "C" fn clock_ms() -> u64 {
    u64::from(timer::ticks()) * 10
}

unsafe extern "C" fn alloc(size: usize, align: usize) -> *mut u8 {
    match Layout::from_size_align(size, align) {
        // SAFETY: the global allocator is the machine's bump heap, which
        // accepts any valid layout.
        Ok(layout) => unsafe { alloc::alloc::alloc(layout) },
        Err(_) => core::ptr::null_mut(),
    }
}

extern "C" fn poll() {
    crate::net::poll();
}

unsafe extern "C" fn config_get(
    key: *const u8,
    key_len: usize,
    out: *mut u8,
    out_len: usize,
) -> isize {
    if key.is_null() || out.is_null() {
        return REFUSED;
    }
    // SAFETY: the module owns both buffers for the call.
    let key = unsafe { core::slice::from_raw_parts(key, key_len) };
    let out = unsafe { core::slice::from_raw_parts_mut(out, out_len) };
    crate::config::get(key, out)
}

stub!(console_read(u64, *mut u8, usize) -> isize = REFUSED);
safe_stub!(console_total() -> u64 = 0);
safe_stub!(reset() -> () = ());
safe_stub!(poweroff() -> () = ());
stub!(status(*mut u8, usize) -> isize = REFUSED);
/// Machine information.  Query 1 is the node text the wasm policy module reads:
/// `mac: aa:bb:cc:dd:ee:ff`.  Other queries are refused.
unsafe extern "C" fn info(which: u32, out: *mut u8, len: usize) -> isize {
    // Query 0 is the machine status text the mesh reads for its identity;
    // query 1 is the network text the wasm policy module reads.  Both carry
    // the mac line this machine can answer.
    if which > 1 || out.is_null() {
        return REFUSED;
    }
    let Some(mac) = crate::net::mac() else {
        return REFUSED;
    };
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut text = [0u8; 24];
    text[..5].copy_from_slice(b"mac: ");
    for (i, byte) in mac.iter().enumerate() {
        let at = 5 + i * 3;
        text[at] = DIGITS[(byte >> 4) as usize];
        text[at + 1] = DIGITS[(byte & 0xF) as usize];
        if i < 5 {
            text[at + 2] = b':';
        }
    }
    let count = text.len().min(len);
    // SAFETY: the module passes a buffer of `len` bytes it owns for the call.
    unsafe { core::ptr::copy_nonoverlapping(text.as_ptr(), out, count) };
    count as isize
}
stub!(screen_read(u32, *mut u8, usize) -> isize = REFUSED);
/// The block service exposes only the epoch store, at target 3, mapped to a
/// reserved region of the machine's disk.
const STORE: module_block::Store = module_block::Store::mapped(3, crate::virtio_blk::STORE_OFFSET);

pub(crate) unsafe extern "C" fn block_read(target: u8, lba: u32, ptr: *mut u8) -> i32 {
    let Some(disk) = crate::virtio_blk::disk() else {
        return -1;
    };
    // SAFETY: the module owns the destination for the call.
    unsafe { STORE.read(disk, target, lba, ptr) }
}

pub(crate) unsafe extern "C" fn block_write(target: u8, lba: u32, ptr: *const u8) -> i32 {
    let Some(disk) = crate::virtio_blk::disk() else {
        return -1;
    };
    // SAFETY: the module owns the source for the call.
    unsafe { STORE.write(disk, target, lba, ptr) }
}
stub!(module_arena(*mut ModuleArena) -> i32 = -1);
stub!(module_load(*const ModuleLoad, *mut ModuleHandle) -> i32 = -1);
stub!(module_execute(ModuleHandle, u32, *const ModuleContext, *mut i32) -> i32 = -1);
stub!(module_find(*const u8, u32, *const u8, u32, *mut ModuleHandle) -> i32 = -1);
stub!(module_find_export(*const u8, u32, u16, *mut ModuleExportBinding) -> i32 = -1);
stub!(module_query(ModuleHandle, *mut ModuleInfo) -> i32 = -1);
stub!(module_unload(ModuleHandle) -> i32 = -1);
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
    tcp_listen: tcp::listen,
    tcp_state: tcp::state,
    tcp_recv: tcp::recv,
    tcp_send: tcp::send,
    tcp_close: tcp::close,
    tcp_abort: tcp::abort,
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
    tcp_connect: tcp::connect,
    tcp_events: tcp::events,
    tcp_slots: tcp::slots,
    udp_open,
    udp_send,
    udp_recv,
    udp_close,
    udp_slots,
    udp_state,
};
