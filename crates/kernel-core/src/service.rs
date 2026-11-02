// SPDX-License-Identifier: GPL-3.0-only
//! The module service table, shared by every machine.
//!
//! A module sees one ABI: [`module_abi::Services`].  Before this crate each
//! kernel built its own copy of the same 36 entries, with its own argument
//! guards and its own idea of which entries refuse.  Three copies of an ABI
//! that must agree byte for byte is how a module gets a different contract per
//! machine.
//!
//! The table lives here once.  A machine implements [`Platform`] for the
//! resources it actually owns — console bytes, the clock, the network stack,
//! the block device, the module arena — and installs one instance at boot.
//! Entries the machine does not provide keep their default refusal, so an
//! unsupported service is a documented error rather than a missing pointer or
//! a per-machine surprise.
//!
//! ## Threading
//!
//! Every kernel is single-threaded while it calls a module.  [`install`] is
//! called once during boot and the platform reference is then immutable.  The
//! dispatch functions tolerate being called before installation: they refuse
//! rather than dereference an absent platform.

use core::alloc::Layout;
use core::ptr::null_mut;

use module_abi::{
    ModuleArena, ModuleContext, ModuleExportBinding, ModuleHandle, ModuleInfo, ModuleLoad,
    Services, ABI_VERSION, MODULE_ERR_ARGUMENT, MODULE_ERR_STATE,
};

/// Largest console write a module may request in one call.
pub const MAX_WRITE: usize = 8192;
/// Largest configuration key a module may request.
pub const MAX_CONFIG_KEY: usize = 64;
/// A block sector is 512 bytes on every machine.
pub const SECTOR: usize = 512;

/// The machine resources a module may use.
///
/// Defaults are refusals.  A machine overrides only what it provides; the
/// shared dispatch still applies the pointer and length guards first.
pub trait Platform: Sync {
    /// Write bytes to the console.
    fn write(&self, bytes: &[u8]);
    /// Milliseconds since boot.
    fn clock_ms(&self) -> u64;

    /// Allocate from the kernel heap.  The heap never frees.
    fn alloc(&self, _layout: Layout) -> *mut u8 {
        null_mut()
    }

    /// Drive the network stack once.
    fn poll(&self) {}

    /// Listen on `port` in module TCP slot `slot`.
    fn tcp_listen(&self, _slot: u32, _port: u16) -> i32 {
        -1
    }

    /// Module TCP slot state: 0 closed, 1 listening, 2 established, 3 closing.
    fn tcp_state(&self, _slot: u32) -> u32 {
        0
    }

    /// Receive into `out`; bytes, 0 for none, -1 for no socket.
    fn tcp_recv(&self, _slot: u32, _out: &mut [u8]) -> isize {
        -1
    }

    /// Send from `bytes`; bytes accepted, -1 for no socket.
    fn tcp_send(&self, _slot: u32, _bytes: &[u8]) -> isize {
        -1
    }

    /// Close the connection.
    fn tcp_close(&self, _slot: u32) {}

    /// Abort the connection.
    fn tcp_abort(&self, _slot: u32) {}

    /// Connect slot `slot` to IPv4 `address` (network byte order).
    fn tcp_connect(&self, _slot: u32, _address: u32, _port: u16) -> i32 {
        -1
    }

    /// Level-triggered readiness for module TCP slot `slot`.
    fn tcp_events(&self, _slot: u32) -> u32 {
        0
    }

    /// Number of module TCP slots.
    fn tcp_slots(&self) -> u32 {
        0
    }

    /// Copy console output from absolute `offset`.
    fn console_read(&self, _offset: u64, _out: &mut [u8]) -> isize {
        -1
    }

    /// Total bytes ever written to the console.
    fn console_total(&self) -> u64 {
        0
    }

    /// Restart the machine.  A machine without a reset path does nothing.
    fn reset(&self) {}

    /// Halt the machine.  A machine without a power-off path does nothing.
    fn poweroff(&self) {}

    /// Format the machine status block.
    fn status(&self, _out: &mut [u8]) -> isize {
        -1
    }

    /// Read a boot configuration value by name.
    fn config_get(&self, _key: &[u8], _out: &mut [u8]) -> isize {
        -1
    }

    /// Format an information block: 0 status, 1 net, 2 mem, 3 time.
    fn info(&self, _kind: u32, _out: &mut [u8]) -> isize {
        -1
    }

