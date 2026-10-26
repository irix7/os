// SPDX-License-Identifier: GPL-3.0-only
//! Runtime graph root for the M2 mesh transport dependency.
//!
//! This image is linked at arena region 3 (`0x88580000`) and declares an exact
//! dependency on `m2-mesh@1.0.0`. It does **not** link `mesh-transport`: the
//! manager loads region 2 first, then this image resolves
//! `irix7.mesh.transport@1` through `module_find_export`, checks the owner
//! handle and table version, and calls the dependency.
//!
//! The printed line is the M2 runtime-graph evidence:
//!
//! ```text
//! m2-root: dep=m2-mesh@1.0.0 interface=irix7.mesh.transport version=1 \
//!   owner=0x... result=ok magic=0x4d324d31 len=340 max=65536 bounds=0/-1/-2
//! ```

#![cfg_attr(not(test), no_std)]
#![cfg_attr(not(test), no_main)]
#![cfg_attr(test, allow(dead_code))]

#[cfg(not(test))]
use module_abi::{ModuleContext, ModuleExportBinding, ModuleInfo, MODULE_OK};
use module_abi::{ModuleVtable, Services};

/// Arena region the manager must place `m2-mesh` in.
const MESH_REGION: u32 = 2;
/// Interface version this root was built for.
const INTERFACE_VERSION: u16 = 1;
/// Returned by the dependency's probe after its framing self-check.
const EXPECTED_PROBE_MAGIC: u32 = 0x4d32_4d31;
/// Expected encoded probe length and transport maximum.
const EXPECTED_RECORD_LEN: u32 = 340;
const EXPECTED_MAX_RECORD_LEN: u32 = 64 * 1024;

/// The dependency's versioned C table. This is a layout copy, not a link
/// against `mesh-transport`; the table crosses the runtime interface boundary.
#[repr(C)]
struct MeshTransportApi {
    abi: u32,
    probe: unsafe extern "C" fn() -> u32,
    record_len: unsafe extern "C" fn() -> u32,
    max_record_len: unsafe extern "C" fn() -> u32,
    check_bound: unsafe extern "C" fn(u32) -> i32,
}

static VTABLE_OK: ModuleVtable = ModuleVtable {
    status: 0,
    tick: None,
};

static VTABLE_FAILED: ModuleVtable = ModuleVtable {
    status: 1,
    tick: None,
};

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
    if services.version < module_abi::ABI_VERSION {
        fail(services, b"services", services.version as i32);
        return &VTABLE_FAILED;
    }

    let interface = b"irix7.mesh.transport";
    let mut binding = ModuleExportBinding::EMPTY;
    let status = unsafe {
        (services.module_find_export)(
            interface.as_ptr(),
            interface.len() as u32,
            INTERFACE_VERSION,
            &mut binding,
        )
    };
    if status != MODULE_OK {
        fail(services, b"find_export", status);
        return &VTABLE_FAILED;
    }
    if binding.table.is_null() {
        fail(services, b"table", -1);
        return &VTABLE_FAILED;
    }
    if binding.version != INTERFACE_VERSION {
        fail(services, b"version", binding.version as i32);
        return &VTABLE_FAILED;
    }
    if binding.handle == 0 || (binding.handle & 0xffff_ffff) as u32 != MESH_REGION {
        fail(services, b"owner", -2);
        return &VTABLE_FAILED;
    }

    let mut info = ModuleInfo::EMPTY;
    let status = unsafe { (services.module_query)(binding.handle, &mut info) };
    if status != MODULE_OK {
        fail(services, b"owner_query", status);
        return &VTABLE_FAILED;
    }
    if info.region != MESH_REGION {
        fail(services, b"owner_region", info.region as i32);
        return &VTABLE_FAILED;
    }
    let base = info.base as usize;
    let table = binding.table as usize;
    let Some(end) = base.checked_add(info.image_size as usize) else {
        fail(services, b"owner_range", -3);
        return &VTABLE_FAILED;
    };
    let Some(table_end) = table.checked_add(4) else {
        fail(services, b"table_range", -3);
        return &VTABLE_FAILED;
    };
    if table < base || table_end > end {
        fail(services, b"table_range", -3);
        return &VTABLE_FAILED;
    }

    // SAFETY: module_load registered this table at the manifest offset inside
    // the verified dependency image; the owner and range checks above prove it
    // is still live and in region 2.
    let api = unsafe { &*(binding.table as *const MeshTransportApi) };
    if api.abi != INTERFACE_VERSION as u32 {
        fail(services, b"abi", api.abi as i32);
        return &VTABLE_FAILED;
    }
    let magic = unsafe { (api.probe)() };
    if magic != EXPECTED_PROBE_MAGIC {
        fail(services, b"probe", magic as i32);
        return &VTABLE_FAILED;
    }
    let record_len = unsafe { (api.record_len)() };
    if record_len != EXPECTED_RECORD_LEN {
        fail(services, b"record_len", record_len as i32);
        return &VTABLE_FAILED;
    }
    let max = unsafe { (api.max_record_len)() };
    if max != EXPECTED_MAX_RECORD_LEN {
        fail(services, b"max_record_len", max as i32);
        return &VTABLE_FAILED;
    }
    let bound_ok = unsafe { (api.check_bound)(EXPECTED_MAX_RECORD_LEN) };
    let bound_over = unsafe { (api.check_bound)(EXPECTED_MAX_RECORD_LEN + 1) };
    let bound_short = unsafe { (api.check_bound)(319) };
    if (bound_ok, bound_over, bound_short) != (0, -1, -2) {
        fail(services, b"bounds", bound_over);
        return &VTABLE_FAILED;
    }

    let mut line = Line::new();
    line.str(b"m2-root: dep=m2-mesh@1.0.0 interface=irix7.mesh.transport version=1 owner=0x");
    line.hex_u64(binding.handle);
    line.str(b" result=ok magic=0x");
    line.hex_u32(magic);
    line.str(b" len=");
    line.dec(record_len);
    line.str(b" max=");
    line.dec(max);
    line.str(b" bounds=0/-1/-2\n");
    write(services, line.slice());
    &VTABLE_OK
}

