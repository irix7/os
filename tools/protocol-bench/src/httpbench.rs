// SPDX-License-Identifier: GPL-3.0-only
//! MCP control-plane HTTP cost: `Connection: close` versus keep-alive.
//!
//! The shipped MCP module answers `HTTP/1.0` with `Connection: close`, one
//! request per connection, and re-arms a single listening slot afterwards
//! (`modules/mcp/src/main.rs`).  This section measures what that costs on a
//! host loopback and models the module's 10 ms phase machine in `tick` mode.

use std::io::{Read, Write};
use std::net::{Shutdown, SocketAddr, TcpListener, TcpStream};
use std::thread;
use std::time::{Duration, Instant};

use crate::stats::{header, ms, row, Series};

/// The module tick cadence.
const TICK: Duration = Duration::from_millis(10);

/// Response header the module writes before every body.
fn response_header(body: usize, keepalive: bool) -> Vec<u8> {
    let connection = if keepalive { "keep-alive" } else { "close" };
    format!(
        "HTTP/1.0 200 OK\r\nContent-Type: application/json\r\nConnection: {connection}\r\nContent-Length: {body}\r\n\r\n"
    )
    .into_bytes()
}

/// A request with the header set a real MCP client sends.
fn request(body: &[u8], token: Option<&str>, keepalive: bool) -> Vec<u8> {
    let mut out = Vec::with_capacity(320 + body.len());
    out.extend_from_slice(b"POST /mcp HTTP/1.1\r\nHost: 192.168.0.2\r\n");
    out.extend_from_slice(b"Content-Type: application/json\r\n");
    out.extend_from_slice(b"Accept: application/json, text/event-stream\r\n");
    out.extend_from_slice(if keepalive {
        b"Connection: keep-alive\r\n".as_slice()
    } else {
        b"Connection: close\r\n".as_slice()
    });
    if let Some(token) = token {
        out.extend_from_slice(b"Authorization: Bearer ");
        out.extend_from_slice(token.as_bytes());
        out.extend_from_slice(b"\r\n");
    }
    out.extend_from_slice(format!("Content-Length: {}\r\n\r\n", body.len()).as_bytes());
    out.extend_from_slice(body);
    out
}

/// Read exactly one response (header block plus `Content-Length` bytes).
fn read_one_response(stream: &mut TcpStream) -> usize {
    let mut buffer = [0u8; 4096];
    let mut pending: Vec<u8> = Vec::with_capacity(4096);
    loop {
        if let Some(end) = pending
            .windows(4)
            .position(|window| window == b"\r\n\r\n")
            .map(|index| index + 4)
        {
            let body_len = content_length(&pending[..end]);
            if pending.len() >= end + body_len {
                return end + body_len;
            }
        }
        match stream.read(&mut buffer) {
            Ok(0) => return pending.len(),
            Ok(count) => pending.extend_from_slice(&buffer[..count]),
            Err(_) => return pending.len(),
        }
    }
}

