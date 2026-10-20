//! ROM entry point: initialise the serial console and Seeq, print the bring-up
//! marker, run DHCP, resolve the R2 endpoint, fetch the kernel via HTTP/1.1,
//! verify its SHA-256 digest, build a BootInfo, flush caches, and jump to the
//! kernel entry point.
//!
//! Built for `mips2-sgi-irix7-o32` (o32/MIPS II) with `-Z build-std`. The PROM
//! (ARCS) or iris loads this ELF and jumps to `_start`; this crate takes over
//! from the first instruction.

#![cfg_attr(not(test), no_main)]
#![no_std]

use core::panic::PanicInfo;
use rom::boot::{build_bootinfo, BootInfo, MEM_MAP_ENTRY_SIZE, MEM_MAP_USABLE_RAM};
use rom::net::{dhcpv4_configure, SeeqDevice};
use rom::sha256::Sha256;
use rom::{serial_init, serial_write_byte, serial_write_str};

fn hex_nibble(n: u8) -> u8 {
    match n {
        0..=9 => b'0' + n,
        _ => b'A' + (n - 10),
    }
}

fn print_hex8(v: u8) {
    serial_write_byte(hex_nibble(v >> 4));
    serial_write_byte(hex_nibble(v & 0x0F));
}

#[cfg(not(test))]
#[no_mangle]
pub extern "C" fn _start() -> ! {
    // The PROM (ARCS) or iris loader sets $sp on entry to a valid stack in RAM.

    serial_init();
    serial_write_str("hello from ROM\r\n");

    // Initialise the Seeq Ethernet controller.
    let mut seeq = match SeeqDevice::new() {
        Some(s) => s,
        None => {
            serial_write_str("seeq: init failed (bad MAC)\r\n");
            loop {
                core::hint::spin_loop();
            }
        }
    };
    serial_write_str("seeq: mac ");
    for (i, b) in seeq.mac_address().as_bytes().iter().enumerate() {
        if i > 0 {
            serial_write_byte(b':');
        }
        print_hex8(*b);
    }
    serial_write_str("\r\n");

    // Run DHCPv4 to obtain an IP address.
    serial_write_str("seeq: dhcp...");
    match dhcpv4_configure(&mut seeq) {
        Some(ip) => {
            serial_write_str("ok, ip ");
            for (i, octet) in ip.address().octets().iter().enumerate() {
                if i > 0 {
                    serial_write_byte(b'.');
                }
                for nibble in (0..4).rev() {
                    print_hex8((*octet >> (nibble * 4)) & 0x0F);
                }
            }
            serial_write_str("\r\n");
        }
        None => serial_write_str("failed\r\n"),
    }

    // Hang: HTTP fetch, SHA-256 verification, BootInfo construction, and
    // cache flush + jump to kernel are follow-up steps (issue #2).
    loop {
        core::hint::spin_loop();
    }
}

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    serial_write_str("ROM panic\r\n");
    loop {
        core::hint::spin_loop();
    }
}

/// Minimal memory operations required by smoltcp and core when compiled for a
/// freestanding target without libc. compiler_builtins does not always emit
/// these as linkable symbols on MIPS targets.
use core::ptr::{read_volatile, write_volatile};

use core::ffi::c_void;

/// Copy `n` bytes from `src` to `dst`, handling overlap correctly.
#[no_mangle]
pub unsafe extern "C" fn memcpy(dst: *mut c_void, src: *const c_void, n: usize) -> *mut c_void {
    let mut dst = dst as *mut u8;
    let src = src as *const u8;
    if (dst as usize) < (src as usize) {
        for i in 0..n {
            write_volatile(dst.add(i), read_volatile(src.add(i)));
        }
    } else {
        let mut i = n;
        while i > 0 {
            i -= 1;
            write_volatile(dst.add(i), read_volatile(src.add(i)));
        }
    }
    dst as *mut c_void
}

/// Fill `n` bytes of `dst` with `val`.
#[no_mangle]
pub unsafe extern "C" fn memset(dst: *mut c_void, val: i32, n: usize) -> *mut c_void {
    let mut dst = dst as *mut u8;
    let v = val as u8;
    for i in 0..n {
        write_volatile(dst.add(i), v);
    }
    dst as *mut c_void
}

/// Compare `n` bytes at `s1` and `s2`, returning `< 0`, `0`, or `> 0`.
#[no_mangle]
pub unsafe extern "C" fn memcmp(s1: *const c_void, s2: *const c_void, n: usize) -> i32 {
    let s1 = s1 as *const u8;
    let s2 = s2 as *const u8;
    for i in 0..n {
        let a = read_volatile(s1.add(i));
        let b = read_volatile(s2.add(i));
        if a != b {
            return (a as i32) - (b as i32);
        }
    }
    0
}

/// Copy `n` bytes from `src` to `dst`, handling overlap correctly.
#[no_mangle]
pub unsafe extern "C" fn memmove(dst: *mut c_void, src: *const c_void, n: usize) -> *mut c_void {
    let mut dst = dst as *mut u8;
    let src = src as *const u8;
    if (dst as usize) < (src as usize) {
        for i in 0..n {
            write_volatile(dst.add(i), read_volatile(src.add(i)));
        }
    } else {
        let mut i = n;
        while i > 0 {
            i -= 1;
            write_volatile(dst.add(i), read_volatile(src.add(i)));
        }
    }
    dst as *mut c_void
}
