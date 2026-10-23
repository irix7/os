// SPDX-License-Identifier: GPL-3.0-only
//! Telnet console server for the kernel.
//!
//! One listening TCP socket on guest port 23.  The console mirrors the
//! serial log (through [`write_output`]) and accepts a small command set so
//! the machine can be inspected over the network after the ROM handoff.
//!
//! The socket lives in the shared smoltcp `SocketSet` owned by [`crate::net`];
//! this module only services it.  It must therefore be initialised after
//! `net::init()`.

use core::fmt::Write;
use core::sync::atomic::{AtomicBool, Ordering};
use smoltcp::iface::SocketSet;
use smoltcp::socket::tcp::{Socket as TcpSocket, SocketBuffer, State};
use smoltcp::wire::Ipv4Address;

use crate::net;

const TELNET_PORT: u16 = 23;
const LINE_LEN: usize = 256;

/// Receive and transmit buffers for the console socket.
static mut TELNET_RX_BUF: [u8; 2048] = [0; 2048];
static mut TELNET_TX_BUF: [u8; 16384] = [0; 16384];

/// Set when the client asks the kernel to continue booting.
static BOOT_REQUESTED: AtomicBool = AtomicBool::new(false);

/// Console session state.
struct Console {
    handle: smoltcp::iface::SocketHandle,
    line: [u8; LINE_LEN],
    line_len: usize,
    connected: bool,
    /// Telnet IAC parser state: 0 = data, 1 = after IAC, 2 = swallowing option.
    iac: u8,
    /// The previous byte was CR, so a following LF must not run the line twice.
    last_cr: bool,
}

static mut CONSOLE: Option<Console> = None;

/// Register the listening socket.  Call after `net::init()`.
pub fn init() {
    let handle = net::sockets_with(|sockets| {
        // SAFETY: the buffers are static and used by this one socket only.
        let rx = unsafe { &mut TELNET_RX_BUF[..] };
        let tx = unsafe { &mut TELNET_TX_BUF[..] };
        let socket = TcpSocket::new(SocketBuffer::new(rx), SocketBuffer::new(tx));
        let handle = sockets.add(socket);
        let _ = sockets.get_mut::<TcpSocket>(handle).listen(TELNET_PORT);
        handle
    });

    if let Some(handle) = handle {
        // SAFETY: single-threaded initialisation.
        unsafe {
            CONSOLE = Some(Console {
                handle,
                line: [0; LINE_LEN],
                line_len: 0,
                connected: false,
                iac: 0,
                last_cr: false,
            });
        }
    }
}

/// Service the listener and session.  Call after `net::poll()`.
pub fn poll() {
    // SAFETY: single-threaded kernel; the console is set once by init().
    let console = unsafe { CONSOLE.as_mut() };
    let Some(console) = console else { return };
    net::sockets_with(|sockets| service(sockets, console));
}

/// Whether a telnet client asked the kernel to continue booting.
pub fn boot_requested() -> bool {
    BOOT_REQUESTED.load(Ordering::SeqCst)
}

/// Mirror console output to the telnet client, if one is connected.
pub fn write_output(data: &[u8]) {
    // SAFETY: single-threaded kernel; the console is set once by init().
    let console = unsafe { CONSOLE.as_ref() };
    let Some(console) = console else { return };
    if !console.connected {
        return;
    }
    net::sockets_with(|sockets| {
        let sock = sockets.get_mut::<TcpSocket>(console.handle);
        if sock.can_send() {
            let _ = sock.send_slice(data);
        }
    });
}

fn service(sockets: &mut SocketSet<'static>, console: &mut Console) {
    let sock = sockets.get_mut::<TcpSocket>(console.handle);

    // Once the peer has gone, re-arm the listener for the next client.
    if matches!(
        sock.state(),
        State::Closed | State::TimeWait | State::LastAck
    ) {
        sock.abort();
        let _ = sock.listen(TELNET_PORT);
        console.connected = false;
        return;
    }

    let now_connected = sock.remote_endpoint().is_some()
        && matches!(
            sock.state(),
            State::Established | State::FinWait1 | State::FinWait2 | State::CloseWait
        );

    if now_connected && !console.connected {
        console.connected = true;
        console.line_len = 0;
        console.iac = 0;
        console.last_cr = false;
        // IAC WILL ECHO, IAC WILL SUPPRESS-GO-AHEAD.
        let _ = sock.send_slice(&[0xFF, 0xFB, 0x01, 0xFF, 0xFB, 0x03]);
        let _ = sock.send_slice(b"\r\nIRIX 7 kernel console\r\nType `help` for commands.\r\n");
        prompt(sock);
    } else if !now_connected {
        console.connected = false;
    }

    let mut buf = [0u8; 64];
    while sock.can_recv() {
        let n = match sock.recv_slice(&mut buf) {
            Ok(n) => n,
            Err(_) => break,
        };
        for &b in &buf[..n] {
            input(sock, console, b);
        }
    }

    // The peer sent FIN.  Close our side so the next poll reaches Closed and
    // the socket can listen again.
    if sock.state() == State::CloseWait {
        sock.close();
        console.connected = false;
    }
}

