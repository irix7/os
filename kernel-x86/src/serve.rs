// SPDX-License-Identifier: GPL-3.0-only
//! The serve loop on the x86_64 machine.
//!
//! The MCP control plane and the Wasm policy are linked into this image
//! (ADR M1-0003).  This file runs MCP's tick for the boot's serve window and
//! then lets the machine power off.

use crate::net;
use crate::print_line;
use crate::serial;
use crate::timer;

/// Serve window for a standalone boot: 60 s at 100 Hz.
///
/// The smoke gate runs QEMU under a 150 s cap and expects the guest to power
/// off on its own.
pub const SMOKE_SERVE_TICKS: u32 = 6_000;

/// Serve window for a mesh run: 300 s at 100 Hz.
///
/// Join, homes, route, migration and the two restarts share one boot.
pub const MESH_SERVE_TICKS: u32 = 30_000;

/// Serve: poll the stack and call MCP's tick for the serve window.  A tick
/// that returns non-zero stops the ticks, as the Indy kernel does.
///
/// A peer address in the configuration means the rig is running the mesh
/// scenario, which needs the long window; otherwise the machine powers off
/// soon after the standalone markers.
pub fn serve() {
    serial::write(b"x86: services serving\n");
    // Run one tick so MCP arms its listeners, then announce readiness.
    let _ = mcp::tick(&kernel_core::service::SERVICES);
    serial::write(b"BOOT-READY v=1 kernel=up network=up mcp=listening mesh=single\n");
    let mut peer = [0u8; 64];
    let serve_ticks = if crate::config::get(b"mesh.peer", &mut peer) > 0 {
        MESH_SERVE_TICKS
    } else {
        SMOKE_SERVE_TICKS
    };
    let start = timer::ticks();
    let mut ticking = true;
    while timer::ticks().wrapping_sub(start) < serve_ticks {
        net::poll();
        if ticking && mcp::tick(&kernel_core::service::SERVICES) != 0 {
            ticking = false;
        }
        core::hint::spin_loop();
    }
    print_line(&[b"x86: serve window closed"]);
}