/// A one-request-per-connection server, like the module.
fn serve_close(listener: TcpListener, response: Vec<u8>, requests: usize) {
    for _ in 0..requests {
        let (stream, _) = match listener.accept() {
            Ok(pair) => pair,
            Err(_) => return,
        };
        stream.set_nodelay(true).ok();
        let mut stream = stream;
        let mut buffer = [0u8; 2048];
        let mut used = 0;
        loop {
            match stream.read(&mut buffer[used..]) {
                Ok(0) => break,
                Ok(count) => {
                    used += count;
                    if complete(&buffer[..used]) || used == buffer.len() {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
        if stream.write_all(&response).is_err() {
            return;
        }
        let _ = stream.shutdown(Shutdown::Both);
    }
}

fn complete(request: &[u8]) -> bool {
    let Some(end) = request
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .map(|index| index + 4)
    else {
        return false;
    };
    let content = content_length(&request[..end]);
    request.len() >= end + content
}

fn content_length(headers: &[u8]) -> usize {
    let text = String::from_utf8_lossy(headers).to_ascii_lowercase();
    for line in text.split("\r\n") {
        if let Some(value) = line.strip_prefix("content-length:") {
            return value.trim().parse().unwrap_or(0);
        }
    }
    0
}

/// A ticked server with the module's HTTP phase machine.
///
/// `Arm -> WaitRequest -> (respond) -> WaitClose -> Closing -> Arm`, one phase
/// transition per 10 ms tick, exactly the shape of `main.rs`.
fn serve_tick(listener: TcpListener, response: Vec<u8>, requests: usize) {
    listener.set_nonblocking(true).ok();
    let mut connection: Option<TcpStream> = None;
    let mut phase = 0u8; // 0 arm, 1 wait-request, 2 wait-close, 3 closing
    let mut used = 0usize;
    let mut buffer = [0u8; 2048];
    let mut served = 0usize;
    let mut next = Instant::now() + TICK;
    while served < requests {
        let now = Instant::now();
        if next > now {
            thread::sleep(next - now);
        }
        next += TICK;
        match phase {
            0 => {
                if let Ok((stream, _)) = listener.accept() {
                    stream.set_nodelay(true).ok();
                    stream.set_nonblocking(true).ok();
                    connection = Some(stream);
                    used = 0;
                    phase = 1;
                }
            }
            1 => {
                if let Some(stream) = connection.as_mut() {
                    match stream.read(&mut buffer[used..]) {
                        Ok(0) => {}
                        Ok(count) => used += count,
                        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {}
                        Err(_) => {}
                    }
                    if complete(&buffer[..used]) {
                        let mut offset = 0;
                        while offset < response.len() {
                            match stream.write(&response[offset..]) {
                                Ok(written) => offset += written,
                                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {}
                                Err(_) => break,
                            }
                        }
                        phase = 2;
                    }
                }
            }
            2 => {
                if let Some(stream) = connection.as_mut() {
                    let mut scratch = [0u8; 64];
                    if matches!(stream.read(&mut scratch), Ok(0)) {
                        let _ = stream.shutdown(Shutdown::Both);
                        served += 1;
                        phase = 3;
                    }
                }
            }
            _ => {
                connection = None;
                // One tick to re-arm the listener, as `finish_close` does.
                phase = 0;
            }
        }
    }
}

/// A keep-alive server: one connection, many requests.
fn serve_keepalive(listener: TcpListener, response: Vec<u8>, requests: usize) {
    let (stream, _) = listener.accept().expect("accept");
    stream.set_nodelay(true).ok();
    let mut stream = stream;
    let mut buffer = [0u8; 2048];
    let mut used = 0usize;
    let mut served = 0usize;
    while served < requests {
        match stream.read(&mut buffer[used..]) {
            Ok(0) => return,
            Ok(count) => used += count,
            Err(_) => return,
        }
        while complete(&buffer[..used]) {
            let end = buffer[..used]
                .windows(4)
                .position(|window| window == b"\r\n\r\n")
                .map(|index| index + 4)
                .unwrap_or(used);
            let content = content_length(&buffer[..end]);
            let total = end + content;
            if used < total {
                break;
            }
            if stream.write_all(&response).is_err() {
                return;
            }
            served += 1;
            buffer.copy_within(total..used, 0);
            used -= total;
        }
    }
}

fn drive_close(address: SocketAddr, body: &[u8], count: usize, token: Option<&str>) -> Series {
    let mut samples = Vec::with_capacity(count);
    let request = request(body, token, false);
    for _ in 0..count {
        let start = Instant::now();
        let mut stream = TcpStream::connect(address).expect("connect");
        stream.set_nodelay(true).ok();
        stream.write_all(&request).expect("request");
        read_one_response(&mut stream);
        drop(stream);
        samples.push(start.elapsed().as_nanos() as u64);
    }
    Series::new(samples)
}

fn drive_keepalive(address: SocketAddr, body: &[u8], count: usize, token: Option<&str>) -> Series {
    let mut samples = Vec::with_capacity(count);
    let request = request(body, token, true);
    let mut stream = TcpStream::connect(address).expect("connect");
    stream.set_nodelay(true).ok();
    let mut buffer = [0u8; 4096];
    let mut pending: Vec<u8> = Vec::with_capacity(4096);
    for _ in 0..count {
        let start = Instant::now();
        stream.write_all(&request).expect("request");
        // Read one complete response, keeping any bytes that belong to the
        // next one.
        loop {
            if let Some(end) = pending
                .windows(4)
                .position(|window| window == b"\r\n\r\n")
                .map(|index| index + 4)
            {
                let body_len = content_length(&pending[..end]);
                if pending.len() >= end + body_len {
                    pending.drain(..end + body_len);
                    break;
                }
            }
            let read = stream.read(&mut buffer).expect("response");
            pending.extend_from_slice(&buffer[..read]);
        }
        samples.push(start.elapsed().as_nanos() as u64);
    }
    Series::new(samples)
}

fn report(title: &str, series: &Series, count: usize, wire: usize) {
    let median = series.pct(50.0).max(1);
    row(&[
        title.to_string(),
        count.to_string(),
        wire.to_string(),
        format!("{:.3}", ms(series.pct(50.0))),
        format!("{:.3}", ms(series.pct(90.0))),
        format!("{:.3}", ms(series.pct(99.0))),
        format!("{:.0}", 1_000_000_000.0 / median as f64),
    ]);
}

/// Run the HTTP section.
pub fn run() {
    println!("\n## MCP control plane: HTTP/1.0 close versus keep-alive (measured on this host)\n");
    let token = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
    let sample_body = br#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"status","arguments":{}}}"#;
    let request_bytes = request(sample_body, Some(token), false).len();
    println!(
        "A tool call is {request_bytes} request bytes (header block {} B plus a {} B JSON body).\n",
        200,
        sample_body.len()
    );
    header(&["path", "n", "resp B", "p50 ms", "p90 ms", "p99 ms", "req/s"]);

    const N_CLOSE: usize = 300;
    const N_TICK: usize = 60;
    const N_KEEPALIVE: usize = 2_000;

    for body_len in [200usize, 436, 1_100, 2_258, 8_300] {
        let response = response_header(body_len, false)
            .into_iter()
            .chain(std::iter::repeat_n(b'x', body_len))
            .collect::<Vec<u8>>();
        let wire = response.len();

        let (listener, address) = {
            let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
            let address = listener.local_addr().expect("address");
            (listener, address)
        };
        let server = {
            let response = response.clone();
            thread::spawn(move || serve_close(listener, response, N_CLOSE))
        };
        let series = drive_close(address, sample_body, N_CLOSE, Some(token));
        server.join().ok();
        report(
            &format!("close-per-request {body_len} B"),
            &series,
            N_CLOSE,
            wire,
        );

        let (listener, address) = {
            let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
            let address = listener.local_addr().expect("address");
            (listener, address)
        };
        let server = {
            let response = response_header(body_len, true)
                .into_iter()
                .chain(std::iter::repeat_n(b'x', body_len))
                .collect::<Vec<u8>>();
            thread::spawn(move || serve_keepalive(listener, response, N_KEEPALIVE))
        };
        let series = drive_keepalive(address, sample_body, N_KEEPALIVE, Some(token));
        server.join().ok();
        report(
            &format!("keep-alive {body_len} B"),
            &series,
            N_KEEPALIVE,
            wire,
        );

        let (listener, address) = {
            let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
            let address = listener.local_addr().expect("address");
            (listener, address)
        };
        let server = {
            let response = response.clone();
            thread::spawn(move || serve_tick(listener, response, N_TICK))
        };
        let series = drive_close(address, sample_body, N_TICK, Some(token));
        server.join().ok();
        report(
            &format!("close + 10 ms tick {body_len} B"),
            &series,
            N_TICK,
            wire,
        );
    }

    println!();
    println!("Notes:");
    println!("- `close-per-request` is `modules/mcp/src/main.rs`: HTTP/1.0, `Connection: close`,");
    println!("  one request per connection, one listening slot re-armed after the close.");
    println!("- `close + 10 ms tick` additionally runs the module phase machine");
    println!("  (`Arm -> WaitRequest -> WaitClose -> Closing -> Arm`) at the 10 ms kernel tick.");
    println!("- `keep-alive` reuses one connection; it is a v2 proposal, not shipped behaviour.");
    println!("- Response sizes 200/436/2258/1100/8300 correspond to status, resources/list,");
    println!(
        "  tools/list, block.read (512 B as 1024 hex chars) and fs.read (8 KiB of file bytes)."
    );
}
