// SPDX-License-Identifier: GPL-3.0-only
//! MCP control-plane module (HTTP transport).
//!
//! Listens on guest port 80 through the version 3 TCP services.  `GET /`
//! returns a small status page; `POST /mcp` speaks JSON-RPC 2.0 with the MCP
//! methods `initialize`, `tools/list` and `tools/call` (`status`, `echo`,
//! `console`, `block.read`, `fs.read`, `fs.list`, `fs.stat`, `fs.readlink`,
//! `fs.write`, `fs.remove`).  This is the first long-running module and the
//! prototype for the control plane: the compressed PROM image does not grow.
//!
//! M2-1 adds an interim mesh listener on guest port 7000 in the same module.
//! When NVRAM carries `mesh.peer`, the mesh owns module TCP slot 1 and HTTP is
//! confined to slot 0; without it the module keeps the original two-slot HTTP
//! server.  M2-3 adds `GET /mesh/homes`, a bounded rendezvous-home evidence
//! surface computed from the confirmed membership snapshot.  See `mesh.rs` for
//! the transport state machine and the listen-only deviation from M2-ADR-0002.

#![no_std]
#![no_main]

mod mesh;
mod netprobe;

extern crate alloc;

use core::alloc::{GlobalAlloc, Layout};
use core::ptr::{self, null_mut};
use core::sync::atomic::{AtomicPtr, Ordering};

use alloc::string::String;
use block_cache::Lru;
use module_abi::{ModuleVtable, Services};
use overlay::{
    DirEntry as OverlayDirEntry, EntryKind as OverlayKind, Lower as OverlayLower, Overlay,
};

/// Listening phase.
const ARM: u32 = 0;
const WAIT_REQUEST: u32 = 1;
const WAIT_CLOSE: u32 = 2;
const CLOSING: u32 = 3;
const SEND_SCREEN: u32 = 4;

const SCREEN_W: u32 = 1280;
const SCREEN_H: u32 = 1024;
const SCREEN_ROW_BYTES: usize = SCREEN_W as usize * 3;
const SCREEN_DATA: u32 = SCREEN_W * SCREEN_H * 3;

const PORT: u16 = 80;
const REQUEST_CAP: usize = 2048;

static VTABLE: ModuleVtable = ModuleVtable {
    status: 0,
    tick: Some(tick),
};
static ERROR_VTABLE: ModuleVtable = ModuleVtable {
    status: 1,
    tick: None,
};

/// The service table, published before the first filesystem allocation.
static SERVICES: AtomicPtr<Services> = AtomicPtr::new(ptr::null_mut());

/// An allocator over the kernel heap service; the heap never frees.
struct ModuleAllocator;

unsafe impl GlobalAlloc for ModuleAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let table = SERVICES.load(Ordering::Acquire);
        if table.is_null() {
            return null_mut();
        }
        // SAFETY: the table came from `_start` and stays valid for the module.
        let allocate = unsafe { (*table).alloc };
        unsafe { allocate(layout.size(), layout.align()) }
    }

    unsafe fn dealloc(&self, _ptr: *mut u8, _layout: Layout) {
        // The kernel heap is a bump allocator; memory is reclaimed by reboot.
    }
}

#[global_allocator]
static ALLOCATOR: ModuleAllocator = ModuleAllocator;

/// The EROFS fixture on SCSI target 2, LBA 0.
const FS_TARGET: u8 = 2;
const FS_BLOCK: usize = 4096;
/// The module-side read-ahead cache: 64 x 4 KiB = 256 KiB.
const FS_CACHE_BLOCKS: usize = 64;
const _: () = assert!(FS_CACHE_BLOCKS * FS_BLOCK == 256 * 1024);
const FS_PRINT_MAX: usize = 8192;

/// The longest symbolic link target a `fs.stat` or `fs.readlink` response
/// carries.  The response buffer is shared with every other tool, so a link
/// target is never copied in unbounded.
const LINK_MAX: usize = 512;

struct ScsiDisk {
    services: &'static Services,
    target: u8,
}

impl block_cache::ReadAt for ScsiDisk {
    type Error = erofs::Error;

    fn read_at(&mut self, offset: u64, buf: &mut [u8]) -> Result<(), Self::Error> {
        let mut done = 0usize;
        while done < buf.len() {
            let position = offset.checked_add(done as u64).ok_or(erofs::Error::Io)?;
            let lba = u32::try_from(position / 512).map_err(|_| erofs::Error::Io)?;
            let within = (position % 512) as usize;
            let mut sector = [0u8; 512];
            let result =
                unsafe { (self.services.block_read)(self.target, lba, sector.as_mut_ptr()) };
            if result != 0 {
                return Err(erofs::Error::Io);
            }
            let count = (512 - within).min(buf.len() - done);
            buf[done..done + count].copy_from_slice(&sector[within..within + count]);
            done += count;
        }
        Ok(())
    }
}

struct CachedDisk {
    cache: Lru<ScsiDisk, FS_CACHE_BLOCKS>,
}

impl erofs::BlockDevice for CachedDisk {
    fn read_at(&mut self, offset: u64, buf: &mut [u8]) -> erofs::Result<()> {
        self.cache
            .read_at(offset, buf)
            .map_err(|_| erofs::Error::Io)
    }
}

/// The read-only EROFS lower owned by the RAM overlay.
struct LowerView {
    fs: erofs::Erofs<CachedDisk>,
}

/// Inode metadata read from the lower without following a final symlink.
struct LowerMeta {
    file_type: erofs::FileType,
    size: u64,
    mode: u16,
    uid: u32,
    gid: u32,
}

impl LowerView {
    /// Look up `path` in the EROFS image without following a final symlink.
    ///
    /// The overlay's own lookup follows links and models only files and
    /// directories; `fs.stat` and `fs.readlink` need the node the path names.
    fn metadata(&mut self, path: &str) -> Option<LowerMeta> {
        let inode = self.fs.lookup_path(path).ok()?;
        Some(LowerMeta {
            file_type: inode.file_type,
            size: inode.size,
            mode: inode.mode,
            uid: inode.uid,
            gid: inode.gid,
        })
    }

    /// Read a symbolic link's target.  The caller has already established that
    /// `path` names a link.
    fn link_target(&mut self, path: &str) -> Option<alloc::vec::Vec<u8>> {
        let inode = self.fs.lookup_path(path).ok()?;
        self.fs.read_link(&inode).ok()
    }
}

impl OverlayLower for LowerView {
    fn read_file(&mut self, path: &str) -> overlay::Result<Option<alloc::vec::Vec<u8>>> {
        match self.fs.lookup_path_follow(path) {
            Ok(inode) if !inode.is_dir() => self
                .fs
                .read_file(&inode)
                .map(Some)
                .map_err(|_| overlay::Error::Lower("erofs read")),
            Ok(_) => Ok(None),
            Err(erofs::Error::NotFound | erofs::Error::NotDirectory) => Ok(None),
            Err(_) => Err(overlay::Error::Lower("erofs lookup")),
        }
    }

    fn read_dir(
        &mut self,
        path: &str,
    ) -> overlay::Result<Option<alloc::vec::Vec<OverlayDirEntry>>> {
        let inode = match self.fs.lookup_path_follow(path) {
            Ok(inode) if inode.is_dir() => inode,
            Ok(_) => return Ok(None),
            Err(erofs::Error::NotFound | erofs::Error::NotDirectory) => return Ok(None),
            Err(_) => return Err(overlay::Error::Lower("erofs lookup")),
        };
        let entries = self
            .fs
            .read_dir(&inode)
            .map_err(|_| overlay::Error::Lower("erofs readdir"))?;
        Ok(Some(
            entries
                .into_iter()
                .map(|entry| OverlayDirEntry {
                    name: String::from_utf8_lossy(&entry.name).into_owned(),
                    kind: if entry.file_type == erofs::FileType::Directory {
                        OverlayKind::Directory
                    } else {
                        OverlayKind::File
                    },
                })
                .collect(),
        ))
    }
}

/// Mounted lazily on the first `fs.*` tool call.  The server is single-threaded.
static mut OVERLAY: Option<Overlay<LowerView>> = None;

#[allow(static_mut_refs)]
fn ensure_fs() -> bool {
    unsafe {
        if OVERLAY.is_none() {
            let services = SERVICES.load(Ordering::Acquire);
            let Some(services) = services.as_ref() else {
                return false;
            };
            let Some(cache) = Lru::new(
                ScsiDisk {
                    services,
                    target: FS_TARGET,
                },
                FS_BLOCK,
            )
            .ok() else {
                return false;
            };
            if let Ok(fs) = erofs::Erofs::mount(CachedDisk { cache }) {
                let mut overlay = Overlay::new(LowerView { fs });
                // The writable roots promised by the M4 milestone.  `/etc`
                // already exists in the EROFS lower; creating it here makes the
                // upper namespace explicit and lets later writes descend into
                // it.  `/home` is normally absent from the read-only image.
                for path in ["/tmp", "/etc", "/home"] {
                    let _ = overlay.create_dir(path);
                }
                OVERLAY = Some(overlay);
            }
        }
        OVERLAY.is_some()
    }
}

