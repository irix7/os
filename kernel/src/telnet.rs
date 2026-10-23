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
/// Set when the client asks the kernel to restart from the PROM.
static REBOOT_REQUESTED: AtomicBool = AtomicBool::new(false);

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
    /// A deferred HTTP fetch, run outside the socket-set borrow.
    pending_http: Option<HttpRequest>,
    /// A deferred NTP query, run outside the socket-set borrow.
    pending_ntp: Option<NtpRequest>,
    /// A deferred ICMP echo run, outside the socket-set borrow.
    pending_ping: Option<PingRequest>,
}

/// Parsed `ntp <ip>[:port]` request.
struct NtpRequest {
    host: [u8; 64],
    host_len: usize,
    port: u16,
}

/// Parsed `ping <host>` request.
struct PingRequest {
    host: [u8; 64],
    host_len: usize,
}

/// Parsed `http <ip>:<port><path>` request.
struct HttpRequest {
    host: [u8; 64],
    host_len: usize,
    port: u16,
    path: [u8; 96],
    path_len: usize,
    tls: bool,
    expected: Option<[u8; 32]>,
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
                pending_http: None,
                pending_ntp: None,
                pending_ping: None,
            });
        }
    }
}

/// Service the listener and session.  Call after `net::poll()`.
pub fn poll() {
    // Run any deferred HTTP fetch first: it needs the interface and the
    // socket set to itself, so it must not run inside `service`'s borrow.
    let request = unsafe { CONSOLE.as_mut() }.and_then(|console| console.pending_http.take());
    if let Some(req) = request {
        let path = core::str::from_utf8(&req.path[..req.path_len]).unwrap_or("/");
        let host = core::str::from_utf8(&req.host[..req.host_len]).unwrap_or("");
        let scheme = if req.tls { "https" } else { "http" };
        let expected = req.expected;
        let result = match resolve_host(&req.host[..req.host_len]) {
            Some(ip) => {
                if req.tls {
                    crate::tls::get(ip, req.port, host, path)
                } else {
                    crate::http::get(ip, req.port, host, path)
                }
            }
            None => Err("unknown host"),
        };
        let handle = unsafe { CONSOLE.as_ref() }.map(|console| console.handle);
        if let Some(handle) = handle {
            net::sockets_with(|sockets| {
                let sock = sockets.get_mut::<TcpSocket>(handle);
                let mut w = SocketWriter { sock };
                match result {
                    Ok(summary) => {
                        match summary.content_length {
                            Some(expected) if expected != summary.body_len => {
                                let _ = write!(
                                    w,
                                    "{}: {} {}/{} bytes\r\n",
                                    scheme, summary.status, summary.body_len, expected
                                );
                            }
                            _ => {
                                let _ = write!(
                                    w,
                                    "{}: {} {} bytes\r\n",
                                    scheme, summary.status, summary.body_len
                                );
                            }
                        }
                        let hex = summary.sha256.to_hex();
                        if let Ok(text) = core::str::from_utf8(&hex) {
                            let _ = write!(w, "sha256: {}\r\n", text);
                        }
                        if let Some(want) = expected {
                            if summary.sha256.eq_digest(&crate::sha256::Sha256(want)) {
                                let _ = w.write_str("verify: ok\r\n");
                            } else {
                                let _ = w.write_str("verify: MISMATCH\r\n");
                            }
                        }
                    }
                    Err(err) => {
                        let _ = write!(w, "{}: {}\r\n", scheme, err);
                    }
                }
                let _ = w.sock.send_slice(b"kernel> ");
            });
        }
    }

    // Run any deferred NTP query, for the same reason.
    let ntp_request = unsafe { CONSOLE.as_mut() }.and_then(|console| console.pending_ntp.take());
    if let Some(req) = ntp_request {
        let resolved = resolve_host(&req.host[..req.host_len]);
        let result = resolved.and_then(|ip| crate::ntp::query(ip, req.port));
        let handle = unsafe { CONSOLE.as_ref() }.map(|console| console.handle);
        if let Some(handle) = handle {
            net::sockets_with(|sockets| {
                let sock = sockets.get_mut::<TcpSocket>(handle);
                let mut w = SocketWriter { sock };
                match (resolved, result) {
                    (Some(_), Some(unix)) => {
                        let mut buf = [0u8; 20];
                        let n = crate::ntp::format_utc(unix, &mut buf);
                        let text = core::str::from_utf8(&buf[..n]).unwrap_or("?");
                        let _ = write!(w, "ntp: {} UTC ({})\r\n", text, unix);
                    }
                    (Some(_), None) => {
                        let _ = w.write_str("ntp: no reply\r\n");
                    }
                    (None, _) => {
                        let _ = w.write_str("ntp: unknown host\r\n");
                    }
                }
                let _ = w.sock.send_slice(b"kernel> ");
            });
        }
    }

    // Run any deferred ICMP echo run, outside the socket-set borrow.
    let ping_request = unsafe { CONSOLE.as_mut() }.and_then(|console| console.pending_ping.take());
    if let Some(req) = ping_request {
        let resolved = resolve_host(&req.host[..req.host_len]);
        let result = resolved.and_then(crate::ping::run);
        let handle = unsafe { CONSOLE.as_ref() }.map(|console| console.handle);
        if let Some(handle) = handle {
            net::sockets_with(|sockets| {
                let sock = sockets.get_mut::<TcpSocket>(handle);
                let mut w = SocketWriter { sock };
                match (resolved, result) {
                    (Some(_), Some(summary)) => {
                        let _ = write!(
                            w,
                            "ping: {}/{} replies, avg {} ms\r\n",
                            summary.received, summary.sent, summary.avg_ms
                        );
                    }
                    (Some(_), None) => {
                        let _ = w.write_str("ping: no reply\r\n");
                    }
                    (None, _) => {
                        let _ = w.write_str("ping: unknown host\r\n");
                    }
                }
                let _ = w.sock.send_slice(b"kernel> ");
            });
        }
    }

    // SAFETY: single-threaded kernel; the console is set once by init().
    let console = unsafe { CONSOLE.as_mut() };
    let Some(console) = console else { return };
    net::sockets_with(|sockets| service(sockets, console));
}

