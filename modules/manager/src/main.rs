// SPDX-License-Identifier: GPL-3.0-only
//! Downloadable runtime module manager using the ABI v12 module services.
//!
//! The manager reads `manager.index=<url>,<sha256>` and `manager.root=name@version`
//! from the NVRAM environment, loads the package-index image into the reserved
//! metadata region, verifies its release signature and the signature on every
//! module record, resolves the exact dependency graph, loads every image into
//! its declared arena region, validates each manifest v2 footer, registers the
//! declared exports, and only then enters the modules in dependency order.
//!
//! A failed load unloads every image from the transaction. The legacy
//! `manager.dep=<url>,<sha256>` path remains as a single-module fallback.
//!
//! Resolution, manifests, signatures and package policy live here, not in the
//! kernel. The kernel only fetches, checks SHA-256, copies and enters verified
//! images.

#![no_std]
#![no_main]

extern crate alloc;

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use core::alloc::{GlobalAlloc, Layout};
use core::sync::atomic::{AtomicPtr, Ordering};

use loader_abi::{ModuleStatus, STATUS_ADDR, STATUS_MAGIC};
use module_abi::{
    ModuleArena, ModuleContext, ModuleExport, ModuleExportBinding, ModuleHandle, ModuleInfo,
    ModuleLoad, ModuleVtable, Services, ABI_VERSION, MODULE_CONTEXT_VERSION, MODULE_ERR_NOT_FOUND,
    MODULE_LOAD_REPLACE, MODULE_OK,
};
use module_resolver::index::PackageIndex;
use module_resolver::runtime::{
    self, ArenaInfo, BackendError, ExportBinding, Handle, ImageInfo, LoadRequest, Plan,
    RuntimeBackend,
};
use module_resolver::Abi;
use module_trust::{SignatureRecord, TrustPolicy};

/// Largest metadata image the manager accepts.
const METADATA_MAX: usize = 64 * 1024;
/// NVRAM value buffer; longer values are truncated and fail closed.
const CONFIG_MAX: usize = 1024;

static VTABLE: ModuleVtable = ModuleVtable {
    status: 0,
    tick: None,
};

static SERVICES: AtomicPtr<Services> = AtomicPtr::new(core::ptr::null_mut());

struct KernelAlloc;

// SAFETY: the kernel `alloc` service is the module heap. It never frees, so
// `dealloc` is a no-op. The manager is single-threaded.
unsafe impl GlobalAlloc for KernelAlloc {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let services = SERVICES.load(Ordering::Relaxed);
        if services.is_null() {
            return core::ptr::null_mut();
        }
        // SAFETY: `_start` installed the kernel service table for this module.
        unsafe { ((*services).alloc)(layout.size(), layout.align()) }
    }

    unsafe fn dealloc(&self, _ptr: *mut u8, _layout: Layout) {}
}

#[global_allocator]
static ALLOCATOR: KernelAlloc = KernelAlloc;

#[no_mangle]
#[allow(clippy::not_unsafe_ptr_arg_deref)]
pub extern "C" fn _start(services: *const Services) -> *const ModuleVtable {
    let Some(services) = (unsafe { services.as_ref() }) else {
        write_status(b"runtime manager: null service table");
        return &VTABLE;
    };
    SERVICES.store(
        services as *const Services as *mut Services,
        Ordering::Relaxed,
    );
    if services.version < ABI_VERSION {
        write_status(b"runtime manager: ABI v12 required");
        return &VTABLE;
    }
    let text = match run_manager(services) {
        Ok(text) => text,
        Err(text) => text,
    };
    (services.write)(text.as_ptr(), text.len());
    if !text.ends_with('\n') {
        (services.write)(b"\n".as_ptr(), 1);
    }
    write_status(text.as_bytes());
    &VTABLE
}

