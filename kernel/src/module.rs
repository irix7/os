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

use crate::sha256::Sha256;
use alloc::alloc::{alloc, Layout};
use core::mem::{align_of, size_of};
use smoltcp::iface::SocketHandle;
use smoltcp::socket::tcp::{Socket as TcpSocket, SocketBuffer};

/// The loadable-module ABI, shared with every module image.
pub use module_abi::{
    ModuleArena, ModuleContext, ModuleEntry, ModuleExportBinding, ModuleHandle, ModuleInfo,
    ModuleLoad, ModuleVtable, Services, ABI_VERSION, MODULE_BASE, MODULE_ERR_ARGUMENT,
    MODULE_ERR_BUSY, MODULE_ERR_DIGEST, MODULE_ERR_LIMIT, MODULE_ERR_REGION, MODULE_ERR_STATE,
    MODULE_LOAD_REPLACE, MODULE_MAX, MODULE_OK, MODULE_STATE_ENTERED, MODULE_STATE_LOADED,
};

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

/// First runtime arena region, after the two boot slots.
pub const ARENA_BASE: usize = 0x884c_0000;
/// Size of one runtime arena region.
pub const ARENA_REGION_SIZE: usize = 0x0004_0000;
/// Number of runtime arena regions.
pub const ARENA_REGION_COUNT: usize = 5;
/// One multiplexer entry per boot slot and runtime region.
pub const MODULE_REGION_COUNT: usize = 2 + ARENA_REGION_COUNT;

/// Base of multiplexer region `index`.
pub const fn region_base(index: usize) -> usize {
    match index {
        0 => slot_base(Slot::A),
        1 => slot_base(Slot::B),
        _ => ARENA_BASE + (index - 2) * ARENA_REGION_SIZE,
    }
}

/// Capacity of multiplexer region `index`.
pub const fn region_len(index: usize) -> usize {
    match index {
        0 => slot_size(Slot::A),
        1 => slot_size(Slot::B),
        _ => ARENA_REGION_SIZE,
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
    module_arena: service_module_arena,
    module_load: service_module_load,
    module_execute: service_module_execute,
    module_find: service_module_find,
    module_find_export: service_module_find_export,
    module_query: service_module_query,
    module_unload: service_module_unload,
};

/// Periodic callbacks for every multiplexer region.
static mut TICKS: [Option<unsafe extern "C" fn(*const Services) -> i32>; MODULE_REGION_COUNT] =
    [None; MODULE_REGION_COUNT];
static mut LAST_TICKS: [u64; MODULE_REGION_COUNT] = [0; MODULE_REGION_COUNT];

/// Drop a region's callback; used by load, unload and failed entries.
fn clear_tick(index: usize) {
    if index < MODULE_REGION_COUNT {
        // SAFETY: single-threaded kernel; every writer is on the boot/idle path.
        unsafe {
            TICKS[index] = None;
        }
    }
}

/// A module slot as a writable slice, for downloading straight into place.
pub fn area(slot: Slot) -> &'static mut [u8] {
    // SAFETY: the region is reserved for exactly this purpose; the kernel is
    // single-threaded and only the selected slot is used at a time.
    unsafe { core::slice::from_raw_parts_mut(slot_base(slot) as *mut u8, slot_size(slot)) }
}

/// Zero one module slot before a download.
pub fn clear(slot: Slot) {
    let index = slot as usize;
    clear_tick(index);
    if slot == Slot::B {
        crate::logical::forget_wasm();
    }
    // SAFETY: the region is reserved for modules and is not aliased.
    unsafe {
        core::ptr::write_bytes(slot_base(slot) as *mut u8, 0, slot_size(slot));
    }
    crate::cache::dcache_writeback_invalidate(slot_base(slot), slot_size(slot));
    crate::cache::sync();
}

fn vtable_in_slot(p: *const ModuleVtable, slot: Slot, len: usize) -> bool {
    let index = slot as usize;
    let base = region_base(index);
    let image_len = len.min(region_len(index));
    let a = p as usize;
    if a < base || a % align_of::<ModuleVtable>() != 0 {
        return false;
    }
    match a.checked_add(size_of::<ModuleVtable>()) {
        Some(end) => end <= base + image_len,
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

    // A new image replaces any previous callback in this region.
    clear_tick(index);

    let entry: Entry = core::mem::transmute(base);
    let vtable = entry(&SERVICES);
    if vtable.is_null() {
        return 0;
    }
    if !vtable_in_slot(vtable, slot, len) {
        return -1;
    }
    // SAFETY: the vtable lives inside the verified image; read it directly.
    let status = unsafe { core::ptr::read(vtable).status };
    if status != 0 {
        return status;
    }
    // SAFETY: same verified image; single-threaded kernel.
    unsafe {
        let tick = (*vtable).tick;
        LAST_TICKS[index] = service_clock_ms();
        TICKS[index] = tick;
    }
    0
}

/// Run the periodic callbacks of every registered multiplexer region.
pub fn poll() {
    let now = service_clock_ms();
    for index in 0..MODULE_REGION_COUNT {
        // SAFETY: single-threaded kernel; the arrays are only written here
        // and by load/execute/unload on the same thread.
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
            clear_tick(index);
        }
    }
}

