// SPDX-License-Identifier: GPL-3.0-only
//! ROM network stage: bring the Seeq up, take a DHCP lease and download the
//! kernel over HTTPS (M1-ADR-0002).
//!
//! No statics: the ROM does not clear `.bss` before the RAM test.  All state
//! lives in this function's frame, or in the RAM window the allocator owns.

use core::alloc::{GlobalAlloc, Layout};

use irix7_seeq::{SeeqDevice, SEEQ_AREA};
use smoltcp::iface::{Config, Interface, SocketSet, SocketStorage};
use smoltcp::socket::{dhcpv4, dns};
use smoltcp::time::Instant;
use smoltcp::wire::{EthernetAddress, HardwareAddress, IpAddress, IpCidr};

use crate::fetch::{self, Stack};

extern "C" {
    /// `serial_putc` in reset.S: send the byte in `$a0`.
    fn serial_putc(byte: u8);
}

/// CP0 Count frequency assumed by the kernel and iris (33 MHz).
const COUNT_PER_MS: u64 = 33_000;
/// How long DHCP may take before the stage gives up.
const DHCP_TIMEOUT_MS: u64 = 15_000;
/// Station address until the NVRAM read is wired in; locally administered.
const STATION_MAC: [u8; 6] = [0x02, 0x00, 0x69, 0x12, 0x34, 0x56];

/// The kernel is downloaded to kseg1 0xA8040000 (physical 0x08040000) and runs
/// from its kseg0 alias 0x88040000.  The window ends where the heap starts.
const KERNEL_DST: usize = 0xA804_0000;
const KERNEL_WINDOW: usize = 0x0030_0000;
/// Kseg1 heap for TLS records, TCP buffers and the certificate chain.  It sits
/// between the kernel window and the module area at physical 0x08400000.  The
/// allocator keeps its cursor in the last word of the heap.
const HEAP_BASE: usize = 0xA834_0000;
const HEAP_END: usize = 0xA840_0000;
const HEAP_CURSOR: usize = HEAP_END - 8;

/// Release asset with a stable name.  `releases/latest/download` resolves to
/// the newest release that carries it.
const KERNEL_URL: &str =
    "https://github.com/irix7/os/releases/latest/download/kernel-indy-latest.bin";

/// Bump allocator over the stage's heap.  Blocks are never freed; the stage
/// is short and runs once.  The cursor is a RAM word, not a static, because the
/// ROM does not copy `.data` or clear `.bss`.
struct StageHeap;

// SAFETY: the stage is single-threaded, and every block lies in the heap window.
unsafe impl GlobalAlloc for StageHeap {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let cursor = core::ptr::read_volatile(HEAP_CURSOR as *const usize);
        let align = layout.align();
        let start = (cursor + align - 1) & !(align - 1);
        let Some(stop) = start.checked_add(layout.size()) else {
            return core::ptr::null_mut();
        };
        if stop > HEAP_CURSOR {
            return core::ptr::null_mut();
        }
        core::ptr::write_volatile(HEAP_CURSOR as *mut usize, stop);
        start as *mut u8
    }

    unsafe fn dealloc(&self, _ptr: *mut u8, _layout: Layout) {}
}

#[global_allocator]
static HEAP: StageHeap = StageHeap;

/// Zeroed buffer of `len` bytes from the stage heap, valid for the rest of the
/// stage.  `None` when the heap is full.
pub(crate) fn buffer(len: usize) -> Option<&'static mut [u8]> {
    let layout = Layout::from_size_align(len, 8).ok()?;
    // SAFETY: the allocator returns disjoint blocks inside the heap window.
    let ptr = unsafe { alloc::alloc::alloc_zeroed(layout) };
    if ptr.is_null() {
        return None;
    }
    // SAFETY: the block is `len` bytes, zeroed, and never handed out again.
    Some(unsafe { core::slice::from_raw_parts_mut(ptr, len) })
}

pub(crate) fn put(text: &str) {
    for byte in text.bytes() {
        // SAFETY: serial_putc is the ROM's own routine and takes a byte in $a0.
        unsafe { serial_putc(byte) };
    }
}

pub(crate) fn put_dec(mut value: u32) {
    let mut digits = [0u8; 10];
    let mut count = 0;
    loop {
        digits[count] = b'0' + (value % 10) as u8;
        count += 1;
        value /= 10;
        if value == 0 {
            break;
        }
    }
    while count > 0 {
        count -= 1;
        // SAFETY: as above.
        unsafe { serial_putc(digits[count]) };
    }
}

/// Raw CP0 Count, which runs at half the CPU clock.
pub(crate) fn count() -> u32 {
    let value: u32;
    // SAFETY: reading CP0 Count has no side effects.
    unsafe { core::arch::asm!("mfc0 {0}, $9", out(reg) value) };
    value
}

/// Monotonic time from CP0 Count.
pub(crate) fn now() -> Instant {
    Instant::from_millis((count() as u64 / COUNT_PER_MS) as i64)
}

