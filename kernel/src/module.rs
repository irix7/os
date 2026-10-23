// SPDX-License-Identifier: GPL-3.0-only
//! Loadable modules: flat images fetched into reserved RAM and called through
//! a small C service table.
//!
//! The compressed PROM image is meant to stop growing.  Code that belongs to
//! the running system but is not needed to boot (the TLS client is the first
//! candidate) can be linked at [`MODULE_BASE`], downloaded over the network,
//! verified, and called in place.  Modules run in kernel mode and use the
//! service table rather than linking against kernel symbols, so the core and
//! a module can be rebuilt independently.

use alloc::alloc::{alloc, Layout};
use core::mem::{align_of, size_of};
use smoltcp::iface::SocketHandle;
use smoltcp::socket::tcp::{Socket as TcpSocket, SocketBuffer};

/// The loadable-module ABI, shared with every module image.
pub use module_abi::{ModuleVtable, Services, ABI_VERSION, MODULE_BASE, MODULE_MAX};

/// A module entry point.
pub type Entry = unsafe extern "C" fn(*const Services) -> *const ModuleVtable;

static SERVICES: Services = Services {
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
};

/// The periodic callback of the loaded module, if it registered one.
static mut TICK: Option<unsafe extern "C" fn(*const Services) -> i32> = None;
static mut LAST_TICK: u64 = 0;

/// The module area as a writable slice, for downloading straight into place.
pub fn area() -> &'static mut [u8] {
    // SAFETY: the region is reserved for exactly this purpose; the kernel is
    // single-threaded and only one module runs at a time.
    unsafe { core::slice::from_raw_parts_mut(MODULE_BASE as *mut u8, MODULE_MAX) }
}

/// Zero the module area before a download.
///
/// The flat image contains `.text`, `.rodata` and `.data` but not `.bss`, so
/// zeroing the area gives the module's `.bss` a defined initial value and
/// discards the previous module's code.
pub fn clear() {
    // SAFETY: the region is reserved for modules and is not aliased.
    unsafe {
        core::ptr::write_bytes(MODULE_BASE as *mut u8, 0, MODULE_MAX);
    }
    crate::cache::dcache_writeback_invalidate(MODULE_BASE, MODULE_MAX);
    crate::cache::sync();
}

fn vtable_in_area(p: *const ModuleVtable) -> bool {
    let a = p as usize;
    if a < MODULE_BASE || a % align_of::<ModuleVtable>() != 0 {
        return false;
    }
    match a.checked_add(size_of::<ModuleVtable>()) {
        Some(end) => end <= MODULE_BASE + MODULE_MAX,
        None => false,
    }
}

/// Call the module currently loaded at [`MODULE_BASE`].
///
/// # Safety
/// The caller must have verified the image and its hash; arbitrary code runs
/// in kernel mode from here.
pub unsafe fn call_entry(len: usize) -> i32 {
    // The image was written through the cacheable kseg0 mapping.  Write the
    // data cache back to RAM and invalidate stale instruction lines before the
    // first fetch of module code.
    crate::cache::dcache_writeback_invalidate(MODULE_BASE, len);
    crate::cache::icache_invalidate(MODULE_BASE, len);
    crate::cache::sync();

    // A new image replaces any previous callback.
    TICK = None;

    let entry: Entry = core::mem::transmute(MODULE_BASE);
    let vtable = entry(&SERVICES);
    if vtable.is_null() {
        return 0;
    }
    if !vtable_in_area(vtable) {
        return -1;
    }
    // The vtable lives inside the verified image; read it directly.
    let status = core::ptr::read(vtable).status;
    if status != 0 {
        return status;
    }
    let tick = core::ptr::read(vtable).tick;
    if let Some(tick) = tick {
        LAST_TICK = service_clock_ms();
        TICK = Some(tick);
    }
    0
}

/// Run the loaded module's periodic callback, if any.
pub fn poll() {
    // SAFETY: single-threaded kernel; TICK is only written here and in
    // call_entry, which is not re-entered.
    let tick = unsafe { TICK };
    let Some(tick) = tick else {
        return;
    };
    let now = service_clock_ms();
    let last = unsafe { LAST_TICK };
    if now.saturating_sub(last) < 10 {
        return;
    }
    unsafe {
        LAST_TICK = now;
    }
    let status = unsafe { tick(&SERVICES) };
    if status != 0 {
        unsafe {
            TICK = None;
        }
    }
}

