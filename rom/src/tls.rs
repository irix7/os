//! HTTPS/TLS client for the ROM's netboot path.
//!
//! Built only with the `tls` feature (pulls in `embedded-tls` + `rand_core`).
//! Performs a TLS 1.3 handshake over a smoltcp TCP socket and speaks HTTP/1.1
//! inside the tunnel.  The smoltcp stack is polled (not async), so the TLS I/O
//! adapter blocks by polling the interface/device/socket set until data is
//! available — the same poll discipline used by DHCP, NTP and HTTP.
//!
//! Certificate verification is deliberately permissive (`NoVerify`) for the
//! bring-up phase: the ROM accepts the server's certificate without validating
//! a chain.  Swap in `embedded_tls::config::CertVerifier` with a baked-in root
//! CA once the ROM has a certificate store.
//!
//! The RNG is a tiny xorshift128+ PRNG seeded from CP0 Count (good enough for
//! bring-up nonces; a hardware RNG is a follow-up).
// SPDX-License-Identifier: 0BSD

#![cfg_attr(feature = "tls", allow(dead_code))]

use alloc::boxed::Box;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use core::fmt;
use rand_core::{CryptoRng, CryptoRngCore, Error, RngCore};
use smoltcp::iface::{Interface, SocketSet};
use smoltcp::socket::tcp::{Socket as TcpSocket, State};
use smoltcp::time::Duration;
use smoltcp::wire::{IpAddress, Ipv4Address};

use crate::net::SeeqDevice;

use embedded_io::{ErrorType, Read, Write};
use embedded_tls::blocking::TlsConnection;
use embedded_tls::blocking::{
    Aes128GcmSha256, CryptoProvider, NoVerify, TlsConfig, TlsContext, TlsVerifier,
};
use embedded_tls::TlsError;

const HTTP_TIMEOUT_MS: u64 = 30_000;

/// Result of an HTTPS fetch.
pub struct HttpsFetchResult {
    pub body: Vec<u8>,
    pub server_ip: Ipv4Address,
}

/// A blocking I/O adapter over a smoltcp TCP socket.
///
/// `read`/`write` block by polling `iface`/`device`/`socket_set` until the
/// socket can make progress, which lets the blocking `embedded-tls` state
/// machine run inside our polled event loop.
///
/// Raw pointers are used to avoid lifetime complications: `TlsConnection`
/// owns the adapter, but the adapter borrows from `iface`/`device`/`socket_set`
/// which live in `rom_main` and outlive the TLS connection.
struct SocketIo {
    socket_handle: smoltcp::iface::SocketHandle,
    iface: *mut Interface,
    device: *mut SeeqDevice,
    socket_set: *mut SocketSet<'static>,
}

impl SocketIo {
    /// Safety: pointers valid for duration of TLS fetch.
    unsafe fn sock_set_mut(&mut self) -> &mut SocketSet<'static> {
        &mut *self.socket_set
    }

    /// Safety: pointers valid for duration of TLS fetch.
    unsafe fn iface_mut(&mut self) -> &mut Interface {
        &mut *self.iface
    }

    /// Safety: pointers valid for duration of TLS fetch.
    unsafe fn device_mut(&mut self) -> &mut SeeqDevice {
        &mut *self.device
    }
}

/// I/O error surfaced to embedded-tls as `ErrorKind::Other`.
#[derive(Debug)]
enum TlsIoError {
    Closed,
    WouldBlock,
    Timeout,
}

impl fmt::Display for TlsIoError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TlsIoError::Closed => write!(f, "connection closed"),
            TlsIoError::WouldBlock => write!(f, "would block"),
            TlsIoError::Timeout => write!(f, "timeout"),
        }
    }
}

impl core::error::Error for TlsIoError {}

impl embedded_io::Error for TlsIoError {
    fn kind(&self) -> embedded_io::ErrorKind {
        embedded_io::ErrorKind::Other
    }
}

impl ErrorType for SocketIo {
    type Error = TlsIoError;
}

impl Read for SocketIo {
    fn read(&mut self, buf: &mut [u8]) -> Result<usize, Self::Error> {
        let start = crate::net::clock_now();
        let deadline = start + Duration::from_millis(HTTP_TIMEOUT_MS);
        loop {
            // Safety: pointers valid for duration of TLS fetch; the socket
            // reference is used locally and does not escape.
            let sock = unsafe { &mut *self.socket_set }.get_mut::<TcpSocket>(self.socket_handle);
            if let Ok(n) = sock.recv_slice(buf) {
                if n > 0 {
                    return Ok(n);
                }
                // recv_slice returned 0: no data available yet, or EOF if the
                // remote closed the connection (CloseWait / Closed / LastAck).
                let state = sock.state();
                if state == State::Closed || state == State::CloseWait || state == State::LastAck {
                    return Ok(0);
                }
            }
            if crate::net::clock_now() > deadline {
                return Err(TlsIoError::Timeout);
            }
            self.poll_once();
        }
    }
}

