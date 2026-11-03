// SPDX-License-Identifier: GPL-3.0-only
//! Loadable modules: the Indy's slots, transport and tick loop.
//!
//! The compressed PROM image is meant to stop growing.  Code that belongs to
//! the running system but is not needed to boot (the TLS client is the first
//! candidate) can be linked at [`MODULE_BASE`], downloaded over the network,
//! verified, and called in place.  Modules run in kernel mode and use the
//! shared service table rather than linking against kernel symbols.
//!
//! The entry checks, the vtable bounds check and the service table are
//! `kernel-core`, the same code every machine runs.  This file keeps what is
//! specific to the Indy: the two fixed boot slots, the SCSI and HTTP fetch
//! transports, the runtime arena and the 10 ms tick loop.

use smoltcp::iface::{Interface, SocketHandle, SocketSet};
use smoltcp::socket::udp::{PacketBuffer, RecvError, SendError, Socket as UdpSocket, UdpMetadata};
use smoltcp::storage::PacketMetadata;

use crate::net_irq;

/// The loadable-module ABI, shared with every module image.
pub use module_abi::{
    ModuleArena, ModuleContext, ModuleEntry, ModuleExportBinding, ModuleHandle, ModuleInfo,
    ModuleLoad, MODULE_BASE, MODULE_ERR_ARGUMENT, MODULE_ERR_BUSY, MODULE_ERR_DIGEST,
    MODULE_ERR_LIMIT, MODULE_ERR_REGION, MODULE_ERR_STATE, MODULE_LOAD_REPLACE, MODULE_MAX,
    MODULE_OK, MODULE_STATE_ENTERED, MODULE_STATE_LOADED,
};

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

// The M1 bridge record sits below the module area so no slot or arena write
// can reach it.
const _: () = assert!(
    m1_policy_bridge::BRIDGE_BASE + m1_policy_bridge::BRIDGE_SIZE <= ARENA_BASE,
    "M1 bridge overlaps the runtime arena"
);
const _: () = assert!(
    m1_policy_bridge::BRIDGE_BASE + m1_policy_bridge::BRIDGE_SIZE <= MODULE_BASE,
    "M1 bridge overlaps the module area"
);
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

/// Periodic callbacks for every multiplexer region.
static mut TICKS: [Option<unsafe extern "C" fn(*const module_abi::Services) -> i32>;
    MODULE_REGION_COUNT] = [None; MODULE_REGION_COUNT];
static mut LAST_TICKS: [u64; MODULE_REGION_COUNT] = [0; MODULE_REGION_COUNT];

/// Owning module region for each TCP slot, or [`net_irq::NO_OWNER`] when the
/// socket was not created inside a module call.
static mut SLOT_OWNER: [u8; MODULE_TCP_SLOTS] = [net_irq::NO_OWNER; MODULE_TCP_SLOTS];

/// The module region whose entry or tick is currently running.
///
/// A socket created inside a module call is attributed to that region, so a
/// ready socket wakes only the module that owns it instead of every loaded
/// module.  `usize::MAX` means "no module call"; sockets created there are
/// unowned and wake every region.
static mut CURRENT_REGION: usize = usize::MAX;

/// Run `f` with `region` as the current module.
fn with_region<R>(region: usize, f: impl FnOnce() -> R) -> R {
    // SAFETY: single-threaded; the nested save/restore keeps the caller's
    // value if a module calls back into the loader.
    let previous = unsafe { CURRENT_REGION };
    unsafe { CURRENT_REGION = region };
    let result = f();
    unsafe { CURRENT_REGION = previous };
    result
}

/// Attribute `slot` to the region that is creating it.
fn adopt_slot(slot: u32) {
    let slot = slot as usize;
    if slot >= MODULE_TCP_SLOTS {
        return;
    }
    // SAFETY: single-threaded; module calls run one at a time.
    let owner = unsafe { CURRENT_REGION };
    let tag = if owner == usize::MAX {
        net_irq::NO_OWNER
    } else {
        owner as u8
    };
    unsafe {
        SLOT_OWNER[slot] = tag;
    }
}

