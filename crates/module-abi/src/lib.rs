// SPDX-License-Identifier: GPL-3.0-only
//! The loadable-module ABI shared by the kernel and every module image.
//!
//! The table is append-only: new services go at the end and [`ABI_VERSION`]
//! is bumped.  A module links this crate, so the kernel and the module cannot
//! disagree about a field offset.  The compile-time assertions below cover the
//! n32 layout (32-bit pointers); a host build is 64-bit and skips them.
//!
//! ABI 16 appends the UDP, timer/deadline and CSPRNG services that QUIC v1
//! needs as the primary transport.  TCP stays for POSIX sockets and the
//! bootstrap paths; UDP is the only transport the kernel exposes to modules
//! for native flows.

#![no_std]

/// Load address of the module area (kseg0, physical `0x0840_0000`).
pub const MODULE_BASE: usize = 0x8840_0000;

/// Maximum module image size.
pub const MODULE_MAX: usize = 0x0020_0000; // 2 MiB

/// Service-table version understood by this ABI.
pub const ABI_VERSION: u32 = 16;

/// Entry offset used by a legacy `_start(const Services *)` image.
pub const MODULE_ENTRY_LEGACY: u32 = 0;

/// Byte alignment required for a module entry or export table offset.
pub const MODULE_OFFSET_ALIGN: u32 = 4;

/// Maximum number of arena regions represented by [`ModuleArena::used_lo`]
/// and [`ModuleArena::used_hi`].
pub const MODULE_REGION_MAX: u32 = 64;

/// Maximum module name bytes copied into the loaded-module table.
pub const MODULE_NAME_MAX: u32 = 63;

/// Maximum module version bytes copied into the loaded-module table.
pub const MODULE_VERSION_MAX: u32 = 31;

/// Maximum interface name bytes copied into the loaded-module table.
pub const MODULE_INTERFACE_MAX: u32 = 63;

/// Maximum exports accepted by one [`ModuleLoad`] request.
pub const MODULE_EXPORT_MAX: u32 = 16;

/// Maximum image URL bytes accepted by one [`ModuleLoad`] request.
pub const MODULE_URL_MAX: u32 = 511;

/// Version of [`ModuleContext`].
pub const MODULE_CONTEXT_VERSION: u32 = 1;

/// [`ModuleLoad::flags`]: replace the module already in the chosen region.
pub const MODULE_LOAD_REPLACE: u32 = 1 << 0;

/// [`ModuleArena::flags`]: the regions are executable kseg0 memory.
pub const MODULE_ARENA_EXECUTABLE: u32 = 1 << 0;

/// [`ModuleInfo::state`]: the verified image was copied into the region.
pub const MODULE_STATE_LOADED: u32 = 1;

/// [`ModuleInfo::state`]: the module entry was entered at least once.
pub const MODULE_STATE_ENTERED: u32 = 2;

/// Loader status: success.
pub const MODULE_OK: i32 = 0;
/// Loader status: null pointer, bad length, unaligned offset or wild range.
pub const MODULE_ERR_ARGUMENT: i32 = -1;
/// Loader status: region index is outside [`ModuleArena::region_count`].
pub const MODULE_ERR_REGION: i32 = -2;
/// Loader status: the region is in use and no replace flag was supplied.
pub const MODULE_ERR_BUSY: i32 = -3;
/// Loader status: the fetched bytes do not match the expected SHA-256.
pub const MODULE_ERR_DIGEST: i32 = -4;
/// Loader status: the image size does not fit the entry or the region bound.
pub const MODULE_ERR_SIZE: i32 = -5;
/// Loader status: fetch failed (DNS, TLS, HTTP or a short read).
pub const MODULE_ERR_TRANSPORT: i32 = -6;
/// Loader status: the identity or export lookup found no match.
pub const MODULE_ERR_NOT_FOUND: i32 = -7;
/// Loader status: name, version, interface or export-count limit exceeded.
pub const MODULE_ERR_LIMIT: i32 = -8;
/// Loader status: stale handle, missing module or invalid entry offset.
pub const MODULE_ERR_STATE: i32 = -9;