extern "C" fn service_write(ptr: *const u8, len: usize) {
    if ptr.is_null() || len == 0 || len > 8192 {
        return;
    }
    let data = unsafe { core::slice::from_raw_parts(ptr, len) };
    for &byte in data {
        crate::write_byte(byte);
    }
    crate::telnet::write_output(data);
}

extern "C" fn service_clock_ms() -> u64 {
    let ms = crate::net::clock_now().total_millis();
    if ms < 0 {
        0
    } else {
        ms as u64
    }
}

unsafe extern "C" fn service_alloc(size: usize, align: usize) -> *mut u8 {
    if size == 0 {
        return core::ptr::null_mut();
    }
    let align = align.max(1).next_power_of_two();
    match Layout::from_size_align(size, align) {
        Ok(layout) => alloc(layout),
        Err(_) => core::ptr::null_mut(),
    }
}

extern "C" fn service_poll() {
    crate::net::poll();
}

/// Module TCP slots.  Two is enough for an HTTP server plus a client.
const MODULE_TCP_SLOTS: usize = 2;

static mut MODULE_TCP: [Option<SocketHandle>; MODULE_TCP_SLOTS] = [None; MODULE_TCP_SLOTS];
static mut MODULE_TCP_RX: [[u8; 2048]; MODULE_TCP_SLOTS] = [[0; 2048]; MODULE_TCP_SLOTS];
static mut MODULE_TCP_TX: [[u8; 4096]; MODULE_TCP_SLOTS] = [[0; 4096]; MODULE_TCP_SLOTS];

fn module_handle(slot: u32) -> Option<SocketHandle> {
    let slot = slot as usize;
    if slot >= MODULE_TCP_SLOTS {
        return None;
    }
    // SAFETY: single-threaded kernel; the table is only written by the
    // service functions below.
    unsafe { MODULE_TCP[slot] }
}

unsafe extern "C" fn service_tcp_listen(slot: u32, port: u16) -> i32 {
    let slot_usize = slot as usize;
    if slot_usize >= MODULE_TCP_SLOTS || port == 0 {
        return -1;
    }
    if let Some(old) = module_handle(slot) {
        let _ = crate::net::sockets_with(|sockets| sockets.remove(old));
        // SAFETY: single-threaded; the old socket has been removed.
        unsafe {
            MODULE_TCP[slot_usize] = None;
        }
    }
    // SAFETY: each slot's buffers are owned by at most one socket at a time.
    let rx = unsafe { &mut *core::ptr::addr_of_mut!(MODULE_TCP_RX[slot_usize]) };
    let tx = unsafe { &mut *core::ptr::addr_of_mut!(MODULE_TCP_TX[slot_usize]) };
    let mut socket = TcpSocket::new(
        SocketBuffer::new(&mut rx[..]),
        SocketBuffer::new(&mut tx[..]),
    );
    socket.set_keep_alive(Some(smoltcp::time::Duration::from_secs(60)));
    let Some(handle) = crate::net::sockets_with(|sockets| sockets.add(socket)) else {
        return -1;
    };
    let listened = crate::net::sockets_with(|sockets| {
        sockets.get_mut::<TcpSocket>(handle).listen(port).is_ok()
    });
    if listened == Some(true) {
        // SAFETY: single-threaded.
        unsafe {
            MODULE_TCP[slot_usize] = Some(handle);
        }
        0
    } else {
        let _ = crate::net::sockets_with(|sockets| sockets.remove(handle));
        -1
    }
}

extern "C" fn service_tcp_state(slot: u32) -> u32 {
    let Some(handle) = module_handle(slot) else {
        return 0;
    };
    use smoltcp::socket::tcp::State;
    let state = crate::net::sockets_with(|sockets| sockets.get::<TcpSocket>(handle).state());
    match state {
        Some(State::Listen) | Some(State::SynSent) | Some(State::SynReceived) => 1,
        Some(State::Established) => 2,
        Some(State::FinWait1)
        | Some(State::FinWait2)
        | Some(State::CloseWait)
        | Some(State::Closing)
        | Some(State::LastAck)
        | Some(State::TimeWait) => 3,
        Some(State::Closed) | None => 0,
    }
}