#[allow(static_mut_refs)]
fn fs_read(path: &str) -> Option<alloc::vec::Vec<u8>> {
    unsafe { OVERLAY.as_mut()?.read_file(path).ok() }
}

#[allow(static_mut_refs)]
fn fs_list(path: &str) -> Option<String> {
    unsafe {
        let entries = OVERLAY.as_mut()?.read_dir(path).ok()?;
        let mut out = String::new();
        for entry in entries {
            out.push_str(&entry.name);
            out.push('\n');
        }
        Some(out)
    }
}

#[allow(static_mut_refs)]
fn fs_write(path: &str, data: &[u8]) -> bool {
    unsafe {
        OVERLAY
            .as_mut()
            .map(|overlay| overlay.write_file(path, data).is_ok())
            .unwrap_or(false)
    }
}

#[allow(static_mut_refs)]
fn fs_remove(path: &str) -> bool {
    unsafe {
        OVERLAY
            .as_mut()
            .map(|overlay| overlay.remove_file(path).is_ok())
            .unwrap_or(false)
    }
}

/// What a path names in the merged namespace.
#[derive(Clone, Copy, PartialEq, Eq)]
enum FsKind {
    File,
    Dir,
    Symlink,
}

impl FsKind {
    /// The `type` field of a `fs.stat` result.
    fn name(self) -> &'static [u8] {
        match self {
            Self::File => b"file",
            Self::Dir => b"dir",
            Self::Symlink => b"symlink",
        }
    }
}

/// Why a `fs.stat` or `fs.readlink` request failed.
enum FsError {
    /// The overlay could not be mounted.
    Mount,
    /// The path is not a normalisable absolute path.
    Invalid,
    /// The path is absent from the merged namespace.
    NotFound,
    /// The path is not a symbolic link.
    NotSymlink,
    /// The lower filesystem could not answer.
    Lower,
    /// The link target does not fit the response budget.
    TooLong,
}

impl FsError {
    /// The MCP error message for this failure.
    fn message(&self) -> &'static [u8] {
        match self {
            Self::Mount => b"fs mount failed",
            Self::Invalid => b"fs path is not a valid absolute path",
            Self::NotFound => b"fs path not found",
            Self::NotSymlink => b"fs path is not a symlink",
            Self::Lower => b"fs lower failed",
            Self::TooLong => b"fs link target too long",
        }
    }
}

/// A node's metadata in the merged namespace.
struct FsStat {
    kind: FsKind,
    /// Whether the node lives in the RAM upper rather than the EROFS lower.
    upper: bool,
    size: u64,
    mode: u16,
    uid: u32,
    gid: u32,
    /// A symlink target, truncated to [`LINK_MAX`] bytes.
    target: Option<alloc::vec::Vec<u8>>,
    /// Whether `target` is a prefix of a longer link target.
    target_truncated: bool,
}

/// Stat a path in the merged namespace, without following a final symlink.
#[allow(static_mut_refs)]
fn fs_stat(path: &str) -> Result<FsStat, FsError> {
    unsafe {
        let overlay = OVERLAY.as_mut().ok_or(FsError::Mount)?;
        // Normalise once so the lower sees the same path the overlay merged.
        let path = overlay::normalise(path).map_err(|_| FsError::Invalid)?;
        let merged = overlay.stat(&path).map_err(|error| match error {
            overlay::Error::Lower(_) => FsError::Lower,
            _ => FsError::NotFound,
        })?;
        if merged.upper {
            // The RAM upper stores contents, not a POSIX inode: the size is the
            // stored length and the mode is the one the overlay creates.
            let (kind, size, mode) = match merged.kind {
                OverlayKind::File => (
                    FsKind::File,
                    overlay
                        .read_file(&path)
                        .map(|data| data.len() as u64)
                        .unwrap_or(0),
                    0o100_644,
                ),
                OverlayKind::Directory => (FsKind::Dir, 0, 0o40_755),
            };
            return Ok(FsStat {
                kind,
                upper: true,
                size,
                mode,
                uid: 0,
                gid: 0,
                target: None,
                target_truncated: false,
            });
        }

        let Some(meta) = overlay.lower_mut().metadata(&path) else {
            return Err(FsError::NotFound);
        };
        let kind = match meta.file_type {
            erofs::FileType::Directory => FsKind::Dir,
            erofs::FileType::Symlink => FsKind::Symlink,
            _ => FsKind::File,
        };
        let mut target = None;
        let mut target_truncated = false;
        if kind == FsKind::Symlink {
            if let Some(bytes) = overlay.lower_mut().link_target(&path) {
                target_truncated = bytes.len() > LINK_MAX;
                target = Some(bytes[..bytes.len().min(LINK_MAX)].to_vec());
            }
        }
        Ok(FsStat {
            kind,
            upper: false,
            size: meta.size,
            mode: meta.mode,
            uid: meta.uid,
            gid: meta.gid,
            target,
            target_truncated,
        })
    }
}

/// Read a symbolic link's target from the merged namespace.
#[allow(static_mut_refs)]
fn fs_readlink(path: &str) -> Result<alloc::vec::Vec<u8>, FsError> {
    unsafe {
        let overlay = OVERLAY.as_mut().ok_or(FsError::Mount)?;
        let path = overlay::normalise(path).map_err(|_| FsError::Invalid)?;
        let merged = overlay.stat(&path).map_err(|error| match error {
            overlay::Error::Lower(_) => FsError::Lower,
            _ => FsError::NotFound,
        })?;
        if merged.upper {
            return Err(FsError::NotSymlink);
        }
        let Some(meta) = overlay.lower_mut().metadata(&path) else {
            return Err(FsError::NotFound);
        };
        if meta.file_type != erofs::FileType::Symlink {
            return Err(FsError::NotSymlink);
        }
        let target = overlay
            .lower_mut()
            .link_target(&path)
            .ok_or(FsError::Lower)?;
        if target.len() > LINK_MAX {
            return Err(FsError::TooLong);
        }
        Ok(target)
    }
}

/// Render a `fs.stat` result as JSON text.
fn stat_json(stat: &FsStat, out: &mut [u8]) -> usize {
    let mut w = Out::new(out);
    w.push(b"{\"type\":");
    w.json_string(stat.kind.name());
    w.push(b",\"size\":");
    w.u64(stat.size);
    w.push(b",\"mode\":\"");
    w.octal(stat.mode);
    w.push(b"\",\"uid\":");
    w.u64(u64::from(stat.uid));
    w.push(b",\"gid\":");
    w.u64(u64::from(stat.gid));
    w.push(b",\"origin\":");
    w.json_string(if stat.upper { b"overlay" } else { b"erofs" });
    if let Some(target) = &stat.target {
        w.push(b",\"target\":");
        w.json_string(target);
        if stat.target_truncated {
            w.push(b",\"target_truncated\":true");
        }
    }
    w.push(b"}");
    w.len
}

static mut PHASE: u32 = ARM;
/// When the current HTTP connection entered its close handshake.  With the
/// mesh listener owning slot 1, HTTP has only one slot; a client that keeps
/// its side open must not wedge the control plane forever.
static mut CLOSE_START_MS: u64 = 0;
const CLOSE_TIMEOUT_MS: u64 = 2_000;
static mut SCREEN_ROW: u32 = 0;
static mut SCREEN_OFF: usize = 0;
static mut SCREEN_STREAMING: bool = false;
static mut SCREEN_BUF: [u8; SCREEN_ROW_BYTES] = [0; SCREEN_ROW_BYTES];

const THUMB_W: usize = 320;
const THUMB_H: usize = 200;
const THUMB_ROW: usize = THUMB_W / 8; // 40 bytes
const RAW_LEN: usize = THUMB_H * (THUMB_ROW + 1); // filter byte + row
const IDAT_LEN: usize = 2 + 5 + RAW_LEN + 4;
static mut THUMB_RAW: [u8; RAW_LEN] = [0; RAW_LEN];
static mut THUMB_IDAT: [u8; IDAT_LEN] = [0; IDAT_LEN];
static mut THUMB_PNG: [u8; 8400] = [0; 8400];
static mut THUMB_B64: [u8; 12000] = [0; 12000];
static mut REQUEST: [u8; REQUEST_CAP] = [0; REQUEST_CAP];
static mut REQ_LEN: usize = 0;
static mut IN_CONN: bool = false;
static mut RESPONSE: [u8; 16384] = [0; 16384];
/// Scratch for the `fs.stat` JSON text before it is embedded in a `text`
/// field; a link target is bounded by [`LINK_MAX`] so the buffer cannot
/// overflow.
static mut STAT_TEXT: [u8; 2048] = [0; 2048];

/// The slot serving the current connection, and the slot armed for the next
/// one.  Alternating the two hides the close handshake from a back-to-back
/// client.
static mut CURRENT: u32 = 0;
static mut NEXT: u32 = 1;
static mut NEXT_ARMED: bool = false;