impl Write for SocketIo {
    fn write(&mut self, buf: &[u8]) -> Result<usize, Self::Error> {
        let start = crate::net::clock_now();
        let deadline = start + Duration::from_millis(HTTP_TIMEOUT_MS);
        let mut written = 0;
        while written < buf.len() {
            let sock = unsafe { &mut *self.socket_set }.get_mut::<TcpSocket>(self.socket_handle);
            if let Ok(n) = sock.send_slice(&buf[written..]) {
                if n > 0 {
                    written += n;
                    continue;
                }
            }
            if sock.state() == State::Closed || sock.state() == State::LastAck {
                return Err(TlsIoError::Closed);
            }
            if crate::net::clock_now() > deadline {
                return Err(TlsIoError::Timeout);
            }
            self.poll_once();
        }
        Ok(written)
    }

    fn flush(&mut self) -> Result<(), Self::Error> {
        // smoltcp pushes on send_slice; nothing to flush.
        Ok(())
    }
}

impl SocketIo {
    /// Poll the interface once (drives the TCP socket and retransmission
    /// timers) and advance the shared clock.
    fn poll_once(&mut self) {
        let timestamp = crate::net::clock_now();
        // Safety: pointers valid for duration of TLS fetch; all borrows are
        // used locally and do not escape.
        unsafe {
            let iface = &mut *self.iface;
            let device = &mut *self.device;
            let socket_set = &mut *self.socket_set;
            let _ = iface.poll(timestamp, device, socket_set);
        }
        crate::net::clock_tick();
    }
}

/// A tiny xorshift128+ PRNG.  Not cryptographically strong; fine for TLS
/// nonces during bring-up.  Seeded from CP0 Count.
struct SimpleRng {
    s: [u64; 2],
}

impl SimpleRng {
    fn new(seed: u64) -> Self {
        // Mix the seed into both state words so a low-entropy seed still
        // produces well-distributed state.
        let mut s = [
            seed.wrapping_mul(0x9E37_79B9_7F4A_7C15),
            seed.wrapping_add(0x9E37_79B9_7F4A_7C15),
        ];
        // Warm up the generator.
        for _ in 0..8 {
            Self::next_u64(&mut s);
        }
        Self { s }
    }

    fn next_u64(s: &mut [u64; 2]) -> u64 {
        let s0 = s[0];
        let mut s1 = s[1];
        let result = s0.wrapping_add(s1);
        s1 ^= s0;
        s[0] = s0.rotate_left(55) ^ s1 ^ (s1 << 14);
        s[1] = s1.rotate_left(36);
        result
    }
}

impl RngCore for SimpleRng {
    fn next_u32(&mut self) -> u32 {
        Self::next_u64(&mut self.s) as u32
    }

    fn next_u64(&mut self) -> u64 {
        Self::next_u64(&mut self.s)
    }

    fn fill_bytes(&mut self, dest: &mut [u8]) {
        for chunk in dest.chunks_mut(8) {
            let n = Self::next_u64(&mut self.s).to_le_bytes();
            for (i, b) in n.iter().enumerate() {
                if i < chunk.len() {
                    chunk[i] = *b;
                }
            }
        }
    }

    fn try_fill_bytes(&mut self, dest: &mut [u8]) -> Result<(), Error> {
        self.fill_bytes(dest);
        Ok(())
    }
}

impl CryptoRng for SimpleRng {}

/// A cryptographic provider for `embedded-tls`.  Seeds a xorshift PRNG from
/// CP0 Count and accepts any server certificate (`NoVerify`).
struct TlsProvider {
    rng: SimpleRng,
    verifier: NoVerify,
}

impl CryptoProvider for TlsProvider {
    type CipherSuite = Aes128GcmSha256;
    type Signature = Vec<u8>;

    fn rng(&mut self) -> impl CryptoRngCore {
        &mut self.rng
    }

    fn verifier(&mut self) -> Result<&mut impl TlsVerifier<Self::CipherSuite>, TlsError> {
        Ok(&mut self.verifier)
    }
}

