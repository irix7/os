// SPDX-License-Identifier: GPL-3.0-only
//! EROFS module: mount the fixture image on SCSI target 2 and print
//! `/etc/motd`.
//!
//! The image starts at LBA 0.  The module links `crates/erofs` and
//! `crates/block-cache` by path, reads sectors through the ABI version 10
//! `block_read` service, and prints through `write`.  It is a flat image at
//! `0x8840_0000`, so it does not grow the compressed PROM.

#![no_std]
#![no_main]

extern crate alloc;

use core::alloc::{GlobalAlloc, Layout};
use core::cmp::min;
use core::fmt::{self, Write};
use core::ptr::{self, null_mut};
use core::sync::atomic::{AtomicPtr, Ordering};

use alloc::string::String;
use alloc::vec::Vec;

use block_cache::Lru;
use module_abi::{ModuleVtable, Services};
use overlay::{
    DirEntry as OverlayDirEntry, EntryKind as OverlayKind, Lower as OverlayLower, Overlay,
};

/// The EROFS fixture lives on this SCSI target, at LBA 0.
const TARGET: u8 = 2;
/// The file the module prints.
const MOTD: &str = "/etc/motd";
/// EROFS block size passed to the cache.
const BLOCK_SIZE: usize = 4096;
/// The module-side read-ahead cache: 64 x 4 KiB = 256 KiB.
const CACHE_BLOCKS: usize = 64;
const _: () = assert!(CACHE_BLOCKS * BLOCK_SIZE == 256 * 1024);
/// Console chunks stay below the service's 8 KiB write limit.
const CHUNK: usize = 4096;
/// Ceiling on the printed file, so a bad image cannot flood the console.
const MAX_PRINT: usize = 64 * 1024;

static OK: ModuleVtable = ModuleVtable {
    status: 0,
    tick: None,
};
static FAILED: ModuleVtable = ModuleVtable {
    status: 1,
    tick: None,
};

const START: &[u8] = b"erofs: start\n";
const DONE: &[u8] = b"erofs: done\n";
const FAIL: &[u8] = b"erofs: failed\n";
const TRUNCATED: &[u8] = b"erofs: [truncated]\n";
const TOO_OLD: &[u8] = b"erofs: ABI too old (need 10)\n";

/// The service table, published before the first allocation.
static SERVICES: AtomicPtr<Services> = AtomicPtr::new(ptr::null_mut());

/// An allocator over the kernel heap service.
///
/// The kernel heap never frees, so `dealloc` has nothing to do.  The table is
/// stored before any allocating call in `_start`; a module image is zeroed by
/// the loader, so the pointer starts null.
struct ModuleAllocator;

unsafe impl GlobalAlloc for ModuleAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let table = SERVICES.load(Ordering::Acquire);
        if table.is_null() {
            return null_mut();
        }
        // SAFETY: `table` is the service table passed to `_start`; it stays
        // valid for the lifetime of the module.
        let allocate = unsafe { (*table).alloc };
        // SAFETY: the service returns either null or a block of at least
        // `size` bytes aligned to `align`, as the `GlobalAlloc` contract asks.
        unsafe { allocate(layout.size(), layout.align()) }
    }

    unsafe fn dealloc(&self, _ptr: *mut u8, _layout: Layout) {
        // The kernel heap is a bump allocator; memory is reclaimed by reboot.
    }
}

#[global_allocator]
static ALLOCATOR: ModuleAllocator = ModuleAllocator;

/// A byte-addressed disk over 512-byte SCSI sectors.
struct Disk {
    services: *const Services,
    target: u8,
}

impl block_cache::ReadAt for Disk {
    type Error = erofs::Error;

    fn read_at(&mut self, offset: u64, buf: &mut [u8]) -> Result<(), Self::Error> {
        let mut done = 0usize;
        while done < buf.len() {
            let position = offset.checked_add(done as u64).ok_or(erofs::Error::Io)?;
            let lba = u32::try_from(position / 512).map_err(|_| erofs::Error::Io)?;
            let within = (position % 512) as usize;
            let mut sector = [0u8; 512];
            // SAFETY: `services` came from `_start` and is valid for the
            // duration of the call; the sector is a writable 512-byte buffer.
            let result =
                unsafe { ((*self.services).block_read)(self.target, lba, sector.as_mut_ptr()) };
            if result != 0 {
                return Err(erofs::Error::Io);
            }
            let count = min(512 - within, buf.len() - done);
            buf[done..done + count].copy_from_slice(&sector[within..within + count]);
            done += count;
        }
        Ok(())
    }
}

/// The cached disk, adapted to the EROFS reader (the orphan rule keeps
/// `erofs::BlockDevice` off the foreign `Lru` type).
struct CachedDisk {
    cache: Lru<Disk, CACHE_BLOCKS>,
}

impl erofs::BlockDevice for CachedDisk {
    fn read_at(&mut self, offset: u64, buf: &mut [u8]) -> erofs::Result<()> {
        self.cache
            .read_at(offset, buf)
            .map_err(|_| erofs::Error::Io)
    }
}

/// A bounded line buffer for diagnostics.
struct Line {
    buf: [u8; 192],
    len: usize,
}

impl Line {
    fn new() -> Self {
        Self {
            buf: [0; 192],
            len: 0,
        }
    }

    fn write_to(&self, services: &Services) {
        (services.write)(self.buf.as_ptr(), self.len);
    }
}