/// Deferred reset/poweroff: 0 none, 1 reset, 2 poweroff.  The response is
/// queued first; the action fires after a few idle ticks.
static mut ACTION: u32 = 0;
static mut COUNTDOWN: u32 = 0;
/// Diagnostic: log the first HTTP listen failure of a boot.
static mut LISTEN_FAIL_LOGGED: bool = false;

/// Bearer token from `mcp.token`; an empty token leaves the endpoint open.
static mut TOKEN: [u8; 64] = [0; 64];
static mut TOKEN_LEN: usize = 0;

/// Module entry.
///
/// # Safety
/// `services` must point to a valid service table supplied by the kernel.
#[no_mangle]
pub unsafe extern "C" fn _start(services: *const Services) -> *const ModuleVtable {
    let Some(services) = (unsafe { services.as_ref() }) else {
        return &ERROR_VTABLE;
    };
    if services.version < 10 {
        return &ERROR_VTABLE;
    }
    SERVICES.store(
        services as *const Services as *mut Services,
        Ordering::Release,
    );
    // The mesh listener is an M2-1 extension inside this module.  It is off
    // unless NVRAM carries `mesh.peer`; a normal boot keeps the original
    // two-slot HTTP server behaviour.  When it is on, slot 1 belongs to the
    // mesh listener and HTTP is confined to slot 0.
    let mesh_enabled = unsafe { mesh::init(services) };
    // The UDP probe is a transport gate (M3-ADR-0006).  It is inert unless
    // `netprobe.peer` is configured, and it owns a UDP slot rather than a TCP
    // one, so it cannot collide with the HTTP server or the mesh listener.
    unsafe { netprobe::init(services) };
    if mesh_enabled {
        unsafe {
            CURRENT = mesh::HTTP_SLOT;
            NEXT = 1;
            NEXT_ARMED = false;
        }
    }
    let mut token = [0u8; 64];
    let n = unsafe {
        (services.config_get)(
            b"mcp.token".as_ptr(),
            b"mcp.token".len(),
            token.as_mut_ptr(),
            token.len(),
        )
    };
    if n > 0 {
        let n = (n as usize).min(token.len());
        unsafe {
            core::ptr::copy_nonoverlapping(
                token.as_ptr(),
                core::ptr::addr_of_mut!(TOKEN) as *mut u8,
                n,
            );
            TOKEN_LEN = n;
        }
        let message = b"mcp: bearer token required\n";
        (services.write)(message.as_ptr(), message.len());
    } else {
        let message = b"mcp: no bearer token configured\n";
        (services.write)(message.as_ptr(), message.len());
    }
    let message = b"mcp: listening on port 80\n";
    (services.write)(message.as_ptr(), message.len());
    &VTABLE
}

unsafe extern "C" fn tick(services: *const Services) -> i32 {
    let Some(services) = (unsafe { services.as_ref() }) else {
        return 1;
    };

    // The mesh listener owns module TCP slot 1 when configured.  It is a
    // separate protocol and state machine from the HTTP server below.
    mesh::tick(services);

    // The UDP probe drains its socket and sends on its own period.
    netprobe::tick(services);

    // A deferred system action runs after the tool response has had a few
    // poll cycles to reach the client.
    if unsafe { ACTION } != 0 {
        let left = unsafe { COUNTDOWN };
        if left > 0 {
            unsafe {
                COUNTDOWN = left - 1;
            }
        } else {
            if unsafe { ACTION } == 1 {
                (services.reset)();
            } else {
                (services.poweroff)();
            }
        }
    }

    match unsafe { PHASE } {
        ARM => {
            unsafe {
                REQ_LEN = 0;
                IN_CONN = false;
            }
            let slot = unsafe { CURRENT };
            let code = (services.tcp_listen)(slot, PORT);
            if code == 0 {
                unsafe {
                    PHASE = WAIT_REQUEST;
                }
            } else if !unsafe { LISTEN_FAIL_LOGGED } {
                unsafe {
                    LISTEN_FAIL_LOGGED = true;
                }
                let mut line = Out::new(unsafe { &mut *core::ptr::addr_of_mut!(RESPONSE) });
                line.push(b"mcp: http listen failed slot=");
                line.u64(slot as u64);
                line.push(b" state=");
                line.u64((services.tcp_state)(slot) as u64);
                line.byte(b'\n');
                (services.write)(line.buf.as_ptr(), line.len);
            }
        }
        WAIT_REQUEST => {
            let slot = unsafe { CURRENT };
            let state = (services.tcp_state)(slot);
            // A client may half-close immediately after its request.  In that
            // case smoltcp reports the connection as closing (3) with the
            // request still in the receive buffer.  Read it before closing;
            // otherwise a fast FIN can lose a complete request.
            if state == 2 || state == 3 {
                if !unsafe { IN_CONN } {
                    unsafe {
                        REQ_LEN = 0;
                        IN_CONN = true;
                    }
                }
                let request = unsafe { &mut *core::ptr::addr_of_mut!(REQUEST) };
                let used = unsafe { REQ_LEN };
                if used < request.len() {
                    let n = (services.tcp_recv)(
                        slot,
                        request.as_mut_ptr().add(used),
                        request.len() - used,
                    );
                    if n > 0 {
                        unsafe {
                            REQ_LEN = used + n as usize;
                        }
                    } else if n < 0 && state == 2 {
                        (services.tcp_abort)(slot);
                        unsafe {
                            PHASE = ARM;
                        }
                    }
                }
                let used = unsafe { REQ_LEN };
                if used > 0 && request_complete(&request[..used]) {
                    handle_request(services, slot);
                    if unsafe { SCREEN_STREAMING } {
                        unsafe {
                            IN_CONN = false;
                            PHASE = SEND_SCREEN;
                        }
                    } else {
                        // Arm the other slot before the client closes, so the
                        // next connection lands on a listening socket.  With
                        // the mesh listener enabled slot 1 is reserved for
                        // port 7000, so HTTP re-arms slot 0 after the close
                        // handshake instead.
                        if !mesh::enabled() {
                            let next = unsafe { NEXT };
                            if (services.tcp_listen)(next, PORT) == 0 {
                                unsafe {
                                    NEXT_ARMED = true;
                                }
                            }
                        }
                        // The response is already in the TCP send buffer.
                        // Close our side now rather than waiting for a
                        // possibly half-open iris forward to send FIN; the
                        // single HTTP slot is needed by the next control
                        // request.  CLOSING drains or aborts after 2 s.
                        (services.tcp_close)(slot);
                        unsafe {
                            IN_CONN = false;
                            CLOSE_START_MS = (services.clock_ms)();
                            PHASE = CLOSING;
                        }
                    }
                } else if state == 3 {
                    (services.tcp_close)(slot);
                    unsafe {
                        CLOSE_START_MS = (services.clock_ms)();
                        PHASE = CLOSING;
                    }
                }
            } else if state == 0 {
                unsafe {
                    PHASE = ARM;
                }
            }
        }
        SEND_SCREEN => {
            let slot = unsafe { CURRENT };
            let row = unsafe { SCREEN_ROW };
            if row >= SCREEN_H {
                (services.tcp_close)(slot);
                unsafe {
                    SCREEN_STREAMING = false;
                }
                if !mesh::enabled() {
                    let next = unsafe { NEXT };
                    if (services.tcp_listen)(next, PORT) == 0 {
                        unsafe {
                            NEXT_ARMED = true;
                        }
                    }
                }
                unsafe {
                    CLOSE_START_MS = (services.clock_ms)();
                    PHASE = WAIT_CLOSE;
                }
            } else {
                let off = unsafe { SCREEN_OFF };
                if off == 0 {
                    let shadow_row = SCREEN_H - 1 - row;
                    let buf = unsafe { &mut *core::ptr::addr_of_mut!(SCREEN_BUF) };
                    let n = unsafe {
                        (services.screen_read)(
                            shadow_row * SCREEN_ROW_BYTES as u32,
                            buf.as_mut_ptr(),
                            buf.len(),
                        )
                    };
                    if n != SCREEN_ROW_BYTES as isize {
                        (services.tcp_abort)(slot);
                        unsafe {
                            SCREEN_STREAMING = false;
                            PHASE = ARM;
                        }
                    }
                }
                let off = unsafe { SCREEN_OFF };
                if off < SCREEN_ROW_BYTES {
                    let buf = unsafe { &*core::ptr::addr_of!(SCREEN_BUF) };
                    let n = unsafe {
                        (services.tcp_send)(slot, buf.as_ptr().add(off), SCREEN_ROW_BYTES - off)
                    };
                    if n > 0 {
                        let next = off + n as usize;
                        if next >= SCREEN_ROW_BYTES {
                            unsafe {
                                SCREEN_ROW += 1;
                                SCREEN_OFF = 0;
                            }
                        } else {
                            unsafe {
                                SCREEN_OFF = next;
                            }
                        }
                    } else if n < 0 {
                        (services.tcp_abort)(slot);
                        unsafe {
                            SCREEN_STREAMING = false;
                            PHASE = ARM;
                        }
                    }
                }
            }
        }
        WAIT_CLOSE => {
            let slot = unsafe { CURRENT };
            if (services.clock_ms)().saturating_sub(unsafe { CLOSE_START_MS }) >= CLOSE_TIMEOUT_MS {
                // The client is holding its side open.  The response has
                // already been written; reclaim the only HTTP slot.
                let message = b"mcp: http close timeout\n";
                (services.write)(message.as_ptr(), message.len());
                (services.tcp_abort)(slot);
                unsafe {
                    finish_close();
                }
                return 0;
            }
            match (services.tcp_state)(slot) {
                3 => {
                    (services.tcp_close)(slot);
                    unsafe {
                        PHASE = CLOSING;
                    }
                }
                0 => unsafe {
                    finish_close();
                },
                _ => {}
            }
        }
        CLOSING => {
            let slot = unsafe { CURRENT };
            if (services.clock_ms)().saturating_sub(unsafe { CLOSE_START_MS }) >= CLOSE_TIMEOUT_MS {
                let message = b"mcp: http close timeout\n";
                (services.write)(message.as_ptr(), message.len());
                (services.tcp_abort)(slot);
                unsafe {
                    finish_close();
                }
                return 0;
            }
            if (services.tcp_state)(slot) == 0 {
                (services.tcp_abort)(slot);
                unsafe {
                    finish_close();
                }
            }
        }
        _ => unsafe {
            PHASE = ARM;
        },
    }
    0
}