/// Opaque loaded-module handle: `generation << 32 | region`.
///
/// Zero is never a valid handle.  The generation changes on load, replace and
/// unload, so a handle from before a replacement fails with
/// [`MODULE_ERR_STATE`] instead of entering stale bytes.
pub type ModuleHandle = u64;

/// The bounded module arena reported by [`Services::module_arena`].
///
/// Regions are contiguous and equally sized: region `i` starts at
/// `base + i * region_size` and has `region_size` bytes.  `used_lo` bit `i`
/// (and `used_hi` bit `i - 32`) marks a loaded region.  Only
/// [`MODULE_REGION_MAX`] regions are representable.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ModuleArena {
    /// kseg0 address of region 0.
    pub base: u32,
    /// Total bytes reserved for the arena; `region_size * region_count`.
    pub length: u32,
    /// Bytes in each region, chosen by the kernel.
    pub region_size: u32,
    /// Number of regions; at most [`MODULE_REGION_MAX`].
    pub region_count: u32,
    /// Bit `i` is set when region `i` is loaded.
    pub used_lo: u32,
    /// Bit `i` is set when region `32 + i` is loaded.
    pub used_hi: u32,
    /// [`MODULE_ARENA_EXECUTABLE`] when the regions are executable.
    pub flags: u32,
    /// Reserved; zero.
    pub reserved: u32,
}

impl ModuleArena {
    /// A zeroed arena descriptor.
    pub const EMPTY: Self = Self {
        base: 0,
        length: 0,
        region_size: 0,
        region_count: 0,
        used_lo: 0,
        used_hi: 0,
        flags: 0,
        reserved: 0,
    };
}

/// Manager-provided context passed to a module entry.
///
/// The kernel treats this as opaque and forwards the pointer unchanged.  The
/// manager and its dependents agree on any layout beyond this versioned header;
/// no native pointer to kernel state is ever implied by it.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ModuleContext {
    /// [`MODULE_CONTEXT_VERSION`].
    pub version: u32,
    /// Manager-defined flags; zero unless the manager documents them.
    pub flags: u32,
    /// Opaque manager state; the kernel never dereferences it.
    pub manager: *const u8,
    /// Reserved; zero.
    pub reserved: u32,
}

impl ModuleContext {
    /// A context with no manager state.
    pub const EMPTY: Self = Self {
        version: MODULE_CONTEXT_VERSION,
        flags: 0,
        manager: core::ptr::null(),
        reserved: 0,
    };
}

/// One export declared in a [`ModuleLoad`] request.
///
/// `interface` is a module-memory UTF-8 string, not a kernel pointer.  The
/// kernel copies it into the loaded-module table.  `offset` is relative to the
/// start of the loaded image and points at the interface-defined C function
/// table; zero means "interface declaration only, no table".
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ModuleExport {
    /// Interface name bytes in module memory.
    pub interface: *const u8,
    /// Length of `interface`; at most [`MODULE_INTERFACE_MAX`].
    pub interface_len: u32,
    /// Interface version.
    pub version: u16,
    /// Reserved; zero.
    pub reserved: u16,
    /// Image-relative table offset, aligned to [`MODULE_OFFSET_ALIGN`].  Zero
    /// means no table; a non-zero offset must leave at least four bytes inside
    /// the verified image.
    pub offset: u32,
}

impl ModuleExport {
    /// An empty export slot.
    pub const EMPTY: Self = Self {
        interface: core::ptr::null(),
        interface_len: 0,
        version: 0,
        reserved: 0,
        offset: 0,
    };
}

/// The result of [`Services::module_find_export`].
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ModuleExportBinding {
    /// Live handle that owns the export.
    pub handle: ModuleHandle,
    /// Absolute address of the function table, or null when the manifest
    /// declared no table.
    pub table: *const u8,
    /// Interface version that was matched.
    pub version: u16,
    /// Reserved; zero.
    pub reserved: u16,
}