fn run_manager(services: &Services) -> Result<String, String> {
    let mut value = [0u8; CONFIG_MAX];
    if let Some(len) = read_config(services, b"manager.index", &mut value) {
        return run_index(services, &value[..len]);
    }
    if let Some(len) = read_config(services, b"manager.dep", &mut value) {
        return run_legacy(services, &value[..len]);
    }
    Err(String::from(
        "runtime manager: manager.index and manager.dep missing",
    ))
}

fn run_index(services: &Services, value: &[u8]) -> Result<String, String> {
    let (url, digest) = parse_url_digest(value)?;
    let mut root_value = [0u8; CONFIG_MAX];
    let root_len = read_config(services, b"manager.root", &mut root_value)
        .ok_or_else(|| String::from("runtime manager: manager.root missing"))?;
    let (root_name, root_version) = parse_root(&root_value[..root_len])?;

    let arena = arena(services)?;
    let metadata_region = arena.region_count - 1;
    let metadata_max = METADATA_MAX.min(arena.region_size as usize) as u32;
    let policy = load_policy(services, metadata_region, metadata_max)?;

    let index_bytes = load_metadata(
        services,
        metadata_region,
        metadata_max,
        url,
        digest,
        b"manager.index",
        b"1",
    )?;
    if runtime::sha256(&index_bytes) != digest {
        return Err(String::from(
            "runtime manager: index bytes do not match manager.index digest",
        ));
    }

    let mut signature_value = [0u8; CONFIG_MAX];
    let signature = read_config(services, b"manager.index.sig", &mut signature_value)
        .and_then(|len| parse_signature(&signature_value[..len]));
    policy
        .verify(&digest, signature.as_ref())
        .map_err(|error| format!("runtime manager: index signature: {error}"))?;

    let index = PackageIndex::parse(&index_bytes)
        .map_err(|error| format!("runtime manager: index: {error}"))?;
    let host = Abi::new(ABI_VERSION as u16, 0);
    let plan = Plan::from_index(&index, root_name, root_version, host)
        .map_err(|error| error.status_text(None))?;

    let mut backend = ServicesBackend { services };
    let report = runtime::run(&plan, &policy, &mut backend)
        .map_err(|error| error.status_text(Some(&plan.root)))?;
    Ok(report.status_text())
}

fn run_legacy(services: &Services, value: &[u8]) -> Result<String, String> {
    let (url, digest) = parse_url_digest(value)?;
    let mut name_value = [0u8; CONFIG_MAX];
    let name = read_config(services, b"manager.dep.name", &mut name_value)
        .and_then(|len| core::str::from_utf8(&name_value[..len]).ok())
        .unwrap_or("demo-dep");
    let mut version_value = [0u8; CONFIG_MAX];
    let version = read_config(services, b"manager.dep.version", &mut version_value)
        .and_then(|len| core::str::from_utf8(&version_value[..len]).ok())
        .unwrap_or("1.0.0");
    let arena = arena(services)?;
    if arena.region_count == 0 {
        return Err(String::from("runtime manager: empty module arena"));
    }
    let metadata_region = arena.region_count - 1;
    let metadata_max = METADATA_MAX.min(arena.region_size as usize) as u32;
    let policy = load_policy(services, metadata_region, metadata_max)?;
    let mut signature_value = [0u8; CONFIG_MAX];
    let signature = read_config(services, b"manager.dep.sig", &mut signature_value)
        .and_then(|len| parse_signature(&signature_value[..len]));
    policy
        .verify(&digest, signature.as_ref())
        .map_err(|error| format!("runtime manager: dependency signature: {error}"))?;

    let request = ModuleLoad {
        url: url.as_ptr(),
        url_len: url.len() as u32,
        sha256: digest,
        name: name.as_ptr(),
        name_len: name.len() as u32,
        version: version.as_ptr(),
        version_len: version.len() as u32,
        exports: core::ptr::null(),
        export_count: 0,
        region: 0,
        flags: MODULE_LOAD_REPLACE,
        max_size: arena.region_size,
        reserved: 0,
    };
    let mut handle: ModuleHandle = 0;
    let status = unsafe { (services.module_load)(&request, &mut handle) };
    if status != MODULE_OK {
        return Err(format!(
            "runtime manager: dependency load failed ({status})"
        ));
    }
    let context = ModuleContext {
        version: MODULE_CONTEXT_VERSION,
        flags: 0,
        manager: core::ptr::null(),
        reserved: 0,
    };
    let mut module_status = 0i32;
    let status = unsafe {
        (services.module_execute)(
            handle,
            0,
            &context as *const ModuleContext,
            &mut module_status,
        )
    };
    if status == MODULE_OK && (module_status == 0 || module_status == 42) {
        return Ok(format!(
            "status=ok legacy={name}@{version} status={module_status}"
        ));
    }
    unsafe { (services.module_unload)(handle) };
    Err(format!(
        "runtime manager: dependency entry failed (loader={status} module={module_status})"
    ))
}