    /// Copy BGR shadow pixels from `offset`.
    fn screen_read(&self, _offset: u32, _out: &mut [u8]) -> isize {
        -1
    }

    /// Read one sector from `target`.
    fn block_read(&self, _target: u8, _lba: u32, _out: &mut [u8; SECTOR]) -> i32 {
        -1
    }

    /// Write one sector to `target`.
    fn block_write(&self, _target: u8, _lba: u32, _data: &[u8; SECTOR]) -> i32 {
        -1
    }

    /// Report the bounded module arena.
    fn module_arena(&self, _out: &mut ModuleArena) -> i32 {
        MODULE_ERR_ARGUMENT
    }

    /// Fetch, verify and copy one image into an arena region.
    fn module_load(&self, _request: &ModuleLoad, _out: &mut ModuleHandle) -> i32 {
        MODULE_ERR_STATE
    }

    /// Enter a loaded image at an image-relative offset.
    fn module_execute(
        &self,
        _handle: ModuleHandle,
        _entry_offset: u32,
        _context: Option<&ModuleContext>,
        _status: Option<&mut i32>,
    ) -> i32 {
        MODULE_ERR_STATE
    }

    /// Find a loaded module by exact name and version.
    fn module_find(&self, _name: &[u8], _version: &[u8], _out: &mut ModuleHandle) -> i32 {
        MODULE_ERR_STATE
    }

    /// Find an export by exact interface and version.
    fn module_find_export(
        &self,
        _interface: &[u8],
        _version: u16,
        _out: &mut ModuleExportBinding,
    ) -> i32 {
        MODULE_ERR_STATE
    }

    /// Query a loaded module handle.
    fn module_query(&self, _handle: ModuleHandle, _out: &mut ModuleInfo) -> i32 {
        MODULE_ERR_STATE
    }

    /// Unload a module handle and release its region.
    fn module_unload(&self, _handle: ModuleHandle) -> i32 {
        MODULE_ERR_STATE
    }

    /// Open a UDP socket in `slot`, bound to `port`.
    fn udp_open(&self, _slot: u32, _port: u16) -> i32 {
        -1
    }

    /// Send one datagram.
    fn udp_send(&self, _slot: u32, _address: u32, _port: u16, _bytes: &[u8]) -> isize {
        -1
    }

    /// Receive one datagram.
    fn udp_recv(
        &self,
        _slot: u32,
        _out: &mut [u8],
        _src_address: Option<&mut u32>,
        _src_port: Option<&mut u16>,
    ) -> isize {
        -1
    }

    /// Close the UDP socket in `slot`.
    fn udp_close(&self, _slot: u32) {}

    /// Number of module UDP slots.
    fn udp_slots(&self) -> u32 {
        0
    }

    /// Local address and port bound to `slot`.
    fn udp_state(&self, _slot: u32, _address: Option<&mut u32>, _port: Option<&mut u16>) -> i32 {
        -1
    }
}

static mut INSTALLED: Option<&'static dyn Platform> = None;

/// Install the machine's platform.  Called once during boot.
pub fn install(platform: &'static dyn Platform) {
    // SAFETY: single-threaded boot; no reader runs before installation and the
    // reference is immutable afterwards.
    unsafe {
        INSTALLED = Some(platform);
    }
}

fn platform() -> Option<&'static dyn Platform> {
    // SAFETY: the platform is installed once during boot and never replaced.
    unsafe { INSTALLED }
}

// ---------------------------------------------------------------------------
// Dispatch.  These are the only functions the module ABI sees.
// ---------------------------------------------------------------------------

extern "C" fn service_write(ptr: *const u8, len: usize) {
    if ptr.is_null() || len == 0 || len > MAX_WRITE {
        return;
    }
    let Some(platform) = platform() else {
        return;
    };
    // SAFETY: the module owns `ptr..ptr + len` for the duration of the call.
    let bytes = unsafe { core::slice::from_raw_parts(ptr, len) };
    platform.write(bytes);
}

extern "C" fn service_clock_ms() -> u64 {
    platform().map_or(0, Platform::clock_ms)
}

unsafe extern "C" fn service_alloc(size: usize, align: usize) -> *mut u8 {
    let Some(platform) = platform() else {
        return null_mut();
    };
    if size == 0 {
        return null_mut();
    }
    let align = align.max(1).next_power_of_two();
    let Ok(layout) = Layout::from_size_align(size, align) else {
        return null_mut();
    };
    platform.alloc(layout)
}

