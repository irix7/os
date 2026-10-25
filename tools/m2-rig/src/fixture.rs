// SPDX-License-Identifier: GPL-3.0-only
//! A tiny HTTP fixture that serves the MCP module to both guests.
//!
//! The kernel fetches `mcp.url` at boot, verifies `mcp.sha256`, and runs the
//! image.  Pointing that URL at the node's own NAT gateway
//! (`http://192.168.<n>.1:<port>/mcp.module`) makes the guest reach this
//! listener on host loopback.  Serving a freshly built module keeps the rig
//! independent of the public internet and of a released module that predates
//! the current ABI.

use std::io::{Read, Write};
use std::net::{Shutdown, TcpListener, TcpStream};
use std::sync::Arc;
use std::thread;

use crate::relay::{lines, log, note, SharedLog};

/// A running module fixture.
pub struct Fixture {
    /// Host loopback port.
    pub port: u16,
    /// Request log.
    pub log: SharedLog,
}

impl Fixture {
    /// Serve `module` on an ephemeral loopback port.
    pub fn start(module: Vec<u8>) -> std::io::Result<Self> {
        let listener = TcpListener::bind(("127.0.0.1", 0))?;
        let port = listener.local_addr()?.port();
        let log = log();
        note(&log, format!("fixture listening on 127.0.0.1:{port}"));
        note(&log, format!("fixture module is {} bytes", module.len()));
        let body = Arc::new(module);
        let thread_log = log.clone();
        thread::Builder::new()
            .name("m2-fixture".into())
            .spawn(move || accept_loop(listener, body, thread_log))?;
        Ok(Self { port, log })
    }

    /// Snapshot the request log.
    pub fn lines(&self) -> Vec<String> {
        lines(&self.log)
    }
}

fn accept_loop(listener: TcpListener, body: Arc<Vec<u8>>, log: SharedLog) {
    for incoming in listener.incoming() {
        let stream = match incoming {
            Ok(stream) => stream,
            Err(error) => {
                note(&log, format!("fixture accept error: {error}"));
                continue;
            }
        };
        let body = body.clone();
        let log = log.clone();
        let _ = thread::Builder::new()
            .name("m2-fixture-connection".into())
            .spawn(move || serve(stream, body, log));
    }
}

fn serve(mut stream: TcpStream, body: Arc<Vec<u8>>, log: SharedLog) {
    let mut request = Vec::new();
    let mut buffer = [0u8; 1024];
    while request.len() < 4096 {
        match stream.read(&mut buffer) {
            Ok(0) => break,
            Ok(count) => {
                request.extend_from_slice(&buffer[..count]);
                if request.windows(4).any(|window| window == b"\r\n\r\n") {
                    break;
                }
            }
            Err(_) => break,
        }
    }
    let path = request
        .split(|byte| *byte == b'\r' || *byte == b'\n')
        .next()
        .and_then(|line| line.split(|byte| *byte == b' ').nth(1))
        .map(|path| String::from_utf8_lossy(path).into_owned())
        .unwrap_or_else(|| "/".into());

    if path == "/mcp.module" {
        let header = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/octet-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        );
        let _ = stream.write_all(header.as_bytes());
        let _ = stream.write_all(&body);
        note(&log, format!("fixture GET {path} 200 {}", body.len()));
    } else {
        let header = b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";
        let _ = stream.write_all(header);
        note(&log, format!("fixture GET {path} 404"));
    }
    let _ = stream.flush();
    let _ = stream.shutdown(Shutdown::Write);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;

    #[test]
    fn fixture_serves_the_module() {
        let body = b"module-bytes".to_vec();
        let fixture = Fixture::start(body.clone()).unwrap();
        let mut client = TcpStream::connect(("127.0.0.1", fixture.port)).unwrap();
        client
            .write_all(b"GET /mcp.module HTTP/1.1\r\nHost: x\r\n\r\n")
            .unwrap();
        let mut response = Vec::new();
        client.read_to_end(&mut response).unwrap();
        let text = String::from_utf8_lossy(&response);
        assert!(text.starts_with("HTTP/1.1 200 OK"));
        assert!(text.contains("Content-Length: 12"));
        assert!(response.ends_with(&body));
    }

    #[test]
    fn fixture_rejects_other_paths() {
        let fixture = Fixture::start(b"x".to_vec()).unwrap();
        let mut client = TcpStream::connect(("127.0.0.1", fixture.port)).unwrap();
        client.write_all(b"GET /nope HTTP/1.1\r\n\r\n").unwrap();
        let mut response = Vec::new();
        client.read_to_end(&mut response).unwrap();
        assert!(String::from_utf8_lossy(&response).starts_with("HTTP/1.1 404"));
    }
}