/// Move to the pre-armed listener, or re-arm the current slot if the second
/// listener could not be created.
unsafe fn finish_close() {
    unsafe {
        IN_CONN = false;
    }
    if unsafe { NEXT_ARMED } {
        let old = unsafe { CURRENT };
        unsafe {
            CURRENT = NEXT;
            NEXT = old;
            NEXT_ARMED = false;
            PHASE = WAIT_REQUEST;
        }
    } else {
        unsafe {
            PHASE = ARM;
        }
    }
}

// ---------------------------------------------------------------------------
// HTTP request handling
// ---------------------------------------------------------------------------

fn request_complete(req: &[u8]) -> bool {
    let Some(header_end) = find(req, b"\r\n\r\n").map(|i| i + 4) else {
        // An oversized request without a header terminator is dispatched as-is.
        return req.len() == REQUEST_CAP;
    };
    let content = content_length(&req[..header_end]);
    req.len() >= header_end + content
}

fn content_length(headers: &[u8]) -> usize {
    let mut p = 0;
    while p < headers.len() {
        let end = find(&headers[p..], b"\r\n")
            .map(|i| p + i)
            .unwrap_or(headers.len());
        let line = &headers[p..end];
        if starts_with_ci(line, b"content-length:") {
            let mut q = b"content-length:".len();
            while q < line.len() && (line[q] == b' ' || line[q] == b'\t') {
                q += 1;
            }
            let mut value = 0usize;
            while q < line.len() && line[q].is_ascii_digit() {
                value = value * 10 + (line[q] - b'0') as usize;
                q += 1;
            }
            return value;
        }
        p = end + 2;
    }
    0
}

fn handle_request(services: &Services, slot: u32) {
    let request = unsafe { &*core::ptr::addr_of!(REQUEST) };
    let used = unsafe { REQ_LEN };
    let request = &request[..used.min(REQUEST_CAP)];
    let header_end = find(request, b"\r\n\r\n")
        .map(|i| i + 4)
        .unwrap_or(request.len());
    let body = &request[header_end..];
    let (path, is_post) = request_line(request);
    if is_post && path.starts_with(b"/mcp") && !authorized(request) {
        send_unauthorized(services, slot);
        return;
    }
    if !is_post && path == b"/screen.bmp" {
        screen_start(services, slot);
        return;
    }
    if !is_post && path == b"/modules" {
        let response = unsafe { &mut *core::ptr::addr_of_mut!(RESPONSE) };
        let n = module_status(services, response);
        send_http(services, slot, &b"text/plain"[..], &response[..n]);
        return;
    }
    if !is_post && (path == b"/mesh/homes" || path.starts_with(b"/mesh/homes?")) {
        let response = unsafe { &mut *core::ptr::addr_of_mut!(RESPONSE) };
        let (status, n) = mesh::homes_response(path, response);
        send_http_status(services, slot, status, &b"text/plain"[..], &response[..n]);
        return;
    }
    if !is_post && (path == b"/mesh/shard" || path.starts_with(b"/mesh/shard?")) {
        let response = unsafe { &mut *core::ptr::addr_of_mut!(RESPONSE) };
        let (status, n) = mesh::shard_response(path, response);
        send_http_status(services, slot, status, &b"text/plain"[..], &response[..n]);
        return;
    }
    if !is_post && (path == b"/mesh/route" || path.starts_with(b"/mesh/route?")) {
        let response = unsafe { &mut *core::ptr::addr_of_mut!(RESPONSE) };
        let (status, n) = mesh::route_response(services, path, response);
        send_http_status(services, slot, status, &b"text/plain"[..], &response[..n]);
        return;
    }
    if !is_post && (path == b"/mesh/migrate" || path.starts_with(b"/mesh/migrate?")) {
        let response = unsafe { &mut *core::ptr::addr_of_mut!(RESPONSE) };
        let (status, n) = mesh::migrate_response(services, path, response);
        send_http_status(services, slot, status, &b"text/plain"[..], &response[..n]);
        return;
    }
    if !is_post && (path == b"/mesh/latency" || path.starts_with(b"/mesh/latency?")) {
        let response = unsafe { &mut *core::ptr::addr_of_mut!(RESPONSE) };
        let (status, n) = mesh::latency_response(services, path, response);
        send_http_status(services, slot, status, &b"text/plain"[..], &response[..n]);
        return;
    }
    if !is_post && (path == b"/mesh/bulk" || path.starts_with(b"/mesh/bulk?")) {
        let response = unsafe { &mut *core::ptr::addr_of_mut!(RESPONSE) };
        let (status, n) = mesh::bulk_response(services, path, response);
        send_http_status(services, slot, status, &b"text/plain"[..], &response[..n]);
        return;
    }

    let response = unsafe { &mut *core::ptr::addr_of_mut!(RESPONSE) };
    let (content_type, n) = if is_post && path.starts_with(b"/mcp") {
        (
            &b"application/json"[..],
            mcp_response(services, body, response),
        )
    } else {
        (&b"text/plain"[..], status_page(services, response))
    };
    send_http(services, slot, content_type, &response[..n]);
}

fn request_line(request: &[u8]) -> (&[u8], bool) {
    let end = find(request, b"\r\n").unwrap_or(request.len());
    let line = &request[..end];
    let is_post = starts_with_ci(line, b"POST ");
    let path = match find(line, b" ") {
        Some(i) => {
            let rest = &line[i + 1..];
            match find(rest, b" ") {
                Some(j) => &rest[..j],
                None => rest,
            }
        }
        None => b"/",
    };
    (path, is_post)
}

fn module_status(_services: &Services, out: &mut [u8]) -> usize {
    let mut w = Out::new(out);
    let status = unsafe { &*(loader_abi::STATUS_ADDR as *const loader_abi::ModuleStatus) };
    if status.magic != loader_abi::STATUS_MAGIC {
        w.push(b"runtime manager: unavailable\n");
        return w.len;
    }
    let len = (status.len as usize).min(status.text.len());
    w.push(b"runtime module graph:\n");
    w.push(&status.text[..len]);
    w.len
}

fn status_page(services: &Services, out: &mut [u8]) -> usize {
    let mut w = Out::new(out);
    w.push(b"IRIX 7 module HTTP server\nuptime: ");
    w.u64((services.clock_ms)() / 1000);
    w.push(b" s\n");
    w.len
}

fn authorized(request: &[u8]) -> bool {
    let token = unsafe { &*core::ptr::addr_of!(TOKEN) };
    let token = &token[..unsafe { TOKEN_LEN }];
    if token.is_empty() {
        return true;
    }
    let mut p = 0;
    while p < request.len() {
        let end = find(&request[p..], b"\r\n")
            .map(|i| p + i)
            .unwrap_or(request.len());
        let line = &request[p..end];
        if starts_with_ci(line, b"authorization:") {
            let mut q = b"authorization:".len();
            while q < line.len() && (line[q] == b' ' || line[q] == b'\t') {
                q += 1;
            }
            if !starts_with_ci(&line[q..], b"bearer ") {
                return false;
            }
            return &line[q + 7..] == token;
        }
        p = end + 2;
    }
    false
}

fn send_unauthorized(services: &Services, slot: u32) {
    send_all(
        services,
        slot,
        b"HTTP/1.1 401 Unauthorized\r\nWWW-Authenticate: Bearer\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
    );
}