/// Bring up the Seeq, run DHCP and download the kernel.
///
/// Called from reset.S once the RAM test has passed.  Returns 0 when the kernel
/// is in RAM at 0xA8040000, and 1 otherwise.  The caller halts on 1.
#[no_mangle]
pub extern "C" fn rom_netstage() -> u32 {
    // SAFETY: the heap window is RAM the stage owns; the cursor word lives there.
    unsafe { core::ptr::write_volatile(HEAP_CURSOR as *mut usize, HEAP_BASE) };
    put("rom-net: seeq\n");
    let Some(dev) = SeeqDevice::new(STATION_MAC) else {
        put("rom-net: no seeq\n");
        return 1;
    };
    // SAFETY: SEEQ_AREA is the uncached DMA window reserved for the device; the
    // stage owns it until the kernel takes over.
    let seeq = unsafe {
        let slot = SEEQ_AREA as *mut SeeqDevice;
        core::ptr::write(slot, dev);
        &mut *slot
    };
    seeq.start();

    let mut iface = Interface::new(
        Config::new(HardwareAddress::Ethernet(EthernetAddress(STATION_MAC))),
        seeq,
        now(),
    );
    // DHCP, DNS and the TCP connection: at most one of each at a time.
    let mut storage = [SocketStorage::EMPTY; 3];
    let mut sockets = SocketSet::new(&mut storage[..]);
    let dhcp = sockets.add(dhcpv4::Socket::new());
    let mut queries: [Option<dns::DnsQuery>; 2] = [None, None];

    let start = now();
    loop {
        let t = now();
        iface.poll(t, seeq, &mut sockets);
        if let Some(dhcpv4::Event::Configured(config)) = sockets.get_mut::<dhcpv4::Socket>(dhcp).poll() {
            put_lease(&config);
            // Apply the lease: the address and the default route, so the DNS
            // server and the origin server are reachable.
            iface.update_ip_addrs(|addrs| {
                addrs.clear();
                let _ = addrs.push(IpCidr::Ipv4(config.address));
            });
            if let Some(router) = config.router {
                let _ = iface.routes_mut().add_default_ipv4_route(router);
            }
            let Some(server) = config.dns_servers.first() else {
                put("rom-net: no dns server\n");
                return 1;
            };
            let servers = [IpAddress::Ipv4(*server)];
            let dns = sockets.add(dns::Socket::new(&servers[..], &mut queries[..]));
            let mut stack = Stack {
                iface,
                dev: seeq,
                sockets,
                dns,
            };
            // SAFETY: the window is RAM the ROM owns until the kernel runs.
            let out = unsafe { core::slice::from_raw_parts_mut(KERNEL_DST as *mut u8, KERNEL_WINDOW) };
            match fetch::download(&mut stack, KERNEL_URL, out) {
                Ok(n) => {
                    put("rom-net: kernel bytes ");
                    put_dec(n as u32);
                    put("\n");
                    return 0;
                }
                Err(reason) => {
                    put("rom-net: kernel download failed: ");
                    put(reason);
                    put("\n");
                    return 1;
                }
            }
        }
        if (t - start).total_millis() > DHCP_TIMEOUT_MS {
            put("rom-net: no lease\n");
            return 1;
        }
    }
}

fn put_lease(config: &dhcpv4::Config) {
    let [a, b, c, d] = config.address.address().octets();
    put("rom-net: lease ");
    put_dec(a as u32);
    put(".");
    put_dec(b as u32);
    put(".");
    put_dec(c as u32);
    put(".");
    put_dec(d as u32);
    put("/");
    put_dec(config.address.prefix_len() as u32);
    put("\n");
}

// The ROM links without compiler-builtins-mem (CI's ROM command), so the
// memory routines smoltcp and the Seeq driver call are provided here.
#[no_mangle]
pub unsafe extern "C" fn memset(dst: *mut u8, value: i32, len: usize) -> *mut u8 {
    for i in 0..len {
        core::ptr::write_volatile(dst.add(i), value as u8);
    }
    dst
}

#[no_mangle]
pub unsafe extern "C" fn memcpy(dst: *mut u8, src: *const u8, len: usize) -> *mut u8 {
    for i in 0..len {
        core::ptr::write_volatile(dst.add(i), core::ptr::read_volatile(src.add(i)));
    }
    dst
}

#[no_mangle]
pub unsafe extern "C" fn memmove(dst: *mut u8, src: *const u8, len: usize) -> *mut u8 {
    if (dst as usize) <= (src as usize) {
        memcpy(dst, src, len)
    } else {
        for i in (0..len).rev() {
            core::ptr::write_volatile(dst.add(i), core::ptr::read_volatile(src.add(i)));
        }
        dst
    }
}

#[no_mangle]
pub unsafe extern "C" fn memcmp(a: *const u8, b: *const u8, len: usize) -> i32 {
    for i in 0..len {
        let (x, y) = (*a.add(i), *b.add(i));
        if x != y {
            return x as i32 - y as i32;
        }
    }
    0
}