fn input(sock: &mut TcpSocket, console: &mut Console, b: u8) {
    // Telnet command stream: IAC (0xFF) followed by a command byte, then an
    // option byte for WILL/WONT/DO/DONT.  Ignore the whole negotiation.
    if console.iac == 1 {
        console.iac = if b == 0xFF { 0 } else { 2 };
        return;
    }
    if console.iac == 2 {
        console.iac = 0;
        return;
    }
    if b == 0xFF {
        console.iac = 1;
        return;
    }

    match b {
        b'\r' => {
            console.last_cr = true;
            execute(sock, console);
        }
        b'\n' => {
            if console.last_cr {
                console.last_cr = false;
            } else {
                execute(sock, console);
            }
        }
        0x08 | 0x7F => {
            if console.line_len > 0 {
                console.line_len -= 1;
                let _ = sock.send_slice(b"\x08 \x08");
            }
        }
        0x03 => {
            console.line_len = 0;
            let _ = sock.send_slice(b"^C\r\n");
            prompt(sock);
        }
        0x15 => {
            console.line_len = 0;
            let _ = sock.send_slice(b"\r\n");
            prompt(sock);
        }
        b if (0x20..0x7F).contains(&b) => {
            if console.line_len < LINE_LEN {
                console.line[console.line_len] = b;
                console.line_len += 1;
                let _ = sock.send_slice(&[b]);
            }
        }
        _ => {}
    }
}

fn execute(sock: &mut TcpSocket, console: &mut Console) {
    let _ = sock.send_slice(b"\r\n");
    let line = &console.line[..console.line_len];
    let line = trim(line);

    match line {
        b"" => {}
        b"help" => {
            let _ = sock.send_slice(
                b"commands:\r\n\
                  \x20 help          this text\r\n\
                  \x20 status        kernel and network state\r\n\
                  \x20 net           interface addresses\r\n\
                  \x20 mem           RAM accounting\r\n\
                  \x20 time          time since boot\r\n\
                  \x20 echo <text>   echo the argument\r\n\
                  \x20 boot          continue booting\r\n",
            );
        }
        b"status" => {
            let mut w = SocketWriter { sock };
            let uptime = net::clock_now().total_millis() / 1000;
            let _ = write!(w, "stage: kernel\r\n");
            match net::ip_addr() {
                Some(ip) => {
                    let _ = write!(w, "ip: {}\r\n", DisplayIpv4(ip));
                }
                None => {
                    let _ = w.write_str("ip: none\r\n");
                }
            }
            let mac = net::mac_address();
            let _ = write!(
                w,
                "mac: {:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}\r\n",
                mac[0], mac[1], mac[2], mac[3], mac[4], mac[5]
            );
            let _ = write!(
                w,
                "ram: {} MiB\r\n",
                crate::frames::total_ram() / (1024 * 1024)
            );
            let _ = write!(w, "uptime: {}s\r\n", uptime);
        }
        b"net" => {
            let mut w = SocketWriter { sock };
            match net::ip_addr() {
                Some(ip) => {
                    let _ = write!(w, "ip: {}\r\n", DisplayIpv4(ip));
                }
                None => {
                    let _ = w.write_str("ip: none\r\n");
                }
            }
            let mac = net::mac_address();
            let _ = write!(
                w,
                "mac: {:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}\r\n",
                mac[0], mac[1], mac[2], mac[3], mac[4], mac[5]
            );
            let _ = w.write_str("link: Seeq 8003 via HPC3 PDMA\r\n");
        }
        b"mem" => {
            let mut w = SocketWriter { sock };
            let total = crate::frames::total_ram();
            let _ = write!(w, "total: {} MiB\r\n", total / (1024 * 1024));
        }
        b"time" => {
            let mut w = SocketWriter { sock };
            let ms = net::clock_now().total_millis();
            let _ = write!(w, "uptime: {}.{:03}s\r\n", ms / 1000, ms % 1000);
        }
        b"boot" => {
            let _ = sock.send_slice(b"booting\r\n");
            BOOT_REQUESTED.store(true, Ordering::SeqCst);
        }
        _ if line.starts_with(b"echo ") => {
            let _ = sock.send_slice(&line[5..]);
            let _ = sock.send_slice(b"\r\n");
        }
        _ => {
            let _ = sock.send_slice(b"unknown command; type `help`\r\n");
        }
    }

    console.line_len = 0;
    prompt(sock);
}

fn prompt(sock: &mut TcpSocket) {
    let _ = sock.send_slice(b"kernel> ");
}

fn trim(mut line: &[u8]) -> &[u8] {
    while let Some((&b, rest)) = line.split_first() {
        if b == b' ' || b == b'\t' {
            line = rest;
        } else {
            break;
        }
    }
    while let Some((&b, rest)) = line.split_last() {
        if b == b' ' || b == b'\t' {
            line = rest;
        } else {
            break;
        }
    }
    line
}

/// `core::fmt::Write` adapter that pushes bytes straight at the socket.
struct SocketWriter<'a, 'b> {
    sock: &'a mut TcpSocket<'b>,
}

impl Write for SocketWriter<'_, '_> {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        if self.sock.can_send() {
            let _ = self.sock.send_slice(s.as_bytes());
        }
        Ok(())
    }
}

/// Dotted-decimal wrapper for `Ipv4Address` (`Display` is not implemented).
struct DisplayIpv4(Ipv4Address);

impl core::fmt::Display for DisplayIpv4 {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        let o = self.0.octets();
        write!(f, "{}.{}.{}.{}", o[0], o[1], o[2], o[3])
    }
}