unsafe extern "C" fn service_tcp_recv(slot: u32, ptr: *mut u8, len: usize) -> isize {
    if ptr.is_null() || len == 0 {
        return -1;
    }
    let Some(handle) = module_handle(slot) else {
        return -1;
    };
    // SAFETY: the module owns the destination buffer for the duration.
    let buf = unsafe { core::slice::from_raw_parts_mut(ptr, len) };
    match crate::net::sockets_with(|sockets| sockets.get_mut::<TcpSocket>(handle).recv_slice(buf)) {
        Some(Ok(n)) => n as isize,
        _ => -1,
    }
}

unsafe extern "C" fn service_tcp_send(slot: u32, ptr: *const u8, len: usize) -> isize {
    if ptr.is_null() || len == 0 {
        return -1;
    }
    let Some(handle) = module_handle(slot) else {
        return -1;
    };
    // SAFETY: the module owns the source buffer for the duration.
    let buf = unsafe { core::slice::from_raw_parts(ptr, len) };
    match crate::net::sockets_with(|sockets| sockets.get_mut::<TcpSocket>(handle).send_slice(buf)) {
        Some(Ok(n)) => n as isize,
        _ => -1,
    }
}

extern "C" fn service_tcp_close(slot: u32) {
    let Some(handle) = module_handle(slot) else {
        return;
    };
    let _ = crate::net::sockets_with(|sockets| sockets.get_mut::<TcpSocket>(handle).close());
}

unsafe extern "C" fn service_console_read(offset: u64, ptr: *mut u8, len: usize) -> isize {
    if ptr.is_null() || len == 0 {
        return -1;
    }
    // SAFETY: the module owns the destination buffer for the duration.
    let buf = unsafe { core::slice::from_raw_parts_mut(ptr, len) };
    crate::console_read(offset, buf) as isize
}

extern "C" fn service_console_total() -> u64 {
    crate::console_total()
}

extern "C" fn service_reset() {
    crate::reboot();
}

extern "C" fn service_poweroff() {
    crate::poweroff();
}

unsafe extern "C" fn service_screen_read(offset: u32, ptr: *mut u8, len: usize) -> isize {
    if ptr.is_null() || len == 0 {
        return -1;
    }
    let buf = unsafe { core::slice::from_raw_parts_mut(ptr, len) };
    crate::gfx::shadow_read(offset as usize, buf) as isize
}

unsafe extern "C" fn service_info(kind: u32, ptr: *mut u8, len: usize) -> isize {
    if ptr.is_null() || len == 0 {
        return -1;
    }
    let buf = unsafe { core::slice::from_raw_parts_mut(ptr, len) };
    let n = match kind {
        0 => crate::status::text(buf),
        1 => crate::status::net(buf),
        2 => crate::status::mem(buf),
        3 => crate::status::time(buf),
        _ => return -1,
    };
    n as isize
}

unsafe extern "C" fn service_config_get(
    name: *const u8,
    name_len: usize,
    out: *mut u8,
    out_len: usize,
) -> isize {
    if name.is_null() || out.is_null() || name_len == 0 || name_len > 64 || out_len == 0 {
        return -1;
    }
    // SAFETY: the module owns both buffers for the duration.
    let name = unsafe { core::slice::from_raw_parts(name, name_len) };
    let Ok(name) = core::str::from_utf8(name) else {
        return -1;
    };
    let out = unsafe { core::slice::from_raw_parts_mut(out, out_len) };
    match crate::nvram::get(name, out) {
        Some(n) => n as isize,
        None => -1,
    }
}

unsafe extern "C" fn service_status(ptr: *mut u8, len: usize) -> isize {
    if ptr.is_null() || len == 0 {
        return -1;
    }
    // SAFETY: the module owns the destination buffer for the duration.
    let buf = unsafe { core::slice::from_raw_parts_mut(ptr, len) };
    crate::status::text(buf) as isize
}

extern "C" fn service_tcp_abort(slot: u32) {
    let Some(handle) = module_handle(slot) else {
        return;
    };
    let _ = crate::net::sockets_with(|sockets| sockets.get_mut::<TcpSocket>(handle).abort());
}