fn load_policy(
    services: &Services,
    metadata_region: u32,
    metadata_max: u32,
) -> Result<TrustPolicy, String> {
    let mut value = [0u8; CONFIG_MAX];
    if let Some(len) = read_config(services, b"manager.trust", &mut value) {
        let (url, digest) = parse_url_digest(&value[..len])?;
        let bytes = load_metadata(
            services,
            metadata_region,
            metadata_max,
            url,
            digest,
            b"manager.trust",
            b"1",
        )?;
        if runtime::sha256(&bytes) != digest {
            return Err(String::from(
                "runtime manager: trust bytes do not match manager.trust digest",
            ));
        }
        let text = core::str::from_utf8(&bytes)
            .map_err(|_| String::from("runtime manager: trust policy is not UTF-8"))?;
        return TrustPolicy::parse(text)
            .map_err(|error| format!("runtime manager: trust policy: {error}"));
    }
    let text = core::str::from_utf8(include_bytes!(concat!(env!("OUT_DIR"), "/trust.conf")))
        .map_err(|_| String::from("runtime manager: built-in trust policy is not UTF-8"))?;
    TrustPolicy::parse(text)
        .map_err(|error| format!("runtime manager: built-in trust policy: {error}"))
}

fn load_metadata(
    services: &Services,
    region: u32,
    max_size: u32,
    url: &str,
    digest: [u8; 32],
    name: &[u8],
    version: &[u8],
) -> Result<Vec<u8>, String> {
    let request = ModuleLoad {
        url: url.as_ptr(),
        url_len: url.len() as u32,
        sha256: digest,
        name: name.as_ptr(),
        name_len: name.len() as u32,
        version: version.as_ptr(),
        version_len: version.len() as u32,
        exports: core::ptr::null(),
        export_count: 0,
        region,
        flags: MODULE_LOAD_REPLACE,
        max_size,
        reserved: 0,
    };
    let mut handle: ModuleHandle = 0;
    let status = unsafe { (services.module_load)(&request, &mut handle) };
    if status != MODULE_OK {
        return Err(format!("runtime manager: metadata load failed ({status})"));
    }
    let mut info = ModuleInfo::EMPTY;
    let status = unsafe { (services.module_query)(handle, &mut info) };
    if status != MODULE_OK {
        unsafe { (services.module_unload)(handle) };
        return Err(format!("runtime manager: metadata query failed ({status})"));
    }
    let bytes = unsafe {
        core::slice::from_raw_parts(info.base as *const u8, info.image_size as usize).to_vec()
    };
    let status = unsafe { (services.module_unload)(handle) };
    if status != MODULE_OK {
        return Err(format!(
            "runtime manager: metadata unload failed ({status})"
        ));
    }
    Ok(bytes)
}

