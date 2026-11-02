// SPDX-License-Identifier: GPL-3.0-only
//! The shared service table must behave the same on every machine.
//!
//! This is the host gate for the dispatch layer: argument guards, refusals and
//! forwarding are exercised once against a mock platform.  The real machines
//! only supply the platform; they no longer own the ABI.

use std::sync::atomic::{AtomicU32, AtomicU64, AtomicUsize, Ordering};
use std::sync::Mutex;

use kernel_core::service::{self, Platform, SECTOR, SERVICES};
use module_abi::{
    ModuleArena, ModuleContext, ModuleExportBinding, ModuleHandle, ModuleInfo, ModuleLoad,
    ABI_VERSION,
};

static WRITTEN: Mutex<Vec<u8>> = Mutex::new(Vec::new());
static CLOCK: AtomicU64 = AtomicU64::new(1234);
static POLLS: AtomicUsize = AtomicUsize::new(0);
static TCP_LISTEN: AtomicU32 = AtomicU32::new(0);
static TCP_CONNECT: AtomicU32 = AtomicU32::new(0);
static TCP_SENDS: AtomicUsize = AtomicUsize::new(0);
static UDP_SENDS: AtomicUsize = AtomicUsize::new(0);
static RESETS: AtomicUsize = AtomicUsize::new(0);
static POWEROFFS: AtomicUsize = AtomicUsize::new(0);
static BLOCK_TARGET: AtomicU32 = AtomicU32::new(0);
static BLOCK_LBA: AtomicU32 = AtomicU32::new(0);
static BLOCK_WRITES: AtomicUsize = AtomicUsize::new(0);

static mut HEAP: [u64; 8] = [0; 8];

struct Mock;

impl Platform for Mock {
    fn write(&self, bytes: &[u8]) {
        WRITTEN.lock().unwrap().extend_from_slice(bytes);
    }

    fn clock_ms(&self) -> u64 {
        CLOCK.load(Ordering::Relaxed)
    }

    fn alloc(&self, layout: core::alloc::Layout) -> *mut u8 {
        if layout.size() <= core::mem::size_of::<[u64; 8]>() {
            core::ptr::addr_of_mut!(HEAP) as *mut u8
        } else {
            core::ptr::null_mut()
        }
    }

    fn poll(&self) {
        POLLS.fetch_add(1, Ordering::Relaxed);
    }

    fn tcp_listen(&self, slot: u32, port: u16) -> i32 {
        TCP_LISTEN.store((slot << 16) | u32::from(port), Ordering::Relaxed);
        0
    }

    fn tcp_connect(&self, slot: u32, address: u32, port: u16) -> i32 {
        TCP_CONNECT.store(address ^ (slot << 16) ^ u32::from(port), Ordering::Relaxed);
        0
    }

    fn tcp_state(&self, slot: u32) -> u32 {
        slot + 2
    }

    fn tcp_recv(&self, _slot: u32, out: &mut [u8]) -> isize {
        out.fill(b'r');
        out.len() as isize
    }

    fn tcp_send(&self, _slot: u32, bytes: &[u8]) -> isize {
        TCP_SENDS.fetch_add(bytes.len(), Ordering::Relaxed);
        bytes.len() as isize
    }

    fn tcp_events(&self, slot: u32) -> u32 {
        1 << slot
    }

    fn tcp_slots(&self) -> u32 {
        8
    }

    fn console_read(&self, offset: u64, out: &mut [u8]) -> isize {
        out.fill(b'c');
        (offset + out.len() as u64) as isize
    }

    fn console_total(&self) -> u64 {
        4096
    }

    fn reset(&self) {
        RESETS.fetch_add(1, Ordering::Relaxed);
    }

    fn poweroff(&self) {
        POWEROFFS.fetch_add(1, Ordering::Relaxed);
    }

    fn status(&self, out: &mut [u8]) -> isize {
        let text = b"mock: status\n";
        let count = text.len().min(out.len());
        out[..count].copy_from_slice(&text[..count]);
        count as isize
    }