impl ModuleExportBinding {
    /// An empty binding.
    pub const EMPTY: Self = Self {
        handle: 0,
        table: core::ptr::null(),
        version: 0,
        reserved: 0,
    };
}

/// A verified-image load request for [`Services::module_load`].
///
/// The kernel fetches `url`, checks exactly the fetched bytes against
/// `sha256`, copies them into the chosen region and registers the identity and
/// exports.  The identity strings and export descriptors are copied before the
/// call returns, so the caller may reuse those buffers.  The operation is
/// atomic: a failure leaves the region untouched and no lookup entry visible.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ModuleLoad {
    /// Immutable image URL in module memory; at most [`MODULE_URL_MAX`].
    pub url: *const u8,
    /// Length of `url`.
    pub url_len: u32,
    /// Expected SHA-256 of the complete fetched file, including any manifest
    /// and footer.
    pub sha256: [u8; 32],
    /// Exact module name in module memory; at most [`MODULE_NAME_MAX`].
    pub name: *const u8,
    /// Length of `name`.
    pub name_len: u32,
    /// Exact module version in module memory; at most [`MODULE_VERSION_MAX`].
    pub version: *const u8,
    /// Length of `version`.
    pub version_len: u32,
    /// Export declarations, or null when `export_count` is zero.
    pub exports: *const ModuleExport,
    /// Number of entries in `exports`; at most [`MODULE_EXPORT_MAX`].
    pub export_count: u32,
    /// Arena region index to load into.
    pub region: u32,
    /// [`MODULE_LOAD_REPLACE`] or zero.
    pub flags: u32,
    /// Maximum accepted image bytes; zero means the region size.
    pub max_size: u32,
    /// Reserved; zero.
    pub reserved: u32,
}

impl ModuleLoad {
    /// An empty request.
    pub const EMPTY: Self = Self {
        url: core::ptr::null(),
        url_len: 0,
        sha256: [0; 32],
        name: core::ptr::null(),
        name_len: 0,
        version: core::ptr::null(),
        version_len: 0,
        exports: core::ptr::null(),
        export_count: 0,
        region: 0,
        flags: 0,
        max_size: 0,
        reserved: 0,
    };
}

/// Metadata for a loaded handle, returned by [`Services::module_query`].
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ModuleInfo {
    /// The queried handle.
    pub handle: ModuleHandle,
    /// Address of the first byte of the loaded image.
    pub base: u32,
    /// Bytes written into the region: the verified file length, including any
    /// manifest and footer.
    pub image_size: u32,
    /// Arena region index.
    pub region: u32,
    /// Number of exports registered at load.
    pub export_count: u32,
    /// [`MODULE_STATE_LOADED`] or [`MODULE_STATE_ENTERED`].
    pub state: u32,
    /// Reserved; zero.
    pub reserved: u32,
}

impl ModuleInfo {
    /// An empty query result.
    pub const EMPTY: Self = Self {
        handle: 0,
        base: 0,
        image_size: 0,
        region: 0,
        export_count: 0,
        state: 0,
        reserved: 0,
    };
}

/// A module entry point reached through [`Services::module_execute`].
///
/// Both arguments are passed to the callee; a legacy
/// `_start(const Services *)` image ignores the second argument.  The return
/// value is a [`ModuleVtable`] inside the image, or null.  The kernel performs
/// the data-cache writeback and instruction-cache invalidate before the call.
pub type ModuleEntry =
    unsafe extern "C" fn(*const Services, *const ModuleContext) -> *const ModuleVtable;

