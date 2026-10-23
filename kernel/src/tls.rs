// SPDX-License-Identifier: GPL-3.0-only
//! HTTPS/TLS client for the kernel console.
//!
//! Performs a TLS 1.3 handshake over a smoltcp TCP socket using
//! `embedded-tls`, then speaks HTTP/1.0 inside the tunnel.  The stack is
//! polled, not async: the blocking I/O adapter polls the shared interface
//! until the socket can make progress.
//!
//! Certificate verification is deliberately permissive (`NoVerify`) during
//! bring-up; a baked-in root store is the follow-up once the kernel has a
//! filesystem to carry one.  The RNG is xorshift128+ seeded from CP0 Count.

use alloc::boxed::Box;
use alloc::format;
use core::fmt;

use embedded_io::{ErrorType, Read, Write};
use embedded_tls::blocking::{
    Aes128GcmSha256, CryptoProvider, NoVerify, TlsConfig, TlsConnection, TlsContext, TlsVerifier,
};
use embedded_tls::TlsError;
use rand_core::{CryptoRng, CryptoRngCore, Error as RngError, RngCore};
use smoltcp::iface::{Interface, SocketHandle, SocketSet};
use smoltcp::socket::tcp::{Socket as TcpSocket, SocketBuffer, State};
use smoltcp::wire::{IpAddress, Ipv4Address};

use crate::http::Summary;
use crate::net;

const TIMEOUT_MS: i64 = 30_000;

/// A blocking I/O adapter over a smoltcp TCP socket.
struct SocketIo {
    handle: SocketHandle,
    iface: *mut Interface,
    sockets: *mut SocketSet<'static>,
    dev: *mut crate::net::SeeqDevice,
}

impl SocketIo {
    fn poll_once(&mut self) {
        // SAFETY: the pointers come from net::raw_stack() and are used only
        // by this adapter for the duration of the fetch.
        unsafe { net::poll_raw(self.iface, self.sockets, self.dev) };
    }
}

/// I/O error surfaced to embedded-tls.
#[derive(Debug)]
enum TlsIoError {
    Closed,
    Timeout,
}

impl fmt::Display for TlsIoError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TlsIoError::Closed => write!(f, "connection closed"),
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
        let deadline = net::clock_now().total_millis() + TIMEOUT_MS;
        loop {
            // SAFETY: the socket lives in the static set.
            let sock = unsafe { &mut *self.sockets }.get_mut::<TcpSocket>(self.handle);
            if let Ok(n) = sock.recv_slice(buf) {
                if n > 0 {
                    return Ok(n);
                }
                let state = sock.state();
                if matches!(state, State::Closed | State::CloseWait | State::LastAck) {
                    return Ok(0);
                }
            }
            if net::clock_now().total_millis() > deadline {
                return Err(TlsIoError::Timeout);
            }
            self.poll_once();
        }
    }
}

impl Write for SocketIo {
    fn write(&mut self, buf: &[u8]) -> Result<usize, Self::Error> {
        let deadline = net::clock_now().total_millis() + TIMEOUT_MS;
        let mut written = 0;
        while written < buf.len() {
            // SAFETY: the socket lives in the static set.
            let sock = unsafe { &mut *self.sockets }.get_mut::<TcpSocket>(self.handle);
            if let Ok(n) = sock.send_slice(&buf[written..]) {
                if n > 0 {
                    written += n;
                    continue;
                }
            }
            if matches!(sock.state(), State::Closed | State::LastAck) {
                return Err(TlsIoError::Closed);
            }
            if net::clock_now().total_millis() > deadline {
                return Err(TlsIoError::Timeout);
            }
            self.poll_once();
        }
        Ok(written)
    }

    fn flush(&mut self) -> Result<(), Self::Error> {
        Ok(())
    }
}

/// Xorshift128+ PRNG, seeded from CP0 Count.
struct SimpleRng {
    s: [u64; 2],
}