/// Whether a telnet client asked the kernel to continue booting.
pub fn boot_requested() -> bool {
    BOOT_REQUESTED.load(Ordering::SeqCst)
}

/// Whether a telnet client asked the kernel to reboot.
pub fn reboot_requested() -> bool {
    REBOOT_REQUESTED.load(Ordering::SeqCst)
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
                  \x20 http <host>:<port><path> [sha256]  fetch over HTTP\r\n\
                  \x20 https <host>:<port><path> [sha256]  fetch over HTTPS (TLS 1.3)\r\n\
                  \x20 ntp <ip>[:port]  set the clock from an NTP server\r\n\
                  \x20 ping <host>     ICMP echo\r\n\
                  \x20 boot          continue booting\r\n\
                  \x20 reboot        restart from the PROM\r\n",
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
            match crate::ntp::now_unix() {
                Some(unix) => {
                    let mut buf = [0u8; 20];
                    let n = crate::ntp::format_utc(unix, &mut buf);
                    let text = core::str::from_utf8(&buf[..n]).unwrap_or("?");
                    let _ = write!(w, "date: {} UTC\r\n", text);
                }
                None => {
                    let _ = w.write_str("date: unsynchronised\r\n");
                }
            }
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
            let free = crate::frames::free_frames();
            let _ = write!(
                w,
                "total: {} MiB\r\nfree: {} MiB ({} frames)\r\n",
                total / (1024 * 1024),
                (free * 4096) / (1024 * 1024),
                free
            );
        }
        b"time" => {
            let mut w = SocketWriter { sock };
            let ms = net::clock_now().total_millis();
            let _ = write!(w, "uptime: {}.{:03}s\r\n", ms / 1000, ms % 1000);
            if let Some(unix) = crate::ntp::now_unix() {
                let mut buf = [0u8; 20];
                let n = crate::ntp::format_utc(unix, &mut buf);
                let text = core::str::from_utf8(&buf[..n]).unwrap_or("?");
                let _ = write!(w, "date: {} UTC\r\n", text);
            }
        }
        b"boot" => {
            let _ = sock.send_slice(b"booting\r\n");
            BOOT_REQUESTED.store(true, Ordering::SeqCst);
        }
        b"reboot" => {
            let _ = sock.send_slice(b"rebooting\r\n");
            REBOOT_REQUESTED.store(true, Ordering::SeqCst);
        }
        _ if line.starts_with(b"http ") => match parse_http(trim(&line[5..]), false) {
            Some(request) => {
                let _ = sock.send_slice(b"http: fetching\r\n");
                console.pending_http = Some(request);
            }
            None => {
                let _ = sock.send_slice(b"usage: http <host>:<port><path>\r\n");
            }
        },
        _ if line.starts_with(b"https ") => match parse_http(trim(&line[6..]), true) {
            Some(request) => {
                let _ = sock.send_slice(b"https: fetching\r\n");
                console.pending_http = Some(request);
            }
            None => {
                let _ = sock.send_slice(b"usage: https <host>:<port><path>\r\n");
            }
        },
        _ if line.starts_with(b"ping ") => match parse_ping(trim(&line[5..])) {
            Some(request) => {
                let _ = sock.send_slice(b"ping: probing\r\n");
                console.pending_ping = Some(request);
            }
            None => {
                let _ = sock.send_slice(b"usage: ping <host>\r\n");
            }
        },
        _ if line.starts_with(b"ntp ") => match parse_ntp(trim(&line[4..])) {
            Some(request) => {
                let _ = sock.send_slice(b"ntp: querying\r\n");
                console.pending_ntp = Some(request);
            }
            None => {
                let _ = sock.send_slice(b"usage: ntp <ip>[:port]\r\n");
            }
        },
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