/// Service table passed to a module entry point.
#[repr(C)]
pub struct Services {
    pub version: u32,
    /// Write bytes to the serial and telnet consoles.
    pub write: extern "C" fn(*const u8, usize),
    /// Milliseconds since boot (monotonic).
    pub clock_ms: extern "C" fn() -> u64,
    /// Allocate `size` bytes aligned to `align`.  The kernel heap never frees.
    pub alloc: unsafe extern "C" fn(usize, usize) -> *mut u8,
    /// Drive the shared network stack once.
    pub poll: extern "C" fn(),
    /// Listen on `port` in slot `slot` (0..[`Services::tcp_slots`]).  Returns
    /// 0 or -1.  Each slot owns one socket and its own bounded RX/TX rings; a
    /// slot that already owns a socket is aborted first.
    pub tcp_listen: unsafe extern "C" fn(u32, u16) -> i32,
    /// 0 closed, 1 listening, 2 established, 3 closing.
    pub tcp_state: extern "C" fn(u32) -> u32,
    /// Receive into `ptr`; returns bytes, 0 for none, -1 for no socket.
    pub tcp_recv: unsafe extern "C" fn(u32, *mut u8, usize) -> isize,
    /// Send from `ptr`; returns bytes accepted, -1 for no socket.
    pub tcp_send: unsafe extern "C" fn(u32, *const u8, usize) -> isize,
    /// Close the connection (send FIN).
    pub tcp_close: extern "C" fn(u32),
    /// Abort the connection (send RST).
    pub tcp_abort: extern "C" fn(u32),
    /// Copy serial output from absolute `offset`; returns bytes copied.
    pub console_read: unsafe extern "C" fn(u64, *mut u8, usize) -> isize,
    /// Total bytes ever written to the serial console.
    pub console_total: extern "C" fn() -> u64,
    /// Restart from the PROM entry point.  Does not return.
    pub reset: extern "C" fn(),
    /// Halt the machine.  Does not return.
    pub poweroff: extern "C" fn(),
    /// Format the machine status block; returns bytes written.
    pub status: unsafe extern "C" fn(*mut u8, usize) -> isize,
    /// Read a PROM environment value by name; returns bytes or -1.
    pub config_get: unsafe extern "C" fn(*const u8, usize, *mut u8, usize) -> isize,
    /// Format an information block: 0 status, 1 net, 2 mem, 3 time.
    pub info: unsafe extern "C" fn(u32, *mut u8, usize) -> isize,
    /// Copy BGR shadow pixels from `offset`; returns bytes copied.
    pub screen_read: unsafe extern "C" fn(u32, *mut u8, usize) -> isize,
    /// Read one 512-byte SCSI sector from `target` at `lba` into `buffer`.
    /// Returns 0 on success, -1 on failure.
    pub block_read: unsafe extern "C" fn(u8, u32, *mut u8) -> i32,
    /// Write one 512-byte SCSI sector to `target` at `lba` from `buffer`.
    /// Returns 0 on success, -1 on failure.  The kernel validates the
    /// buffer pointer; the target device controls the durability.
    pub block_write: unsafe extern "C" fn(u8, u32, *const u8) -> i32,
    /// Query the bounded module arena.
    pub module_arena: unsafe extern "C" fn(*mut ModuleArena) -> i32,
    /// Fetch, verify and copy one image into a region; writes the handle on
    /// success.  Returns a `MODULE_*` status.
    pub module_load: unsafe extern "C" fn(*const ModuleLoad, *mut ModuleHandle) -> i32,
    /// Enter a loaded image at `entry_offset` with an optional manager context.
    ///
    /// `entry_offset` is image-relative and must be
    /// [`MODULE_OFFSET_ALIGN`]-aligned.  `out_status` receives the vtable
    /// status when the entry ran; it may be null.  Returns a `MODULE_*` status,
    /// not the module's own status.
    pub module_execute:
        unsafe extern "C" fn(ModuleHandle, u32, *const ModuleContext, *mut i32) -> i32,
    /// Find a loaded module by exact name and version.
    pub module_find: unsafe extern "C" fn(*const u8, u32, *const u8, u32, *mut ModuleHandle) -> i32,
    /// Find an export by exact interface and version.
    pub module_find_export:
        unsafe extern "C" fn(*const u8, u32, u16, *mut ModuleExportBinding) -> i32,
    /// Query the region, base, size and state of a loaded handle.
    pub module_query: unsafe extern "C" fn(ModuleHandle, *mut ModuleInfo) -> i32,
    /// Unload a handle and release its region.  The manager must ensure that
    /// no dependent still holds the handle or an export table.
    pub module_unload: unsafe extern "C" fn(ModuleHandle) -> i32,
    /// Connect slot `slot` to IPv4 `address` (network byte order) and `port`.
    ///
    /// The slot is re-created with the same bounded RX/TX rings as
    /// `tcp_listen`; a previous socket in the slot is aborted.  The kernel
    /// selects a distinct ephemeral local port per slot, so several slots can
    /// be connected at once.  Returns 0 when the SYN was queued, -1 on an
    /// invalid slot/address or no socket.  The module polls `tcp_state` for `2`
    /// (established) and applies its own connect deadline.
    pub tcp_connect: unsafe extern "C" fn(u32, u32, u16) -> i32,
    /// Level-triggered readiness for module TCP slot `slot`.
    ///
    /// Bits: 0 RX, 1 TX (a previous send was short and space is available),
    /// 2 CLOSED (reported once per connection), 3 LISTEN, 4 ESTABLISHED,
    /// 31 no socket.  The kernel recomputes this from socket state; it is not
    /// an edge queue and a lost notification cannot lose work.
    pub tcp_events: extern "C" fn(u32) -> u32,
    /// Number of module TCP slots the kernel provides.
    ///
    /// Slots are `0..count`; every slot can listen or connect, and one slot
    /// owns at most one socket.  The count is a kernel policy value, not a
    /// compile-time constant in the module: a module that needs several peer
    /// links asks first and degrades or refuses rather than assuming two.
    pub tcp_slots: extern "C" fn() -> u32,
    // -----------------------------------------------------------------------
    // ABI 16: QUIC v1 transport services
    // -----------------------------------------------------------------------
    /// Open a UDP socket bound to local `port` in `slot` (0..`udp_slots()`).
    /// Returns a socket handle (opaque nonzero) or -1.  A slot owns at most one
    /// socket; opening in a slot that already has one replaces it.
    pub udp_open: unsafe extern "C" fn(u32, u16) -> i32,
    /// Send `len` bytes from `buf` to the connected peer of socket `slot`.
    /// Returns bytes accepted (may be less than `len` when the send buffer is
    /// full; the caller retries the remainder), or -1 on no socket / error.
    pub udp_send: unsafe extern "C" fn(u32, *const u8, usize) -> isize,
    /// Receive into `ptr`; returns bytes copied, 0 when none is pending,
    /// -1 when `slot` has no socket.  Non-blocking: returns immediately.
    pub udp_recv: unsafe extern "C" fn(u32, *mut u8, usize) -> isize,
    /// Close UDP socket `slot` and release its buffer.
    pub udp_close: extern "C" fn(u32),
    /// Number of module UDP slots the kernel provides.
    pub udp_slots: extern "C" fn() -> u32,
    /// Arm a one-shot deadline.  `slot` identifies the deadline among those
    /// owned by this module region; `deadline_ms` is the absolute monotonic
    /// millisecond at which it fires.  Returns 0 on success, -1 on no slot.
    pub timer_arm: unsafe extern "C" fn(u32, u64) -> i32,
    /// Disarm deadline `slot`.  Returns 0 on success, -1 when disarmed or
    /// already fired.
    pub timer_disarm: extern "C" fn(u32) -> i32,
    /// Poll deadline `slot`.  Returns 1 when the deadline has fired since the
    /// last poll (auto-disarmed), 0 when not yet fired, -1 when `slot` has no
    /// armed deadline.
    pub timer_poll: extern "C" fn(u32) -> i32,
    /// Fill `buf` with `len` bytes from the kernel CSPRNG.  Returns bytes
    /// written (always `len` on success) or -1 on failure.
    pub csprng_fill: unsafe extern "C" fn(*mut u8, usize) -> isize,
}

