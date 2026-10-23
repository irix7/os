// SPDX-License-Identifier: GPL-3.0-only
//! Hardware and kernel inventory for the boot menu's diagnostics entry.

use crate::{frames, idle, net, print_hex32, print_str, println};

/// Print the inventory to the serial and telnet consoles.
pub fn run() {
    println("");
    println("diagnostics: IRIX 7 kernel");
    print_str("  cpu prid: 0x");
    print_hex32(crate::traps::prid_read());
    println("");
    print_str("  cp0 config: 0x");
    print_hex32(crate::traps::config_read());
    println("");
    println("  memory:");
    print_str("    total: ");
    print_dec((frames::total_ram() / (1024 * 1024)) as u32);
    println(" MiB");
    println("    reserved below 0x08B00000: exception vector, ROM state, kernel, stack");
    println("  network:");
    let mac = net::mac_address();
    print_str("    mac: ");
    for (i, b) in mac.iter().enumerate() {
        if i > 0 {
            crate::write_byte(b':');
        }
        crate::print_hex8(*b);
    }
    println("");
    match net::ip_addr() {
        Some(ip) => {
            let o = ip.octets();
            print_str("    ip: ");
            print_dec(o[0] as u32);
            crate::write_byte(b'.');
            print_dec(o[1] as u32);
            crate::write_byte(b'.');
            print_dec(o[2] as u32);
            crate::write_byte(b'.');
            print_dec(o[3] as u32);
            println("  (telnet 23)");
        }
        None => println("    ip: none"),
    }
    print_str("  timer ticks: ");
    print_dec(idle::ticks());
    println("");
}

/// Print an unsigned decimal value.
fn print_dec(mut v: u32) {
    let mut buf = [0u8; 10];
    let mut n = 0;
    if v == 0 {
        crate::write_byte(b'0');
        return;
    }
    while v > 0 && n < buf.len() {
        buf[n] = b'0' + (v % 10) as u8;
        v /= 10;
        n += 1;
    }
    while n > 0 {
        n -= 1;
        crate::write_byte(buf[n]);
    }
}
