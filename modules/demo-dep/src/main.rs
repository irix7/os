// SPDX-License-Identifier: GPL-3.0-only
//! Runtime dependency demo entered by the downloadable manager.

#![no_std]
#![no_main]

use module_abi::{ModuleContext, ModuleVtable, Services};

static VTABLE: ModuleVtable = ModuleVtable {
    status: 42,
    tick: None,
};

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

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo<'_>) -> ! {
    loop {
        core::hint::spin_loop();
    }
}