/// Timeout class for `timer_arm`: relative milliseconds from `clock_ms()`.
pub const TIMER_RELATIVE: u32 = 0;

/// What a module returns from its entry point.
///
/// A null pointer means "success, no callback".  Otherwise the pointer must
/// point inside the module image and stay valid while the module is loaded.
#[repr(C)]
pub struct ModuleVtable {
    /// Result of the module's one-shot initialisation; 0 is success.
    pub status: i32,
    /// Called from the kernel's idle loop at most every 10 ms.  Return 0 to
    /// keep running, non-zero to unregister.  A manager-aware module that
    /// needs its context on ticks must retain the pointer from its entry.
    pub tick: Option<unsafe extern "C" fn(*const Services) -> i32>,
}

#[cfg(target_pointer_width = "32")]
const _: () = {
    use core::mem::{offset_of, size_of};
    assert!(size_of::<Services>() == 0xa0);
    assert!(offset_of!(Services, version) == 0x00);
    assert!(offset_of!(Services, write) == 0x04);
    assert!(offset_of!(Services, clock_ms) == 0x08);
    assert!(offset_of!(Services, alloc) == 0x0c);
    assert!(offset_of!(Services, poll) == 0x10);
    assert!(offset_of!(Services, tcp_listen) == 0x14);
    assert!(offset_of!(Services, tcp_state) == 0x18);
    assert!(offset_of!(Services, tcp_recv) == 0x1c);
    assert!(offset_of!(Services, tcp_send) == 0x20);
    assert!(offset_of!(Services, tcp_close) == 0x24);
    assert!(offset_of!(Services, tcp_abort) == 0x28);
    assert!(offset_of!(Services, console_read) == 0x2c);
    assert!(offset_of!(Services, console_total) == 0x30);
    assert!(offset_of!(Services, reset) == 0x34);
    assert!(offset_of!(Services, poweroff) == 0x38);
    assert!(offset_of!(Services, status) == 0x3c);
    assert!(offset_of!(Services, config_get) == 0x40);
    assert!(offset_of!(Services, info) == 0x44);
    assert!(offset_of!(Services, screen_read) == 0x48);
    assert!(offset_of!(Services, block_read) == 0x4c);
    assert!(offset_of!(Services, block_write) == 0x50);
    assert!(offset_of!(Services, module_arena) == 0x54);
    assert!(offset_of!(Services, module_load) == 0x58);
    assert!(offset_of!(Services, module_execute) == 0x5c);
    assert!(offset_of!(Services, module_find) == 0x60);
    assert!(offset_of!(Services, module_find_export) == 0x64);
    assert!(offset_of!(Services, module_query) == 0x68);
    assert!(offset_of!(Services, module_unload) == 0x6c);
    assert!(offset_of!(Services, tcp_connect) == 0x70);
    assert!(offset_of!(Services, tcp_events) == 0x74);
    assert!(offset_of!(Services, tcp_slots) == 0x78);
    assert!(offset_of!(Services, udp_open) == 0x7c);
    assert!(offset_of!(Services, udp_send) == 0x80);
    assert!(offset_of!(Services, udp_recv) == 0x84);
    assert!(offset_of!(Services, udp_close) == 0x88);
    assert!(offset_of!(Services, udp_slots) == 0x8c);
    assert!(offset_of!(Services, timer_arm) == 0x90);
    assert!(offset_of!(Services, timer_disarm) == 0x94);
    assert!(offset_of!(Services, timer_poll) == 0x98);
    assert!(offset_of!(Services, csprng_fill) == 0x9c);
    assert!(size_of::<ModuleArena>() == 0x20);
    assert!(offset_of!(ModuleArena, base) == 0x00);
    assert!(offset_of!(ModuleArena, length) == 0x04);
    assert!(offset_of!(ModuleArena, region_size) == 0x08);
    assert!(offset_of!(ModuleArena, region_count) == 0x0c);
    assert!(offset_of!(ModuleArena, used_lo) == 0x10);
    assert!(offset_of!(ModuleArena, used_hi) == 0x14);
    assert!(offset_of!(ModuleArena, flags) == 0x18);
    assert!(offset_of!(ModuleArena, reserved) == 0x1c);
    assert!(size_of::<ModuleContext>() == 0x10);
    assert!(offset_of!(ModuleContext, version) == 0x00);
    assert!(offset_of!(ModuleContext, flags) == 0x04);
    assert!(offset_of!(ModuleContext, manager) == 0x08);
    assert!(offset_of!(ModuleContext, reserved) == 0x0c);
    assert!(size_of::<ModuleExport>() == 0x10);
    assert!(offset_of!(ModuleExport, interface) == 0x00);
    assert!(offset_of!(ModuleExport, interface_len) == 0x04);
    assert!(offset_of!(ModuleExport, version) == 0x08);
    assert!(offset_of!(ModuleExport, reserved) == 0x0a);
    assert!(offset_of!(ModuleExport, offset) == 0x0c);
    assert!(size_of::<ModuleExportBinding>() == 0x10);
    assert!(offset_of!(ModuleExportBinding, handle) == 0x00);
    assert!(offset_of!(ModuleExportBinding, table) == 0x08);
    assert!(offset_of!(ModuleExportBinding, version) == 0x0c);
    assert!(offset_of!(ModuleExportBinding, reserved) == 0x0e);
    assert!(size_of::<ModuleLoad>() == 0x50);
    assert!(offset_of!(ModuleLoad, url) == 0x00);
    assert!(offset_of!(ModuleLoad, url_len) == 0x04);
    assert!(offset_of!(ModuleLoad, sha256) == 0x08);
    assert!(offset_of!(ModuleLoad, name) == 0x28);
    assert!(offset_of!(ModuleLoad, name_len) == 0x2c);
    assert!(offset_of!(ModuleLoad, version) == 0x30);
    assert!(offset_of!(ModuleLoad, version_len) == 0x34);
    assert!(offset_of!(ModuleLoad, exports) == 0x38);
    assert!(offset_of!(ModuleLoad, export_count) == 0x3c);
    assert!(offset_of!(ModuleLoad, region) == 0x40);
    assert!(offset_of!(ModuleLoad, flags) == 0x44);
    assert!(offset_of!(ModuleLoad, max_size) == 0x48);
    assert!(offset_of!(ModuleLoad, reserved) == 0x4c);
    assert!(size_of::<ModuleInfo>() == 0x20);
    assert!(offset_of!(ModuleInfo, handle) == 0x00);
    assert!(offset_of!(ModuleInfo, base) == 0x08);
    assert!(offset_of!(ModuleInfo, image_size) == 0x0c);
    assert!(offset_of!(ModuleInfo, region) == 0x10);
    assert!(offset_of!(ModuleInfo, export_count) == 0x14);
    assert!(offset_of!(ModuleInfo, state) == 0x18);
    assert!(offset_of!(ModuleInfo, reserved) == 0x1c);
    assert!(size_of::<Services>() == 0xa0);
    assert!(offset_of!(Services, module_unload) == 0x6c);
    assert!(offset_of!(Services, tcp_connect) == 0x70);
    assert!(offset_of!(Services, tcp_events) == 0x74);
    assert!(offset_of!(Services, tcp_slots) == 0x78);
    assert!(size_of::<ModuleVtable>() == 8);
    assert!(offset_of!(ModuleVtable, status) == 0);
    assert!(offset_of!(ModuleVtable, tick) == 4);
};