extern "C" fn service_poll() {
    if let Some(platform) = platform() {
        platform.poll();
    }
}

unsafe extern "C" fn service_tcp_listen(slot: u32, port: u16) -> i32 {
    platform().map_or(-1, |p| p.tcp_listen(slot, port))
}

extern "C" fn service_tcp_state(slot: u32) -> u32 {
    platform().map_or(0, |p| p.tcp_state(slot))
}

unsafe extern "C" fn service_tcp_recv(slot: u32, ptr: *mut u8, len: usize) -> isize {
    if ptr.is_null() {
        return -1;
    }
    let Some(platform) = platform() else {
        return -1;
    };
    // SAFETY: the module owns the destination buffer for the call.
    let out = unsafe { core::slice::from_raw_parts_mut(ptr, len) };
    platform.tcp_recv(slot, out)
}

unsafe extern "C" fn service_tcp_send(slot: u32, ptr: *const u8, len: usize) -> isize {
    if ptr.is_null() {
        return -1;
    }
    let Some(platform) = platform() else {
        return -1;
    };
    // SAFETY: the module owns the source buffer for the call.
    let bytes = unsafe { core::slice::from_raw_parts(ptr, len) };
    platform.tcp_send(slot, bytes)
}

extern "C" fn service_tcp_close(slot: u32) {
    if let Some(platform) = platform() {
        platform.tcp_close(slot);
    }
}

extern "C" fn service_tcp_abort(slot: u32) {
    if let Some(platform) = platform() {
        platform.tcp_abort(slot);
    }
}

unsafe extern "C" fn service_tcp_connect(slot: u32, address: u32, port: u16) -> i32 {
    platform().map_or(-1, |p| p.tcp_connect(slot, address, port))
}

extern "C" fn service_tcp_events(slot: u32) -> u32 {
    platform().map_or(0, |p| p.tcp_events(slot))
}

extern "C" fn service_tcp_slots() -> u32 {
    platform().map_or(0, Platform::tcp_slots)
}

unsafe extern "C" fn service_console_read(offset: u64, ptr: *mut u8, len: usize) -> isize {
    if ptr.is_null() || len == 0 {
        return -1;
    }
    let Some(platform) = platform() else {
        return -1;
    };
    // SAFETY: the module owns the destination buffer for the call.
    let out = unsafe { core::slice::from_raw_parts_mut(ptr, len) };
    platform.console_read(offset, out)
}

extern "C" fn service_console_total() -> u64 {
    platform().map_or(0, Platform::console_total)
}

extern "C" fn service_reset() {
    if let Some(platform) = platform() {
        platform.reset();
    }
}

extern "C" fn service_poweroff() {
    if let Some(platform) = platform() {
        platform.poweroff();
    }
}

unsafe extern "C" fn service_status(ptr: *mut u8, len: usize) -> isize {
    if ptr.is_null() || len == 0 {
        return -1;
    }
    let Some(platform) = platform() else {
        return -1;
    };
    // SAFETY: the module owns the destination buffer for the call.
    let out = unsafe { core::slice::from_raw_parts_mut(ptr, len) };
    platform.status(out)
}

unsafe extern "C" fn service_config_get(
    key: *const u8,
    key_len: usize,
    out: *mut u8,
    out_len: usize,
) -> isize {
    if key.is_null() || out.is_null() || key_len == 0 || key_len > MAX_CONFIG_KEY || out_len == 0 {
        return -1;
    }
    let Some(platform) = platform() else {
        return -1;
    };
    // SAFETY: the module owns both buffers for the call.
    let key = unsafe { core::slice::from_raw_parts(key, key_len) };
    let out = unsafe { core::slice::from_raw_parts_mut(out, out_len) };
    platform.config_get(key, out)
}

unsafe extern "C" fn service_info(kind: u32, ptr: *mut u8, len: usize) -> isize {
    if ptr.is_null() || len == 0 {
        return -1;
    }
    let Some(platform) = platform() else {
        return -1;
    };
    // SAFETY: the module owns the destination buffer for the call.
    let out = unsafe { core::slice::from_raw_parts_mut(ptr, len) };
    platform.info(kind, out)
}

