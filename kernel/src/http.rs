// SPDX-License-Identifier: GPL-3.0-only
//! Minimal HTTP/1.1 client over the shared smoltcp stack.
//!
//! One fetch at a time.  The body is not buffered: the client parses the
//! status line and counts body bytes, which is enough for a console command
//! and keeps the kernel heap-free.

use core::sync::atomic::{AtomicBool, Ordering};

use smoltcp::socket::tcp::{Socket as TcpSocket, SocketBuffer, State};
use smoltcp::wire::{IpAddress, IpEndpoint, Ipv4Address};

use crate::net;
use crate::range::{append_decimal, ByteRange};

/// Socket buffers for the single in-flight fetch.
static mut HTTP_RX_BUF: [u8; 4096] = [0; 4096];
static mut HTTP_TX_BUF: [u8; 2048] = [0; 2048];

/// Only one fetch may run at a time.
static FETCHING: AtomicBool = AtomicBool::new(false);

/// Maximum `Location` header captured for a redirect.
pub(crate) const LOCATION_MAX: usize = 512;

/// A bounded redirect target.
#[derive(Clone, Copy)]
pub struct Location {
    bytes: [u8; LOCATION_MAX],
    len: usize,
}

impl Location {
    pub fn as_str(&self) -> Option<&str> {
        core::str::from_utf8(&self.bytes[..self.len]).ok()
    }
}

/// Result of a completed fetch.
pub struct Summary {
    /// HTTP status code from the response line.
    pub status: u16,
    /// Body length in bytes as counted by the client.
    pub body_len: usize,
    /// `Content-Length` from the response headers, when present.
    pub content_length: Option<usize>,
    /// SHA-256 of the body bytes.
    pub sha256: crate::sha256::Sha256,
    /// `Location` header, when the response has one.
    pub location: Option<Location>,
}

/// Fetch `path` from `ip:port` and return the status and body length.
pub fn get(ip: Ipv4Address, port: u16, host: &str, path: &str) -> Result<Summary, &'static str> {
    if FETCHING.swap(true, Ordering::SeqCst) {
        return Err("a fetch is already running");
    }
    let result = get_inner(ip, port, host, path, None, None).map(|(summary, _)| summary);
    FETCHING.store(false, Ordering::SeqCst);
    result
}

/// Fetch `path` and copy the body into `out`, returning the summary and the
/// number of bytes captured.  The fetch stops capturing when `out` is full;
/// the body is still consumed and hashed.
pub fn get_to(
    ip: Ipv4Address,
    port: u16,
    host: &str,
    path: &str,
    out: &mut [u8],
) -> Result<(Summary, usize), &'static str> {
    if FETCHING.swap(true, Ordering::SeqCst) {
        return Err("a fetch is already running");
    }
    let result = get_inner(ip, port, host, path, None, Some(out));
    FETCHING.store(false, Ordering::SeqCst);
    result
}

/// Fetch an inclusive byte range and verify a 206 response with the requested
/// length.  Returns the summary; the body is hashed but not buffered.
pub fn get_range(
    ip: Ipv4Address,
    port: u16,
    host: &str,
    path: &str,
    range: ByteRange,
) -> Result<Summary, &'static str> {
    if FETCHING.swap(true, Ordering::SeqCst) {
        return Err("a fetch is already running");
    }
    let result = (|| {
        let (summary, _) = get_inner(ip, port, host, path, Some(range), None)?;
        verify_range(&summary, range)?;
        Ok(summary)
    })();
    FETCHING.store(false, Ordering::SeqCst);
    result
}

/// Fetch an inclusive byte range into `out` and verify a 206 response with the
/// requested length.  The whole body is hashed; `out` must be at least as long
/// as the requested range or the call fails after consuming the response.
pub fn get_range_to(
    ip: Ipv4Address,
    port: u16,
    host: &str,
    path: &str,
    range: ByteRange,
    out: &mut [u8],
) -> Result<(Summary, usize), &'static str> {
    if FETCHING.swap(true, Ordering::SeqCst) {
        return Err("a fetch is already running");
    }
    let result = (|| {
        let (summary, captured) = get_inner(ip, port, host, path, Some(range), Some(out))?;
        let want = verify_range(&summary, range)?;
        if captured != want {
            return Err("range buffer too small");
        }
        Ok((summary, captured))
    })();
    FETCHING.store(false, Ordering::SeqCst);
    result
}

/// Require a 206 response whose body is exactly the requested range, and
/// return that length.  Shared with the TLS client.
pub fn verify_range(summary: &Summary, range: ByteRange) -> Result<usize, &'static str> {
    crate::range::check_response(
        summary.status,
        summary.body_len,
        summary.content_length,
        range,
    )
}

