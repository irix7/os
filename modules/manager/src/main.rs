// SPDX-License-Identifier: GPL-3.0-only
//! First downloadable runtime manager using the ABI v12 module services.
//!
//! It loads one dependency from `manager.dep.url`/`manager.dep.sha256` through
//! `Services::module_load`, executes it through `module_execute`, and publishes
//! the result in the fixed runtime status block for MCP. Resolution,
//! signatures, package metadata and multiple dependencies are later slices.

#![no_std]
#![no_main]

use loader_abi::{ModuleStatus, STATUS_ADDR, STATUS_MAGIC};
use module_abi::{
    ModuleContext, ModuleLoad, ModuleVtable, Services, MODULE_CONTEXT_VERSION, MODULE_OK,
};

static VTABLE: ModuleVtable = ModuleVtable {
    status: 0,
    tick: None,
};

#[allow(clippy::not_unsafe_ptr_arg_deref)]
#[no_mangle]
pub extern "C" fn _start(services: *const Services) -> *const ModuleVtable {
    let Some(services) = (unsafe { services.as_ref() }) else {
        return &VTABLE;
    };
    let mut dep = [0u8; 192];
    let n = unsafe {
        (services.config_get)(
            b"manager.dep".as_ptr(),
            b"manager.dep".len(),
            dep.as_mut_ptr(),
            dep.len(),
        )
    };
    if n <= 0 {
        write_status(b"runtime manager: manager.dep missing");
        return &VTABLE;
    }
    let dep = &dep[..n as usize];
    let Some(comma) = dep.iter().position(|byte| *byte == b',') else {
        write_status(b"runtime manager: manager.dep malformed");
        return &VTABLE;
    };
    let url = &dep[..comma];
    let digest_text = &dep[comma + 1..];
    let url_len = url.len();
    let Some(digest) = parse_digest(digest_text) else {
        write_status(b"runtime manager: manager.dep digest invalid");
        return &VTABLE;
    };
    if url.len() > module_abi::MODULE_URL_MAX as usize {
        write_status(b"runtime manager: dependency URL too long");
        return &VTABLE;
    }
    let name = b"demo-dep";
    let version = b"1.0.0";
    let request = ModuleLoad {
        url: url.as_ptr(),
        url_len: url_len as u32,
        sha256: digest,
        name: name.as_ptr(),
        name_len: name.len() as u32,
        version: version.as_ptr(),
        version_len: version.len() as u32,
        exports: core::ptr::null(),
        export_count: 0,
        region: 0,
        flags: 0,
        max_size: 0,
        reserved: 0,
    };
    let mut handle = 0u64;
    let status = unsafe { (services.module_load)(&request, &mut handle) };
    if status != MODULE_OK {
        write_status(b"runtime manager: dependency load failed");
        (services.write)(
            b"manager: dependency load failed\n".as_ptr(),
            b"manager: dependency load failed\n".len(),
        );
        return &VTABLE;
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
    if status == MODULE_OK && module_status == 42 {
        write_status(b"runtime manager: demo-dep loaded and returned 42");
        (services.write)(
            b"manager: demo-dep returned 42\n".as_ptr(),
            b"manager: demo-dep returned 42\n".len(),
        );
    } else {
        write_status(b"runtime manager: dependency entry failed");
        (services.write)(
            b"manager: dependency entry failed\n".as_ptr(),
            b"manager: dependency entry failed\n".len(),
        );
    }
    &VTABLE
}

fn parse_digest(text: &[u8]) -> Option<[u8; 32]> {
    if text.len() != 64 {
        return None;
    }
    let mut out = [0u8; 32];
    for (index, slot) in out.iter_mut().enumerate() {
        let hi = hex(text[index * 2])?;
        let lo = hex(text[index * 2 + 1])?;
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

fn copy_text(status: &mut ModuleStatus, text: &[u8]) {
    let count = text.len().min(status.text.len());
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

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo<'_>) -> ! {
    loop {
        core::hint::spin_loop();
    }
}
