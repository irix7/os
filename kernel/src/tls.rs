// SPDX-License-Identifier: GPL-3.0-only
//! HTTPS/TLS client for the kernel console.
//!
//! Performs a TLS 1.3 handshake over a smoltcp TCP socket using
//! `embedded-tls`, then speaks HTTP/1.0 inside the tunnel.  The stack is
//! polled, not async: the blocking I/O adapter polls the shared interface
//! until the socket can make progress.
//!
//! A full-body GET is the module transport.  `get_range` and `get_range_to`
//! add a single `Range: bytes=start-end` request: the response must be 206
//! with exactly the requested length, and the body is hashed as it arrives.
//! Certificate verification is identical on both paths.
//!
//! Certificate chains are verified with the `rustpki` verifier against a
//! small embedded root set (see `ca/`): SSL.com, USERTrust ECC and ISRG Root
//! X1.  GitHub's raw and release hosts send a Let's Encrypt chain whose last
//! entry is `Root YR` cross-signed by ISRG Root X1; the kernel selects the
//! anchor from that issuer.  The verifier clock comes from NTP; before a sync
//! the validity dates are not checked, but the chain signatures and hostname
//! still are.  The RNG is xorshift128+ seeded from CP0 Count.

use alloc::boxed::Box;
use alloc::format;
use core::fmt;

use embedded_io::{ErrorType, Read, Write};
use embedded_tls::blocking::{
    Aes128GcmSha256, CryptoProvider, TlsConfig, TlsConnection, TlsContext, TlsVerifier,
};
use embedded_tls::pki::CertVerifier;
use embedded_tls::{
    Certificate, CertificateEntryRef, CertificateRef, TlsCipherSuite, TlsClock, TlsError,
};
use rand_core::{CryptoRng, CryptoRngCore, Error as RngError, RngCore};
use smoltcp::iface::{Interface, SocketHandle, SocketSet};
use smoltcp::socket::tcp::{Socket as TcpSocket, SocketBuffer, State};
use smoltcp::wire::{IpAddress, Ipv4Address};
use x509_cert::der::Decode;
use x509_cert::Certificate as X509Certificate;

use crate::http::{verify_range, Location, Summary};
use crate::net;
use crate::range::{append_decimal, ByteRange};

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

/// Crypto provider: xorshift RNG and an X.509 verifier.
struct TlsProvider<V> {
    rng: SimpleRng,
    verifier: V,
}

impl<V> CryptoProvider for TlsProvider<V>
where
    V: TlsVerifier<Aes128GcmSha256>,
{
    type CipherSuite = Aes128GcmSha256;
    type Signature = alloc::vec::Vec<u8>;

    fn rng(&mut self) -> impl CryptoRngCore {
        &mut self.rng
    }

    fn verifier(&mut self) -> Result<&mut impl TlsVerifier<Self::CipherSuite>, TlsError> {
        Ok(&mut self.verifier)
    }
}

/// TLS clock backed by the NTP-synchronised wall clock.
struct WallClock;

impl TlsClock for WallClock {
    fn now() -> Option<u64> {
        crate::ntp::now_unix()
    }
}

/// CA roots used to verify server chains.
///
/// The list is deliberately small: SSL.com for the project's signed hosts,
/// USERTrust ECC for GitHub, and ISRG Root X1 for Let's Encrypt-backed GitHub
/// release and Pages endpoints.  Let's Encrypt's current chains use RSA
/// signatures, so the `rsa` feature of `embedded-tls` is enabled as well.
const CA_ROOTS: &[&[u8]] = &[
    include_bytes!("ca/sslcom-ecc-root.der"),
    include_bytes!("ca/usertrust-ecc-root.der"),
    include_bytes!("ca/isrg-root-x1.der"),
];

/// Maximum server certificate size the verifier buffers.
const CERT_SIZE: usize = 8192;

