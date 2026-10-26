// SPDX-License-Identifier: GPL-3.0-only
//! Runtime M2 mesh transport dependency.
//!
//! This module is linked at arena region 2 (`0x88540000`) and exports the
//! versioned `irix7.mesh.transport@1` C table. It is a real runtime dependency:
//! `modules/m2-root` declares its exact digest, the manager loads this image
//! first, and the root resolves the table with `module_find_export`.
//!
//! The exported probe encodes a fixed M2 Ping envelope with `mesh-transport`,
//! decodes it again, and checks the framing bounds. A failure returns a
//! distinct non-magic code so the dependent can fail closed rather than trust
//! a partially initialised transport.

#![cfg_attr(not(test), no_std)]
#![cfg_attr(not(test), no_main)]
#![cfg_attr(test, allow(dead_code))]

use core::alloc::{GlobalAlloc, Layout};
use core::sync::atomic::{AtomicPtr, Ordering};

use mesh_transport::{
    decode_record, encode_record, Error as TransportError, MAX_RECORD_LEN, MIN_ENVELOPE_LEN,
};
#[cfg(not(test))]
use module_abi::ModuleContext;
use module_abi::{ModuleVtable, Services};
use sys_primitives::{
    MeshEnvelope, MeshHeader, MessageFrame, MessageKind, NodeId, OpCode, StreamId, Tag,
};

/// Interface version exported as `irix7.mesh.transport@1`.
pub const INTERFACE_VERSION: u16 = 1;
/// Fixed source node used by the deterministic probe.
pub const PROBE_NODE: [u8; 16] = [
    0x02, 0x11, 0x22, 0x33, 0x44, 0x55, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
];
/// Returned when the encode/decode probe and the framing bounds all pass.
pub const PROBE_MAGIC: u32 = 0x4d32_4d31; // "M2M1"
/// Encoded length of the fixed probe record: a 4-byte length prefix, the
/// 128-byte mesh header, the 192-byte frame header/descriptor, and the 16-byte
/// node body.
pub const PROBE_RECORD_LEN: usize = 340;

/// Versioned C table exported at `MESH_API`.
#[repr(C)]
pub struct MeshTransportApi {
    /// Table generation; must be [`INTERFACE_VERSION`].
    pub abi: u32,
    /// Encode/decode round-trip plus framing-bound self-check.
    pub probe: unsafe extern "C" fn() -> u32,
    /// Exact encoded length of the fixed probe record.
    pub record_len: unsafe extern "C" fn() -> u32,
    /// Maximum envelope length accepted by the transport (`64 KiB`).
    pub max_record_len: unsafe extern "C" fn() -> u32,
    /// Classify an envelope length: 0 valid, -1 oversize, -2 too short.
    pub check_bound: unsafe extern "C" fn(u32) -> i32,
}

unsafe extern "C" fn mesh_probe() -> u32 {
    probe_inner()
}

unsafe extern "C" fn mesh_record_len() -> u32 {
    PROBE_RECORD_LEN as u32
}

unsafe extern "C" fn mesh_max_record_len() -> u32 {
    MAX_RECORD_LEN as u32
}

unsafe extern "C" fn mesh_check_bound(declared: u32) -> i32 {
    check_bound_inner(declared)
}

#[link_section = ".m2_exports"]
#[no_mangle]
pub static MESH_API: MeshTransportApi = MeshTransportApi {
    abi: INTERFACE_VERSION as u32,
    probe: mesh_probe,
    record_len: mesh_record_len,
    max_record_len: mesh_max_record_len,
    check_bound: mesh_check_bound,
};

static VTABLE_OK: ModuleVtable = ModuleVtable {
    status: 0,
    tick: None,
};

static VTABLE_FAILED: ModuleVtable = ModuleVtable {
    status: 1,
    tick: None,
};

static SERVICES: AtomicPtr<Services> = AtomicPtr::new(core::ptr::null_mut());

struct KernelAlloc;

// SAFETY: the kernel `alloc` service is the module heap. It never frees, so
// `dealloc` is a no-op. Modules are single-threaded.
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

#[cfg(not(test))]
#[global_allocator]
static ALLOCATOR: KernelAlloc = KernelAlloc;