fn thumbnail(services: &Services) -> usize {
    let raw = unsafe { &mut *core::ptr::addr_of_mut!(THUMB_RAW) };
    let stride = THUMB_ROW + 1;
    for oy in 0..THUMB_H {
        let src = unsafe { &mut *core::ptr::addr_of_mut!(SCREEN_BUF) };
        let offset = (oy as u32) * 4 * SCREEN_ROW_BYTES as u32;
        let n = unsafe { (services.screen_read)(offset, src.as_mut_ptr(), src.len()) };
        if n != SCREEN_ROW_BYTES as isize {
            return 0;
        }
        raw[oy * stride] = 0; // PNG filter: none
        for ox in 0..THUMB_W {
            let i = ox * 4 * 3;
            let b = src[i] as u32;
            let g = src[i + 1] as u32;
            let r = src[i + 2] as u32;
            if (r * 3 + g * 6 + b) / 10 > 40 {
                raw[oy * stride + 1 + ox / 8] |= 0x80 >> (ox % 8);
            }
        }
    }

    let raw = unsafe { &*core::ptr::addr_of!(THUMB_RAW) };
    let idat = unsafe { &mut *core::ptr::addr_of_mut!(THUMB_IDAT) };
    idat[0] = 0x78;
    idat[1] = 0x01;
    idat[2] = 0x01; // final stored deflate block
    let len = RAW_LEN as u16;
    idat[3..5].copy_from_slice(&len.to_le_bytes());
    idat[5..7].copy_from_slice(&(!len).to_le_bytes());
    idat[7..7 + RAW_LEN].copy_from_slice(raw);
    let adler = adler32(raw);
    idat[7 + RAW_LEN..11 + RAW_LEN].copy_from_slice(&adler.to_be_bytes());

    let mut png = Png::new(unsafe { &mut *core::ptr::addr_of_mut!(THUMB_PNG) });
    png.push(b"\x89PNG\r\n\x1a\n");
    let mut ihdr = [0u8; 13];
    ihdr[0..4].copy_from_slice(&(THUMB_W as u32).to_be_bytes());
    ihdr[4..8].copy_from_slice(&(THUMB_H as u32).to_be_bytes());
    ihdr[8] = 1; // bit depth
    ihdr[9] = 0; // greyscale
    png.chunk(b"IHDR", &ihdr);
    png.chunk(b"IDAT", &idat[..IDAT_LEN]);
    png.chunk(b"IEND", b"");
    let n = png.len;
    b64_encode(&png.buf[..n], unsafe {
        &mut *core::ptr::addr_of_mut!(THUMB_B64)
    })
}

struct Png<'a> {
    buf: &'a mut [u8],
    len: usize,
}

impl<'a> Png<'a> {
    fn new(buf: &'a mut [u8]) -> Self {
        Self { buf, len: 0 }
    }

    fn push(&mut self, data: &[u8]) {
        let n = (self.buf.len() - self.len).min(data.len());
        self.buf[self.len..self.len + n].copy_from_slice(&data[..n]);
        self.len += n;
    }

    fn u32(&mut self, value: u32) {
        self.push(&value.to_be_bytes());
    }

    fn chunk(&mut self, kind: &[u8; 4], data: &[u8]) {
        self.u32(data.len() as u32);
        self.push(kind);
        self.push(data);
        self.u32(crc32(kind, data));
    }
}

fn crc32(a: &[u8], b: &[u8]) -> u32 {
    let mut crc = 0xffff_ffffu32;
    for &byte in a.iter().chain(b) {
        crc ^= u32::from(byte);
        for _ in 0..8 {
            crc = if crc & 1 != 0 {
                (crc >> 1) ^ 0xedb8_8320
            } else {
                crc >> 1
            };
        }
    }
    !crc
}

fn adler32(data: &[u8]) -> u32 {
    let mut a = 1u32;
    let mut b = 0u32;
    for &byte in data {
        a = (a + u32::from(byte)) % 65521;
        b = (b + a) % 65521;
    }
    (b << 16) | a
}

fn b64_encode(src: &[u8], dst: &mut [u8]) -> usize {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = 0;
    for chunk in src.chunks(3) {
        if out + 4 > dst.len() {
            break;
        }
        let b0 = chunk[0];
        let b1 = *chunk.get(1).unwrap_or(&0);
        let b2 = *chunk.get(2).unwrap_or(&0);
        dst[out] = TABLE[(b0 >> 2) as usize];
        dst[out + 1] = TABLE[(((b0 & 3) << 4) | (b1 >> 4)) as usize];
        dst[out + 2] = if chunk.len() > 1 {
            TABLE[(((b1 & 0xf) << 2) | (b2 >> 6)) as usize]
        } else {
            b'='
        };
        dst[out + 3] = if chunk.len() > 2 {
            TABLE[(b2 & 0x3f) as usize]
        } else {
            b'='
        };
        out += 4;
    }
    out
}

fn screen_start(services: &Services, slot: u32) {
    let mut header = [0u8; 160];
    let mut w = Out::new(&mut header);
    w.push(b"HTTP/1.0 200 OK\r\nContent-Type: image/bmp\r\nContent-Length: ");
    w.u64(u64::from(SCREEN_DATA) + 54);
    w.push(b"\r\nConnection: close\r\n\r\n");
    let n = w.len;

    let mut bmp = [0u8; 54];
    bmp[0] = b'B';
    bmp[1] = b'M';
    le32(&mut bmp[2..6], 54 + SCREEN_DATA);
    le32(&mut bmp[10..14], 54);
    le32(&mut bmp[14..18], 40);
    le32(&mut bmp[18..22], SCREEN_W);
    le32(&mut bmp[22..26], SCREEN_H);
    le16(&mut bmp[26..28], 1);
    le16(&mut bmp[28..30], 24);
    le32(&mut bmp[34..38], SCREEN_DATA);
    le32(&mut bmp[38..42], 2835);

    send_all(services, slot, &header[..n]);
    send_all(services, slot, &bmp);
    unsafe {
        SCREEN_ROW = 0;
        SCREEN_OFF = 0;
        SCREEN_STREAMING = true;
    }
}

fn le32(dst: &mut [u8], value: u32) {
    dst[..4].copy_from_slice(&value.to_le_bytes());
}

fn le16(dst: &mut [u8], value: u16) {
    dst[..2].copy_from_slice(&value.to_le_bytes());
}

fn send_http(services: &Services, slot: u32, content_type: &[u8], body: &[u8]) {
    send_http_status(services, slot, 200, content_type, body);
}

fn send_http_status(services: &Services, slot: u32, status: u16, content_type: &[u8], body: &[u8]) {
    let reason: &[u8] = match status {
        200 => b"200 OK",
        202 => b"202 Accepted",
        400 => b"400 Bad Request",
        404 => b"404 Not Found",
        409 => b"409 Conflict",
        500 => b"500 Internal Server Error",
        _ => b"500 Internal Server Error",
    };
    let mut header = [0u8; 160];
    let mut w = Out::new(&mut header);
    w.push(b"HTTP/1.0 ");
    w.push(reason);
    w.push(b"\r\nContent-Type: ");
    w.push(content_type);
    w.push(b"\r\nConnection: close\r\nContent-Length: ");
    w.u64(body.len() as u64);
    w.push(b"\r\n\r\n");
    let n = w.len;
    send_all(services, slot, &header[..n]);
    send_all(services, slot, body);
}

// ---------------------------------------------------------------------------
// JSON-RPC / MCP
// ---------------------------------------------------------------------------

/// Write a JSON-RPC result carrying one `text` content item.
fn mcp_text(w: &mut Out, id: &[u8], text: &[u8]) {
    w.push(b"{\"jsonrpc\":\"2.0\",\"id\":");
    w.push(id);
    w.push(b",\"result\":{\"content\":[{\"type\":\"text\",\"text\":");
    w.json_string(text);
    w.push(b"}]}}");
}

/// Write a JSON-RPC error object.
fn mcp_error(w: &mut Out, id: &[u8], code: &[u8], message: &[u8]) {
    w.push(b"{\"jsonrpc\":\"2.0\",\"id\":");
    w.push(id);
    w.push(b",\"error\":{\"code\":");
    w.push(code);
    w.push(b",\"message\":");
    w.json_string(message);
    w.push(b"}}");
}

