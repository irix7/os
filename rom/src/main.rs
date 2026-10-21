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
#![feature(asm)]

use core::ffi::c_void;
use core::panic::PanicInfo;
use rom::boot::{BOOTINFO_MAGIC, BOOTINFO_SIZE, BOOTINFO_VERSION_MAJOR, BootInfo, CONSOLE_UART, MEM_MAP_ENTRY_SIZE, bootinfo_checksum};
use rom::http::http_get;
use rom::net::{dhcpv4_configure, SeeqDevice};
use rom::sha256::Sha256;
use rom::{serial_init, serial_write_byte, serial_write_str};
use smoltcp::iface::{Config, Interface, SocketSet, SocketStorage};
use smoltcp::socket::tcp::Socket as TcpSocket;
use smoltcp::time::Instant;
use smoltcp::wire::{HardwareAddress, Ipv4Address};

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
    let ip = match dhcpv4_configure(&mut seeq) {
        Some(ip) => {
            serial_write_str("ok, ip ");
            for (i, octet) in ip.addr.octets().iter().enumerate() {
                if i > 0 {
                    serial_write_byte(b'.');
                }
                for nibble in (0..4).rev() {
                    print_hex8((*octet >> (nibble * 4)) & 0x0F);
                }
            }
            serial_write_str("\r\n");
            ip
        }
        None => {
            serial_write_str("failed\r\n");
            loop {
                core::hint::spin_loop();
            }
        }
    };

    // HTTP GET the kernel from the server.
    serial_write_str("rom: http get...");
    let mut sockets = [SocketStorage::EMPTY; 4];
    let mut socket_set = SocketSet::new(&mut sockets[..]);
    let mut iface = Interface::new(
        Config::new(HardwareAddress::Ethernet(seeq.mac_address())),
        &mut seeq,
        Instant::from_millis(0),
    );

    match http_get(
        &mut iface,
        &mut socket_set,
        ip.addr,
        "192.168.1.1",
        "/kernel",
        None,
    ) {
        Ok(result) => {
            serial_write_str("ok, ");
            serial_write_str(&result.body.len().to_string());
            serial_write_str(" bytes\r\n");

            // SHA-256 verify
            let digest = Sha256::digest(&result.body);
            serial_write_str("rom: sha256 ");
            for b in digest.0 {
                print_hex8(b);
            }
            serial_write_str("\r\n");

            // Build BootInfo
            let mut boot_info = BootInfo::default();
            boot_info.magic = BOOTINFO_MAGIC;
            boot_info.version_major = BOOTINFO_VERSION_MAJOR;
            boot_info.size = BOOTINFO_SIZE as u32;
            boot_info.mem_map_entry_size = MEM_MAP_ENTRY_SIZE;
            boot_info.kernel_phys = 0x8004_0000;
            boot_info.kernel_size = result.body.len() as u64;
            boot_info.kernel_sha256 = digest.0;
            boot_info.bundle_phys = 0x8044_0000;
            boot_info.bundle_size = 0;
            boot_info.console_kind = CONSOLE_UART;
            boot_info.cmdline_len = 1;
            boot_info.mac = *seeq.mac_address().as_bytes();
            boot_info.ip = ip.addr;
            boot_info.netmask = !(0xFFFF_FFFFu32 << (32 - ip.prefix_len));
            boot_info.checksum = bootinfo_checksum(&boot_info);

            serial_write_str("rom: boot info ready\r\n");

            // Flush caches and jump to kernel
            unsafe {
                core::arch::asm!("sync");
            }

            // Jump to kernel at kseg0 address
            unsafe {
                core::arch::asm!("j 0x8000_0000");
            }

            loop {
                core::hint::spin_loop();
            }
        }
        Err(_) => {
            serial_write_str("failed\r\n");
            loop {
                core::hint::spin_loop();
            }
        }
    }

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

use core::ptr::{read_volatile, write_volatile};

#[no_mangle]
pub unsafe extern "C" fn memcpy(dst: *mut u8, src: *const u8, n: usize) -> *mut u8 {
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
    dst
}

#[no_mangle]
pub unsafe extern "C" fn memset(dst: *mut u8, val: i32, n: usize) -> *mut u8 {
    let mut dst = dst as *mut u8;
    let v = val as u8;
    for i in 0..n {
        write_volatile(dst.add(i), v);
    }
    dst
}

#[no_mangle]
pub unsafe extern "C" fn memcmp(s1: *const u8, s2: *const u8, n: usize) -> i32 {
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

#[no_mangle]
pub unsafe extern "C" fn memmove(dst: *mut u8, src: *const u8, n: usize) -> *mut u8 {
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