const REGION_SIZE: usize = ARENA_REGION_SIZE;
const REGION_COUNT: usize = ARENA_REGION_COUNT;

#[derive(Clone, Copy)]
struct LoadedExport {
    interface: [u8; 64],
    interface_len: u32,
    version: u16,
    offset: u32,
}

impl LoadedExport {
    const EMPTY: Self = Self {
        interface: [0; 64],
        interface_len: 0,
        version: 0,
        offset: 0,
    };
}

#[derive(Clone, Copy)]
struct LoadedModule {
    live: bool,
    generation: u32,
    image_size: u32,
    state: u32,
    name: [u8; 64],
    name_len: u32,
    version: [u8; 32],
    version_len: u32,
    exports: [LoadedExport; 16],
    export_count: u32,
}

impl LoadedModule {
    const EMPTY: Self = Self {
        live: false,
        generation: 0,
        image_size: 0,
        state: 0,
        name: [0; 64],
        name_len: 0,
        version: [0; 32],
        version_len: 0,
        exports: [LoadedExport::EMPTY; 16],
        export_count: 0,
    };
}

static mut MODULES: [LoadedModule; REGION_COUNT] = [LoadedModule::EMPTY; REGION_COUNT];

unsafe extern "C" fn service_module_arena(out: *mut ModuleArena) -> i32 {
    if out.is_null() {
        return MODULE_ERR_ARGUMENT;
    }
    let mut used_lo = 0u32;
    let mut used_hi = 0u32;
    // SAFETY: single-threaded kernel.
    let modules = unsafe { &*core::ptr::addr_of!(MODULES) };
    for (index, module) in modules.iter().enumerate() {
        if module.live {
            if index < 32 {
                used_lo |= 1 << index;
            } else {
                used_hi |= 1 << (index - 32);
            }
        }
    }
    let arena = ModuleArena {
        base: ARENA_BASE as u32,
        length: (REGION_SIZE * REGION_COUNT) as u32,
        region_size: REGION_SIZE as u32,
        region_count: REGION_COUNT as u32,
        used_lo,
        used_hi,
        flags: module_abi::MODULE_ARENA_EXECUTABLE,
        reserved: 0,
    };
    unsafe { core::ptr::write(out, arena) };
    MODULE_OK
}

