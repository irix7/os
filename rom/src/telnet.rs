//! Polled ROM console over telnet.
//!
//! iris forwards host `127.0.0.1:2323` to guest port 23 by default.  The
//! console is a single listening smoltcp TCP socket: when a client connects
//! it becomes the session, and when it closes the socket is re-armed for the
//! next client.  There are no threads; `service()` is called from the ROM's
//! network wait loops.

use crate::net::{clock_advance, clock_now, SeeqDevice};
use alloc::boxed::Box;
use alloc::vec;
use core::fmt::{self, Write};
use smoltcp::iface::{Interface, SocketSet};
use smoltcp::socket::tcp::{Socket as TcpSocket, SocketBuffer, State};
use smoltcp::wire::{Ipv4Address, Ipv4Cidr};

const TELNET_PORT: u16 = 23;

/// Live information shown by the console's `status`/`net` commands.
pub struct ConsoleInfo {
    pub stage: &'static str,
    pub ip: Option<Ipv4Cidr>,
    pub gateway: Option<Ipv4Address>,
    pub mac: [u8; 6],
    pub ram_size: usize,
    pub ntp_time: Option<u64>,
}

pub struct TelnetConsole {
    handle: smoltcp::iface::SocketHandle,
    line: [u8; 128],
    line_len: usize,
    iac: u8,
    last_cr: bool,
    connected: bool,
    boot_requested: bool,
}

impl TelnetConsole {
    /// Add the listening socket to `socket_set` and bind guest port 23.
    pub fn new(socket_set: &mut SocketSet) -> Option<Self> {
        let rx = Box::leak(Box::new(vec![0u8; 2048]));
        let tx = Box::leak(Box::new(vec![0u8; 2048]));
        let socket = TcpSocket::new(
            SocketBuffer::new(&mut rx[..]),
            SocketBuffer::new(&mut tx[..]),
        );
        let handle = socket_set.add(socket);
        socket_set
            .get_mut::<TcpSocket>(handle)
            .listen(TELNET_PORT)
            .ok()?;
        Some(Self {
            handle,
            line: [0; 128],
            line_len: 0,
            iac: 0,
            last_cr: false,
            connected: false,
            boot_requested: false,
        })
    }

    pub fn connected(&self) -> bool {
        self.connected
    }

    pub fn boot_requested(&self) -> bool {
        self.boot_requested
    }

    /// Service the listener/session.  Call after `iface.poll`.
    pub fn service(&mut self, socket_set: &mut SocketSet, info: &ConsoleInfo) {
        let sock = socket_set.get_mut::<TcpSocket>(self.handle);

        // Once the peer has gone, re-arm the listener for the next client.
        if matches!(
            sock.state(),
            State::Closed | State::TimeWait | State::LastAck
        ) {
            sock.abort();
            let _ = sock.listen(TELNET_PORT);
            self.connected = false;
        }

        let now_connected = sock.remote_endpoint().is_some()
            && matches!(
                sock.state(),
                State::Established | State::FinWait1 | State::FinWait2 | State::CloseWait
            );
        if now_connected && !self.connected {
            self.connected = true;
            self.line_len = 0;
            self.iac = 0;
            // IAC WILL ECHO, IAC WILL SUPPRESS-GO-AHEAD.
            let _ = sock.send_slice(&[0xFF, 0xFB, 0x01, 0xFF, 0xFB, 0x03]);
            let _ = sock.send_slice(b"\r\nIRIX 7 ROM console\r\n--- boot log ---\r\n");
            for chunk in crate::boot_log().chunks(512) {
                let _ = sock.send_slice(chunk);
            }
            let _ = sock.send_slice(b"\r\n--- console ---\r\nType 'help' for commands.\r\nROM> ");
        } else if !now_connected {
            self.connected = false;
        }

        let mut buf = [0u8; 64];
        while sock.can_recv() {
            let n = match sock.recv_slice(&mut buf) {
                Ok(n) => n,
                Err(_) => break,
            };
            for &b in &buf[..n] {
                self.input(sock, b, info);
            }
        }

        // The peer sent FIN.  Close our side so the next poll reaches Closed
        // and the socket can listen again; otherwise the console would only
        // ever serve its first client.
        if sock.state() == State::CloseWait {
            sock.close();
            self.connected = false;
        }
    }

