// SPDX-License-Identifier: GPL-3.0-only
//! The machine status block, shared by the telnet console and MCP.
//!
//! Keeping one formatter means a remote agent and a person at the console see
//! the same numbers.  The text uses CRLF because the telnet console writes it
//! directly to a socket.

use smoltcp::wire::Ipv4Address;

use crate::{frames, net, ntp};

struct Buf<'a> {
    data: &'a mut [u8],
    len: usize,
}

impl<'a> Buf<'a> {
    fn new(data: &'a mut [u8]) -> Self {
        Self { data, len: 0 }
    }

    fn push(&mut self, bytes: &[u8]) {
        let n = (self.data.len() - self.len).min(bytes.len());
        self.data[self.len..self.len + n].copy_from_slice(&bytes[..n]);
        self.len += n;
    }

    fn str(&mut self, s: &str) {
        self.push(s.as_bytes());
    }

    fn u32(&mut self, mut value: u32) {
        let mut digits = [0u8; 10];
        let mut n = 0;
        loop {
            digits[n] = b'0' + (value % 10) as u8;
            value /= 10;
            n += 1;
            if value == 0 {
                break;
            }
        }
        while n > 0 {
            n -= 1;
            self.push(&digits[n..n + 1]);
        }
    }

    fn hex8(&mut self, value: u8) {
        const HEX: &[u8; 16] = b"0123456789abcdef";
        self.push(&[HEX[(value >> 4) as usize], HEX[(value & 0xf) as usize]]);
    }

    fn ipv4(&mut self, ip: Ipv4Address) {
        let octets = ip.octets();
        for (i, octet) in octets.iter().enumerate() {
            if i > 0 {
                self.push(b".");
            }
            self.u32(u32::from(*octet));
        }
    }
}

/// Network summary.
pub fn net(dst: &mut [u8]) -> usize {
    let mut w = Buf::new(dst);
    w.str("ip: ");
    match net::ip_addr() {
        Some(ip) => w.ipv4(ip),
        None => w.str("none"),
    }
    w.str("\r\nmac: ");
    let mac = net::mac_address();
    for (i, byte) in mac.iter().enumerate() {
        if i > 0 {
            w.push(b":");
        }
        w.hex8(*byte);
    }
    w.str("\r\nlink: Seeq 8003 via HPC3 PDMA\r\n");
    w.len
}

/// Frame allocator summary.
pub fn mem(dst: &mut [u8]) -> usize {
    let total = frames::total_ram();
    let free = frames::free_frames();
    let mut w = Buf::new(dst);
    w.str("total: ");
    w.u32((total / (1024 * 1024)) as u32);
    w.str(" MiB\r\nfree: ");
    w.u32(((free as u64 * 4096) / (1024 * 1024)) as u32);
    w.str(" MiB (");
    w.u32(free as u32);
    w.str(" frames)\r\n");
    w.len
}

/// Uptime and wall-clock summary.
pub fn time(dst: &mut [u8]) -> usize {
    let ms = net::clock_now().total_millis();
    let mut w = Buf::new(dst);
    w.str("uptime: ");
    w.u32((ms / 1000) as u32);
    w.push(b".");
    let frac = (ms % 1000) as u32;
    w.push(&[
        b'0' + (frac / 100) as u8,
        b'0' + ((frac / 10) % 10) as u8,
        b'0' + (frac % 10) as u8,
    ]);
    w.str("s\r\ndate: ");
    match ntp::now_unix() {
        Some(unix) => {
            let mut buf = [0u8; 20];
            let n = ntp::format_utc(unix, &mut buf);
            w.push(&buf[..n]);
            w.str(" UTC");
        }
        None => w.str("unsynchronised"),
    }
    w.str("\r\n");
    w.len
}

/// Format the status block into `dst`; returns the byte count.
pub fn text(dst: &mut [u8]) -> usize {
    let mut w = Buf::new(dst);
    w.str("stage: kernel\r\n");
    w.str("ip: ");
    match net::ip_addr() {
        Some(ip) => w.ipv4(ip),
        None => w.str("none"),
    }
    w.str("\r\nmac: ");
    let mac = net::mac_address();
    for (i, byte) in mac.iter().enumerate() {
        if i > 0 {
            w.push(b":");
        }
        w.hex8(*byte);
    }
    w.str("\r\nram: ");
    w.u32((frames::total_ram() / (1024 * 1024)) as u32);
    w.str(" MiB\r\nuptime: ");
    w.u32((net::clock_now().total_millis() / 1000) as u32);
    w.str("s\r\ndate: ");
    match ntp::now_unix() {
        Some(unix) => {
            let mut buf = [0u8; 20];
            let n = ntp::format_utc(unix, &mut buf);
            w.push(&buf[..n]);
            w.str(" UTC");
        }
        None => w.str("unsynchronised"),
    }
    w.str("\r\n");
    w.len
}
