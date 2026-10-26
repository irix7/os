// SPDX-License-Identifier: GPL-3.0-only
//! Runtime dependency demo entered by the downloadable manager.
//!
//! The image is linked at arena region 0 and exports `irix7.demo.service@1`.
//! `modules/demo-root` resolves that export through `module_find_export` and
//! calls the table after the manager has entered this module.

#![no_std]
#![no_main]

use module_abi::{ModuleContext, ModuleVtable, Services};

/// The exported interface table. The release tool records its image offset.
#[repr(C)]
pub struct DemoApi {
    /// Interface generation.
    pub abi: u32,
    /// Returns a fixed value so the dependent can prove the call.
    pub magic: unsafe extern "C" fn() -> u32,
}

const DEMO_MAGIC: u32 = 0x217;

unsafe extern "C" fn demo_magic() -> u32 {
    DEMO_MAGIC
}

#[link_section = ".demo_exports"]
#[no_mangle]
pub static DEMO_API: DemoApi = DemoApi {
    abi: 1,
    magic: demo_magic,
};

static VTABLE: ModuleVtable = ModuleVtable {
    status: 0,
    tick: None,
};

/// Module entry.
///
/// # Safety
/// `services` must point to the kernel service table; `_context` is the
/// manager's opaque context pointer and may be null.
#[no_mangle]
pub unsafe extern "C" fn _start(
    services: *const Services,
    _context: *const ModuleContext,
) -> *const ModuleVtable {
    if let Some(services) = unsafe { services.as_ref() } {
        (services.write)(
            b"demo-dep: entered by runtime manager\n".as_ptr(),
            b"demo-dep: entered by runtime manager\n".len(),
        );
    }
    &VTABLE
}

#[cfg(not(test))]
#[panic_handler]
fn panic(_info: &core::panic::PanicInfo<'_>) -> ! {
    loop {
        core::hint::spin_loop();
    }
}