fn arena(services: &Services) -> Result<ModuleArena, String> {
    let mut arena = ModuleArena::EMPTY;
    let status = unsafe { (services.module_arena)(&mut arena) };
    if status != MODULE_OK {
        return Err(format!("runtime manager: module_arena failed ({status})"));
    }
    if arena.region_count == 0 {
        return Err(String::from("runtime manager: empty module arena"));
    }
    Ok(arena)
}

fn read_config(services: &Services, name: &[u8], out: &mut [u8]) -> Option<usize> {
    let len =
        unsafe { (services.config_get)(name.as_ptr(), name.len(), out.as_mut_ptr(), out.len()) };
    if len <= 0 {
        None
    } else {
        Some((len as usize).min(out.len()))
    }
}

fn parse_signature(value: &[u8]) -> Option<SignatureRecord> {
    let text = core::str::from_utf8(value).ok()?;
    SignatureRecord::parse(text).ok()
}

fn parse_url_digest(value: &[u8]) -> Result<(&str, [u8; 32]), String> {
    let text = core::str::from_utf8(value)
        .map_err(|_| String::from("runtime manager: configuration is not UTF-8"))?;
    let (url, digest_text) = text
        .rsplit_once(',')
        .ok_or_else(|| String::from("runtime manager: configuration is not <url>,<sha256>"))?;
    if url.is_empty() || url.len() > module_abi::MODULE_URL_MAX as usize {
        return Err(String::from(
            "runtime manager: configuration URL is invalid",
        ));
    }
    let digest = parse_digest(digest_text)
        .ok_or_else(|| String::from("runtime manager: configuration SHA-256 is invalid"))?;
    Ok((url, digest))
}

fn parse_root(value: &[u8]) -> Result<(&str, &str), String> {
    let text = core::str::from_utf8(value)
        .map_err(|_| String::from("runtime manager: manager.root is not UTF-8"))?;
    let (name, version) = text
        .split_once('@')
        .ok_or_else(|| String::from("runtime manager: manager.root is not <name>@<version>"))?;
    if name.is_empty()
        || version.is_empty()
        || name.len() > module_abi::MODULE_NAME_MAX as usize
        || version.len() > module_abi::MODULE_VERSION_MAX as usize
    {
        return Err(String::from("runtime manager: manager.root is invalid"));
    }
    Ok((name, version))
}

fn parse_digest(text: &str) -> Option<[u8; 32]> {
    if text.len() != 64 {
        return None;
    }
    let mut out = [0u8; 32];
    for (index, slot) in out.iter_mut().enumerate() {
        let hi = hex(text.as_bytes()[index * 2])?;
        let lo = hex(text.as_bytes()[index * 2 + 1])?;
        *slot = (hi << 4) | lo;
    }
    Some(out)
}

fn hex(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

struct ServicesBackend<'a> {
    services: &'a Services,
}

impl ServicesBackend<'_> {
    fn info_inner(&self, handle: Handle) -> Result<ModuleInfo, BackendError> {
        let mut info = ModuleInfo::EMPTY;
        let status = unsafe { (self.services.module_query)(handle, &mut info) };
        if status != MODULE_OK {
            return Err(BackendError(status));
        }
        Ok(info)
    }
}

