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

/// Fixed module slots in the 2 MiB module area.
///
/// Slot A keeps the resident/control module (MCP and the small probes).
/// Slot B is large enough for the wasmi runtime probe.  Modules are linked at
/// their slot base, so this is a fixed two-slot layout, not a relocatable
/// multiplexer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Slot {
    /// 512 KiB at `0x8840_0000`.
    A,
    /// 1.5 MiB at `0x8848_0000`.
    B,
}

/// Base address of a module slot.
pub const fn slot_base(slot: Slot) -> usize {
    match slot {
        Slot::A => MODULE_BASE,
        Slot::B => MODULE_BASE + 0x0008_0000,
    }
}

/// Capacity of a module slot.
pub const fn slot_size(slot: Slot) -> usize {
    match slot {
        Slot::A => 0x0008_0000,
        Slot::B => 0x0018_0000,
    }
}

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
    block_read: service_block_read,
    block_write: service_block_write,
};

/// Periodic callbacks for the two slots.
static mut TICKS: [Option<unsafe extern "C" fn(*const Services) -> i32>; 2] = [None; 2];
static mut LAST_TICKS: [u64; 2] = [0; 2];

/// A module slot as a writable slice, for downloading straight into place.
pub fn area(slot: Slot) -> &'static mut [u8] {
    // SAFETY: the region is reserved for exactly this purpose; the kernel is
    // single-threaded and only the selected slot is used at a time.
    unsafe { core::slice::from_raw_parts_mut(slot_base(slot) as *mut u8, slot_size(slot)) }
}

/// Zero one module slot before a download.
pub fn clear(slot: Slot) {
    // SAFETY: the region is reserved for modules and is not aliased.
    unsafe {
        core::ptr::write_bytes(slot_base(slot) as *mut u8, 0, slot_size(slot));
    }
    crate::cache::dcache_writeback_invalidate(slot_base(slot), slot_size(slot));
    crate::cache::sync();
}

fn vtable_in_slot(p: *const ModuleVtable, slot: Slot) -> bool {
    let a = p as usize;
    let base = slot_base(slot);
    if a < base || a % align_of::<ModuleVtable>() != 0 {
        return false;
    }
    match a.checked_add(size_of::<ModuleVtable>()) {
        Some(end) => end <= base + slot_size(slot),
        None => false,
    }
}

/// Call the module currently loaded in `slot`.
///
/// # Safety
/// The caller must have verified the image and its hash; arbitrary code runs
/// in kernel mode from here.
pub unsafe fn call_entry(slot: Slot, len: usize) -> i32 {
    let base = slot_base(slot);
    let index = slot as usize;
    if len == 0 || len > slot_size(slot) {
        return -1;
    }
    // The image was written through the cacheable kseg0 mapping.  Write the
    // data cache back to RAM and invalidate stale instruction lines before the
    // first fetch of module code.
    crate::cache::dcache_writeback_invalidate(base, len);
    crate::cache::icache_invalidate(base, len);
    crate::cache::sync();

    // A new image replaces any previous callback in this slot.
    TICKS[index] = None;

    let entry: Entry = core::mem::transmute(base);
    let vtable = entry(&SERVICES);
    if vtable.is_null() {
        return 0;
    }
    if !vtable_in_slot(vtable, slot) {
        return -1;
    }
    // The vtable lives inside the verified image; read it directly.
    let status = core::ptr::read(vtable).status;
    if status != 0 {
        return status;
    }
    let tick = core::ptr::read(vtable).tick;
    if let Some(tick) = tick {
        LAST_TICKS[index] = service_clock_ms();
        TICKS[index] = Some(tick);
    }
    0
}

/// Run the periodic callbacks of both slots, if registered.
pub fn poll() {
    let now = service_clock_ms();
    for index in 0..2 {
        // SAFETY: single-threaded kernel; slot arrays are only written here
        // and in call_entry, which is not re-entered.
        let tick = unsafe { TICKS[index] };
        let Some(tick) = tick else {
            continue;
        };
        let last = unsafe { LAST_TICKS[index] };
        if now.saturating_sub(last) < 10 {
            continue;
        }
        unsafe {
            LAST_TICKS[index] = now;
        }
        let status = unsafe { tick(&SERVICES) };
        if status != 0 {
            unsafe {
                TICKS[index] = None;
            }
        }
    }
}

extern "C" fn service_write(ptr: *const u8, len: usize) {
    if ptr.is_null() || len == 0 || len > 8192 {
        return;
    }
    let data = unsafe { core::slice::from_raw_parts(ptr, len) };
    crate::write_bytes(data);
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
static mut MODULE_TCP_TX: [[u8; 16384]; MODULE_TCP_SLOTS] = [[0; 16384]; MODULE_TCP_SLOTS];

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

unsafe extern "C" fn service_block_read(target: u8, lba: u32, ptr: *mut u8) -> i32 {
    if ptr.is_null() {
        return -1;
    }
    let mut sector = [0u8; 512];
    match crate::scsi::read_sector(target, lba, &mut sector) {
        Ok(()) => {
            // SAFETY: the module passed a writable 512-byte buffer.
            unsafe {
                core::ptr::copy_nonoverlapping(sector.as_ptr(), ptr, sector.len());
            }
            0
        }
        Err(_) => -1,
    }
}

unsafe extern "C" fn service_block_write(target: u8, lba: u32, ptr: *const u8) -> i32 {
    if ptr.is_null() {
        return -1;
    }
    // SAFETY: the module passed a readable 512-byte buffer.
    let sector = unsafe { core::slice::from_raw_parts(ptr, 512) };
    let mut data = [0u8; 512];
    data.copy_from_slice(sector);
    match crate::scsi::write_sector(target, lba, &data) {
        Ok(()) => 0,
        Err(_) => -1,
    }
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
