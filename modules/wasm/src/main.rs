// SPDX-License-Identifier: GPL-3.0-only
//! Minimal Wasm core probe for M1.
//!
//! Loads a tiny embedded module through the no_std `wasm-host` surface, runs
//! its exported `run` function and prints the result. This is the first
//! Wasm-on-iris proof, not the final policy core.

#![no_std]
#![no_main]

extern crate alloc;

use core::alloc::{GlobalAlloc, Layout};
use core::ptr::null_mut;
use core::sync::atomic::{AtomicPtr, Ordering};

use module_abi::{ModuleVtable, Services};
use wasm_host::{Host, HostHooks};

/// Guest source is in `guest.wat`; this is the flattened binary.
const RUN_WASM: &[u8] = include_bytes!("guest.wasm");

static SERVICES: AtomicPtr<Services> = AtomicPtr::new(null_mut());

struct ModuleAllocator;

unsafe impl GlobalAlloc for ModuleAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let table = SERVICES.load(Ordering::Acquire);
        if table.is_null() {
            return null_mut();
        }
        // SAFETY: the table is the kernel's v11 service table.
        let allocate = unsafe { (*table).alloc };
        unsafe { allocate(layout.size(), layout.align()) }
    }

    unsafe fn dealloc(&self, _ptr: *mut u8, _layout: Layout) {
        // The kernel heap is a bump allocator; memory is reclaimed by reboot.
    }
}

#[global_allocator]
static ALLOCATOR: ModuleAllocator = ModuleAllocator;

static OK: ModuleVtable = ModuleVtable {
    status: 0,
    tick: None,
};
static BAD: ModuleVtable = ModuleVtable {
    status: 1,
    tick: None,
};

fn log(services: &Services, text: &[u8]) {
    (services.write)(text.as_ptr(), text.len());
}

fn services() -> Option<&'static Services> {
    let ptr = SERVICES.load(Ordering::Acquire);
    if ptr.is_null() {
        None
    } else {
        // SAFETY: the table lives for the kernel lifetime.
        Some(unsafe { &*ptr })
    }
}

fn kernel_clock_ms() -> u64 {
    services()
        .map(|services| (services.clock_ms)() as u64)
        .unwrap_or(0)
}

fn kernel_console_write(bytes: &[u8]) {
    if let Some(services) = services() {
        (services.write)(bytes.as_ptr(), bytes.len());
    }
}

fn kernel_cap_check(_handle: i64, _rights: i32, _object: i32) -> i32 {
    // The M1 probe has one implicitly granted root capability.
    0
}

static HOST_HOOKS: HostHooks = HostHooks {
    clock_ms: kernel_clock_ms,
    console_write: kernel_console_write,
    cap_check: kernel_cap_check,
};

/// # Safety
/// `services` must point to a valid v11 kernel service table.
#[no_mangle]
pub unsafe extern "C" fn _start(services: *const Services) -> *const ModuleVtable {
    let Some(services) = (unsafe { services.as_ref() }) else {
        return &BAD;
    };
    if services.version < 11 {
        log(services, b"wasm: ABI too old\n");
        return &BAD;
    }
    SERVICES.store(
        services as *const Services as *mut Services,
        Ordering::Release,
    );

    let host = match Host::with_hooks(HOST_HOOKS) {
        Ok(host) => host,
        Err(_) => {
            log(services, b"wasm: host setup failed\n");
            return &BAD;
        }
    };
    match host.run_i32(RUN_WASM, "run", 1_000_000) {
        Ok(42) => {
            log(services, b"wasm: run=42\n");
            &OK
        }
        Ok(_) => {
            log(services, b"wasm: wrong result\n");
            &BAD
        }
        Err(_) => {
            log(services, b"wasm: run failed\n");
            &BAD
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

// The module links only core/alloc and compiler_builtins.
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
/// `dst` must be valid for writes of `n` bytes and `src` for reads of `n` bytes.
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
        let av = unsafe { *a.add(i) };
        let bv = unsafe { *b.add(i) };
        if av != bv {
            return av as i32 - bv as i32;
        }
        i += 1;
    }
    0
}

/// # Safety
/// `dst` and `src` must be valid for `n` bytes; regions may overlap.
#[no_mangle]
pub unsafe extern "C" fn memmove(
    dst: *mut core::ffi::c_void,
    src: *const core::ffi::c_void,
    n: usize,
) -> *mut core::ffi::c_void {
    let d = dst as *mut u8;
    let s = src as *const u8;
    if (d as usize) < (s as usize) {
        let mut i = 0;
        while i < n {
            unsafe {
                *d.add(i) = *s.add(i);
            }
            i += 1;
        }
    } else {
        let mut i = n;
        while i > 0 {
            i -= 1;
            unsafe {
                *d.add(i) = *s.add(i);
            }
        }
    }
    dst
}