/// Perform an HTTPS/1.1 GET request over TLS 1.3 and return the body bytes.
///
/// `server_ip`/`server_port` identify the server; `host` is the `Host:` header
/// and SNI name.  `path` is the request path.  The caller owns the interface,
/// socket set and Seeq device, so the same network stack carries DHCP, NTP,
/// HTTP and HTTPS.
#[allow(clippy::too_many_arguments)]
pub fn https_get(
    iface: &mut Interface,
    socket_set: &mut SocketSet,
    device: &mut SeeqDevice,
    server_ip: Ipv4Address,
    server_port: u16,
    host: &str,
    path: &str,
) -> Result<HttpsFetchResult, ()> {
    // Leaked TCP buffers (live for the duration of the fetch).
    let rx_buf = Box::leak(Box::new(Vec::from([0u8; 4096])));
    let tx_buf = Box::leak(Box::new(Vec::from([0u8; 4096])));
    let tcp_socket = TcpSocket::new(
        smoltcp::socket::tcp::SocketBuffer::new(&mut rx_buf[..]),
        smoltcp::socket::tcp::SocketBuffer::new(&mut tx_buf[..]),
    );
    let socket_handle = socket_set.add(tcp_socket);

    let cx = iface.context();
    socket_set
        .get_mut::<TcpSocket>(socket_handle)
        .connect(cx, (IpAddress::Ipv4(server_ip), server_port), 49152)
        .map_err(|_| ())?;

    let start = crate::net::clock_now();
    let mut timestamp = start;
    let deadline = start + Duration::from_millis(HTTP_TIMEOUT_MS);

    // Wait for connection establishment.
    loop {
        let _ = iface.poll(timestamp, device, socket_set);
        let state = socket_set.get_mut::<TcpSocket>(socket_handle).state();
        if state == State::Established {
            break;
        }
        if state == State::Closed || state == State::LastAck {
            return Err(());
        }
        crate::net::clock_tick();
        timestamp = crate::net::clock_now();
        if timestamp > deadline {
            return Err(());
        }
    }

    // Leaked TLS record buffers (embedded-tls wants a ~16 KiB read buffer).
    let record_read_buf = Box::leak(Box::new([0u8; 16384]));
    let record_write_buf = Box::leak(Box::new([0u8; 4096]));

    // Seed xorshift from CP0 count plus a fixed constant.
    let seed: u64;
    unsafe {
        core::arch::asm!(".set noat", "mfc0 {0}, $9", out(reg) seed);
    }
    let mut provider = TlsProvider {
        rng: SimpleRng::new(seed),
        verifier: NoVerify,
    };

    // `TlsConfig::new()` populates signature schemes and named groups; SNI /
    // ALPN are `pub(crate)` and left unset, which is fine for `NoVerify`.
    let config = TlsConfig::new();

    let mut tls = TlsConnection::new(
        SocketIo {
            socket_handle,
            iface: iface as *mut _ as *mut Interface,
            device: device as *mut _ as *mut SeeqDevice,
            socket_set: socket_set as *mut _ as *mut SocketSet<'static>,
        },
        record_read_buf,
        record_write_buf,
    );

    let context = TlsContext::new(&config, provider);
    tls.open(context).map_err(|_| ())?;

    // Send the HTTP/1.1 request inside the TLS tunnel.
    let mut req = String::new();
    req.push_str("GET ");
    req.push_str(path);
    req.push_str(" HTTP/1.1\r\nHost: ");
    req.push_str(host);
    if server_port != 443 {
        req.push_str(&format!(":{}", server_port));
    }
    req.push_str("\r\nConnection: close\r\n\r\n");
    tls.write(req.as_bytes()).map_err(|_| ())?;
    tls.flush().map_err(|_| ())?;

    // Read the response: headers first, then the body.  Use Content-Length
    // when available (like http::http_get) so we don't depend on the server
    // closing the TCP connection.
    let mut headers = Vec::new();
    let mut headers_done = false;
    let mut content_length: Option<usize> = None;
    let mut body = Vec::new();
    let mut scratch = [0u8; 2048];
    loop {
        let n = tls.read(&mut scratch).map_err(|_| ())?;
        if n == 0 {
            break;
        }
        if !headers_done {
            headers.extend_from_slice(&scratch[..n]);
            if let Some(pos) = find_header_end(&headers) {
                content_length = parse_content_length(&headers[..pos]);
                body.extend_from_slice(&headers[pos + 4..]);
                headers.clear();
                headers_done = true;
            }
        } else {
            body.extend_from_slice(&scratch[..n]);
        }
        if let Some(expected) = content_length {
            if body.len() >= expected {
                break;
            }
        }
    }

    Ok(HttpsFetchResult { body, server_ip })
}

/// Find `\r\n\r\n` in a byte slice.
fn find_header_end(buf: &[u8]) -> Option<usize> {
    let mut i = 0;
    while i + 3 < buf.len() {
        if buf[i] == b'\r' && buf[i + 1] == b'\n' && buf[i + 2] == b'\r' && buf[i + 3] == b'\n' {
            return Some(i);
        }
        i += 1;
    }
    None
}

/// Parse a `Content-Length` header from the header block.
fn parse_content_length(headers: &[u8]) -> Option<usize> {
    let needle = b"content-length:";
    let mut i = 0;
    while i + needle.len() <= headers.len() {
        if headers[i..i + needle.len()].eq_ignore_ascii_case(needle) {
            let mut j = i + needle.len();
            while j < headers.len() && headers[j] == b' ' {
                j += 1;
            }
            let mut value = 0usize;
            let mut any = false;
            while j < headers.len() && headers[j].is_ascii_digit() {
                value = value
                    .saturating_mul(10)
                    .saturating_add((headers[j] - b'0') as usize);
                any = true;
                j += 1;
            }
            if any {
                return Some(value);
            }
        }
        i += 1;
    }
    None
}