#[cfg(test)]
mod tests {
    use super::*;
    use core::mem::{offset_of, size_of};

    #[test]
    fn abi_is_v16() {
        assert_eq!(ABI_VERSION, 16);
    }

    #[test]
    fn arena_is_pointer_free_and_stable() {
        assert_eq!(size_of::<ModuleArena>(), 0x20);
        assert_eq!(offset_of!(ModuleArena, base), 0x00);
        assert_eq!(offset_of!(ModuleArena, length), 0x04);
        assert_eq!(offset_of!(ModuleArena, region_size), 0x08);
        assert_eq!(offset_of!(ModuleArena, region_count), 0x0c);
        assert_eq!(offset_of!(ModuleArena, used_lo), 0x10);
        assert_eq!(offset_of!(ModuleArena, used_hi), 0x14);
        assert_eq!(offset_of!(ModuleArena, flags), 0x18);
        assert_eq!(offset_of!(ModuleArena, reserved), 0x1c);
    }

    #[test]
    fn info_is_pointer_free_and_stable() {
        assert_eq!(size_of::<ModuleInfo>(), 0x20);
        assert_eq!(offset_of!(ModuleInfo, handle), 0x00);
        assert_eq!(offset_of!(ModuleInfo, base), 0x08);
        assert_eq!(offset_of!(ModuleInfo, image_size), 0x0c);
        assert_eq!(offset_of!(ModuleInfo, region), 0x10);
        assert_eq!(offset_of!(ModuleInfo, export_count), 0x14);
        assert_eq!(offset_of!(ModuleInfo, state), 0x18);
        assert_eq!(offset_of!(ModuleInfo, reserved), 0x1c);
    }