    fn config_get(&self, key: &[u8], out: &mut [u8]) -> isize {
        if key == b"test.key" {
            let value = b"config-ok";
            let count = value.len().min(out.len());
            out[..count].copy_from_slice(&value[..count]);
            count as isize
        } else {
            -1
        }
    }

    fn info(&self, kind: u32, out: &mut [u8]) -> isize {
        let text: &[u8] = match kind {
            0 => b"mock: machine\n",
            1 => b"mock: net\n",
            _ => return -1,
        };
        let count = text.len().min(out.len());
        out[..count].copy_from_slice(&text[..count]);
        count as isize
    }

    fn screen_read(&self, _offset: u32, out: &mut [u8]) -> isize {
        out.fill(0x5a);
        out.len() as isize
    }

    fn block_read(&self, target: u8, lba: u32, out: &mut [u8; SECTOR]) -> i32 {
        BLOCK_TARGET.store(u32::from(target), Ordering::Relaxed);
        BLOCK_LBA.store(lba, Ordering::Relaxed);
        out.fill(0xa5);
        0
    }

    fn block_write(&self, target: u8, lba: u32, data: &[u8; SECTOR]) -> i32 {
        BLOCK_TARGET.store(u32::from(target), Ordering::Relaxed);
        BLOCK_LBA.store(lba, Ordering::Relaxed);
        if data.iter().all(|byte| *byte == 0x3c) {
            BLOCK_WRITES.fetch_add(1, Ordering::Relaxed);
            0
        } else {
            -1
        }
    }

    fn module_arena(&self, out: &mut ModuleArena) -> i32 {
        out.region_count = 3;
        out.region_size = 4096;
        0
    }

    fn module_load(&self, _request: &ModuleLoad, out: &mut ModuleHandle) -> i32 {
        *out = 7;
        0
    }

    fn module_execute(
        &self,
        _handle: ModuleHandle,
        _entry_offset: u32,
        _context: Option<&ModuleContext>,
        status: Option<&mut i32>,
    ) -> i32 {
        if let Some(status) = status {
            *status = 9;
        }
        0
    }

    fn module_find(&self, name: &[u8], version: &[u8], out: &mut ModuleHandle) -> i32 {
        if name == b"mcp" && version == b"1" {
            *out = 11;
            0
        } else {
            -9
        }
    }

    fn module_find_export(
        &self,
        interface: &[u8],
        version: u16,
        out: &mut ModuleExportBinding,
    ) -> i32 {
        if interface == b"iface" && version == 2 {
            out.version = version;
            0
        } else {
            -9
        }
    }

    fn module_query(&self, handle: ModuleHandle, out: &mut ModuleInfo) -> i32 {
        out.handle = handle;
        out.state = 2;
        0
    }

    fn module_unload(&self, _handle: ModuleHandle) -> i32 {
        0
    }

    fn udp_open(&self, slot: u32, port: u16) -> i32 {
        if slot < 8 && port != 0 {
            0
        } else {
            -1
        }
    }

    fn udp_send(&self, _slot: u32, _address: u32, _port: u16, bytes: &[u8]) -> isize {
        UDP_SENDS.fetch_add(bytes.len(), Ordering::Relaxed);
        bytes.len() as isize
    }

    fn udp_recv(
        &self,
        _slot: u32,
        out: &mut [u8],
        src_address: Option<&mut u32>,
        src_port: Option<&mut u16>,
    ) -> isize {
        out.fill(b'u');
        if let Some(address) = src_address {
            *address = 0x0a00_0202;
        }
        if let Some(port) = src_port {
            *port = 53;
        }
        out.len() as isize
    }

    fn udp_slots(&self) -> u32 {
        8
    }

    fn udp_state(&self, _slot: u32, address: Option<&mut u32>, port: Option<&mut u16>) -> i32 {
        if let Some(address) = address {
            *address = 0x0a00_020f;
        }
        if let Some(port) = port {
            *port = 80;
        }
        0
    }
}

static MOCK: Mock = Mock;