fn fail(services: &Services, stage: &[u8], value: i32) {
    let mut line = Line::new();
    line.str(b"m2-root: dep=m2-mesh@1.0.0 result=fail stage=");
    line.str(stage);
    line.str(b" value=");
    line.dec_signed(value);
    line.str(b"\n");
    write(services, line.slice());
}

fn write(services: &Services, bytes: &[u8]) {
    (services.write)(bytes.as_ptr(), bytes.len());
}

struct Line {
    buf: [u8; 256],
    len: usize,
}

impl Line {
    fn new() -> Self {
        Self {
            buf: [0u8; 256],
            len: 0,
        }
    }

    fn push(&mut self, byte: u8) {
        if self.len < self.buf.len() {
            self.buf[self.len] = byte;
            self.len += 1;
        }
    }

    fn str(&mut self, bytes: &[u8]) {
        for &byte in bytes {
            self.push(byte);
        }
    }

    fn dec(&mut self, value: u32) {
        let mut digits = [0u8; 10];
        let mut index = digits.len();
        let mut value = value;
        loop {
            index -= 1;
            digits[index] = b'0' + (value % 10) as u8;
            value /= 10;
            if value == 0 {
                break;
            }
        }
        self.str(&digits[index..]);
    }

    fn dec_signed(&mut self, value: i32) {
        if value < 0 {
            self.push(b'-');
            self.dec(value.unsigned_abs());
        } else {
            self.dec(value as u32);
        }
    }

    fn hex_u32(&mut self, value: u32) {
        for shift in (0..8).rev() {
            self.push(HEX[((value >> (shift * 4)) & 0xf) as usize]);
        }
    }

    fn hex_u64(&mut self, value: u64) {
        for shift in (0..16).rev() {
            self.push(HEX[((value >> (shift * 4)) & 0xf) as usize]);
        }
    }

    fn slice(&self) -> &[u8] {
        &self.buf[..self.len]
    }
}

const HEX: &[u8; 16] = b"0123456789abcdef";

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
    fn evidence_line_has_the_expected_shape() {
        let mut line = Line::new();
        line.str(b"m2-root: dep=m2-mesh@1.0.0 ... owner=0x");
        line.hex_u64(0x0000_0001_0000_0002);
        line.str(b" result=ok magic=0x");
        line.hex_u32(EXPECTED_PROBE_MAGIC);
        line.str(b" len=");
        line.dec(EXPECTED_RECORD_LEN);
        line.str(b" max=");
        line.dec(EXPECTED_MAX_RECORD_LEN);
        line.str(b" bounds=0/-1/-2\n");
        assert_eq!(
            core::str::from_utf8(line.slice()).expect("line"),
            "m2-root: dep=m2-mesh@1.0.0 ... owner=0x0000000100000002 result=ok \
             magic=0x4d324d31 len=340 max=65536 bounds=0/-1/-2\n"
        );
    }

    #[test]
    fn signed_decimal_formatting_covers_failure_values() {
        let mut line = Line::new();
        line.dec_signed(-7);
        line.push(b'/');
        line.dec_signed(0);
        assert_eq!(line.slice(), b"-7/0");
    }
}