    #[test]
    fn loader_statuses_are_distinct_and_negative() {
        let statuses = [
            MODULE_ERR_ARGUMENT,
            MODULE_ERR_REGION,
            MODULE_ERR_BUSY,
            MODULE_ERR_DIGEST,
            MODULE_ERR_SIZE,
            MODULE_ERR_TRANSPORT,
            MODULE_ERR_NOT_FOUND,
            MODULE_ERR_LIMIT,
            MODULE_ERR_STATE,
        ];
        assert_eq!(MODULE_OK, 0);
        for (index, status) in statuses.iter().enumerate() {
            assert!(*status < 0, "status {index} is not negative");
            for other in &statuses[index + 1..] {
                assert_ne!(status, other);
            }
        }
    }

    #[test]
    fn limits_are_nonzero_and_bounded() {
        const {
            assert!(MODULE_NAME_MAX > 0 && MODULE_NAME_MAX < MODULE_URL_MAX);
        }
        const {
            assert!(MODULE_VERSION_MAX > 0);
        }
        const {
            assert!(MODULE_INTERFACE_MAX > 0);
        }
        const {
            assert!(MODULE_EXPORT_MAX > 0);
        }
        const {
            assert!(MODULE_REGION_MAX == 64);
        }
        const {
            assert!(MODULE_OFFSET_ALIGN.is_power_of_two());
        }
    }