unsafe extern "C" fn service_module_load(
    request: *const ModuleLoad,
    out: *mut ModuleHandle,
) -> i32 {
    if request.is_null() || out.is_null() {
        return MODULE_ERR_ARGUMENT;
    }
    // SAFETY: the manager supplies a complete request for this synchronous call.
    let request = unsafe { &*request };
    if request.url.is_null()
        || request.url_len == 0
        || request.url_len > module_abi::MODULE_URL_MAX
        || request.name.is_null()
        || request.name_len == 0
        || request.name_len > module_abi::MODULE_NAME_MAX
        || request.version.is_null()
        || request.version_len == 0
        || request.version_len > module_abi::MODULE_VERSION_MAX
        || request.region as usize >= REGION_COUNT
        || request.export_count > module_abi::MODULE_EXPORT_MAX
    {
        return MODULE_ERR_ARGUMENT;
    }
    if request.max_size != 0 && request.max_size > REGION_SIZE as u32 {
        return MODULE_ERR_LIMIT;
    }
    let region = request.region as usize;
    let base = region_base(2 + region);
    // SAFETY: single-threaded kernel.
    let modules = unsafe { &mut *core::ptr::addr_of_mut!(MODULES) };
    if modules[region].live && request.flags & MODULE_LOAD_REPLACE == 0 {
        return MODULE_ERR_BUSY;
    }
    // Replacing a region invalidates any old callback even if the fetch fails.
    clear_tick(2 + region);
    // SAFETY: the request points at module-memory bytes for this synchronous call.
    let url = unsafe { core::slice::from_raw_parts(request.url, request.url_len as usize) };
    let Ok(url) = core::str::from_utf8(url) else {
        return MODULE_ERR_ARGUMENT;
    };
    let destination = unsafe { core::slice::from_raw_parts_mut(base as *mut u8, REGION_SIZE) };
    let summary = match crate::loader::fetch_url(url, destination) {
        Ok(summary) => summary,
        Err(_) => return MODULE_ERR_STATE,
    };
    if !summary.sha256.eq_digest(&Sha256(request.sha256)) {
        return MODULE_ERR_DIGEST;
    }
    let image_size = summary.body_len as u32;
    if request.max_size != 0 && image_size > request.max_size {
        return MODULE_ERR_LIMIT;
    }
    if request.export_count > 0 {
        if request.exports.is_null() {
            return MODULE_ERR_ARGUMENT;
        }
        // SAFETY: the module supplies the export descriptor array.
        let exports =
            unsafe { core::slice::from_raw_parts(request.exports, request.export_count as usize) };
        for export in exports {
            if export.interface.is_null()
                || export.interface_len == 0
                || export.interface_len > module_abi::MODULE_INTERFACE_MAX
                || export.offset % module_abi::MODULE_OFFSET_ALIGN != 0
                || export.offset != 0 && export.offset + 4 > image_size
            {
                return MODULE_ERR_ARGUMENT;
            }
        }
    }
    let generation = modules[region].generation.wrapping_add(1).max(1);
    let mut loaded = LoadedModule::EMPTY;
    loaded.live = true;
    loaded.generation = generation;
    loaded.image_size = image_size;
    loaded.state = MODULE_STATE_LOADED;
    loaded.name_len = request.name_len;
    loaded.version_len = request.version_len;
    // SAFETY: bounded request pointers copied synchronously.
    unsafe {
        core::ptr::copy_nonoverlapping(
            request.name,
            loaded.name.as_mut_ptr(),
            request.name_len as usize,
        );
        core::ptr::copy_nonoverlapping(
            request.version,
            loaded.version.as_mut_ptr(),
            request.version_len as usize,
        );
    }
    loaded.export_count = request.export_count;
    if request.export_count > 0 {
        // SAFETY: bounded pointer array validated above.
        let exports =
            unsafe { core::slice::from_raw_parts(request.exports, request.export_count as usize) };
        for (slot, export) in loaded.exports.iter_mut().zip(exports.iter()) {
            slot.interface_len = export.interface_len;
            slot.version = export.version;
            slot.offset = export.offset;
            // SAFETY: bounded interface bytes copied synchronously.
            unsafe {
                core::ptr::copy_nonoverlapping(
                    export.interface,
                    slot.interface.as_mut_ptr(),
                    export.interface_len as usize,
                );
            }
        }
    }
    modules[region] = loaded;
    let handle = ((generation as u64) << 32) | region as u64;
    // SAFETY: out is a validated manager pointer.
    unsafe { core::ptr::write(out, handle) };
    MODULE_OK
}