fn mcp_response(services: &Services, body: &[u8], out: &mut [u8]) -> usize {
    let id = json_id(body);
    let mut w = Out::new(out);
    match json_string(body, b"\"method\"") {
        Some(b"initialize") => {
            let version = json_string(body, b"\"protocolVersion\"").unwrap_or(b"2026-07-28");
            w.push(b"{\"jsonrpc\":\"2.0\",\"id\":");
            w.push(id);
            w.push(b",\"result\":{\"protocolVersion\":");
            w.json_string(version);
            w.push(b",\"capabilities\":{\"tools\":{},\"resources\":{\"subscribe\":false,\"listChanged\":false}},\"serverInfo\":{\"name\":\"irix7-mcp\",\"version\":\"0.1.0\"}}}");
        }
        Some(b"ping") | Some(b"notifications/initialized") => {
            w.push(b"{\"jsonrpc\":\"2.0\",\"id\":");
            w.push(id);
            w.push(b",\"result\":{}}");
        }
        Some(b"tools/list") => {
            w.push(b"{\"jsonrpc\":\"2.0\",\"id\":");
            w.push(id);
            w.push(b",\"result\":{\"tools\":[{\"name\":\"status\",\"description\":\"Guest status\",\"inputSchema\":{\"type\":\"object\"}},{\"name\":\"echo\",\"description\":\"Echo text\",\"inputSchema\":{\"type\":\"object\",\"properties\":{\"text\":{\"type\":\"string\"}}}},{\"name\":\"console\",\"description\":\"Write a line to the guest console\",\"inputSchema\":{\"type\":\"object\",\"properties\":{\"text\":{\"type\":\"string\"}}}},{\"name\":\"serial.read\",\"description\":\"Read the serial console ring\",\"inputSchema\":{\"type\":\"object\",\"properties\":{\"offset\":{\"type\":\"integer\"},\"length\":{\"type\":\"integer\"}}}},{\"name\":\"serial.write\",\"description\":\"Write a line to the guest console\",\"inputSchema\":{\"type\":\"object\",\"properties\":{\"text\":{\"type\":\"string\"}}}},{\"name\":\"system.reset\",\"description\":\"Restart the guest from the PROM\",\"inputSchema\":{\"type\":\"object\"}},{\"name\":\"system.poweroff\",\"description\":\"Halt the guest\",\"inputSchema\":{\"type\":\"object\"}},{\"name\":\"mesh.reconnect\",\"description\":\"Reset the mesh link and re-arm the listener (M2 restart harness)\",\"inputSchema\":{\"type\":\"object\"}},{\"name\":\"shell.exec\",\"description\":\"Run a small guest shell command\",\"inputSchema\":{\"type\":\"object\",\"properties\":{\"command\":{\"type\":\"string\"}}}},{\"name\":\"block.read\",\"description\":\"Read a 512-byte SCSI sector\",\"inputSchema\":{\"type\":\"object\",\"properties\":{\"target\":{\"type\":\"integer\"},\"lba\":{\"type\":\"integer\"}}}},{\"name\":\"fs.read\",\"description\":\"Read a file from the EROFS fixture\",\"inputSchema\":{\"type\":\"object\",\"properties\":{\"path\":{\"type\":\"string\"}}}},{\"name\":\"fs.list\",\"description\":\"List a directory from the EROFS fixture\",\"inputSchema\":{\"type\":\"object\",\"properties\":{\"path\":{\"type\":\"string\"}}}},{\"name\":\"fs.stat\",\"description\":\"Stat a file, directory or symlink in the EROFS fixture and RAM overlay\",\"inputSchema\":{\"type\":\"object\",\"properties\":{\"path\":{\"type\":\"string\"}}}},{\"name\":\"fs.readlink\",\"description\":\"Read a symbolic link target from the EROFS fixture\",\"inputSchema\":{\"type\":\"object\",\"properties\":{\"path\":{\"type\":\"string\"}}}},{\"name\":\"fs.write\",\"description\":\"Write a file to the RAM overlay\",\"inputSchema\":{\"type\":\"object\",\"properties\":{\"path\":{\"type\":\"string\"},\"data\":{\"type\":\"string\"}}}},{\"name\":\"fs.remove\",\"description\":\"Remove an overlay file or lower file\",\"inputSchema\":{\"type\":\"object\",\"properties\":{\"path\":{\"type\":\"string\"}}}},{\"name\":\"screen.capture\",\"description\":\"Capture a PNG thumbnail and the BMP URL\",\"inputSchema\":{\"type\":\"object\"}}]}}");
        }
        Some(b"resources/list") => {
            w.push(b"{\"jsonrpc\":\"2.0\",\"id\":");
            w.push(id);
            w.push(b",\"result\":{\"resources\":[{\"uri\":\"irix7://serial\",\"name\":\"serial\",\"mimeType\":\"text/plain\",\"description\":\"Kernel serial ring\"},{\"uri\":\"irix7://status\",\"name\":\"status\",\"mimeType\":\"text/plain\",\"description\":\"Machine status\"},{\"uri\":\"irix7://net\",\"name\":\"net\",\"mimeType\":\"text/plain\",\"description\":\"Network summary\"},{\"uri\":\"irix7://mem\",\"name\":\"mem\",\"mimeType\":\"text/plain\",\"description\":\"Frame allocator summary\"}]}}");
        }
        Some(b"resources/read") => {
            let uri = json_string(body, b"\"uri\"").unwrap_or(b"");
            let mut data = [0u8; 1024];
            let n: isize = if uri == b"irix7://serial" {
                let total = (services.console_total)();
                let offset = total.saturating_sub(1024);
                unsafe { (services.console_read)(offset, data.as_mut_ptr(), data.len()) }
            } else if uri == b"irix7://status" {
                unsafe { (services.info)(0, data.as_mut_ptr(), data.len()) }
            } else if uri == b"irix7://net" {
                unsafe { (services.info)(1, data.as_mut_ptr(), data.len()) }
            } else if uri == b"irix7://mem" {
                unsafe { (services.info)(2, data.as_mut_ptr(), data.len()) }
            } else {
                -1
            };
            if n < 0 {
                w.push(b"{\"jsonrpc\":\"2.0\",\"id\":");
                w.push(id);
                w.push(b",\"error\":{\"code\":-32602,\"message\":\"unknown resource\"}}");
            } else {
                w.push(b"{\"jsonrpc\":\"2.0\",\"id\":");
                w.push(id);
                w.push(b",\"result\":{\"contents\":[{\"uri\":");
                w.json_string(uri);
                w.push(b",\"mimeType\":\"text/plain\",\"text\":");
                w.json_string(&data[..n as usize]);
                w.push(b"}]}}");
            }
        }
        Some(b"resources/templates/list") => {
            w.push(b"{\"jsonrpc\":\"2.0\",\"id\":");
            w.push(id);
            w.push(b",\"result\":{\"resourceTemplates\":[]}}");
        }
        Some(b"tools/call") => {
            let name = json_string(body, b"\"name\"");
            let text = json_string(body, b"\"text\"").unwrap_or(b"");
            match name {
                Some(b"status") => {
                    let mut data = [0u8; 512];
                    let n = unsafe { (services.status)(data.as_mut_ptr(), data.len()) };
                    if n < 0 {
                        w.push(b"{\"jsonrpc\":\"2.0\",\"id\":");
                        w.push(id);
                        w.push(b",\"error\":{\"code\":-32603,\"message\":\"status failed\"}}");
                    } else {
                        w.push(b"{\"jsonrpc\":\"2.0\",\"id\":");
                        w.push(id);
                        w.push(b",\"result\":{\"content\":[{\"type\":\"text\",\"text\":");
                        w.json_string(&data[..n as usize]);
                        w.push(b"}]}}");
                    }
                }
                Some(b"echo") => {
                    w.push(b"{\"jsonrpc\":\"2.0\",\"id\":");
                    w.push(id);
                    w.push(b",\"result\":{\"content\":[{\"type\":\"text\",\"text\":");
                    w.json_string(text);
                    w.push(b"}]}}");
                }
                Some(b"console") | Some(b"serial.write") => {
                    (services.write)(text.as_ptr(), text.len());
                    (services.write)(b"\r\n".as_ptr(), 2);
                    w.push(b"{\"jsonrpc\":\"2.0\",\"id\":");
                    w.push(id);
                    w.push(b",\"result\":{\"content\":[{\"type\":\"text\",\"text\":\"ok\"}]}}");
                }
                Some(b"system.reset") => {
                    unsafe {
                        ACTION = 1;
                        COUNTDOWN = 5;
                    }
                    w.push(b"{\"jsonrpc\":\"2.0\",\"id\":");
                    w.push(id);
                    w.push(
                        b",\"result\":{\"content\":[{\"type\":\"text\",\"text\":\"resetting\"}]}}",
                    );
                }
                Some(b"system.poweroff") => {
                    unsafe {
                        ACTION = 2;
                        COUNTDOWN = 5;
                    }
                    w.push(b"{\"jsonrpc\":\"2.0\",\"id\":");
                    w.push(id);
                    w.push(b",\"result\":{\"content\":[{\"type\":\"text\",\"text\":\"powering off\"}]}}");
                }
                Some(b"mesh.reconnect") => {
                    let ok = mesh::admin_reconnect(services);
                    w.push(b"{\"jsonrpc\":\"2.0\",\"id\":");
                    w.push(id);
                    w.push(if ok {
                        b",\"result\":{\"content\":[{\"type\":\"text\",\"text\":\"mesh reconnect requested\"}]}}"
                    } else {
                        b",\"result\":{\"content\":[{\"type\":\"text\",\"text\":\"mesh unavailable\"}]}}"
                    });
                }
                Some(b"fs.write") => {
                    let path = json_string(body, b"\"path\"").unwrap_or(b"/tmp/motd");
                    let data = json_string(body, b"\"data\"").unwrap_or(b"hello from MCP");
                    let path = core::str::from_utf8(path).unwrap_or("/tmp/motd");
                    if ensure_fs() && fs_write(path, data) {
                        w.push(b"{\"jsonrpc\":\"2.0\",\"id\":");
                        w.push(id);
                        w.push(b",\"result\":{\"content\":[{\"type\":\"text\",\"text\":\"ok\"}]}}");
                    } else {
                        w.push(b"{\"jsonrpc\":\"2.0\",\"id\":");
                        w.push(id);
                        w.push(b",\"error\":{\"code\":-32603,\"message\":\"fs write failed\"}}");
                    }
                }
                Some(b"fs.remove") => {
                    let path = json_string(body, b"\"path\"").unwrap_or(b"/tmp/motd");
                    let path = core::str::from_utf8(path).unwrap_or("/tmp/motd");
                    if ensure_fs() && fs_remove(path) {
                        w.push(b"{\"jsonrpc\":\"2.0\",\"id\":");
                        w.push(id);
                        w.push(b",\"result\":{\"content\":[{\"type\":\"text\",\"text\":\"ok\"}]}}");
                    } else {
                        w.push(b"{\"jsonrpc\":\"2.0\",\"id\":");
                        w.push(id);
                        w.push(b",\"error\":{\"code\":-32603,\"message\":\"fs remove failed\"}}");
                    }
                }
                Some(b"fs.read") => {
                    let path = json_string(body, b"\"path\"").unwrap_or(b"/etc/motd");
                    let path = core::str::from_utf8(path).unwrap_or("/etc/motd");
                    if ensure_fs() {
                        if let Some(data) = fs_read(path) {
                            let shown = data.len().min(FS_PRINT_MAX);
                            w.push(b"{\"jsonrpc\":\"2.0\",\"id\":");
                            w.push(id);
                            w.push(b",\"result\":{\"content\":[{\"type\":\"text\",\"text\":");
                            w.json_string(&data[..shown]);
                            w.push(b"}]}}");
                        } else {
                            w.push(b"{\"jsonrpc\":\"2.0\",\"id\":");
                            w.push(id);
                            w.push(b",\"error\":{\"code\":-32603,\"message\":\"fs read failed\"}}");
                        }
                    } else {
                        w.push(b"{\"jsonrpc\":\"2.0\",\"id\":");
                        w.push(id);
                        w.push(b",\"error\":{\"code\":-32603,\"message\":\"fs mount failed\"}}");
                    }
                }
                Some(b"fs.list") => {
                    let path = json_string(body, b"\"path\"").unwrap_or(b"/");
                    let path = core::str::from_utf8(path).unwrap_or("/");
                    if ensure_fs() {
                        if let Some(list) = fs_list(path) {
                            w.push(b"{\"jsonrpc\":\"2.0\",\"id\":");
                            w.push(id);
                            w.push(b",\"result\":{\"content\":[{\"type\":\"text\",\"text\":");
                            w.json_string(list.as_bytes());
                            w.push(b"}]}}");
                        } else {
                            w.push(b"{\"jsonrpc\":\"2.0\",\"id\":");
                            w.push(id);
                            w.push(b",\"error\":{\"code\":-32603,\"message\":\"fs list failed\"}}");
                        }
                    } else {
                        w.push(b"{\"jsonrpc\":\"2.0\",\"id\":");
                        w.push(id);
                        w.push(b",\"error\":{\"code\":-32603,\"message\":\"fs mount failed\"}}");
                    }
                }
                Some(b"fs.stat") => {
                    let path = json_string(body, b"\"path\"").unwrap_or(b"/");
                    let path = core::str::from_utf8(path).unwrap_or("/");
                    if !ensure_fs() {
                        mcp_error(&mut w, id, b"-32603", FsError::Mount.message());
                    } else {
                        match fs_stat(path) {
                            Ok(stat) => {
                                let text = unsafe { &mut *core::ptr::addr_of_mut!(STAT_TEXT) };
                                let n = stat_json(&stat, text);
                                mcp_text(&mut w, id, &text[..n]);
                            }
                            Err(error) => mcp_error(&mut w, id, b"-32603", error.message()),
                        }
                    }
                }
                Some(b"fs.readlink") => {
                    let path = json_string(body, b"\"path\"").unwrap_or(b"/");
                    let path = core::str::from_utf8(path).unwrap_or("/");
                    if !ensure_fs() {
                        mcp_error(&mut w, id, b"-32603", FsError::Mount.message());
                    } else {
                        match fs_readlink(path) {
                            Ok(target) => mcp_text(&mut w, id, &target),
                            Err(error) => mcp_error(&mut w, id, b"-32603", error.message()),
                        }
                    }
                }
                Some(b"block.read") => {
                    let target = json_u64(body, b"\"target\"").unwrap_or(1) as u8;
                    let lba = json_u64(body, b"\"lba\"").unwrap_or(0) as u32;
                    if target > 7 {
                        w.push(b"{\"jsonrpc\":\"2.0\",\"id\":");
                        w.push(id);
                        w.push(
                            b",\"error\":{\"code\":-32602,\"message\":\"target must be 0..7\"}}",
                        );
                    } else {
                        let mut sector = [0u8; 512];
                        let result =
                            unsafe { (services.block_read)(target, lba, sector.as_mut_ptr()) };
                        if result != 0 {
                            w.push(b"{\"jsonrpc\":\"2.0\",\"id\":");
                            w.push(id);
                            w.push(
                                b",\"error\":{\"code\":-32603,\"message\":\"block read failed\"}}",
                            );
                        } else {
                            const HEX: &[u8; 16] = b"0123456789abcdef";
                            let mut text = [0u8; 1024];
                            for (index, byte) in sector.iter().enumerate() {
                                text[index * 2] = HEX[(byte >> 4) as usize];
                                text[index * 2 + 1] = HEX[(byte & 0x0f) as usize];
                            }
                            w.push(b"{\"jsonrpc\":\"2.0\",\"id\":");
                            w.push(id);
                            w.push(b",\"result\":{\"content\":[{\"type\":\"text\",\"text\":");
                            w.json_string(&text);
                            w.push(b"}]}}");
                        }
                    }
                }
                Some(b"screen.capture") => {
                    let n = thumbnail(services);
                    if n == 0 {
                        w.push(b"{\"jsonrpc\":\"2.0\",\"id\":");
                        w.push(id);
                        w.push(b",\"error\":{\"code\":-32603,\"message\":\"screen read failed\"}}");
                    } else {
                        w.push(b"{\"jsonrpc\":\"2.0\",\"id\":");
                        w.push(id);
                        w.push(b",\"result\":{\"content\":[{\"type\":\"text\",\"text\":\"GET http://127.0.0.1:8088/screen.bmp for the full 1280x1024 BMP\"},{\"type\":\"image\",\"data\":\"");
                        let b64 = unsafe { &*core::ptr::addr_of!(THUMB_B64) };
                        w.push(&b64[..n]);
                        w.push(b"\",\"mimeType\":\"image/png\"}");
                        w.push(b"]}}");
                    }
                }
                Some(b"shell.exec") => {
                    let command = json_string(body, b"\"command\"").unwrap_or(b"");
                    shell_exec(services, command, &mut w, id);
                }
                Some(b"serial.read") => {
                    let total = (services.console_total)();
                    let offset =
                        json_u64(body, b"\"offset\"").unwrap_or(total.saturating_sub(2048));
                    let length = json_u64(body, b"\"length\"").unwrap_or(1024).min(1024) as usize;
                    let mut data = [0u8; 1024];
                    let n = unsafe { (services.console_read)(offset, data.as_mut_ptr(), length) };
                    if n < 0 {
                        w.push(b"{\"jsonrpc\":\"2.0\",\"id\":");
                        w.push(id);
                        w.push(
                            b",\"error\":{\"code\":-32603,\"message\":\"console read failed\"}}",
                        );
                    } else {
                        w.push(b"{\"jsonrpc\":\"2.0\",\"id\":");
                        w.push(id);
                        w.push(b",\"result\":{\"content\":[{\"type\":\"text\",\"text\":");
                        w.json_string(&data[..n as usize]);
                        w.push(b"}]}}");
                    }
                }
                _ => {
                    w.push(b"{\"jsonrpc\":\"2.0\",\"id\":");
                    w.push(id);
                    w.push(b",\"error\":{\"code\":-32602,\"message\":\"unknown tool\"}}");
                }
            }
        }
        _ => {
            w.push(b"{\"jsonrpc\":\"2.0\",\"id\":");
            w.push(id);
            w.push(b",\"error\":{\"code\":-32601,\"message\":\"method not found\"}}");
        }
    }
    w.len
}

