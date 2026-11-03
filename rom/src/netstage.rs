// SPDX-License-Identifier: GPL-3.0-only
//! ROM network stage: bring the Seeq up and take a DHCP lease, before the
//! kernel is loaded.  M1-ADR-0002 moves the kernel download into this stage;
//! this is its first piece.
//!
//! No statics: the ROM does not clear `.bss` before the RAM test.  All state
//! lives in this function's frame, which the assembly stub places in RAM.

use irix7_seeq::{SeeqDevice, SEEQ_AREA};
use smoltcp::iface::{Config, Interface, SocketSet, SocketStorage};
use smoltcp::socket::dhcpv4;
use smoltcp::time::Instant;
use smoltcp::wire::{EthernetAddress, HardwareAddress};

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

fn put(text: &str) {
    for byte in text.bytes() {
        // SAFETY: serial_putc is the ROM's own routine and takes a byte in $a0.
        unsafe { serial_putc(byte) };
    }
}

fn put_dec(mut value: u32) {
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

/// Monotonic time from CP0 Count, which runs at half the CPU clock.
fn now() -> Instant {
    let count: u32;
    // SAFETY: reading CP0 Count has no side effects.
    unsafe { core::arch::asm!("mfc0 {0}, $9", out(reg) count) };
    Instant::from_millis((count as u64 / COUNT_PER_MS) as i64)
}

/// Bring up the Seeq, run DHCP and report the lease on the serial console.
///
/// Called from reset.S once the RAM test has passed.  Returns in every case:
/// the kernel path must not depend on the network stage succeeding.
#[no_mangle]
pub extern "C" fn rom_netstage() {
    put("rom-net: seeq\n");
    let Some(dev) = SeeqDevice::new(STATION_MAC) else {
        put("rom-net: no seeq\n");
        return;
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
    let mut storage = [SocketStorage::EMPTY; 1];
    let mut sockets = SocketSet::new(&mut storage[..]);
    let dhcp = sockets.add(dhcpv4::Socket::new());

    let start = now();
    loop {
        let t = now();
        iface.poll(t, seeq, &mut sockets);
        if let Some(dhcpv4::Event::Configured(config)) = sockets.get_mut::<dhcpv4::Socket>(dhcp).poll() {
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
            return;
        }
        if (t - start).total_millis() > DHCP_TIMEOUT_MS {
            put("rom-net: no lease\n");
            return;
        }
    }
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
