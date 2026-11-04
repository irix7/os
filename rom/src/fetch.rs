// SPDX-License-Identifier: GPL-3.0-only
//! HTTPS download for the ROM network stage (M1-ADR-0002).
//!
//! The ROM fetches the kernel over TLS 1.3 and follows redirects.  This module
//! mirrors the kernel's client (`kernel/src/tls.rs`, `kernel/src/http.rs`) but
//! takes its buffers from the stage's bump allocator (`netstage::buffer`),
//! which hands out RAM the stage owns.
//!
//! The ROM has no wall clock, so certificate validity dates are not checked.
//! Chain signatures and the host name are.  The kernel does the same before
//! its NTP sync.  The body is not hashed: M1-ADR-0002 accepts an unverified
//! kernel.

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
use smoltcp::socket::dns;
use smoltcp::socket::tcp::{Socket as TcpSocket, SocketBuffer, State};
use smoltcp::wire::{DnsQueryType, IpAddress, IpEndpoint, Ipv4Address};
use x509_cert::der::Decode;
use x509_cert::Certificate as X509Certificate;

use crate::netstage::{buffer, count, now, put, put_dec, stamp};

/// Longest URL the stage follows, including the scheme.
const URL_MAX: usize = 2048;
/// Longest `Location` value copied out of a redirect.
const LOCATION_MAX: usize = 2048;
/// Redirects followed before the fetch gives up.
const MAX_HOPS: usize = 4;
/// Headers larger than this fail the fetch.  GitHub redirects carry about 3 KiB.
const HEAD_MAX: usize = 8192;
/// embedded-tls asks for 16640 bytes of record buffer: a full 16 KiB record
/// plus the AEAD tag and content type.
const RECORD_READ_SIZE: usize = 16640;
const RECORD_WRITE_SIZE: usize = 4096;
const TCP_BUF: usize = 4096;
/// Per-operation timeout for TCP and TLS I/O.
const IO_TIMEOUT_MS: i64 = 30_000;
const CONNECT_TIMEOUT_MS: i64 = 15_000;
const DNS_TIMEOUT_MS: i64 = 10_000;
/// Local TCP port of the first hop; each later hop adds one.
const LOCAL_PORT_BASE: u16 = 49152;

/// CA roots: SSL.com ECC for project-signed hosts, USERTrust ECC for
/// github.com (Sectigo), ISRG Root X1 for Let's Encrypt release hosts.
const CA_ROOTS: &[&[u8]] = &[
    include_bytes!("ca/sslcom-ecc-root.der"),
    include_bytes!("ca/usertrust-ecc-root.der"),
    include_bytes!("ca/isrg-root-x1.der"),
];

/// Maximum server certificate size the verifier buffers.
const CERT_SIZE: usize = 8192;

/// The network stack the stage built during DHCP.
pub struct Stack<'a, 'b> {
    pub iface: Interface,
    pub dev: &'a mut irix7_seeq::SeeqDevice,
    pub sockets: SocketSet<'b>,
    pub dns: SocketHandle,
}

impl Stack<'_, '_> {
    fn poll(&mut self) {
        self.iface.poll(now(), &mut *self.dev, &mut self.sockets);
    }
}

/// How one request ended.
enum Reply {
    /// A 200 response; the body length in `out`.
    Body(usize),
    /// A 3xx response; the `Location` length in the caller's buffer.
    Redirect(usize),
}