unsafe extern "C" fn service_module_execute(
    handle: ModuleHandle,
    entry_offset: u32,
    context: *const ModuleContext,
    out_status: *mut i32,
) -> i32 {
    let region = (handle & 0xffff_ffff) as usize;
    let generation = (handle >> 32) as u32;
    if region >= REGION_COUNT || entry_offset % module_abi::MODULE_OFFSET_ALIGN != 0 {
        return MODULE_ERR_ARGUMENT;
    }
    // SAFETY: single-threaded kernel.
    let modules = unsafe { &mut *core::ptr::addr_of_mut!(MODULES) };
    let module = &mut modules[region];
    if !module.live || module.generation != generation {
        return MODULE_ERR_STATE;
    }
    if entry_offset + 4 > module.image_size {
        return MODULE_ERR_ARGUMENT;
    }
    let entry = ARENA_BASE + region * REGION_SIZE + entry_offset as usize;
    crate::cache::dcache_writeback_invalidate(
        ARENA_BASE + region * REGION_SIZE,
        module.image_size as usize,
    );
    crate::cache::icache_invalidate(
        ARENA_BASE + region * REGION_SIZE,
        module.image_size as usize,
    );
    crate::cache::sync();
    // SAFETY: the region was verified and the offset is inside the image. The
    // manager owns the opaque context for the duration of the call.
    let entry: ModuleEntry = unsafe { core::mem::transmute(entry) };
    let vtable = unsafe { entry(&SERVICES, context) };
    module.state = MODULE_STATE_ENTERED;
    let tick_index = 2 + region;
    let status = if vtable.is_null() {
        clear_tick(tick_index);
        0
    } else {
        let base = region_base(2 + region);
        let image_len = module.image_size as usize;
        let address = vtable as usize;
        if address < base
            || address % align_of::<ModuleVtable>() != 0
            || address
                .checked_add(size_of::<ModuleVtable>())
                .map_or(true, |end| end > base + image_len)
        {
            clear_tick(tick_index);
            MODULE_ERR_STATE
        } else {
            // SAFETY: the table is inside the verified image.
            let status = unsafe { (*vtable).status };
            if status == 0 {
                unsafe {
                    LAST_TICKS[tick_index] = service_clock_ms();
                    TICKS[tick_index] = (*vtable).tick;
                }
            } else {
                clear_tick(tick_index);
            }
            status
        }
    };
    if !out_status.is_null() {
        // SAFETY: optional manager out pointer.
        unsafe {
            *out_status = if status == MODULE_ERR_STATE {
                0
            } else {
                status
            };
        }
    }
    if status == MODULE_ERR_STATE {
        MODULE_ERR_STATE
    } else {
        MODULE_OK
    }
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
    let name = unsafe { core::slice::from_raw_parts(name, name_len as usize) };
    let version = unsafe { core::slice::from_raw_parts(version, version_len as usize) };
    // SAFETY: single-threaded kernel.
    let modules = unsafe { &*core::ptr::addr_of!(MODULES) };
    for (region, module) in modules.iter().enumerate() {
        if module.live
            && &module.name[..module.name_len as usize] == name
            && &module.version[..module.version_len as usize] == version
        {
            let handle = ((module.generation as u64) << 32) | region as u64;
            unsafe { core::ptr::write(out, handle) };
            return MODULE_OK;
        }
    }
    MODULE_ERR_STATE
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
    let interface = unsafe { core::slice::from_raw_parts(interface, interface_len as usize) };
    // SAFETY: single-threaded kernel.
    let modules = unsafe { &*core::ptr::addr_of!(MODULES) };
    for (region, module) in modules.iter().enumerate() {
        if !module.live {
            continue;
        }
        for export in &module.exports[..module.export_count as usize] {
            if export.version == version
                && &export.interface[..export.interface_len as usize] == interface
            {
                let table = if export.offset == 0 {
                    core::ptr::null()
                } else {
                    (ARENA_BASE + region * REGION_SIZE + export.offset as usize) as *const u8
                };
                let binding = ModuleExportBinding {
                    handle: ((module.generation as u64) << 32) | region as u64,
                    table,
                    version,
                    reserved: 0,
                };
                unsafe { core::ptr::write(out, binding) };
                return MODULE_OK;
            }
        }
    }
    MODULE_ERR_STATE
}

unsafe extern "C" fn service_module_query(handle: ModuleHandle, out: *mut ModuleInfo) -> i32 {
    if out.is_null() {
        return MODULE_ERR_ARGUMENT;
    }
    let region = (handle & 0xffff_ffff) as usize;
    let generation = (handle >> 32) as u32;
    if region >= REGION_COUNT {
        return MODULE_ERR_ARGUMENT;
    }
    // SAFETY: single-threaded kernel.
    let modules = unsafe { &*core::ptr::addr_of!(MODULES) };
    let module = &modules[region];
    if !module.live || module.generation != generation {
        return MODULE_ERR_STATE;
    }
    let info = ModuleInfo {
        handle,
        base: (ARENA_BASE + region * REGION_SIZE) as u32,
        image_size: module.image_size,
        region: region as u32,
        export_count: module.export_count,
        state: module.state,
        reserved: 0,
    };
    unsafe { core::ptr::write(out, info) };
    MODULE_OK
}

unsafe extern "C" fn service_module_unload(handle: ModuleHandle) -> i32 {
    let region = (handle & 0xffff_ffff) as usize;
    let generation = (handle >> 32) as u32;
    if region >= REGION_COUNT {
        return MODULE_ERR_ARGUMENT;
    }
    // SAFETY: single-threaded kernel.
    let modules = unsafe { &mut *core::ptr::addr_of_mut!(MODULES) };
    let module = &mut modules[region];
    if !module.live || module.generation != generation {
        return MODULE_ERR_STATE;
    }
    clear_tick(2 + region);
    let base = region_base(2 + region);
    // SAFETY: the region is owned by this module slot and is being released.
    unsafe { core::ptr::write_bytes(base as *mut u8, 0, REGION_SIZE) };
    crate::cache::dcache_writeback_invalidate(base, REGION_SIZE);
    crate::cache::icache_invalidate(base, REGION_SIZE);
    crate::cache::sync();
    module.live = false;
    module.generation = generation.wrapping_add(1).max(1);
    module.image_size = 0;
    module.state = 0;
    module.export_count = 0;
    MODULE_OK
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