/// Regions with a ready socket, as a bit per region.
///
/// The 10 ms gate still runs every region, so a region with no socket is
/// delayed by at most one tick; a ready socket wakes its owner immediately.
pub(crate) fn ready_regions() -> u64 {
    let mut owners = [net_irq::NO_OWNER; MODULE_TCP_SLOTS];
    let mut ready = [false; MODULE_TCP_SLOTS];
    for (slot, (owner, ready_slot)) in owners.iter_mut().zip(ready.iter_mut()).enumerate() {
        let mask = tcp_events(slot as u32);
        *ready_slot = net_irq::slot_ready(mask);
        // SAFETY: single-threaded; the owner table is written by adopt_slot.
        *owner = unsafe { SLOT_OWNER[slot] };
    }
    net_irq::ready_regions(&owners, &ready, MODULE_REGION_COUNT)
}

/// Whether any region has a ready socket.
pub fn any_ready() -> bool {
    ready_regions() != 0
}

/// Drop a region's callback; used by load, unload and failed entries.
fn clear_tick(index: usize) {
    if index < MODULE_REGION_COUNT {
        // SAFETY: single-threaded kernel; every writer is on the boot/idle path.
        unsafe {
            TICKS[index] = None;
            // A slot cannot outlive the module that created it: a reloaded
            // region re-adopts its sockets, and a removed region must not wake
            // on a stale ready bit.
            for slot in 0..MODULE_TCP_SLOTS {
                if SLOT_OWNER[slot] == index as u8 {
                    SLOT_OWNER[slot] = net_irq::NO_OWNER;
                }
            }
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
///
/// Clearing slot B invalidates the Wasm policy publication record, but not the
/// kernel's adopted policy: a fetch, digest or entry failure must leave the
/// previous live policy unchanged.
pub fn clear(slot: Slot) {
    let index = slot as usize;
    clear_tick(index);
    if slot == Slot::B {
        crate::logical::begin_wasm_load();
    }
    // SAFETY: the region is reserved for modules and is not aliased.
    unsafe {
        core::ptr::write_bytes(slot_base(slot) as *mut u8, 0, slot_size(slot));
    }
    crate::cache::dcache_writeback_invalidate(slot_base(slot), slot_size(slot));
    crate::cache::sync();
}

/// The Indy's slot and kseg0 cache maintenance, for the shared entry call.
pub(crate) struct IndyHost;

impl kernel_core::image::Host for IndyHost {
    fn clear(&mut self, image: &kernel_core::image::Image) {
        // SAFETY: the caller reserved the window for modules.
        unsafe {
            core::ptr::write_bytes(image.base as *mut u8, 0, image.window);
        }
        crate::cache::dcache_writeback_invalidate(image.base, image.window);
        crate::cache::sync();
    }

    fn copy(&mut self, image: &kernel_core::image::Image, offset: usize, bytes: &[u8]) {
        // SAFETY: the caller bounded `offset + bytes.len()` by the window.
        unsafe {
            core::ptr::copy_nonoverlapping(
                bytes.as_ptr(),
                (image.base as *mut u8).add(offset),
                bytes.len(),
            );
        }
    }

    fn flush_code(&mut self, image: &kernel_core::image::Image, length: usize) {
        crate::cache::dcache_writeback_invalidate(image.base, length);
        crate::cache::icache_invalidate(image.base, length);
        crate::cache::sync();
    }
}

/// Call the module currently loaded in `slot`.
///
/// # Safety
/// The caller must have verified the image and its hash; arbitrary code runs
/// in kernel mode from here.
pub unsafe fn call_entry(slot: Slot, len: usize) -> i32 {
    let index = slot as usize;
    // A new image replaces any previous callback in this region.
    clear_tick(index);
    let image = kernel_core::image::Image::new(slot_base(slot), slot_size(slot), "indy");
    // SAFETY: the caller verified the image and its hash.  Sockets this entry
    // creates belong to this boot slot, not to every loaded module:
    // `ready_regions` uses the tag to wake only the owner.
    match with_region(index, || unsafe {
        kernel_core::image::enter(&image, len, &mut IndyHost)
    }) {
        Ok(report) => {
            if let Some(vtable) = report.vtable() {
                // SAFETY: the vtable lives inside the verified image, and the
                // kernel is single-threaded.
                unsafe {
                    LAST_TICKS[index] = crate::services::clock_ms();
                    TICKS[index] = (*vtable).tick;
                }
            }
            0
        }
        Err(kernel_core::image::Error::Status(status)) => status,
        Err(_) => -1,
    }
}

/// Last time a module was woken by socket readiness, for the short-wait
/// decision.  The 10 ms gate does not set this: a tick that only honoured the
/// deadline is not traffic, and treating it as activity would keep an idle
/// node on the 0.5 ms observation forever.
static mut LAST_ACTIVITY_MS: u64 = 0;

/// Whether a module serviced a ready socket recently enough that a short wait
/// can catch the peer's next record.  This is a wake hint, not a work queue.
pub fn recently_active() -> bool {
    let now = crate::services::clock_ms();
    // SAFETY: single-threaded kernel.
    unsafe { now.saturating_sub(LAST_ACTIVITY_MS) < 50 }
}

/// Run the due and ready module regions.
///
/// Returns whether any region ran because a socket was ready, not merely
/// because its 10 ms deadline elapsed.  The caller uses that answer to decide
/// between the 10 ms and 0.5 ms waits, so a deadline tick must not count as
/// activity.
pub fn poll() -> bool {
    let now = crate::services::clock_ms();
    let ready = ready_regions();
    let mut woke = false;
    for index in 0..MODULE_REGION_COUNT {
        // SAFETY: single-threaded kernel; the arrays are only written here
        // and by load/execute/unload on the same thread.
        let tick = unsafe { TICKS[index] };
        let Some(tick) = tick else {
            continue;
        };
        let last = unsafe { LAST_TICKS[index] };
        // Readiness wakes the owning region immediately; the 10 ms gate stays
        // the deadline/fallback cadence for retries, leases and heartbeats in
        // every region, including ones with no socket of their own.
        let ready_region = ready & (1u64 << index) != 0;
        let due = now.saturating_sub(last) >= 10;
        if !due && !ready_region {
            continue;
        }
        unsafe {
            LAST_TICKS[index] = now;
        }
        let status = with_region(index, || unsafe { tick(&kernel_core::service::SERVICES) });
        woke |= ready_region;
        if status != 0 {
            clear_tick(index);
        }
    }
    if woke {
        // SAFETY: single-threaded kernel.
        unsafe {
            LAST_ACTIVITY_MS = now;
        }
    }
    woke
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

pub(crate) fn module_arena(out: &mut ModuleArena) -> i32 {
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
    *out = ModuleArena {
        base: ARENA_BASE as u32,
        length: (REGION_SIZE * REGION_COUNT) as u32,
        region_size: REGION_SIZE as u32,
        region_count: REGION_COUNT as u32,
        used_lo,
        used_hi,
        flags: module_abi::MODULE_ARENA_EXECUTABLE,
        reserved: 0,
    };
    MODULE_OK
}

pub(crate) fn module_load(request: &ModuleLoad, out: &mut ModuleHandle) -> i32 {
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
    if kernel_core::image::verify_digest(&request.sha256, &summary.sha256.0).is_err() {
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
    *out = ((generation as u64) << 32) | region as u64;
    MODULE_OK
}

pub(crate) fn module_execute(
    handle: ModuleHandle,
    entry_offset: u32,
    context: Option<&ModuleContext>,
    out_status: Option<&mut i32>,
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
    let base = ARENA_BASE + region * REGION_SIZE;
    let entry = base + entry_offset as usize;
    crate::cache::dcache_writeback_invalidate(base, module.image_size as usize);
    crate::cache::icache_invalidate(base, module.image_size as usize);
    crate::cache::sync();
    // SAFETY: the region was verified and the offset is inside the image. The
    // manager owns the opaque context for the duration of the call.
    let entry: ModuleEntry = unsafe { core::mem::transmute(entry) };
    let context = context.map_or(core::ptr::null(), |context| context as *const ModuleContext);
    let vtable = with_region(2 + region, || unsafe {
        entry(&kernel_core::service::SERVICES, context)
    });
    module.state = MODULE_STATE_ENTERED;
    let tick_index = 2 + region;
    let status = if vtable.is_null() {
        clear_tick(tick_index);
        0
    } else {
        let image_len = module.image_size as usize;
        if !kernel_core::image::validate_vtable(vtable, base, image_len) {
            clear_tick(tick_index);
            MODULE_ERR_STATE
        } else {
            // SAFETY: the table is inside the verified image.
            let status = unsafe { (*vtable).status };
            if status == 0 {
                // SAFETY: single-threaded kernel.
                unsafe {
                    LAST_TICKS[tick_index] = crate::services::clock_ms();
                    TICKS[tick_index] = (*vtable).tick;
                }
            } else {
                clear_tick(tick_index);
            }
            status
        }
    };
    if let Some(out_status) = out_status {
        *out_status = if status == MODULE_ERR_STATE {
            0
        } else {
            status
        };
    }
    if status == MODULE_ERR_STATE {
        MODULE_ERR_STATE
    } else {
        MODULE_OK
    }
}

pub(crate) fn module_find(name: &[u8], version: &[u8], out: &mut ModuleHandle) -> i32 {
    if name.is_empty()
        || name.len() > module_abi::MODULE_NAME_MAX as usize
        || version.is_empty()
        || version.len() > module_abi::MODULE_VERSION_MAX as usize
    {
        return MODULE_ERR_ARGUMENT;
    }
    // SAFETY: single-threaded kernel.
    let modules = unsafe { &*core::ptr::addr_of!(MODULES) };
    for (region, module) in modules.iter().enumerate() {
        if module.live
            && &module.name[..module.name_len as usize] == name
            && &module.version[..module.version_len as usize] == version
        {
            *out = ((module.generation as u64) << 32) | region as u64;
            return MODULE_OK;
        }
    }
    MODULE_ERR_STATE
}

pub(crate) fn module_find_export(
    interface: &[u8],
    version: u16,
    out: &mut ModuleExportBinding,
) -> i32 {
    if interface.is_empty() || interface.len() > module_abi::MODULE_INTERFACE_MAX as usize {
        return MODULE_ERR_ARGUMENT;
    }
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
                *out = ModuleExportBinding {
                    handle: ((module.generation as u64) << 32) | region as u64,
                    table,
                    version,
                    reserved: 0,
                };
                return MODULE_OK;
            }
        }
    }
    MODULE_ERR_STATE
}

pub(crate) fn module_query(handle: ModuleHandle, out: &mut ModuleInfo) -> i32 {
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
    *out = ModuleInfo {
        handle,
        base: (ARENA_BASE + region * REGION_SIZE) as u32,
        image_size: module.image_size,
        region: region as u32,
        export_count: module.export_count,
        state: module.state,
        reserved: 0,
    };
    MODULE_OK
}

pub(crate) fn module_unload(handle: ModuleHandle) -> i32 {
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

/// The Indy's stack, for the shared module TCP slot table.
struct IndyStack;

impl module_tcp::Stack for IndyStack {
    fn with_sockets<R>(f: impl FnOnce(&mut SocketSet<'static>) -> R) -> Option<R> {
        crate::net::sockets_with(f)
    }

    fn with_stack<R>(f: impl FnOnce(&mut Interface, &mut SocketSet<'static>) -> R) -> Option<R> {
        crate::net::with_stack(f)
    }
}

const MODULE_TCP_SLOTS: usize = 8;
/// The receive ring is deliberately larger than the two-node case needs.
///
/// Under the M3 host hub several member records share the root's single mesh
/// slot while the module writes a bounded serial burst.  A 2 KiB buffer fills
/// during that burst, advertises a tiny window, and then trickles the rest of
/// a 388-byte record through the host forward's zero-window probe; the 10 s
/// receive deadline fires on a healthy link.  16 KiB holds a full eight-member
/// join/refresh burst, so the window never collapses during logging.
const MODULE_TCP_BUFFER: usize = 16384;

static mut MODULE_TCP: module_tcp::Slots<MODULE_TCP_SLOTS, MODULE_TCP_BUFFER> =
    module_tcp::Slots::new();

fn module_tcp() -> &'static mut module_tcp::Slots<MODULE_TCP_SLOTS, MODULE_TCP_BUFFER> {
    // SAFETY: single-threaded kernel; the slot table is used only here.
    unsafe { &mut *core::ptr::addr_of_mut!(MODULE_TCP) }
}

pub(crate) fn tcp_listen(slot: u32, port: u16) -> i32 {
    let result = module_tcp().listen::<IndyStack>(slot, port);
    if result == 0 {
        adopt_slot(slot);
    }
    result
}

pub(crate) fn tcp_connect(slot: u32, address: u32, port: u16) -> i32 {
    let result = module_tcp().connect::<IndyStack>(slot, address, port);
    if result == 0 {
        adopt_slot(slot);
    }
    result
}

/// Level-triggered readiness for one module TCP slot.  The bits are recomputed
/// from the live socket; there is no event queue and no lost edge.
pub(crate) fn tcp_events(slot: u32) -> u32 {
    module_tcp().events::<IndyStack>(slot)
}

/// Number of module TCP slots the kernel provides.
pub(crate) fn tcp_slots() -> u32 {
    module_tcp().slots()
}

pub(crate) fn tcp_state(slot: u32) -> u32 {
    module_tcp().state::<IndyStack>(slot)
}

pub(crate) fn tcp_recv(slot: u32, out: &mut [u8]) -> isize {
    module_tcp().recv_into::<IndyStack>(slot, out)
}

pub(crate) fn tcp_send(slot: u32, bytes: &[u8]) -> isize {
    module_tcp().send_from::<IndyStack>(slot, bytes)
}

pub(crate) fn tcp_close(slot: u32) {
    module_tcp().close::<IndyStack>(slot);
}

pub(crate) fn tcp_abort(slot: u32) {
    module_tcp().abort::<IndyStack>(slot);
}

/// Module UDP slots.  A datagram transport needs one slot per peer link plus
/// one for a listener; eight matches the TCP policy and fits the socket set.
const MODULE_UDP_SLOTS: usize = 8;

/// One datagram buffer per direction per slot.  A 2 KiB buffer holds the
/// largest QUIC datagram that fits an Ethernet path (1500-byte MTU after IP
/// and UDP headers) with room for a coalesced short-header packet.
const MODULE_UDP_BUFFER: usize = 2048;

/// Datagrams queued per direction per slot.  The payload ring is the larger
/// bound; this caps how many small datagrams can wait in one poll.
const MODULE_UDP_PACKETS: usize = 8;

static mut MODULE_UDP: [Option<SocketHandle>; MODULE_UDP_SLOTS] = [None; MODULE_UDP_SLOTS];
static mut MODULE_UDP_RX: [[u8; MODULE_UDP_BUFFER]; MODULE_UDP_SLOTS] =
    [[0; MODULE_UDP_BUFFER]; MODULE_UDP_SLOTS];
static mut MODULE_UDP_TX: [[u8; MODULE_UDP_BUFFER]; MODULE_UDP_SLOTS] =
    [[0; MODULE_UDP_BUFFER]; MODULE_UDP_SLOTS];
static mut MODULE_UDP_RX_META: [[PacketMetadata<UdpMetadata>; MODULE_UDP_PACKETS];
    MODULE_UDP_SLOTS] = [[PacketMetadata::EMPTY; MODULE_UDP_PACKETS]; MODULE_UDP_SLOTS];
static mut MODULE_UDP_TX_META: [[PacketMetadata<UdpMetadata>; MODULE_UDP_PACKETS];
    MODULE_UDP_SLOTS] = [[PacketMetadata::EMPTY; MODULE_UDP_PACKETS]; MODULE_UDP_SLOTS];

fn udp_handle(slot: u32) -> Option<SocketHandle> {
    let slot = slot as usize;
    if slot >= MODULE_UDP_SLOTS {
        return None;
    }
    // SAFETY: single-threaded kernel; the table is only written by the
    // service functions below.
    unsafe { MODULE_UDP[slot] }
}

/// Remove any UDP socket previously owned by `slot`.
fn clear_udp_slot(slot: usize) {
    if let Some(old) = unsafe { MODULE_UDP[slot] } {
        let _ = crate::net::sockets_with(|sockets| sockets.remove(old));
        // SAFETY: single-threaded; the old socket has been removed.
        unsafe { MODULE_UDP[slot] = None };
    }
}

pub(crate) fn udp_open(slot: u32, port: u16) -> i32 {
    let slot_usize = slot as usize;
    if slot_usize >= MODULE_UDP_SLOTS || port == 0 {
        return -1;
    }
    clear_udp_slot(slot_usize);
    // SAFETY: each slot's buffers are owned by at most one socket at a time.
    let rx = unsafe { &mut *core::ptr::addr_of_mut!(MODULE_UDP_RX[slot_usize]) };
    let tx = unsafe { &mut *core::ptr::addr_of_mut!(MODULE_UDP_TX[slot_usize]) };
    let rx_meta = unsafe { &mut *core::ptr::addr_of_mut!(MODULE_UDP_RX_META[slot_usize]) };
    let tx_meta = unsafe { &mut *core::ptr::addr_of_mut!(MODULE_UDP_TX_META[slot_usize]) };
    let mut socket = UdpSocket::new(
        PacketBuffer::new(&mut rx_meta[..], &mut rx[..]),
        PacketBuffer::new(&mut tx_meta[..], &mut tx[..]),
    );
    if socket.bind(port).is_err() {
        return -1;
    }
    let Some(handle) = crate::net::sockets_with(|sockets| sockets.add(socket)) else {
        return -1;
    };
    // SAFETY: single-threaded.
    unsafe { MODULE_UDP[slot_usize] = Some(handle) };
    0
}

pub(crate) fn udp_send(slot: u32, address: u32, port: u16, bytes: &[u8]) -> isize {
    if bytes.is_empty() || address == 0 || port == 0 {
        return -1;
    }
    let Some(handle) = udp_handle(slot) else {
        return -1;
    };
    // `address` is the IPv4 address in network byte order.
    let octets = address.to_be_bytes();
    let remote = smoltcp::wire::Ipv4Address::new(octets[0], octets[1], octets[2], octets[3]);
    let result = crate::net::sockets_with(|sockets| {
        sockets
            .get_mut::<UdpSocket>(handle)
            .send_slice(bytes, (smoltcp::wire::IpAddress::Ipv4(remote), port))
    });
    match result {
        Some(Ok(())) => bytes.len() as isize,
        // The socket buffer is full: the module retries after the next poll.
        Some(Err(SendError::BufferFull)) => 0,
        _ => -1,
    }
}

pub(crate) fn udp_recv(
    slot: u32,
    out: &mut [u8],
    src_address: Option<&mut u32>,
    src_port: Option<&mut u16>,
) -> isize {
    if out.is_empty() {
        return -1;
    }
    let Some(handle) = udp_handle(slot) else {
        return -1;
    };
    let result =
        crate::net::sockets_with(|sockets| sockets.get_mut::<UdpSocket>(handle).recv_slice(out));
    match result {
        Some(Ok((n, meta))) => {
            if let smoltcp::wire::IpAddress::Ipv4(address) = meta.endpoint.addr {
                let octets = address.octets();
                if let Some(src_address) = src_address {
                    *src_address = u32::from_be_bytes(octets);
                }
                if let Some(src_port) = src_port {
                    *src_port = meta.endpoint.port;
                }
            }
            n as isize
        }
        Some(Err(RecvError::Exhausted)) => 0,
        // The datagram did not fit the caller's buffer and was dropped.
        Some(Err(RecvError::Truncated)) => -2,
        None => -1,
    }
}

pub(crate) fn udp_close(slot: u32) {
    let slot = slot as usize;
    if slot < MODULE_UDP_SLOTS {
        clear_udp_slot(slot);
    }
}

/// Number of module UDP slots the kernel provides.
pub(crate) fn udp_slots() -> u32 {
    MODULE_UDP_SLOTS as u32
}

pub(crate) fn udp_state(slot: u32, address: Option<&mut u32>, port: Option<&mut u16>) -> i32 {
    let Some(handle) = udp_handle(slot) else {
        return -1;
    };
    let endpoint = crate::net::sockets_with(|sockets| sockets.get::<UdpSocket>(handle).endpoint());
    let Some(endpoint) = endpoint else {
        return -1;
    };
    if let Some(port) = port {
        *port = endpoint.port;
    }
    if let Some(address) = address {
        let local = crate::net::ip_addr().map(|ip| u32::from_be_bytes(ip.octets()));
        *address = local.unwrap_or(0);
    }
    0
}