/// Download `url` into `out` and return the body length.  Only `https://`
/// URLs with a host name are accepted.  Redirects may move to another host.
pub fn download(
    stack: &mut Stack<'_, '_>,
    url: &str,
    out: &mut [u8],
) -> Result<usize, &'static str> {
    let mut target = [0u8; URL_MAX];
    let mut target_len = copy_str(&mut target, url)?;
    let mut location = [0u8; LOCATION_MAX];

    for hop in 0..MAX_HOPS {
        let current = core::str::from_utf8(&target[..target_len]).map_err(|_| "url not utf-8")?;
        let (host, path) = split_https(current).ok_or("url is not https://host/path")?;
        put("rom-net: GET ");
        put(host);
        put(path);
        put("\n");
        match request(stack, hop as u16, host, path, out, &mut location)? {
            Reply::Body(n) => return Ok(n),
            Reply::Redirect(len) => {
                let next =
                    core::str::from_utf8(&location[..len]).map_err(|_| "location not utf-8")?;
                let mut tmp = [0u8; URL_MAX];
                let n = if next.starts_with("https://") {
                    copy_str(&mut tmp, next)?
                } else if next.starts_with('/') {
                    let prefix = "https://";
                    let total = prefix.len() + host.len() + next.len();
                    if total > URL_MAX {
                        return Err("redirect url too long");
                    }
                    let mut n = copy_str(&mut tmp, prefix)?;
                    n += copy_str(&mut tmp[n..], host)?;
                    n += copy_str(&mut tmp[n..], next)?;
                    n
                } else {
                    return Err("unsupported redirect location");
                };
                target[..n].copy_from_slice(&tmp[..n]);
                target_len = n;
            }
        }
    }
    Err("too many redirects")
}

fn request(
    stack: &mut Stack<'_, '_>,
    hop: u16,
    host: &str,
    path: &str,
    out: &mut [u8],
    location: &mut [u8],
) -> Result<Reply, &'static str> {
    let ip = resolve(stack, host)?;
    stamp("dns");
    let handle = connect(stack, ip, LOCAL_PORT_BASE + hop)?;
    stamp("tcp up");
    let result = exchange(stack, handle, host, path, out, location);
    cleanup(stack, handle);
    result
}

/// Resolve `name` to its first IPv4 address through the DHCP DNS server.
fn resolve(stack: &mut Stack<'_, '_>, name: &str) -> Result<Ipv4Address, &'static str> {
    let query = stack
        .sockets
        .get_mut::<dns::Socket>(stack.dns)
        .start_query(stack.iface.context(), name, DnsQueryType::A)
        .map_err(|_| "dns start failed")?;
    let start = now().total_millis();
    loop {
        stack.poll();
        match stack
            .sockets
            .get_mut::<dns::Socket>(stack.dns)
            .get_query_result(query)
        {
            Ok(addrs) => {
                return match addrs.first() {
                    Some(IpAddress::Ipv4(a)) => Ok(*a),
                    _ => Err("dns: no ipv4 answer"),
                };
            }
            Err(dns::GetQueryResultError::Pending) => {}
            Err(_) => return Err("dns query failed"),
        }
        if now().total_millis() - start > DNS_TIMEOUT_MS {
            return Err("dns timed out");
        }
    }
}

fn connect(
    stack: &mut Stack<'_, '_>,
    ip: Ipv4Address,
    local_port: u16,
) -> Result<SocketHandle, &'static str> {
    let rx = buffer(TCP_BUF).ok_or("out of ram")?;
    let tx = buffer(TCP_BUF).ok_or("out of ram")?;
    let handle = stack
        .sockets
        .add(TcpSocket::new(SocketBuffer::new(rx), SocketBuffer::new(tx)));
    let remote = IpEndpoint::new(IpAddress::Ipv4(ip), 443);
    let started = stack
        .sockets
        .get_mut::<TcpSocket>(handle)
        .connect(stack.iface.context(), remote, local_port)
        .is_ok();
    if !started {
        let _ = stack.sockets.remove(handle);
        return Err("tcp connect refused");
    }
    let start = now().total_millis();
    loop {
        stack.poll();
        match stack.sockets.get::<TcpSocket>(handle).state() {
            State::Established => return Ok(handle),
            State::Closed | State::LastAck => {
                let _ = stack.sockets.remove(handle);
                return Err("tcp connection closed");
            }
            _ => {}
        }
        if now().total_millis() - start > CONNECT_TIMEOUT_MS {
            let _ = stack.sockets.remove(handle);
            return Err("tcp connect timed out");
        }
    }
}