unsafe extern "C" fn service_screen_read(offset: u32, ptr: *mut u8, len: usize) -> isize {
    if ptr.is_null() || len == 0 {
        return -1;
    }
    let Some(platform) = platform() else {
        return -1;
    };
    // SAFETY: the module owns the destination buffer for the call.
    let out = unsafe { core::slice::from_raw_parts_mut(ptr, len) };
    platform.screen_read(offset, out)
}

unsafe extern "C" fn service_block_read(target: u8, lba: u32, ptr: *mut u8) -> i32 {
    if ptr.is_null() {
        return -1;
    }
    let Some(platform) = platform() else {
        return -1;
    };
    // SAFETY: the ABI defines one 512-byte sector at `ptr`.
    let out = unsafe { &mut *(ptr as *mut [u8; SECTOR]) };
    platform.block_read(target, lba, out)
}

unsafe extern "C" fn service_block_write(target: u8, lba: u32, ptr: *const u8) -> i32 {
    if ptr.is_null() {
        return -1;
    }
    let Some(platform) = platform() else {
        return -1;
    };
    // SAFETY: the ABI defines one 512-byte sector at `ptr`.
    let data = unsafe { &*(ptr as *const [u8; SECTOR]) };
    platform.block_write(target, lba, data)
}

unsafe extern "C" fn service_module_arena(out: *mut ModuleArena) -> i32 {
    if out.is_null() {
        return MODULE_ERR_ARGUMENT;
    }
    let Some(platform) = platform() else {
        return MODULE_ERR_ARGUMENT;
    };
    // SAFETY: the manager passed a writable arena descriptor.
    platform.module_arena(unsafe { &mut *out })
}

unsafe extern "C" fn service_module_load(
    request: *const ModuleLoad,
    out: *mut ModuleHandle,
) -> i32 {
    if request.is_null() || out.is_null() {
        return MODULE_ERR_ARGUMENT;
    }
    let Some(platform) = platform() else {
        return MODULE_ERR_ARGUMENT;
    };
    // SAFETY: the manager supplies a complete request for this call.
    let request = unsafe { &*request };
    // SAFETY: the manager supplied a writable handle slot.
    platform.module_load(request, unsafe { &mut *out })
}

unsafe extern "C" fn service_module_execute(
    handle: ModuleHandle,
    entry_offset: u32,
    context: *const ModuleContext,
    out_status: *mut i32,
) -> i32 {
    let Some(platform) = platform() else {
        return MODULE_ERR_ARGUMENT;
    };
    // SAFETY: a non-null context is the manager's opaque pointer, valid for
    // the duration of this synchronous call.
    let context = if context.is_null() {
        None
    } else {
        Some(unsafe { &*context })
    };
    // SAFETY: a non-null status pointer is a writable `i32` for the call.
    let status = if out_status.is_null() {
        None
    } else {
        Some(unsafe { &mut *out_status })
    };
    platform.module_execute(handle, entry_offset, context, status)
}

unsafe extern "C" fn service_module_find(
    name: *const u8,
    name_len: u32,
    version: *const u8,
    version_len: u32,
    out: *mut ModuleHandle,
) -> i32 {
    if name.is_null()
        || version.is_null()
        || out.is_null()
        || name_len == 0
        || name_len > module_abi::MODULE_NAME_MAX
        || version_len == 0
        || version_len > module_abi::MODULE_VERSION_MAX
    {
        return MODULE_ERR_ARGUMENT;
    }
    let Some(platform) = platform() else {
        return MODULE_ERR_ARGUMENT;
    };
    // SAFETY: the module owns both name buffers for the call.
    let name = unsafe { core::slice::from_raw_parts(name, name_len as usize) };
    // SAFETY: as above.
    let version = unsafe { core::slice::from_raw_parts(version, version_len as usize) };
    // SAFETY: the manager supplied a writable handle slot.
    platform.module_find(name, version, unsafe { &mut *out })
}

unsafe extern "C" fn service_module_find_export(
    interface: *const u8,
    interface_len: u32,
    version: u16,
    out: *mut ModuleExportBinding,
) -> i32 {
    if interface.is_null()
        || out.is_null()
        || interface_len == 0
        || interface_len > module_abi::MODULE_INTERFACE_MAX
    {
        return MODULE_ERR_ARGUMENT;
    }
    let Some(platform) = platform() else {
        return MODULE_ERR_ARGUMENT;
    };
    // SAFETY: the module owns the interface buffer for the call.
    let interface = unsafe { core::slice::from_raw_parts(interface, interface_len as usize) };
    // SAFETY: the manager supplied a writable binding slot.
    platform.module_find_export(interface, version, unsafe { &mut *out })
}