/// Parse `http <host>:<port><path> [sha256-hex]`.
fn parse_http(arg: &[u8], tls: bool) -> Option<HttpRequest> {
    let s = core::str::from_utf8(arg).ok()?;
    let mut parts = s.split_whitespace();
    let url = parts.next()?;
    let expected = match parts.next() {
        Some(hex) => Some(parse_sha256_hex(hex)?),
        None => None,
    };
    if parts.next().is_some() {
        return None;
    }
    let (host, rest) = url.split_once(':')?;
    let slash = rest.find('/')?;
    let (port_str, path) = rest.split_at(slash);
    let port: u16 = port_str.parse().ok()?;
    if host.is_empty() || host.len() > 64 || path.is_empty() || path.len() > 96 {
        return None;
    }
    let mut host_buf = [0u8; 64];
    host_buf[..host.len()].copy_from_slice(host.as_bytes());
    let mut buf = [0u8; 96];
    buf[..path.len()].copy_from_slice(path.as_bytes());
    Some(HttpRequest {
        host: host_buf,
        host_len: host.len(),
        port,
        path: buf,
        path_len: path.len(),
        tls,
        expected,
    })
}

/// Parse a 64-digit hex SHA-256 digest.
fn parse_sha256_hex(s: &str) -> Option<[u8; 32]> {
    let bytes = s.as_bytes();
    if bytes.len() != 64 {
        return None;
    }
    let mut out = [0u8; 32];
    for i in 0..32 {
        out[i] = (hex_digit(bytes[i * 2])? << 4) | hex_digit(bytes[i * 2 + 1])?;
    }
    Some(out)
}

fn hex_digit(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

/// Parse a dotted-decimal IPv4 address.
fn parse_ipv4(s: &str) -> Option<Ipv4Address> {
    let mut octets = [0u8; 4];
    let mut n = 0;
    for part in s.split('.') {
        if n == 4 {
            return None;
        }
        octets[n] = part.parse().ok()?;
        n += 1;
    }
    if n != 4 {
        return None;
    }
    Some(Ipv4Address::new(octets[0], octets[1], octets[2], octets[3]))
}

/// Parse `ntp <ip>[:port]`.
fn parse_ntp(arg: &[u8]) -> Option<NtpRequest> {
    let s = core::str::from_utf8(arg).ok()?;
    let (host, port) = match s.split_once(':') {
        Some((host, port)) => (host, port.parse().ok()?),
        None => (s, 123),
    };
    if host.is_empty() || host.len() > 64 {
        return None;
    }
    let mut host_buf = [0u8; 64];
    host_buf[..host.len()].copy_from_slice(host.as_bytes());
    Some(NtpRequest {
        host: host_buf,
        host_len: host.len(),
        port,
    })
}

/// Parse `ping <host>`.
fn parse_ping(arg: &[u8]) -> Option<PingRequest> {
    let s = core::str::from_utf8(arg).ok()?;
    if s.is_empty() || s.len() > 64 {
        return None;
    }
    let mut host = [0u8; 64];
    host[..s.len()].copy_from_slice(s.as_bytes());
    Some(PingRequest {
        host,
        host_len: s.len(),
    })
}

/// Resolve a host name or dotted-decimal address.
fn resolve_host(host: &[u8]) -> Option<Ipv4Address> {
    let name = core::str::from_utf8(host).ok()?;
    if let Some(ip) = parse_ipv4(name) {
        return Some(ip);
    }
    let server = net::dns_server()?;
    crate::dns::resolve(name, server)
}