/// Close the TCP connection, let the FIN drain, then drop the socket.
fn cleanup(stack: &mut Stack<'_, '_>, handle: SocketHandle) {
    let _ = stack.sockets.get_mut::<TcpSocket>(handle).close();
    let start = now().total_millis();
    loop {
        stack.poll();
        match stack.sockets.get::<TcpSocket>(handle).state() {
            State::Closed | State::TimeWait | State::LastAck => break,
            _ => {}
        }
        if now().total_millis() - start > 500 {
            break;
        }
    }
    let _ = stack.sockets.remove(handle);
}

fn exchange(
    stack: &mut Stack<'_, '_>,
    handle: SocketHandle,
    host: &str,
    path: &str,
    out: &mut [u8],
    location: &mut [u8],
) -> Result<Reply, &'static str> {
    let record_read = buffer(RECORD_READ_SIZE).ok_or("out of ram")?;
    let record_write = buffer(RECORD_WRITE_SIZE).ok_or("out of ram")?;
    let config = TlsConfig::new()
        .with_server_name(host)
        .enable_rsa_signatures();
    let provider = TlsProvider {
        rng: SimpleRng::new(count() as u64),
        verifier: PathVerifier::new(),
    };
    let io = Io { stack, handle };
    let mut tls = TlsConnection::new(io, record_read, record_write);
    tls.open(TlsContext::new(&config, provider))
        .map_err(|_| "tls handshake failed")?;
    stamp("tls up");

    let mut req = [0u8; URL_MAX + 128];
    let mut n = 0;
    append(&mut req, &mut n, b"GET ")?;
    append(&mut req, &mut n, path.as_bytes())?;
    append(&mut req, &mut n, b" HTTP/1.1\r\nHost: ")?;
    append(&mut req, &mut n, host.as_bytes())?;
    append(
        &mut req,
        &mut n,
        b"\r\nUser-Agent: irix7-rom\r\nConnection: close\r\n\r\n",
    )?;
    tls.write_all(&req[..n]).map_err(|_| "tls write failed")?;
    tls.flush().map_err(|_| "tls flush failed")?;

    let mut head = [0u8; HEAD_MAX];
    let mut head_len = 0usize;
    let mut headers_done = false;
    let mut content_length: Option<usize> = None;
    let mut body = 0usize;
    let mut chunk = [0u8; 2048];

    loop {
        let got = match tls.read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => n,
            // Many servers close the TCP stream without close_notify.
            Err(TlsError::ConnectionClosed) => break,
            Err(_) => return Err("tls read failed"),
        };
        let mut data = &chunk[..got];

        if !headers_done {
            let take = data.len().min(HEAD_MAX - head_len);
            head[head_len..head_len + take].copy_from_slice(&data[..take]);
            head_len += take;
            data = &data[take..];

            let Some(end) = find_header_end(&head[..head_len]) else {
                if data.is_empty() {
                    continue;
                }
                return Err("response headers too large");
            };
            let headers = &head[..end];
            let status = parse_status(headers).ok_or("malformed status line")?;
            put("rom-net: http ");
            put_dec(status as u32);
            put("\n");
            if (300..400).contains(&status) {
                let value =
                    header_value(headers, b"location").ok_or("redirect without location")?;
                if value.len() > location.len() {
                    return Err("location too long");
                }
                location[..value.len()].copy_from_slice(value);
                return Ok(Reply::Redirect(value.len()));
            }
            if status != 200 {
                put("rom-net: http status ");
                put_dec(status as u32);
                put("\n");
                return Err("download did not return 200");
            }
            content_length = header_value(headers, b"content-length").and_then(decimal);
            headers_done = true;
            sink(out, &mut body, &head[end..head_len])?;
            sink(out, &mut body, data)?;
        } else {
            sink(out, &mut body, data)?;
        }
    }

    if !headers_done {
        return Err("no http response");
    }
    if let Some(expected) = content_length {
        if body != expected {
            return Err("download shorter than content-length");
        }
    }
    stamp("body done");
    Ok(Reply::Body(body))
}