impl Write for Line {
    fn write_str(&mut self, text: &str) -> fmt::Result {
        let room = self.buf.len() - self.len;
        if text.len() > room {
            return Err(fmt::Error);
        }
        self.buf[self.len..self.len + text.len()].copy_from_slice(text.as_bytes());
        self.len += text.len();
        Ok(())
    }
}

/// Why the module gave up.
enum Failure {
    /// The block cache could not be created.
    Cache,
    /// Mounting the image failed.
    Mount(erofs::Error),
    /// Resolving `/etc/motd` failed.
    Lookup(erofs::Error),
    /// Reading `/etc/motd` failed.
    Read(erofs::Error),
}

impl Failure {
    fn report(&self, services: &Services) {
        let mut line = Line::new();
        let _ = match self {
            Self::Cache => writeln!(line, "erofs: block cache: out of memory"),
            Self::Mount(error) => writeln!(line, "erofs: mount target {TARGET}: {error}"),
            Self::Lookup(error) => writeln!(line, "erofs: lookup {MOTD}: {error}"),
            Self::Read(error) => writeln!(line, "erofs: read {MOTD}: {error}"),
        };
        line.write_to(services);
    }
}

/// The lower EROFS tree handed to the RAM overlay.
struct LowerView<'a> {
    fs: &'a mut erofs::Erofs<CachedDisk>,
}

impl OverlayLower for LowerView<'_> {
    fn read_file(&mut self, path: &str) -> overlay::Result<Option<Vec<u8>>> {
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

    fn read_dir(&mut self, path: &str) -> overlay::Result<Option<Vec<OverlayDirEntry>>> {
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

/// Mount the fixture and print the file.
fn run(services: &Services) -> Result<(), Failure> {
    let disk = Disk {
        services,
        target: TARGET,
    };
    let cache = Lru::new(disk, BLOCK_SIZE).map_err(|_| Failure::Cache)?;
    let mut fs = erofs::Erofs::mount(CachedDisk { cache }).map_err(Failure::Mount)?;

    let mut line = Line::new();
    let _ = writeln!(
        line,
        "erofs: mounted target {TARGET} (block size {})",
        fs.superblock().block_size()
    );
    line.write_to(services);

    let inode = fs.lookup_path_follow(MOTD).map_err(Failure::Lookup)?;
    let data = fs.read_file(&inode).map_err(Failure::Read)?;

    line = Line::new();
    let _ = writeln!(line, "erofs: {MOTD} ({} bytes)", data.len());
    line.write_to(services);

    let shown = min(data.len(), MAX_PRINT);
    for chunk in data[..shown].chunks(CHUNK) {
        (services.write)(chunk.as_ptr(), chunk.len());
    }
    if shown < data.len() {
        (services.write)(TRUNCATED.as_ptr(), TRUNCATED.len());
    } else if !data.ends_with(b"\n") {
        (services.write)(b"\n".as_ptr(), 1);
    }

    // Exercise the M4 RAM overlay over the read-only EROFS lower.
    let mut overlay = Overlay::new(LowerView { fs: &mut fs });
    let _ = overlay.create_dir("/etc");
    let _ = overlay.create_dir("/home");
    if overlay.create_dir("/tmp").is_ok() && overlay.write_file("/tmp/motd", b"RAM upper\n").is_ok()
    {
        if let Ok(data) = overlay.read_file("/tmp/motd") {
            line = Line::new();
            let _ = writeln!(line, "overlay: /tmp/motd ({} bytes)", data.len());
            line.write_to(services);
            (services.write)(data.as_ptr(), data.len());
        }
        if overlay.remove_file("/tmp/motd").is_ok()
            && matches!(
                overlay.read_file("/tmp/motd"),
                Err(overlay::Error::NotFound)
            )
        {
            const WHITEOUT: &[u8] = b"overlay: whiteout ok\n";
            (services.write)(WHITEOUT.as_ptr(), WHITEOUT.len());
        }
    }
    Ok(())
}

/// Module entry.
///
/// # Safety
/// `services` must point to a valid service table supplied by the kernel.
#[no_mangle]
pub unsafe extern "C" fn _start(services: *const Services) -> *const ModuleVtable {
    let Some(services) = (unsafe { services.as_ref() }) else {
        return &FAILED;
    };
    if services.version < 10 {
        (services.write)(TOO_OLD.as_ptr(), TOO_OLD.len());
        return &FAILED;
    }
    SERVICES.store(
        services as *const Services as *mut Services,
        Ordering::Release,
    );
    (services.write)(START.as_ptr(), START.len());

    match run(services) {
        Ok(()) => {
            (services.write)(DONE.as_ptr(), DONE.len());
            &OK
        }
        Err(failure) => {
            failure.report(services);
            (services.write)(FAIL.as_ptr(), FAIL.len());
            &FAILED
        }
    }
}

#[cfg(not(test))]
#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    loop {
        core::hint::spin_loop();
    }
}

// The module links only core, alloc and compiler_builtins; provide the byte
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
/// `left` and `right` must be valid for reads of `n` bytes.
#[no_mangle]
pub unsafe extern "C" fn memcmp(
    left: *const core::ffi::c_void,
    right: *const core::ffi::c_void,
    n: usize,
) -> i32 {
    let left = left as *const u8;
    let right = right as *const u8;
    let mut i = 0;
    while i < n {
        let (a, b) = unsafe { (*left.add(i), *right.add(i)) };
        if a != b {
            return i32::from(a) - i32::from(b);
        }
        i += 1;
    }
    0
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