unsafe extern "C" fn service_module_query(handle: ModuleHandle, out: *mut ModuleInfo) -> i32 {
    if out.is_null() {
        return MODULE_ERR_ARGUMENT;
    }
    let Some(platform) = platform() else {
        return MODULE_ERR_ARGUMENT;
    };
    // SAFETY: the manager supplied a writable info slot.
    platform.module_query(handle, unsafe { &mut *out })
}

unsafe extern "C" fn service_module_unload(handle: ModuleHandle) -> i32 {
    platform().map_or(MODULE_ERR_ARGUMENT, |p| p.module_unload(handle))
}

unsafe extern "C" fn service_udp_open(slot: u32, port: u16) -> i32 {
    platform().map_or(-1, |p| p.udp_open(slot, port))
}

unsafe extern "C" fn service_udp_send(
    slot: u32,
    address: u32,
    port: u16,
    ptr: *const u8,
    len: usize,
) -> isize {
    if ptr.is_null() || len == 0 || address == 0 || port == 0 {
        return -1;
    }
    let Some(platform) = platform() else {
        return -1;
    };
    // SAFETY: the module owns the source buffer for the call.
    let bytes = unsafe { core::slice::from_raw_parts(ptr, len) };
    platform.udp_send(slot, address, port, bytes)
}

unsafe extern "C" fn service_udp_recv(
    slot: u32,
    ptr: *mut u8,
    len: usize,
    src_address: *mut u32,
    src_port: *mut u16,
) -> isize {
    if ptr.is_null() || len == 0 {
        return -1;
    }
    let Some(platform) = platform() else {
        return -1;
    };
    // SAFETY: the module owns the destination buffer for the call.
    let out = unsafe { core::slice::from_raw_parts_mut(ptr, len) };
    // SAFETY: each non-null out-parameter is a writable value for the call.
    let address = if src_address.is_null() {
        None
    } else {
        Some(unsafe { &mut *src_address })
    };
    // SAFETY: as above.
    let port = if src_port.is_null() {
        None
    } else {
        Some(unsafe { &mut *src_port })
    };
    platform.udp_recv(slot, out, address, port)
}

extern "C" fn service_udp_close(slot: u32) {
    if let Some(platform) = platform() {
        platform.udp_close(slot);
    }
}

extern "C" fn service_udp_slots() -> u32 {
    platform().map_or(0, Platform::udp_slots)
}

extern "C" fn service_udp_state(slot: u32, address: *mut u32, port: *mut u16) -> i32 {
    let Some(platform) = platform() else {
        return -1;
    };
    // SAFETY: each non-null out-parameter is a writable value for the call.
    let address = if address.is_null() {
        None
    } else {
        Some(unsafe { &mut *address })
    };
    // SAFETY: as above.
    let port = if port.is_null() {
        None
    } else {
        Some(unsafe { &mut *port })
    };
    platform.udp_state(slot, address, port)
}

/// The one service table every module receives.
pub static SERVICES: Services = Services {
    version: ABI_VERSION,
    write: service_write,
    clock_ms: service_clock_ms,
    alloc: service_alloc,
    poll: service_poll,
    tcp_listen: service_tcp_listen,
    tcp_state: service_tcp_state,
    tcp_recv: service_tcp_recv,
    tcp_send: service_tcp_send,
    tcp_close: service_tcp_close,
    tcp_abort: service_tcp_abort,
    console_read: service_console_read,
    console_total: service_console_total,
    reset: service_reset,
    poweroff: service_poweroff,
    status: service_status,
    config_get: service_config_get,
    info: service_info,
    screen_read: service_screen_read,
    block_read: service_block_read,
    block_write: service_block_write,
    module_arena: service_module_arena,
    module_load: service_module_load,
    module_execute: service_module_execute,
    module_find: service_module_find,
    module_find_export: service_module_find_export,
    module_query: service_module_query,
    module_unload: service_module_unload,
    tcp_connect: service_tcp_connect,
    tcp_events: service_tcp_events,
    tcp_slots: service_tcp_slots,
    udp_open: service_udp_open,
    udp_send: service_udp_send,
    udp_recv: service_udp_recv,
    udp_close: service_udp_close,
    udp_slots: service_udp_slots,
    udp_state: service_udp_state,
};