impl SimpleRng {
    fn new(seed: u64) -> Self {
        let mut s = [
            seed.wrapping_mul(0x9E37_79B9_7F4A_7C15),
            seed.wrapping_add(0x9E37_79B9_7F4A_7C15),
        ];
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

    fn try_fill_bytes(&mut self, dest: &mut [u8]) -> Result<(), RngError> {
        self.fill_bytes(dest);
        Ok(())
    }
}

impl CryptoRng for SimpleRng {}

/// Crypto provider: xorshift RNG and permissive verification.
struct TlsProvider {
    rng: SimpleRng,
    verifier: NoVerify,
}

impl CryptoProvider for TlsProvider {
    type CipherSuite = Aes128GcmSha256;
    type Signature = alloc::vec::Vec<u8>;

    fn rng(&mut self) -> impl CryptoRngCore {
        &mut self.rng
    }

    fn verifier(&mut self) -> Result<&mut impl TlsVerifier<Self::CipherSuite>, TlsError> {
        Ok(&mut self.verifier)
    }
}

/// Fetch `path` over HTTPS and return the status and body length.
pub fn get(ip: Ipv4Address, port: u16, host: &str, path: &str) -> Result<Summary, &'static str> {
    // SAFETY: single-threaded kernel; the pointers are used by the adapter
    // for the duration of this fetch and nothing else borrows the stack.
    let (iface_ptr, sockets_ptr, dev_ptr) =
        unsafe { net::raw_stack() }.ok_or("network stack is not up")?;

    // TCP buffers must outlive the socket.
    let rx = Box::leak(Box::new([0u8; 4096]));
    let tx = Box::leak(Box::new([0u8; 4096]));
    // SAFETY: the socket set is exclusively ours for this fetch.
    let handle = unsafe {
        (*sockets_ptr).add(TcpSocket::new(
            SocketBuffer::new(&mut rx[..]),
            SocketBuffer::new(&mut tx[..]),
        ))
    };

    let connected = unsafe {
        (*sockets_ptr)
            .get_mut::<TcpSocket>(handle)
            .connect((*iface_ptr).context(), (IpAddress::Ipv4(ip), port), 49152)
            .is_ok()
    };
    if !connected {
        remove_socket(handle);
        return Err("connect failed");
    }

    let deadline = net::clock_now().total_millis() + TIMEOUT_MS;
    loop {
        unsafe { net::poll_raw(iface_ptr, sockets_ptr, dev_ptr) };
        let state = unsafe { (*sockets_ptr).get_mut::<TcpSocket>(handle).state() };
        match state {
            State::Established => break,
            State::Closed | State::LastAck => {
                remove_socket(handle);
                return Err("connection closed");
            }
            _ => {}
        }
        if net::clock_now().total_millis() > deadline {
            remove_socket(handle);
            return Err("connect timed out");
        }
    }

    let record_read = Box::leak(Box::new([0u8; 16384]));
    let record_write = Box::leak(Box::new([0u8; 4096]));

    let seed = crate::traps::count_read() as u64;
    let mut provider = TlsProvider {
        rng: SimpleRng::new(seed),
        verifier: NoVerify,
    };
    let config = TlsConfig::new()
        .with_server_name(host)
        .enable_rsa_signatures();

    let mut tls = TlsConnection::new(
        SocketIo {
            handle,
            iface: iface_ptr,
            sockets: sockets_ptr,
            dev: dev_ptr,
        },
        record_read,
        record_write,
    );
    let context = TlsContext::new(&config, provider);
    if let Err(err) = tls.open(context) {
        crate::print_str("tls: open failed: ");
        crate::println(&format!("{:?}", err));
        remove_socket(handle);
        return Err("tls handshake failed");
    }