    /// Wait after DHCP for a client to attach.  Returns true if the client
    /// asked to continue booting.  If nobody connects, the boot continues
    /// after roughly `window_counts` CP0 Count ticks.
    pub fn wait_for_console(
        &mut self,
        iface: &mut Interface,
        socket_set: &mut SocketSet,
        device: &mut SeeqDevice,
        info: &ConsoleInfo,
        window_counts: u32,
    ) -> bool {
        let start = read_count();
        let mut iterations = 0u32;
        loop {
            let timestamp = clock_now();
            let _ = iface.poll(timestamp, device, socket_set);
            self.service(socket_set, info);
            clock_advance(1);

            if self.boot_requested {
                return true;
            }
            if self.connected {
                // Keep the session alive until the client says `boot` or
                // disconnects; do not auto-continue under their cursor.
                iterations = 0;
                core::hint::spin_loop();
                continue;
            }

            iterations = iterations.saturating_add(1);
            if read_count().wrapping_sub(start) >= window_counts || iterations > 5_000_000 {
                return false;
            }
            core::hint::spin_loop();
        }
    }

    fn input(&mut self, sock: &mut TcpSocket, b: u8, info: &ConsoleInfo) {
        // Telnet command filtering.  We negotiate WILL ECHO and WILL SGA;
        // everything else is ignored.
        if self.iac == 0 {
            if b == 0xFF {
                self.iac = 1;
                return;
            }
        } else if self.iac == 1 {
            match b {
                0xFB..=0xFE => self.iac = 2,
                0xFA => self.iac = 3,
                _ => self.iac = 0,
            }
            return;
        } else if self.iac == 2 {
            self.iac = 0;
            return;
        } else if self.iac == 3 {
            if b == 0xFF {
                self.iac = 4;
            }
            return;
        } else {
            self.iac = if b == 0xF0 { 0 } else { 3 };
            return;
        }

        match b {
            b'\r' => {
                self.last_cr = true;
                let _ = sock.send_slice(b"\r\n");
                self.execute(sock, info);
            }
            b'\n' => {
                // Telnet clients commonly send CRLF.  The CR already ran the
                // command; swallow the LF rather than producing a second,
                // empty prompt.
                if self.last_cr {
                    self.last_cr = false;
                    return;
                }
                let _ = sock.send_slice(b"\r\n");
                self.execute(sock, info);
            }
            0x08 | 0x7F => {
                self.last_cr = false;
                if self.line_len > 0 {
                    self.line_len -= 1;
                    let _ = sock.send_slice(b"\x08 \x08");
                }
            }
            0x20..=0x7E => {
                self.last_cr = false;
                if self.line_len < self.line.len() {
                    self.line[self.line_len] = b;
                    self.line_len += 1;
                    let _ = sock.send_slice(&[b]);
                }
            }
            _ => {}
        }
    }