fn sink(out: &mut [u8], body: &mut usize, bytes: &[u8]) -> Result<(), &'static str> {
    let end = *body + bytes.len();
    if end > out.len() {
        return Err("download larger than the kernel window");
    }
    out[*body..end].copy_from_slice(bytes);
    *body = end;
    Ok(())
}

/// Blocking byte transport over one smoltcp TCP socket.  Every read and write
/// polls the shared interface until the socket can make progress.
struct Io<'s, 'a, 'b> {
    stack: &'s mut Stack<'a, 'b>,
    handle: SocketHandle,
}

#[derive(Debug)]
enum IoError {
    Closed,
    Timeout,
}

impl core::fmt::Display for IoError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            IoError::Closed => write!(f, "connection closed"),
            IoError::Timeout => write!(f, "timeout"),
        }
    }
}

impl core::error::Error for IoError {}

impl embedded_io::Error for IoError {
    fn kind(&self) -> embedded_io::ErrorKind {
        embedded_io::ErrorKind::Other
    }
}

impl ErrorType for Io<'_, '_, '_> {
    type Error = IoError;
}

impl Read for Io<'_, '_, '_> {
    fn read(&mut self, buf: &mut [u8]) -> Result<usize, IoError> {
        let start = now().total_millis();
        loop {
            {
                let sock = self.stack.sockets.get_mut::<TcpSocket>(self.handle);
                if let Ok(n) = sock.recv_slice(buf) {
                    if n > 0 {
                        return Ok(n);
                    }
                }
                if matches!(
                    sock.state(),
                    State::Closed | State::CloseWait | State::LastAck
                ) {
                    return Ok(0);
                }
            }
            if now().total_millis() - start > IO_TIMEOUT_MS {
                return Err(IoError::Timeout);
            }
            self.stack.poll();
        }
    }
}

impl Write for Io<'_, '_, '_> {
    fn write(&mut self, buf: &[u8]) -> Result<usize, IoError> {
        let start = now().total_millis();
        loop {
            {
                let sock = self.stack.sockets.get_mut::<TcpSocket>(self.handle);
                if let Ok(n) = sock.send_slice(buf) {
                    if n > 0 {
                        return Ok(n);
                    }
                }
                if matches!(sock.state(), State::Closed | State::LastAck) {
                    return Err(IoError::Closed);
                }
            }
            if now().total_millis() - start > IO_TIMEOUT_MS {
                return Err(IoError::Timeout);
            }
            self.stack.poll();
        }
    }

    fn flush(&mut self) -> Result<(), IoError> {
        Ok(())
    }
}

/// Xorshift128+ generator seeded from CP0 Count.  Its output is not a
/// cryptographic source; the TLS key exchange uses the ephemeral key it
/// produces.  M1-ADR-0002 accepts this for the kernel download.
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
            Self::next(&mut s);
        }
        Self { s }
    }

    fn next(s: &mut [u64; 2]) -> u64 {
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
        Self::next(&mut self.s) as u32
    }

    fn next_u64(&mut self) -> u64 {
        Self::next(&mut self.s)
    }

    fn fill_bytes(&mut self, dest: &mut [u8]) {
        for chunk in dest.chunks_mut(8) {
            let n = Self::next(&mut self.s).to_le_bytes();
            chunk.copy_from_slice(&n[..chunk.len()]);
        }
    }

    fn try_fill_bytes(&mut self, dest: &mut [u8]) -> Result<(), RngError> {
        self.fill_bytes(dest);
        Ok(())
    }
}

impl CryptoRng for SimpleRng {}

struct TlsProvider<V> {
    rng: SimpleRng,
    verifier: V,
}