fn info_block<'a>(services: &Services, kind: u32, buf: &'a mut [u8; 512]) -> &'a [u8] {
    let n = unsafe { (services.info)(kind, buf.as_mut_ptr(), buf.len()) };
    if n > 0 {
        &buf[..n as usize]
    } else {
        b"info failed"
    }
}

fn shell_exec(services: &Services, command: &[u8], w: &mut Out, id: &[u8]) {
    let mut parts = command.splitn(2, |&b| b == b' ');
    let cmd = parts.next().unwrap_or(b"");
    let mut arg = parts.next().unwrap_or(b"");
    while let Some((&b' ', rest)) = arg.split_first() {
        arg = rest;
    }

    let mut buf = [0u8; 512];
    let text: &[u8] = match cmd {
        b"uname" => b"IRIX 7 indy mips n32",
        b"echo" => arg,
        b"uptime" => {
            let mut n = copy(&mut buf, b"uptime: ");
            n += write_u64(&mut buf[n..], (services.clock_ms)() / 1000);
            n += copy(&mut buf[n..], b" s");
            &buf[..n]
        }
        b"status" => {
            let n = unsafe { (services.status)(buf.as_mut_ptr(), buf.len()) };
            if n > 0 {
                &buf[..n as usize]
            } else {
                b"status failed"
            }
        }
        b"net" => info_block(services, 1, &mut buf),
        b"mem" => info_block(services, 2, &mut buf),
        b"time" => info_block(services, 3, &mut buf),
        b"help" => b"commands: echo, uname, uptime, status, net, mem, help, reboot, poweroff",
        b"reboot" => {
            unsafe {
                ACTION = 1;
                COUNTDOWN = 5;
            }
            b"resetting"
        }
        b"poweroff" => {
            unsafe {
                ACTION = 2;
                COUNTDOWN = 5;
            }
            b"powering off"
        }
        _ => b"unknown command",
    };

    w.push(b"{\"jsonrpc\":\"2.0\",\"id\":");
    w.push(id);
    w.push(b",\"result\":{\"content\":[{\"type\":\"text\",\"text\":");
    w.json_string(text);
    w.push(b"}]}}");
}