impl RuntimeBackend for ServicesBackend<'_> {
    fn arena(&mut self) -> Result<ArenaInfo, BackendError> {
        let mut arena = ModuleArena::EMPTY;
        let status = unsafe { (self.services.module_arena)(&mut arena) };
        if status != MODULE_OK {
            return Err(BackendError(status));
        }
        Ok(ArenaInfo {
            base: arena.base as usize,
            length: arena.length as usize,
            region_size: arena.region_size as usize,
            region_count: arena.region_count,
            used_lo: arena.used_lo,
            used_hi: arena.used_hi,
        })
    }

    fn load(&mut self, request: &LoadRequest<'_>) -> Result<Handle, BackendError> {
        let mut exports = [ModuleExport::EMPTY; module_abi::MODULE_EXPORT_MAX as usize];
        let count = request.exports.len().min(exports.len());
        for (slot, export) in exports.iter_mut().zip(request.exports.iter()) {
            *slot = ModuleExport {
                interface: export.interface.as_ptr(),
                interface_len: export.interface.len() as u32,
                version: export.version,
                reserved: 0,
                offset: export.offset,
            };
        }
        let load = ModuleLoad {
            url: request.url.as_ptr(),
            url_len: request.url.len() as u32,
            sha256: request.sha256,
            name: request.name.as_ptr(),
            name_len: request.name.len() as u32,
            version: request.version.as_ptr(),
            version_len: request.version.len() as u32,
            exports: if count == 0 {
                core::ptr::null()
            } else {
                exports.as_ptr()
            },
            export_count: count as u32,
            region: request.region,
            flags: MODULE_LOAD_REPLACE,
            max_size: request.max_size,
            reserved: 0,
        };
        let mut handle: ModuleHandle = 0;
        let status = unsafe { (self.services.module_load)(&load, &mut handle) };
        if status != MODULE_OK {
            return Err(BackendError(status));
        }
        Ok(handle)
    }

    fn image(&self, handle: Handle) -> Result<&[u8], BackendError> {
        let info = self.info_inner(handle)?;
        Ok(
            unsafe {
                core::slice::from_raw_parts(info.base as *const u8, info.image_size as usize)
            },
        )
    }

    fn info(&mut self, handle: Handle) -> Result<ImageInfo, BackendError> {
        let info = self.info_inner(handle)?;
        Ok(ImageInfo {
            base: info.base as usize,
            image_size: info.image_size,
            region: info.region,
            state: info.state,
        })
    }

    fn execute(&mut self, handle: Handle, entry_offset: u32) -> Result<i32, BackendError> {
        let context = ModuleContext {
            version: MODULE_CONTEXT_VERSION,
            flags: 0,
            manager: core::ptr::null(),
            reserved: 0,
        };
        let mut module_status = 0i32;
        let status = unsafe {
            (self.services.module_execute)(
                handle,
                entry_offset,
                &context as *const ModuleContext,
                &mut module_status,
            )
        };
        if status != MODULE_OK {
            return Err(BackendError(status));
        }
        Ok(module_status)
    }

    fn find_export(
        &mut self,
        interface: &str,
        version: u16,
    ) -> Result<Option<ExportBinding>, BackendError> {
        let mut binding = ModuleExportBinding::EMPTY;
        let status = unsafe {
            (self.services.module_find_export)(
                interface.as_ptr(),
                interface.len() as u32,
                version,
                &mut binding,
            )
        };
        if status == MODULE_ERR_NOT_FOUND {
            return Ok(None);
        }
        if status != MODULE_OK {
            return Err(BackendError(status));
        }
        Ok(Some(ExportBinding {
            handle: binding.handle,
            table: if binding.table.is_null() {
                None
            } else {
                Some(binding.table as usize)
            },
        }))
    }

    fn unload(&mut self, handle: Handle) -> Result<(), BackendError> {
        let status = unsafe { (self.services.module_unload)(handle) };
        if status == MODULE_OK {
            Ok(())
        } else {
            Err(BackendError(status))
        }
    }
}

fn copy_text(status: &mut ModuleStatus, text: &[u8]) {
    let count = text.len().min(status.text.len());
    status.text.fill(0);
    status.text[..count].copy_from_slice(&text[..count]);
    status.len = count as u32;
}

fn write_status(text: &[u8]) {
    // SAFETY: fixed status block; single-threaded manager.
    let status = unsafe { &mut *(STATUS_ADDR as *mut ModuleStatus) };
    status.magic = STATUS_MAGIC;
    status.version = 1;
    copy_text(status, text);
}

#[cfg(not(test))]
#[panic_handler]
fn panic(_info: &core::panic::PanicInfo<'_>) -> ! {
    loop {
        core::hint::spin_loop();
    }
}