    fn execute(&mut self, sock: &mut TcpSocket, info: &ConsoleInfo) {
        let line = core::str::from_utf8(&self.line[..self.line_len]).unwrap_or("");
        let mut parts = line.split_whitespace();
        let cmd = parts.next().unwrap_or("");

        match cmd {
            "" => {}
            "help" => {
                send(sock, "commands: help status net mem time echo boot\r\n");
            }
            "status" => {
                send_fmt(sock, format_args!("stage: {}\r\n", info.stage));
                match info.ip {
                    Some(ip) => {
                        let o = ip.address().octets();
                        send_fmt(
                            sock,
                            format_args!(
                                "ip: {}.{}.{}.{}/{}\r\n",
                                o[0],
                                o[1],
                                o[2],
                                o[3],
                                ip.prefix_len()
                            ),
                        );
                    }
                    None => send(sock, "ip: none\r\n"),
                }
                send_mac(sock, &info.mac);
                send_fmt(
                    sock,
                    format_args!("ram: {} MiB\r\n", info.ram_size / (1024 * 1024)),
                );
                match info.ntp_time {
                    Some(t) => send_fmt(sock, format_args!("ntp: {} (unix)\r\n", t)),
                    None => send(sock, "ntp: not set\r\n"),
                }
            }
            "net" => {
                match info.ip {
                    Some(ip) => {
                        let o = ip.address().octets();
                        send_fmt(
                            sock,
                            format_args!(
                                "ip: {}.{}.{}.{}/{}\r\n",
                                o[0],
                                o[1],
                                o[2],
                                o[3],
                                ip.prefix_len()
                            ),
                        );
                    }
                    None => send(sock, "ip: none\r\n"),
                }
                match info.gateway {
                    Some(ip) => {
                        let o = ip.octets();
                        send_fmt(
                            sock,
                            format_args!("gateway: {}.{}.{}.{}\r\n", o[0], o[1], o[2], o[3]),
                        );
                    }
                    None => send(sock, "gateway: none\r\n"),
                }
                send_mac(sock, &info.mac);
            }
            "mem" => {
                send_fmt(
                    sock,
                    format_args!("ram: {} MiB tested\r\n", info.ram_size / (1024 * 1024)),
                );
            }
            "time" => match info.ntp_time {
                Some(t) => send_fmt(sock, format_args!("unix time: {}\r\n", t)),
                None => send(sock, "ntp time not set\r\n"),
            },
            "echo" => {
                let rest = line.strip_prefix("echo").unwrap_or("").trim_start();
                send(sock, rest);
                send(sock, "\r\n");
            }
            "boot" => {
                self.boot_requested = true;
                send(sock, "continuing boot\r\n");
            }
            _ => send(sock, "unknown command; try 'help'\r\n"),
        }

        self.line_len = 0;
        send(sock, "ROM> ");
    }
}

fn send(sock: &mut TcpSocket, s: &str) {
    let _ = sock.send_slice(s.as_bytes());
}

/// Fixed-size formatter so console commands never allocate from the ROM heap.
struct FmtBuf {
    data: [u8; 256],
    len: usize,
}

impl FmtBuf {
    fn new() -> Self {
        Self {
            data: [0; 256],
            len: 0,
        }
    }
}

impl Write for FmtBuf {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        let bytes = s.as_bytes();
        if self.len + bytes.len() > self.data.len() {
            return Err(fmt::Error);
        }
        self.data[self.len..self.len + bytes.len()].copy_from_slice(bytes);
        self.len += bytes.len();
        Ok(())
    }
}

fn send_fmt(sock: &mut TcpSocket, args: fmt::Arguments<'_>) {
    let mut buf = FmtBuf::new();
    let _ = buf.write_fmt(args);
    if let Ok(s) = core::str::from_utf8(&buf.data[..buf.len]) {
        send(sock, s);
    }
}

fn send_mac(sock: &mut TcpSocket, mac: &[u8; 6]) {
    send_fmt(
        sock,
        format_args!(
            "mac: {:02X}:{:02X}:{:02X}:{:02X}:{:02X}:{:02X}\r\n",
            mac[0], mac[1], mac[2], mac[3], mac[4], mac[5]
        ),
    );
}

#[cfg(target_arch = "mips64")]
fn read_count() -> u32 {
    let v: u32;
    unsafe {
        core::arch::asm!(".set noat", "mfc0 {0}, $9", out(reg) v);
    }
    v
}

#[cfg(not(target_arch = "mips64"))]
fn read_count() -> u32 {
    0
}