#[test]
fn the_service_table_is_one_shared_contract() {
    service::install(&MOCK);

    assert_eq!(SERVICES.version, ABI_VERSION);
    assert_eq!((SERVICES.clock_ms)(), 1234);

    // Console writes are bounded and null-safe.
    (SERVICES.write)(core::ptr::null(), 4);
    (SERVICES.write)(b"unbounded".as_ptr(), service::MAX_WRITE + 1);
    (SERVICES.write)(b"hello".as_ptr(), 5);
    assert_eq!(&*WRITTEN.lock().unwrap(), b"hello");

    // Allocation rejects zero and out-of-heap requests instead of handing out
    // a pointer the platform did not provide.
    assert!(unsafe { (SERVICES.alloc)(0, 1) }.is_null());
    assert!(unsafe { (SERVICES.alloc)(64 * 1024, 8) }.is_null());
    assert!(!unsafe { (SERVICES.alloc)(16, 8) }.is_null());

    (SERVICES.poll)();
    assert_eq!(POLLS.load(Ordering::Relaxed), 1);

    // Configuration keys are bounded before the platform sees them.
    let mut value = [0u8; 16];
    assert_eq!(
        unsafe { (SERVICES.config_get)(b"test.key".as_ptr(), 8, value.as_mut_ptr(), value.len()) },
        9
    );
    assert_eq!(&value[..9], b"config-ok");
    assert_eq!(
        unsafe {
            (SERVICES.config_get)(
                b"k".as_ptr(),
                service::MAX_CONFIG_KEY + 1,
                value.as_mut_ptr(),
                16,
            )
        },
        -1
    );
    assert_eq!(
        unsafe { (SERVICES.config_get)(core::ptr::null(), 1, value.as_mut_ptr(), 16) },
        -1
    );

    let mut info = [0u8; 24];
    assert_eq!(
        unsafe { (SERVICES.info)(1, info.as_mut_ptr(), info.len()) },
        10
    );
    assert_eq!(&info[..10], b"mock: net\n");
    assert_eq!(
        unsafe { (SERVICES.info)(9, info.as_mut_ptr(), info.len()) },
        -1
    );

    let mut status = [0u8; 32];
    assert_eq!(
        unsafe { (SERVICES.status)(status.as_mut_ptr(), status.len()) },
        13
    );
    assert_eq!(&status[..13], b"mock: status\n");

    let mut console = [0u8; 4];
    assert_eq!(
        unsafe { (SERVICES.console_read)(7, console.as_mut_ptr(), console.len()) },
        11
    );
    assert_eq!(console, [b'c'; 4]);
    assert_eq!((SERVICES.console_total)(), 4096);

    let mut sector = [0u8; SECTOR];
    assert_eq!(
        unsafe { (SERVICES.block_read)(3, 64, sector.as_mut_ptr()) },
        0
    );
    assert_eq!(BLOCK_TARGET.load(Ordering::Relaxed), 3);
    assert_eq!(BLOCK_LBA.load(Ordering::Relaxed), 64);
    assert_eq!(sector, [0xa5; SECTOR]);

    sector.fill(0x3c);
    assert_eq!(unsafe { (SERVICES.block_write)(5, 65, sector.as_ptr()) }, 0);
    assert_eq!(BLOCK_WRITES.load(Ordering::Relaxed), 1);

    // TCP and UDP slot entry points forward the machine policy unchanged.
    assert_eq!(unsafe { (SERVICES.tcp_listen)(2, 80) }, 0);
    assert_eq!(TCP_LISTEN.load(Ordering::Relaxed), (2 << 16) | 80);
    assert_eq!(unsafe { (SERVICES.tcp_connect)(4, 0x0a00_0202, 443) }, 0);
    assert_eq!(
        TCP_CONNECT.load(Ordering::Relaxed),
        0x0a00_0202 ^ (4 << 16) ^ 443
    );
    assert_eq!((SERVICES.tcp_state)(1), 3);
    assert_eq!((SERVICES.tcp_events)(3), 8);
    assert_eq!((SERVICES.tcp_slots)(), 8);
    let mut rx = [0u8; 8];
    assert_eq!(
        unsafe { (SERVICES.tcp_recv)(0, rx.as_mut_ptr(), rx.len()) },
        8
    );
    assert_eq!(rx, [b'r'; 8]);
    assert_eq!(unsafe { (SERVICES.tcp_send)(0, b"abcde".as_ptr(), 5) }, 5);
    assert_eq!(TCP_SENDS.load(Ordering::Relaxed), 5);
    (SERVICES.tcp_close)(0);
    (SERVICES.tcp_abort)(1);

    assert_eq!(unsafe { (SERVICES.udp_open)(0, 53) }, 0);
    assert_eq!(unsafe { (SERVICES.udp_open)(9, 53) }, -1);
    assert_eq!(
        unsafe { (SERVICES.udp_send)(0, 0x0a00_0202, 53, b"query".as_ptr(), 5) },
        5
    );
    assert_eq!(UDP_SENDS.load(Ordering::Relaxed), 5);
    let mut udp = [0u8; 4];
    let mut address = 0u32;
    let mut port = 0u16;
    assert_eq!(
        unsafe { (SERVICES.udp_recv)(0, udp.as_mut_ptr(), udp.len(), &mut address, &mut port,) },
        4
    );
    assert_eq!(udp, [b'u'; 4]);
    assert_eq!(address, 0x0a00_0202);
    assert_eq!(port, 53);
    assert_eq!((SERVICES.udp_slots)(), 8);
    assert_eq!(
        unsafe { (SERVICES.udp_state)(0, &mut address, &mut port) },
        0
    );
    assert_eq!(address, 0x0a00_020f);
    assert_eq!(port, 80);
    (SERVICES.udp_close)(0);

    // Module arena and manager entries use typed descriptors, not raw kernel
    // pointers, across the boundary.
    let mut arena = ModuleArena::EMPTY;
    assert_eq!(unsafe { (SERVICES.module_arena)(&mut arena) }, 0);
    assert_eq!(arena.region_count, 3);
    assert_eq!(arena.region_size, 4096);

    let request = ModuleLoad::EMPTY;
    let mut handle = 0;
    assert_eq!(unsafe { (SERVICES.module_load)(&request, &mut handle) }, 0);
    assert_eq!(handle, 7);

    let mut status = -1;
    assert_eq!(
        unsafe { (SERVICES.module_execute)(handle, 0, core::ptr::null(), &mut status,) },
        0
    );
    assert_eq!(status, 9);

    let mut found = 0;
    assert_eq!(
        unsafe { (SERVICES.module_find)(b"mcp".as_ptr(), 3, b"1".as_ptr(), 1, &mut found) },
        0
    );
    assert_eq!(found, 11);
    assert_eq!(
        unsafe { (SERVICES.module_find)(b"x".as_ptr(), 1, b"1".as_ptr(), 1, &mut found) },
        -9
    );

    let mut binding = ModuleExportBinding::EMPTY;
    assert_eq!(
        unsafe { (SERVICES.module_find_export)(b"iface".as_ptr(), 5, 2, &mut binding) },
        0
    );
    assert_eq!(binding.version, 2);

    let mut queried = ModuleInfo::EMPTY;
    assert_eq!(unsafe { (SERVICES.module_query)(handle, &mut queried) }, 0);
    assert_eq!(queried.handle, handle);
    assert_eq!(queried.state, 2);
    assert_eq!(unsafe { (SERVICES.module_unload)(handle) }, 0);

    // Screen and lifecycle entries forward to the machine.
    let mut pixels = [0u8; 8];
    assert_eq!(
        unsafe { (SERVICES.screen_read)(0, pixels.as_mut_ptr(), pixels.len()) },
        8
    );
    assert_eq!(pixels, [0x5a; 8]);
    (SERVICES.reset)();
    (SERVICES.poweroff)();
    assert_eq!(RESETS.load(Ordering::Relaxed), 1);
    assert_eq!(POWEROFFS.load(Ordering::Relaxed), 1);
}