/// Encode and decode one fixed M2 Ping envelope.
///
/// Returns [`PROBE_MAGIC`] only when the encoded record is exactly
/// [`PROBE_RECORD_LEN`] bytes, the decoded identity matches, and the decoder
/// rejects both an oversize and a too-short length prefix.
pub fn probe_inner() -> u32 {
    let node = NodeId(PROBE_NODE);
    let frame = match MessageFrame::inline(
        MessageKind::Request,
        OpCode::Ping,
        StreamId(0),
        Tag(1),
        &PROBE_NODE,
    ) {
        Ok(frame) => frame,
        Err(_) => return 1,
    };
    let envelope = MeshEnvelope {
        header: MeshHeader::new(node, NodeId::ZERO, node),
        frame,
    };
    let mut record = [0u8; 512];
    let len = match encode_record(&envelope, &mut record) {
        Ok(len) => len,
        Err(_) => return 2,
    };
    if len != PROBE_RECORD_LEN {
        return 3;
    }
    let decoded = match decode_record(&record[..len]) {
        Ok(decoded) => decoded,
        Err(_) => return 4,
    };
    if decoded.header.src != node
        || decoded.header.dst != NodeId::ZERO
        || decoded.header.home != node
        || decoded.frame.header.kind != MessageKind::Request
        || decoded.frame.header.opcode != OpCode::Ping
        || decoded.frame.body != &PROBE_NODE[..]
    {
        return 5;
    }
    if check_bound_inner(MAX_RECORD_LEN as u32 + 1) != -1 {
        return 6;
    }
    if check_bound_inner(MIN_ENVELOPE_LEN as u32 - 1) != -2 {
        return 7;
    }
    if !matches!(
        decode_record(&(MAX_RECORD_LEN as u32 + 1).to_le_bytes()),
        Err(TransportError::Oversize)
    ) {
        return 8;
    }
    PROBE_MAGIC
}

fn check_bound_inner(declared: u32) -> i32 {
    if declared as usize > MAX_RECORD_LEN {
        -1
    } else if (declared as usize) < MIN_ENVELOPE_LEN {
        -2
    } else {
        0
    }
}

/// Module entry.
///
/// # Safety
/// `services` must point to the kernel service table; `_context` is the
/// manager's opaque context pointer and may be null.
#[cfg(not(test))]
#[no_mangle]
pub unsafe extern "C" fn _start(
    services: *const Services,
    _context: *const ModuleContext,
) -> *const ModuleVtable {
    let Some(services) = (unsafe { services.as_ref() }) else {
        return &VTABLE_FAILED;
    };
    SERVICES.store(
        services as *const Services as *mut Services,
        Ordering::Relaxed,
    );
    let result = probe_inner();
    if result == PROBE_MAGIC {
        write(
            services,
            b"m2-mesh: entered interface=irix7.mesh.transport version=1 probe=ok\n",
        );
        &VTABLE_OK
    } else {
        write(services, b"m2-mesh: self-check failed code=");
        write_dec(services, result);
        write(services, b"\n");
        &VTABLE_FAILED
    }
}

fn write(services: &Services, bytes: &[u8]) {
    (services.write)(bytes.as_ptr(), bytes.len());
}

fn write_dec(services: &Services, value: u32) {
    let mut buf = [0u8; 10];
    let mut index = buf.len();
    let mut value = value;
    loop {
        index -= 1;
        buf[index] = b'0' + (value % 10) as u8;
        value /= 10;
        if value == 0 {
            break;
        }
    }
    write(services, &buf[index..]);
}

#[cfg(not(test))]
#[panic_handler]
fn panic(_info: &core::panic::PanicInfo<'_>) -> ! {
    loop {
        core::hint::spin_loop();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn probe_round_trips_a_fixed_mesh_record() {
        assert_eq!(probe_inner(), PROBE_MAGIC);
    }

    #[test]
    fn framing_bounds_are_exact() {
        assert_eq!(check_bound_inner(MIN_ENVELOPE_LEN as u32 - 1), -2);
        assert_eq!(check_bound_inner(MIN_ENVELOPE_LEN as u32), 0);
        assert_eq!(check_bound_inner(MAX_RECORD_LEN as u32), 0);
        assert_eq!(check_bound_inner(MAX_RECORD_LEN as u32 + 1), -1);
        assert_eq!(check_bound_inner(u32::MAX), -1);
    }
}
