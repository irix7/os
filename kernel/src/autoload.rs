// SPDX-License-Identifier: GPL-3.0-only
//! Boot-time start of the in-kernel services.
//!
//! The MCP control plane and the logical heartbeat are kernel code.  There is
//! no module loader: every service is linked into the kernel image that the
//! ROM downloads (ADR M1-0003).  This module decides which of them start at
//! boot and reports the outcome for the BOOT-READY marker.

use crate::{console_println, nvram};

/// Outcome of the boot-time start of the MCP service.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Status {
    /// The network is off, so MCP does not start.
    Disabled,
    /// MCP started and listens.
    Loaded,
    /// MCP refused to start.
    Failed,
}

/// Start MCP in the kernel, unless the network is off.
pub fn boot() -> Status {
    // A network-off M1 boot does not need MCP.  The console and local policy
    // remain usable.
    if crate::net::disabled() {
        console_println("mcp: network off; not started");
        return Status::Disabled;
    }
    // MCP runs on the kernel's service table; its tick runs from the idle loop.
    if mcp::start(
        &kernel_core::service::SERVICES,
        &[crate::runtime_demo::REPORT, crate::m2_demo::REPORT],
    ) {
        console_println("mcp: started");
        Status::Loaded
    } else {
        console_println("mcp: refused to start");
        Status::Failed
    }
}

/// Run the Wasm policy probe when NVRAM sets `wasm.policy`.  The guest is part
/// of the kernel image, so the key only selects the run.  An unconfigured boot
/// skips it and keeps the native namespace.
pub fn boot_wasm() {
    let mut policy = [0u8; 16];
    if nvram::get("wasm.policy", &mut policy).is_some() {
        crate::wasm_probe::run();
    }
}
