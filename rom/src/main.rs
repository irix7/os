#![cfg_attr(not(test), no_main)]
#![no_std]
#![feature(asm_experimental_arch)]

extern crate alloc;

use core::panic::PanicInfo;
use core::ptr::{read_volatile, write_volatile};
use rom::net::SeeqDevice;
use rom::{serial_init, serial_write_byte, serial_write_str};
use smoltcp::iface::{Interface, SocketSet, SocketStorage};
use smoltcp::wire::{HardwareAddress, IpCidr, Ipv4Address};

/// Kernel/server used for the bring-up fetch.  The iris NAT lets the guest
/// reach host sockets, so this is the host's LAN address and a local HTTP
/// server.  Replace with the netboot service address for a real deployment.
const HTTP_SERVER_IP: [u8; 4] = [10, 1, 1, 3];
const HTTP_SERVER_PORT: u16 = 18080;
const HTTP_SERVER_HOST: &str = "10.1.1.3";

/// Hand-off addresses (physical, per the boot protocol).
const KERNEL_KSEG0: u32 = 0x8804_0000;
const KERNEL_PHYS: u32 = 0x0804_0000;
const BOOTINFO_KSEG0: u32 = 0x8830_0000;
const BOOTINFO_PHYS: u32 = 0x0830_0000;
const MEMMAP_KSEG0: u32 = 0x8830_0100;
const KERNEL_STACK: u32 = 0x8820_0000;

// Reset entry and stackless bring-up.  The R4000 starts at 0xBFC00000 with no
// stack and no writable data.  reset.S sizes and tests DRAM using registers
// only, and only then builds the stack/.data/.bss.  memsize.S and memtest.S
// are ports of the original IP22 ARCS PROM's lmem_conf.s and memory test.
core::arch::global_asm!(include_str!("boot/reset.S"));
core::arch::global_asm!(include_str!("boot/memsize.S"));
core::arch::global_asm!(include_str!("boot/memtest.S"));

fn print_hex8(v: u8) {
    unsafe {
        serial_write_byte(if v >> 4 < 10 {
            b'0' + (v >> 4)
        } else {
            b'A' + (v >> 4) - 10
        });
        serial_write_byte(if v & 0x0F < 10 {
            b'0' + (v & 0x0F)
        } else {
            b'A' + (v & 0x0F) - 10
        });
    }
}

fn print_dec(mut v: u8) {
    if v >= 100 {
        serial_write_byte(b'0' + v / 100);
        v %= 100;
    }
    if v >= 10 {
        serial_write_byte(b'0' + v / 10);
        v %= 10;
    }
    serial_write_byte(b'0' + v);
}

// The SeeqDevice (and therefore its DMA descriptors and frame buffers) must
// live outside the low 512 KiB of DRAM.  The original IP22 PROM puts its
// descriptor arrays and buffers at BSSADDR 0xA8700000 (7 MiB in); the low
// 512 KiB has a separate alias at physical 0 and is special-cased throughout
// the memory system.  Keep the device in .bss at 0xA8040000 instead of on the
// stack, which starts at 0xA8030000 and would place it in that alias window.
static mut SEEQ: Option<SeeqDevice> = None;

