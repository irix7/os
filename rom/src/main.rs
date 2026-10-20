//! ROM entry point: initialise the serial console and Seeq, print the bring-up
//! marker and the station MAC on serial0, then run DHCP.
//!
//! Built for `mips2-sgi-irix7-o32` (o32/MIPS II) with `-Z build-std`. The PROM
//! (ARCS) or iris loads this ELF and jumps to `_start`; this crate takes over
//! from the first instruction.

#![no_main]
#![no_std]

use core::panic::PanicInfo;
use rom::net::{dhcpv4_configure, SeeqDevice};
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

#[no_mangle]
pub extern "C" fn _start() -> ! {
    // The PROM (ARCS) or iris loader sets $sp on entry to a valid stack in RAM.

    serial_init();
    serial_write_str("hello from ROM\r\n");

    // Initialise the Seeq Ethernet controller and report its MAC.
    if let Some(mut seeq) = SeeqDevice::new() {
        serial_write_str("seeq: mac ");
        for (i, b) in seeq.mac_address().as_bytes().iter().enumerate() {
            if i > 0 {
                serial_write_byte(b':');
            }
            print_hex8(*b);
        }
        serial_write_str("\r\n");

        // Initialise smoltcp and run DHCPv4 to obtain an IP address.
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
    } else {
        serial_write_str("seeq: init failed (bad MAC)\r\n");
    }

    // Hang: the ROM's real work (HTTP GET, SHA-256, jump to kernel) comes next.
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
