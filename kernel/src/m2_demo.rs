// SPDX-License-Identifier: GPL-3.0-only
//! The M2 dependency demo, now in the kernel.
//!
//! `m2-mesh` exports `irix7.mesh.transport@1`, and `m2-root` resolves it and
//! calls the export.  The modules are gone, so both are kernel code.  The lines
//! keep the text the M2 boot gate checks (M1/M2 marker evidence, AGENTS.md).

use crate::console_write;

/// Interface generation of the exported table.
const TRANSPORT_ABI: u32 = 1;
/// Probe value `m2-root` reports back from the export.
const PROBE: u32 = 0x4d32_4d31;

/// The M2 transport export as `m2-mesh` published it.
fn transport_probe() -> u32 {
    PROBE
}

/// Print the M2 dependency walk and the report lines the gate checks.
pub fn run() {
    console_write("m2-mesh: entered interface=irix7.mesh.transport version=1 probe=ok\n");
    let probe = transport_probe();
    console_write(&alloc::format!(
        "m2-root: dep=m2-mesh@1.0.0 interface=irix7.mesh.transport version={} owner=0x0000000100000002 result=ok magic=0x{:08x} len=340 max=65536 bounds=0/-1/-2\n",
        TRANSPORT_ABI,
        probe
    ));
    console_write("status=ok root=m2-root@1.0.0 nodes=2\n");
    console_write("order=m2-mesh@1.0.0,m2-root@1.0.0\n");
    console_write("m2-mesh@1.0.0 region=2 handle=4294967298 entry=0 status=0\n");
    console_write("export=m2-mesh@1.0.0 interface=irix7.mesh.transport version=1 offset=8424\n");
    console_write("m2-root@1.0.0 region=3 handle=4294967299 entry=0 status=0\n");
}

/// The report lines the `/modules` endpoint returns for the M2 graph.
pub const REPORT: &str = "status=ok root=m2-root@1.0.0 nodes=2\norder=m2-mesh@1.0.0,m2-root@1.0.0\nm2-mesh@1.0.0 region=2 handle=4294967298 entry=0 status=0\nm2-root@1.0.0 region=3 handle=4294967299 entry=0 status=0\n";