impl<V> CryptoProvider for TlsProvider<V>
where
    V: TlsVerifier<Aes128GcmSha256>,
{
    type CipherSuite = Aes128GcmSha256;
    /// Only client-certificate signing uses this type, and the stage never
    /// presents a client certificate.
    type Signature = &'static [u8];

    fn rng(&mut self) -> impl CryptoRngCore {
        &mut self.rng
    }

    fn verifier(&mut self) -> Result<&mut impl TlsVerifier<Self::CipherSuite>, TlsError> {
        Ok(&mut self.verifier)
    }
}

/// The ROM has no wall clock until a time source is wired in, so the TLS
/// layer skips validity checks.
struct NoClock;

impl TlsClock for NoClock {
    fn now() -> Option<u64> {
        None
    }
}

/// Choose the embedded root that issued the last certificate in the chain.
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
    chosen: Option<CertVerifier<'static, Aes128GcmSha256, NoClock, CERT_SIZE>>,
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
            put("rom-net: tls: no matching root\n");
            return Err(TlsError::InvalidCertificate);
        };
        let mut verifier =
            CertVerifier::<Aes128GcmSha256, NoClock, CERT_SIZE>::new(Certificate::X509(root));
        verifier.set_hostname_verification(host)?;
        match verifier.verify_certificate(transcript, cert) {
            Ok(()) => {
                self.chosen = Some(verifier);
                Ok(())
            }
            Err(err) => {
                put("rom-net: tls: chain rejected\n");
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

fn find_header_end(head: &[u8]) -> Option<usize> {
    head.windows(4)
        .position(|w| w == b"\r\n\r\n")
        .map(|p| p + 4)
}

fn parse_status(head: &[u8]) -> Option<u16> {
    let line = head.get(..head.iter().position(|&b| b == b'\r')?)?;
    let code = line.split(|&b| b == b' ').nth(1)?;
    decimal(code).map(|v| v as u16)
}

/// Value of the first header named `name`, compared case-insensitively.
fn header_value<'h>(head: &'h [u8], name: &[u8]) -> Option<&'h [u8]> {
    for line in head.split(|&b| b == b'\n').skip(1) {
        let line = line.strip_suffix(b"\r").unwrap_or(line);
        let colon = line.iter().position(|&b| b == b':')?;
        let (key, value) = line.split_at(colon);
        if key.eq_ignore_ascii_case(name) {
            let value = &value[1..];
            let start = value
                .iter()
                .position(|&b| b != b' ' && b != b'\t')
                .unwrap_or(value.len());
            return Some(&value[start..]);
        }
    }
    None
}

fn decimal(text: &[u8]) -> Option<usize> {
    if text.is_empty() {
        return None;
    }
    let mut value: usize = 0;
    for &b in text {
        if !b.is_ascii_digit() {
            return None;
        }
        value = value.checked_mul(10)?.checked_add((b - b'0') as usize)?;
    }
    Some(value)
}

/// Split `https://host/path` into `host` and `path`.  A missing path is `/`.
/// Ports are not supported: a `:port` suffix on the host is rejected by DNS.
fn split_https(url: &str) -> Option<(&str, &str)> {
    let rest = url.strip_prefix("https://")?;
    match rest.find('/') {
        Some(i) => Some((&rest[..i], &rest[i..])),
        None => Some((rest, "/")),
    }
}

fn copy_str(dst: &mut [u8], text: &str) -> Result<usize, &'static str> {
    let bytes = text.as_bytes();
    if bytes.len() > dst.len() {
        return Err("url too long");
    }
    dst[..bytes.len()].copy_from_slice(bytes);
    Ok(bytes.len())
}

fn append(buf: &mut [u8], len: &mut usize, data: &[u8]) -> Result<(), &'static str> {
    if *len + data.len() > buf.len() {
        return Err("request too long");
    }
    buf[*len..*len + data.len()].copy_from_slice(data);
    *len += data.len();
    Ok(())
}