    // Build the request in a fixed buffer.
    let mut req = [0u8; 384];
    let mut n = 0;
    append(&mut req, &mut n, b"GET ")?;
    append(&mut req, &mut n, path.as_bytes())?;
    append(&mut req, &mut n, b" HTTP/1.0\r\nHost: ")?;
    append(&mut req, &mut n, host.as_bytes())?;
    if port != 443 {
        append(&mut req, &mut n, b":")?;
        append_decimal(&mut req, &mut n, port as u32)?;
    }
    append(&mut req, &mut n, b"\r\nConnection: close\r\n\r\n")?;
    if tls.write_all(&req[..n]).is_err() || tls.flush().is_err() {
        remove_socket(handle);
        return Err("tls write failed");
    }

    let mut headers = [0u8; 1024];
    let mut header_len = 0usize;
    let mut headers_done = false;
    let mut status = 0u16;
    let mut content_length: Option<usize> = None;
    let mut body_len = 0usize;
    let mut total_rx = 0usize;
    let mut hasher = crate::sha256::Hasher::new();
    let mut scratch = [0u8; 2048];

    let deadline = net::clock_now().total_millis() + TIMEOUT_MS;
    loop {
        let n = match tls.read(&mut scratch) {
            Ok(0) => break,
            Ok(n) => n,
            // Many servers close the TCP stream without a TLS close_notify.
            // embedded-tls reports that as ConnectionClosed; treat it as a
            // normal end of response instead of a failure.
            Err(embedded_tls::TlsError::ConnectionClosed) => break,
            Err(err) => {
                crate::print_str("tls: read failed: ");
                crate::println(&format!("{:?}", err));
                crate::print_str("tls: bytes before failure: ");
                crate::print_hex32(total_rx as u32);
                crate::println("");
                remove_socket(handle);
                return Err("tls read failed");
            }
        };
        total_rx += n;
        if headers_done {
            body_len += n;
            hasher.update(&scratch[..n]);
        } else {
            let take = n.min(headers.len() - header_len);
            headers[header_len..header_len + take].copy_from_slice(&scratch[..take]);
            header_len += take;
            if let Some(pos) = crate::http::find_header_end(&headers[..header_len]) {
                status = crate::http::parse_status(&headers[..header_len]);
                content_length = crate::http::parse_content_length(&headers[..header_len]);
                headers_done = true;
                body_len = total_rx.saturating_sub(pos + 4);
                let batch_start = total_rx - n;
                let body_off = (pos + 4).saturating_sub(batch_start).min(n);
                hasher.update(&scratch[body_off..n]);
            } else if header_len == headers.len() {
                remove_socket(handle);
                return Err("response headers too large");
            }
        }
        if let Some(expected) = content_length {
            if body_len >= expected {
                break;
            }
        }
        if net::clock_now().total_millis() > deadline {
            remove_socket(handle);
            return Err("receive timed out");
        }
    }

    remove_socket(handle);
    if status == 0 {
        return Err("malformed response");
    }
    Ok(Summary {
        status,
        body_len,
        content_length,
        sha256: hasher.finalize(),
    })
}

fn remove_socket(handle: SocketHandle) {
    // SAFETY: the stack is not borrowed elsewhere after the fetch ends.
    unsafe {
        if let Some((_, sockets, _)) = net::raw_stack() {
            let _ = (*sockets).remove(handle);
        }
    }
}

fn append(buf: &mut [u8], len: &mut usize, data: &[u8]) -> Result<(), &'static str> {
    if *len + data.len() > buf.len() {
        return Err("request too long");
    }
    buf[*len..*len + data.len()].copy_from_slice(data);
    *len += data.len();
    Ok(())
}

fn append_decimal(buf: &mut [u8], len: &mut usize, mut v: u32) -> Result<(), &'static str> {
    let mut digits = [0u8; 10];
    let mut n = 0;
    if v == 0 {
        digits[0] = b'0';
        n = 1;
    }
    while v > 0 && n < digits.len() {
        digits[n] = b'0' + (v % 10) as u8;
        v /= 10;
        n += 1;
    }
    for i in (0..n).rev() {
        append(buf, len, &digits[i..i + 1])?;
    }
    Ok(())
}