/// Choose the embedded root that issued the last certificate in the chain.
///
/// The server sends its chain leaf-first, usually with either its self-signed
/// root or a cross-signed copy of that root appended.  Matching the issuer of
/// the last entry against the embedded root subjects selects the right anchor
/// without copying the certificate chain; the verifier still checks every
/// signature.
fn root_for(cert: &CertificateRef<'_>) -> Option<&'static [u8]> {
    let CertificateEntryRef::X509(last_der) = cert.entries.last()? else {
        return None;
    };
    let last = X509Certificate::from_der(last_der).ok()?;
    for root_der in CA_ROOTS {
        let root = X509Certificate::from_der(root_der).ok()?;
        if root.tbs_certificate.subject == last.tbs_certificate.issuer {
            return Some(root_der);
        }
    }
    None
}

/// Verifier that selects the embedded root for the server chain, then
/// delegates signature checks to the rustpki verifier.
struct PathVerifier {
    host: [u8; 64],
    host_len: usize,
    chosen: Option<CertVerifier<'static, Aes128GcmSha256, WallClock, CERT_SIZE>>,
}

impl PathVerifier {
    fn new() -> Self {
        Self {
            host: [0; 64],
            host_len: 0,
            chosen: None,
        }
    }
}

impl TlsVerifier<Aes128GcmSha256> for PathVerifier {
    fn set_hostname_verification(&mut self, hostname: &str) -> Result<(), TlsError> {
        if hostname.len() > self.host.len() {
            return Err(TlsError::InsufficientSpace);
        }
        self.host[..hostname.len()].copy_from_slice(hostname.as_bytes());
        self.host_len = hostname.len();
        Ok(())
    }

    fn verify_certificate(
        &mut self,
        transcript: &<Aes128GcmSha256 as TlsCipherSuite>::Hash,
        cert: CertificateRef,
    ) -> Result<(), TlsError> {
        let host = core::str::from_utf8(&self.host[..self.host_len]).unwrap_or("");
        let Some(root) = root_for(&cert) else {
            crate::println("tls: no matching root");
            return Err(TlsError::InvalidCertificate);
        };
        let mut verifier =
            CertVerifier::<Aes128GcmSha256, WallClock, CERT_SIZE>::new(Certificate::X509(root));
        verifier.set_hostname_verification(host)?;
        match verifier.verify_certificate(transcript, cert) {
            Ok(()) => {
                self.chosen = Some(verifier);
                Ok(())
            }
            Err(err) => {
                crate::println("tls: chain rejected");
                Err(err)
            }
        }
    }

    fn verify_signature(
        &mut self,
        verify: embedded_tls::CertificateVerifyRef,
    ) -> Result<(), TlsError> {
        match self.chosen.as_mut() {
            Some(verifier) => verifier.verify_signature(verify),
            None => Err(TlsError::InvalidCertificate),
        }
    }
}

/// Fetch `path` over HTTPS and return the status and body length.
pub fn get(ip: Ipv4Address, port: u16, host: &str, path: &str) -> Result<Summary, &'static str> {
    get_inner(ip, port, host, path, None, None).map(|(summary, _)| summary)
}

/// Fetch `path` over TLS and copy the body into `out`, returning the summary
/// and the number of bytes captured.  Fetching continues past a full `out`;
/// only the capture stops.
pub fn get_to(
    ip: Ipv4Address,
    port: u16,
    host: &str,
    path: &str,
    out: &mut [u8],
) -> Result<(Summary, usize), &'static str> {
    get_inner(ip, port, host, path, None, Some(out))
}

/// Fetch an inclusive byte range over HTTPS, as `HTTP Range: bytes=start-end`.
///
/// The response must be `206` and its body exactly the requested range; the
/// body is hashed but not buffered.
pub fn get_range(
    ip: Ipv4Address,
    port: u16,
    host: &str,
    path: &str,
    range: ByteRange,
) -> Result<Summary, &'static str> {
    let (summary, _) = get_inner(ip, port, host, path, Some(range), None)?;
    verify_range(&summary, range)?;
    Ok(summary)
}

/// Fetch an inclusive byte range over HTTPS into `out`.  The response must be
/// `206`, its body exactly the requested range, and `out` must be large
/// enough to hold it; the whole body is hashed.
pub fn get_range_to(
    ip: Ipv4Address,
    port: u16,
    host: &str,
    path: &str,
    range: ByteRange,
    out: &mut [u8],
) -> Result<(Summary, usize), &'static str> {
    let (summary, captured) = get_inner(ip, port, host, path, Some(range), Some(out))?;
    let want = verify_range(&summary, range)?;
    if captured != want {
        return Err("range buffer too small");
    }
    Ok((summary, captured))
}

fn get_inner(
    ip: Ipv4Address,
    port: u16,
    host: &str,
    path: &str,
    range: Option<ByteRange>,
    mut capture: Option<&mut [u8]>,
) -> Result<(Summary, usize), &'static str> {
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
    let provider = TlsProvider {
        rng: SimpleRng::new(seed),
        verifier: PathVerifier::new(),
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
        append_decimal(&mut req, &mut n, port as u32).ok_or("request too long")?;
    }
    if let Some(range) = range {
        append(&mut req, &mut n, b"\r\nRange: ")?;
        let mut value = [0u8; 40];
        let value_len = range.write_value(&mut value).ok_or("request too long")?;
        append(&mut req, &mut n, &value[..value_len])?;
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
    let mut location: Option<Location> = None;
    let mut body_len = 0usize;
    let mut total_rx = 0usize;
    let mut captured = 0usize;
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
            if let Some(buf) = capture.as_deref_mut() {
                let take = n.min(buf.len().saturating_sub(captured));
                buf[captured..captured + take].copy_from_slice(&scratch[..take]);
                captured += take;
            }
        } else {
            let take = n.min(headers.len() - header_len);
            headers[header_len..header_len + take].copy_from_slice(&scratch[..take]);
            header_len += take;
            if let Some(pos) = crate::http::find_header_end(&headers[..header_len]) {
                status = crate::http::parse_status(&headers[..header_len]);
                content_length = crate::http::parse_content_length(&headers[..header_len]);
                location = crate::http::parse_location(&headers[..header_len]);
                headers_done = true;
                body_len = total_rx.saturating_sub(pos + 4);
                let batch_start = total_rx - n;
                let body_off = (pos + 4).saturating_sub(batch_start).min(n);
                hasher.update(&scratch[body_off..n]);
                if let Some(buf) = capture.as_deref_mut() {
                    let body = &scratch[body_off..n];
                    let take = body.len().min(buf.len().saturating_sub(captured));
                    buf[captured..captured + take].copy_from_slice(&body[..take]);
                    captured += take;
                }
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
    Ok((
        Summary {
            status,
            body_len,
            content_length,
            sha256: hasher.finalize(),
            location,
        },
        captured,
    ))
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
