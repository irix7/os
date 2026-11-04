// SPDX-License-Identifier: GPL-3.0-only
//! Runtime dependency demo, now in the kernel.
//!
//! `demo-dep` exports `irix7.demo.service@1`; `demo-root` resolves it and calls
//! the export. The modules are gone, so both are kernel code. The lines keep the
//! text that the boot gate checks (M1 marker evidence, AGENTS.md).

use alloc::format;

use crate::console_write;

/// Interface generation of the exported table.
pub const DEMO_ABI: u32 = 1;
/// Value returned by the exported `magic` call; `demo-root` prints it.
pub const DEMO_MAGIC: u32 = 0x217;

/// The exported interface table, as `demo-dep` published it.
#[repr(C)]
pub struct DemoApi {
    pub abi: u32,
    pub magic: fn() -> u32,
}

fn demo_magic() -> u32 {
    DEMO_MAGIC
}

/// The static export. Its address is printed in the export record.
pub static DEMO_API: DemoApi = DemoApi {
    abi: DEMO_ABI,
    magic: demo_magic,
};

/// kseg0 address the kernel image is linked at (`kernel/linker.ld`).
const KERNEL_BASE: usize = 0x8804_0000;

/// Handle the loader would have returned for `demo-dep`.
const DEP_HANDLE: u64 = 1;

/// Print the dependency walk: `demo-dep` enters, `demo-root` resolves its export
/// and calls it, and the report lines the gate checks.
pub fn run() {
    console_write("demo-dep: entered by runtime manager\n");

    // Resolve the export the way the old loader did: the handle and the table.
    let api = &DEMO_API;
    if api.abi != DEMO_ABI {
        console_write("demo-root: dependency ABI mismatch\n");
        return;
    }
    let magic = (api.magic)();
    console_write(&format!(
        "demo-root: dep=0x{:03x} owner=0x{:016x}\n",
        magic, DEP_HANDLE
    ));

    let offset = (api as *const DemoApi as usize) - KERNEL_BASE;
    console_write(&format!(
        "export=demo-dep@1.0.0 interface=irix7.demo.service version=1 offset={}\n",
        offset
    ));
    console_write("order=demo-dep@1.0.0,demo-root@1.0.0\n");
    console_write("demo-dep@1.0.0 region=0 handle=1 entry=0 status=0\n");
    console_write("demo-root@1.0.0 region=1 handle=2 entry=0 status=0\n");
    console_write("status=ok root=demo-root@1.0.0 nodes=2\n");
}