    #[test]
    fn entry_offsets_use_the_expected_alignment() {
        assert_eq!(MODULE_ENTRY_LEGACY, 0);
        assert_eq!(MODULE_ENTRY_LEGACY % MODULE_OFFSET_ALIGN, 0);
    }

    #[test]
    fn empty_values_are_zeroed() {
        assert_eq!(ModuleArena::EMPTY.base, 0);
        assert_eq!(ModuleInfo::EMPTY.handle, 0);
        assert_eq!(ModuleContext::EMPTY.version, MODULE_CONTEXT_VERSION);
        assert_eq!(ModuleLoad::EMPTY.url.is_null(), true);
        assert_eq!(ModuleLoad::EMPTY.sha256, [0; 32]);
        assert_eq!(ModuleExport::EMPTY.offset, 0);
        assert_eq!(ModuleExportBinding::EMPTY.handle, 0);
    }

    #[test]
    fn quic_services_are_appended() {
        assert_eq!(offset_of!(Services, udp_open), 0x7c);
        assert_eq!(offset_of!(Services, udp_send), 0x80);
        assert_eq!(offset_of!(Services, udp_recv), 0x84);
        assert_eq!(offset_of!(Services, udp_close), 0x88);
        assert_eq!(offset_of!(Services, udp_slots), 0x8c);
        assert_eq!(offset_of!(Services, timer_arm), 0x90);
        assert_eq!(offset_of!(Services, timer_disarm), 0x94);
        assert_eq!(offset_of!(Services, timer_poll), 0x98);
        assert_eq!(offset_of!(Services, csprng_fill), 0x9c);
        assert_eq!(size_of::<Services>(), 0x98);
    }
}