fn json_id(body: &[u8]) -> &[u8] {
    let Some(mut p) = find(body, b"\"id\"") else {
        return b"null";
    };
    p += 4;
    while p < body.len() && (body[p] == b' ' || body[p] == b':') {
        p += 1;
    }
    let start = p;
    if p < body.len() && body[p] == b'"' {
        p += 1;
        while p < body.len() && body[p] != b'"' {
            p += 1;
        }
        if p < body.len() {
            p += 1;
        }
    } else if body[p..].starts_with(b"null") {
        p += 4;
    } else if body[p..].starts_with(b"false") {
        p += 5;
    } else if body[p..].starts_with(b"true") {
        p += 4;
    } else {
        if p < body.len() && body[p] == b'-' {
            p += 1;
        }
        while p < body.len() && body[p].is_ascii_digit() {
            p += 1;
        }
    }
    &body[start..p]
}

fn json_u64(body: &[u8], key: &[u8]) -> Option<u64> {
    let mut p = find(body, key)? + key.len();
    while p < body.len() && (body[p] == b' ' || body[p] == b':') {
        p += 1;
    }
    let mut value = 0u64;
    let mut any = false;
    while p < body.len() && body[p].is_ascii_digit() {
        value = value
            .saturating_mul(10)
            .saturating_add((body[p] - b'0') as u64);
        p += 1;
        any = true;
    }
    if any {
        Some(value)
    } else {
        None
    }
}

fn json_string<'a>(body: &'a [u8], key: &[u8]) -> Option<&'a [u8]> {
    let mut p = find(body, key)? + key.len();
    while p < body.len() && (body[p] == b' ' || body[p] == b':') {
        p += 1;
    }
    if p >= body.len() || body[p] != b'"' {
        return None;
    }
    p += 1;
    let start = p;
    while p < body.len() && body[p] != b'"' {
        p += 1;
    }
    if p >= body.len() {
        return None;
    }
    Some(&body[start..p])
}

// ---------------------------------------------------------------------------
// Small helpers
// ---------------------------------------------------------------------------

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || haystack.len() < needle.len() {
        return None;
    }
    (0..=haystack.len() - needle.len()).find(|&i| &haystack[i..i + needle.len()] == needle)
}

fn starts_with_ci(haystack: &[u8], needle: &[u8]) -> bool {
    haystack.len() >= needle.len() && haystack[..needle.len()].eq_ignore_ascii_case(needle)
}

struct Out<'a> {
    buf: &'a mut [u8],
    len: usize,
}

impl<'a> Out<'a> {
    fn new(buf: &'a mut [u8]) -> Self {
        Self { buf, len: 0 }
    }

    fn push(&mut self, data: &[u8]) {
        let n = (self.buf.len() - self.len).min(data.len());
        self.buf[self.len..self.len + n].copy_from_slice(&data[..n]);
        self.len += n;
    }

    fn byte(&mut self, b: u8) {
        if self.len < self.buf.len() {
            self.buf[self.len] = b;
            self.len += 1;
        }
    }

    fn u64(&mut self, mut value: u64) {
        let mut digits = [0u8; 20];
        let mut n = 0;
        loop {
            digits[n] = b'0' + (value % 10) as u8;
            value /= 10;
            n += 1;
            if value == 0 {
                break;
            }
        }
        while n > 0 {
            n -= 1;
            self.byte(digits[n]);
        }
    }

    /// Write an `st_mode` as six zero-padded octal digits.
    fn octal(&mut self, value: u16) {
        for shift in (0..6).rev() {
            self.byte(b'0' + ((value >> (shift * 3)) & 0o7) as u8);
        }
    }

    fn json_string(&mut self, s: &[u8]) {
        self.byte(b'"');
        for &c in s {
            match c {
                b'"' => self.push(b"\\\""),
                b'\\' => self.push(b"\\\\"),
                b'\n' => self.push(b"\\n"),
                b'\r' => self.push(b"\\r"),
                b'\t' => self.push(b"\\t"),
                _ => self.byte(c),
            }
        }
        self.byte(b'"');
    }
}

fn copy(dst: &mut [u8], src: &[u8]) -> usize {
    let n = dst.len().min(src.len());
    dst[..n].copy_from_slice(&src[..n]);
    n
}

fn write_u64(dst: &mut [u8], mut value: u64) -> usize {
    let mut digits = [0u8; 20];
    let mut n = 0;
    loop {
        digits[n] = b'0' + (value % 10) as u8;
        value /= 10;
        n += 1;
        if value == 0 {
            break;
        }
    }
    let count = n.min(dst.len());
    for i in 0..count {
        dst[i] = digits[n - 1 - i];
    }
    count
}

fn send_all(services: &Services, slot: u32, data: &[u8]) {
    let mut offset = 0;
    while offset < data.len() {
        let n = unsafe { (services.tcp_send)(slot, data[offset..].as_ptr(), data.len() - offset) };
        if n <= 0 {
            return;
        }
        offset += n as usize;
    }
}

#[cfg(not(test))]
#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    loop {
        core::hint::spin_loop();
    }
}

// The module links only core and compiler_builtins; provide the byte
// primitives the compiler may emit for array initialisation and copies.
//
/// # Safety
/// `dst` must be valid for writes of `n` bytes.
#[no_mangle]
pub unsafe extern "C" fn memset(
    dst: *mut core::ffi::c_void,
    c: i32,
    n: usize,
) -> *mut core::ffi::c_void {
    let dst = dst as *mut u8;
    let mut i = 0;
    while i < n {
        unsafe {
            *dst.add(i) = c as u8;
        }
        i += 1;
    }
    dst as *mut core::ffi::c_void
}

/// # Safety
/// `dst` must be valid for writes of `n` bytes and `src` for reads of `n`
/// bytes; the regions must not overlap.
#[no_mangle]
pub unsafe extern "C" fn memcpy(
    dst: *mut core::ffi::c_void,
    src: *const core::ffi::c_void,
    n: usize,
) -> *mut core::ffi::c_void {
    let d = dst as *mut u8;
    let s = src as *const u8;
    let mut i = 0;
    while i < n {
        unsafe {
            *d.add(i) = *s.add(i);
        }
        i += 1;
    }
    dst
}

/// # Safety
/// `dst` must be valid for writes of `n` bytes and `src` for reads of `n`
/// bytes; the regions may overlap.
#[no_mangle]
pub unsafe extern "C" fn memmove(
    dst: *mut core::ffi::c_void,
    src: *const core::ffi::c_void,
    n: usize,
) -> *mut core::ffi::c_void {
    let d = dst as *mut u8;
    let s = src as *const u8;
    if (d as usize) < (s as usize) {
        return unsafe { memcpy(dst, src, n) };
    }
    let mut i = n;
    while i > 0 {
        i -= 1;
        unsafe {
            *d.add(i) = *s.add(i);
        }
    }
    dst
}

/// # Safety
/// `a` and `b` must be valid for reads of `n` bytes.
#[no_mangle]
pub unsafe extern "C" fn memcmp(
    a: *const core::ffi::c_void,
    b: *const core::ffi::c_void,
    n: usize,
) -> i32 {
    let a = a as *const u8;
    let b = b as *const u8;
    let mut i = 0;
    while i < n {
        let x = unsafe { *a.add(i) };
        let y = unsafe { *b.add(i) };
        if x != y {
            return x as i32 - y as i32;
        }
        i += 1;
    }
    0
}