fn get_inner(
    ip: Ipv4Address,
    port: u16,
    host: &str,
    path: &str,
    range: Option<ByteRange>,
    mut capture: Option<&mut [u8]>,
) -> Result<(Summary, usize), &'static str> {
    let handle = net::sockets_with(|sockets| {
        // SAFETY: the buffers are static and used by one fetch at a time.
        let rx = unsafe { &mut HTTP_RX_BUF[..] };
        let tx = unsafe { &mut HTTP_TX_BUF[..] };
        let socket = TcpSocket::new(SocketBuffer::new(rx), SocketBuffer::new(tx));
        sockets.add(socket)
    })
    .ok_or("network stack is not up")?;

    let remote = IpEndpoint::new(IpAddress::Ipv4(ip), port);
    let connected = net::with_stack(|iface, sockets| {
        sockets
            .get_mut::<TcpSocket>(handle)
            .connect(iface.context(), remote, 49152)
            .is_ok()
    });
    if connected != Some(true) {
        cleanup(handle);
        return Err("connect failed");
    }

    // Wait for the connection to establish.
    let deadline = net::clock_now().total_millis() + 5_000;
    loop {
        net::poll();
        let state = net::sockets_with(|sockets| sockets.get_mut::<TcpSocket>(handle).state());
        match state {
            Some(State::Established) => break,
            Some(State::Closed) | Some(State::LastAck) => {
                cleanup(handle);
                return Err("connection closed");
            }
            Some(_) => {}
            None => {
                cleanup(handle);
                return Err("network stack went away");
            }
        }
        if net::clock_now().total_millis() > deadline {
            cleanup(handle);
            return Err("connect timed out");
        }
    }

    // Build and send the request.
    let mut req = [0u8; 256];
    let mut n = 0;
    append(&mut req, &mut n, b"GET ")?;
    append(&mut req, &mut n, path.as_bytes())?;
    append(&mut req, &mut n, b" HTTP/1.1\r\nHost: ")?;
    append(&mut req, &mut n, host.as_bytes())?;
    if port != 80 {
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

    let send_deadline = net::clock_now().total_millis() + 5_000;
    let mut sent = 0;
    while sent < n {
        let wrote = net::sockets_with(|sockets| {
            sockets
                .get_mut::<TcpSocket>(handle)
                .send_slice(&req[sent..n])
                .unwrap_or(0)
        })
        .unwrap_or(0);
        sent += wrote;
        if sent < n {
            net::poll();
            if net::clock_now().total_millis() > send_deadline {
                cleanup(handle);
                return Err("send timed out");
            }
        }
    }

    // Read the response: headers into a small buffer, body counted.
    let mut headers = [0u8; 4096];
    let mut header_len = 0usize;
    let mut headers_done = false;
    let mut status = 0u16;
    let mut body_len = 0usize;
    let mut content_length: Option<usize> = None;
    let mut location: Option<Location> = None;
    let mut total_rx = 0usize;
    let mut captured = 0usize;
    let mut hasher = crate::sha256::Hasher::new();
    let mut last_state;

    let recv_deadline = net::clock_now().total_millis() + 10_000;
    loop {
        net::poll();

        let step = net::sockets_with(|sockets| {
            let sock = sockets.get_mut::<TcpSocket>(handle);
            let mut chunk = [0u8; 2048];
            let mut len = 0;
            while len < chunk.len() && sock.can_recv() {
                match sock.recv_slice(&mut chunk[len..]) {
                    Ok(0) => break,
                    Ok(n) => len += n,
                    Err(_) => break,
                }
            }
            let state = sock.state();
            (len, chunk, state)
        });

        let Some((n, chunk, state)) = step else {
            cleanup(handle);
            return Err("network stack went away");
        };

        if n > 0 {
            total_rx += n;
            let data = &chunk[..n];
            if headers_done {
                body_len += n;
                hasher.update(data);
                if let Some(buf) = capture.as_deref_mut() {
                    let take = n.min(buf.len().saturating_sub(captured));
                    buf[captured..captured + take].copy_from_slice(&data[..take]);
                    captured += take;
                }
            } else {
                let take = n.min(headers.len() - header_len);
                headers[header_len..header_len + take].copy_from_slice(&data[..take]);
                header_len += take;
                if let Some(pos) = find_header_end(&headers[..header_len]) {
                    status = parse_status(&headers[..header_len]);
                    content_length = parse_content_length(&headers[..header_len]);
                    location = parse_location(&headers[..header_len]);
                    headers_done = true;
                    // `pos` is relative to the start of the response, so the
                    // header occupies `pos + 4` bytes of the stream.  Any
                    // bytes already received beyond that are body, including
                    // the part of this batch that did not fit the header
                    // buffer.
                    body_len = total_rx.saturating_sub(pos + 4);
                    let batch_start = total_rx - n;
                    let body_off = (pos + 4).saturating_sub(batch_start).min(n);
                    hasher.update(&data[body_off..]);
                    if let Some(buf) = capture.as_deref_mut() {
                        let body = &data[body_off..];
                        let take = body.len().min(buf.len().saturating_sub(captured));
                        buf[captured..captured + take].copy_from_slice(&body[..take]);
                        captured += take;
                    }
                } else if header_len == headers.len() {
                    cleanup(handle);
                    return Err("response headers too large");
                }
            }
        }

        last_state = state;
        match state {
            // The server closes first because the request asked for
            // `Connection: close`; smoltcp reports that as CloseWait.
            State::Closed | State::TimeWait | State::LastAck | State::CloseWait => break,
            _ => {}
        }
        if net::clock_now().total_millis() > recv_deadline {
            cleanup(handle);
            return Err("receive timed out");
        }
    }

    cleanup(handle);
    crate::print_str("http dbg rx=");
    crate::print_hex32(total_rx as u32);
    crate::print_str(" hdr=");
    crate::print_hex32(header_len as u32);
    crate::print_str(" body=");
    crate::print_hex32(body_len as u32);
    crate::print_str(" cl=");
    crate::print_hex32(content_length.unwrap_or(0) as u32);
    crate::print_str(" state=");
    crate::print_hex32(state_code(last_state));
    crate::println("");
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

/// Remove the socket so repeated fetches do not exhaust the set.
fn cleanup(handle: smoltcp::iface::SocketHandle) {
    net::sockets_with(|sockets| {
        let _ = sockets.remove(handle);
    });
}

/// Numeric state tag for diagnostics.
fn state_code(state: State) -> u32 {
    match state {
        State::Closed => 0,
        State::Listen => 1,
        State::SynSent => 2,
        State::SynReceived => 3,
        State::Established => 4,
        State::FinWait1 => 5,
        State::FinWait2 => 6,
        State::CloseWait => 7,
        State::Closing => 8,
        State::LastAck => 9,
        State::TimeWait => 10,
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

pub(crate) fn find_header_end(haystack: &[u8]) -> Option<usize> {
    haystack.windows(4).position(|w| w == b"\r\n\r\n")
}

/// Find the `Content-Length` header value, case-insensitively.
pub(crate) fn parse_content_length(headers: &[u8]) -> Option<usize> {
    const NAME: &[u8] = b"content-length:";
    let mut i = 0;
    while i + NAME.len() <= headers.len() {
        if headers[i..i + NAME.len()].eq_ignore_ascii_case(NAME) {
            let mut j = i + NAME.len();
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
            return if any { Some(value) } else { None };
        }
        i += 1;
    }
    None
}

/// Find the `Location` header value, case-insensitively.
pub(crate) fn parse_location(headers: &[u8]) -> Option<Location> {
    const NAME: &[u8] = b"location:";
    let mut i = 0;
    while i + NAME.len() <= headers.len() {
        if headers[i..i + NAME.len()].eq_ignore_ascii_case(NAME) {
            let mut j = i + NAME.len();
            while j < headers.len() && (headers[j] == b' ' || headers[j] == b'\t') {
                j += 1;
            }
            let start = j;
            while j < headers.len() && headers[j] != b'\r' && headers[j] != b'\n' {
                j += 1;
            }
            if j == start || j - start > LOCATION_MAX {
                return None;
            }
            let mut bytes = [0u8; LOCATION_MAX];
            bytes[..j - start].copy_from_slice(&headers[start..j]);
            return Some(Location {
                bytes,
                len: j - start,
            });
        }
        i += 1;
    }
    None
}

pub(crate) fn parse_status(headers: &[u8]) -> u16 {
    // "HTTP/1.x NNN ..." — find the first space, then read three digits.
    let Some(space) = headers.iter().position(|&b| b == b' ') else {
        return 0;
    };
    let start = space + 1;
    if start + 3 > headers.len() {
        return 0;
    }
    let digits = &headers[start..start + 3];
    if !digits.iter().all(|b| b.is_ascii_digit()) {
        return 0;
    }
    digits
        .iter()
        .fold(0u16, |acc, &b| acc * 10 + (b - b'0') as u16)
}