#[no_mangle]
pub extern "C" fn rom_main() -> ! {
    serial_init();
    serial_write_str("\r\nROM: ARCS PROM replacement (n32/MIPS III) booting\r\n");

    // Caches are deliberately left in their reset (disabled/uncached) state
    // for now: the DMA engines write guest RAM directly, and the ROM's stack
    // and buffers live in kseg1.  Cache configuration comes after the boot
    // path is coherent end to end.
    serial_write_str("ROM: caches left uncached for bring-up\r\n");

    let ram_size = rom::ramtest::tested_ram_size();
    let ram_fail = rom::ramtest::first_failure();
    if ram_fail != 0 {
        serial_write_str(&alloc::format!(
            "ROM: RAM test FAILED at {:#010x}\r\n",
            ram_fail
        ));
        loop {
            core::hint::spin_loop();
        }
    }
    serial_write_str(&alloc::format!(
        "ROM: RAM test ok, {} MB detected\r\n",
        ram_size / (1024 * 1024)
    ));

    let seeq = unsafe {
        let p = core::ptr::addr_of_mut!(SEEQ);
        *p = SeeqDevice::new();
        match (*p).as_mut() {
            Some(s) => s,
            None => {
                serial_write_str("ROM: seeq init failed (bad MAC)\r\n");
                loop {
                    core::hint::spin_loop();
                }
            }
        }
    };
    seeq.start();
    serial_write_str("ROM: seeq mac ");
    let mac_addr = seeq.mac_address();
    let mac_bytes = mac_addr.as_bytes();
    for (i, b) in mac_bytes.iter().enumerate() {
        if i > 0 {
            serial_write_byte(b':');
        }
        print_hex8(*b);
    }
    serial_write_str("\r\n");

    // One network stack for the whole boot: DHCP, NTP and the kernel fetch
    // share this interface, socket set and Seeq device.
    let mut socket_storage = [SocketStorage::EMPTY; 8];
    let mut socket_set = SocketSet::new(&mut socket_storage[..]);
    let mut iface = Interface::new(
        smoltcp::iface::Config::new(HardwareAddress::Ethernet(seeq.mac_address())),
        seeq,
        smoltcp::time::Instant::from_millis(0),
    );

    serial_write_str("ROM: dhcp...");
    let lease = match rom::net::dhcpv4_configure(&mut iface, &mut socket_set, seeq) {
        Some(lease) => {
            serial_write_str("ok, ip ");
            let octets = lease.address.address().octets();
            for (i, octet) in octets.iter().enumerate() {
                if i > 0 {
                    serial_write_byte(b'.');
                }
                print_dec(*octet);
            }
            serial_write_str("\r\n");
            lease
        }
        None => {
            serial_write_str("failed\r\n");
            loop {
                core::hint::spin_loop();
            }
        }
    };
    let ip = lease.address;

    // Install the lease on the shared interface before any IP traffic.
    iface.update_ip_addrs(|addrs| {
        let _ = addrs.push(IpCidr::Ipv4(ip));
    });
    if let Some(gateway) = lease.router {
        let _ = iface.routes_mut().add_default_ipv4_route(gateway);
    }

    serial_write_str("ROM: ntp...");
    let ntp_server = lease
        .router
        .unwrap_or_else(|| Ipv4Address::new(192, 168, 0, 1));
    let unix_time = match rom::ntp::query_ntp(&mut iface, &mut socket_set, seeq, ntp_server) {
        Some(t) => {
            serial_write_str(&alloc::format!("ok, unix time {}\r\n", t));
            t
        }
        None => {
            serial_write_str("failed (continuing without time)\r\n");
            0
        }
    };
    let _ = unix_time;

    serial_write_str("ROM: http get...");
    let server_ip = Ipv4Address::new(
        HTTP_SERVER_IP[0],
        HTTP_SERVER_IP[1],
        HTTP_SERVER_IP[2],
        HTTP_SERVER_IP[3],
    );
    match rom::http::http_get(
        &mut iface,
        &mut socket_set,
        seeq,
        server_ip,
        HTTP_SERVER_PORT,
        HTTP_SERVER_HOST,
        "/kernel",
        None,
    ) {
        Ok(result) => {
            serial_write_str("ok, ");
            serial_write_str(&alloc::format!("{}", result.body.len()));
            serial_write_str(" bytes\r\n");

            let digest = rom::sha256::Sha256::digest(&result.body);
            serial_write_str("ROM: sha256 ");
            for b in digest.0 {
                print_hex8(b);
            }
            serial_write_str("\r\n");

            // Copy the fetched image to the hand-off address.  The body is in
            // the ROM heap; the kernel expects it at 0x8804_0000.
            let kernel_dest = KERNEL_KSEG0 as *mut u8;
            unsafe {
                core::ptr::copy_nonoverlapping(
                    result.body.as_ptr(),
                    kernel_dest,
                    result.body.len(),
                );
                core::arch::asm!("sync");
            }

            // Build the memory map and BootInfo in reserved RAM above the
            // kernel's stack.  All addresses stored in BootInfo are physical.
            let mem_map = MEMMAP_KSEG0 as *mut rom::boot::MemMapEntry;
            unsafe {
                core::ptr::write_volatile(
                    mem_map,
                    rom::boot::MemMapEntry {
                        base: 0x0800_0000,
                        size: ram_size as u64,
                        kind: rom::boot::MEM_MAP_USABLE_RAM,
                        flags: 0,
                    },
                );
            }

            let mut boot_info = rom::boot::BootInfo::default();
            boot_info.magic = rom::boot::BOOTINFO_MAGIC;
            boot_info.version_major = rom::boot::BOOTINFO_VERSION_MAJOR;
            boot_info.size = rom::boot::BOOTINFO_SIZE as u32;
            boot_info.mem_map_phys = (BOOTINFO_PHYS as u64) + 0x100;
            boot_info.mem_map_count = 1;
            boot_info.mem_map_entry_size = rom::boot::MEM_MAP_ENTRY_SIZE;
            boot_info.kernel_phys = KERNEL_PHYS as u64;
            boot_info.kernel_size = result.body.len() as u64;
            boot_info.kernel_sha256 = digest.0;
            boot_info.bundle_phys = 0;
            boot_info.bundle_size = 0;
            boot_info.console_kind = rom::boot::CONSOLE_UART;
            boot_info.cmdline_len = 1;
            boot_info.mac = [
                mac_bytes[0],
                mac_bytes[1],
                mac_bytes[2],
                mac_bytes[3],
                mac_bytes[4],
                mac_bytes[5],
            ];
            let ip_octets = ip.address().octets();
            boot_info.ip =
                u32::from_be_bytes([ip_octets[0], ip_octets[1], ip_octets[2], ip_octets[3]]);
            boot_info.netmask = !(0xFFFF_FFFFu32 << (32 - ip.prefix_len()));
            boot_info.checksum = rom::boot::bootinfo_checksum(&boot_info);
            unsafe {
                core::ptr::write_volatile(BOOTINFO_KSEG0 as *mut rom::boot::BootInfo, boot_info);
            }

            serial_write_str("ROM: boot info ready\r\n");
            serial_write_str("ROM: entering kernel...\r\n");

            // The boot protocol requires caches on and interrupts off at the
            // kernel entry.  The ROM itself runs from uncached kseg1, so it
            // survives the cache enable.
            unsafe {
                rom::cache::init();
                core::arch::asm!("sync");
                core::arch::asm!(
                    ".set noat",
                    "move $sp, {sp}",
                    "move $a0, {bi}",
                    "move $a1, $zero",
                    "move $a2, $zero",
                    "move $v0, $zero",
                    "move $ra, $zero",
                    "jr {entry}",
                    sp = in(reg) KERNEL_STACK,
                    bi = in(reg) BOOTINFO_PHYS,
                    entry = in(reg) KERNEL_KSEG0,
                );
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
}

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    serial_write_str("ROM panic\r\n");
    loop {
        core::hint::spin_loop();
    }
}

#[no_mangle]
pub extern "C" fn memcpy(dst: *mut u8, src: *const u8, n: usize) -> *mut u8 {
    unsafe {
        let dst = dst as *mut u8;
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
}

#[no_mangle]
pub extern "C" fn memset(dst: *mut u8, val: i32, n: usize) -> *mut u8 {
    unsafe {
        let dst = dst as *mut u8;
        let v = val as u8;
        for i in 0..n {
            write_volatile(dst.add(i), v);
        }
        dst
    }
}

#[no_mangle]
pub extern "C" fn memcmp(s1: *const u8, s2: *const u8, n: usize) -> i32 {
    unsafe {
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
}

#[no_mangle]
pub extern "C" fn memmove(dst: *mut u8, src: *const u8, n: usize) -> *mut u8 {
    unsafe {
        let dst = dst as *mut u8;
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
}
