// SPDX-License-Identifier: GPL-3.0-only
//! A transparent TCP relay for the two-node rig.
//!
//! Each guest NAT maps a connection to its own gateway address onto host
//! loopback.  Two isolated iris NATs cannot address each other's guest
//! directly, and the port-forward path drops data that arrives before its
//! guest-side connection is established.  The relay is the missing seam: the
//! guest connects to `gateway:<relay port>`, the relay opens a host connection
//! to the peer's forward, and then copies bytes in both directions.
//!
//! It is a byte relay, not a directory, protocol translator or membership
//! service.  It records connection and byte counts so the evidence bundle can
//! show that the probe really crossed it.

use std::io::{Read, Write};
use std::net::{Shutdown, SocketAddr, TcpListener, TcpStream};
use std::sync::{Arc, Mutex};
use std::thread;

/// A grow-only log shared with the orchestrator.
pub type SharedLog = Arc<Mutex<Vec<String>>>;

/// Create an empty shared log.
pub fn log() -> SharedLog {
    Arc::new(Mutex::new(Vec::new()))
}

/// Append one line to a shared log.
pub fn note(log: &SharedLog, line: impl Into<String>) {
    if let Ok(mut entries) = log.lock() {
        entries.push(line.into());
    }
}

/// Snapshot a shared log.
pub fn lines(log: &SharedLog) -> Vec<String> {
    log.lock()
        .map(|entries| entries.clone())
        .unwrap_or_default()
}

/// A running relay listener.
pub struct Relay {
    /// The host loopback port the guest connects to through its NAT.
    pub port: u16,
    /// Connection and byte log.
    pub log: SharedLog,
}

impl Relay {
    /// Bind a relay that forwards every accepted connection to `target`.
    pub fn start(target: SocketAddr) -> std::io::Result<Self> {
        let listener = TcpListener::bind(("127.0.0.1", 0))?;
        let port = listener.local_addr()?.port();
        let log = log();
        note(
            &log,
            format!("relay listening on 127.0.0.1:{port} -> {target}"),
        );
        let thread_log = log.clone();
        thread::Builder::new()
            .name(format!("m2-relay-{port}"))
            .spawn(move || accept_loop(listener, target, thread_log))?;
        Ok(Self { port, log })
    }
}

fn accept_loop(listener: TcpListener, target: SocketAddr, log: SharedLog) {
    for incoming in listener.incoming() {
        let stream = match incoming {
            Ok(stream) => stream,
            Err(error) => {
                note(&log, format!("relay accept error: {error}"));
                continue;
            }
        };
        let peer = stream
            .peer_addr()
            .map(|addr| addr.to_string())
            .unwrap_or_else(|_| "?".into());
        note(&log, format!("relay accepted {peer}"));
        let target_stream = match TcpStream::connect(target) {
            Ok(stream) => stream,
            Err(error) => {
                note(&log, format!("relay target {target} failed: {error}"));
                let _ = stream.shutdown(Shutdown::Both);
                continue;
            }
        };
        // A guest MCP listener keeps its side of an HTTP/1.0 connection open
        // after the response, and the emulated CPU can take a moment to
        // service the module tick.  The relay is a byte pump, not an HTTP
        // parser, so a quiet target is treated as the end of the probe and
        // gets a FIN.  Five seconds covers a slow emulated response without
        // making a dead peer wait forever.
        let _ = target_stream.set_read_timeout(Some(std::time::Duration::from_secs(5)));
        note(&log, format!("relay connected to {target} for {peer}"));
        let (client_read, client_write) = match (stream.try_clone(), stream) {
            (Ok(read), write) => (read, write),
            (Err(error), _) => {
                note(&log, format!("relay clone failed: {error}"));
                continue;
            }
        };
        let (target_read, target_write) = match (target_stream.try_clone(), target_stream) {
            (Ok(read), write) => (read, write),
            (Err(error), _) => {
                note(&log, format!("relay target clone failed: {error}"));
                continue;
            }
        };
        let up_log = log.clone();
        let down_log = log.clone();
        let _ = thread::Builder::new()
            .name("m2-relay-up".into())
            .spawn(move || pump(client_read, target_write, "up", up_log));
        let _ = thread::Builder::new()
            .name("m2-relay-down".into())
            .spawn(move || pump(target_read, client_write, "down", down_log));
    }
}

fn pump(mut from: TcpStream, mut to: TcpStream, direction: &str, log: SharedLog) {
    let mut buffer = [0u8; 8192];
    let mut total = 0u64;
    loop {
        match from.read(&mut buffer) {
            Ok(0) => break,
            Ok(count) => {
                total += count as u64;
                if let Err(error) = to.write_all(&buffer[..count]) {
                    note(&log, format!("relay {direction} write failed: {error}"));
                    break;
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(error)
                if error.kind() == std::io::ErrorKind::WouldBlock
                    || error.kind() == std::io::ErrorKind::TimedOut =>
            {
                note(&log, format!("relay {direction} idle after {total} bytes"));
                break;
            }
            Err(error) => {
                note(&log, format!("relay {direction} read failed: {error}"));
                break;
            }
        }
    }
    note(
        &log,
        format!("relay {direction} closed after {total} bytes"),
    );
    // Send the FIN explicitly after the data.  The guest HTTP client waits for
    // the end of the response before it returns to the console, and the guest
    // NAT relays the last host bytes on a later poll than the relay's socket
    // close; a short linger on the target->client half gives it time to drain.
    if direction == "down" {
        std::thread::sleep(std::time::Duration::from_secs(2));
    }
    let _ = to.shutdown(Shutdown::Write);
    drop(to);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};

    #[test]
    fn relay_moves_bytes_both_ways() {
        // Echo server first.
        let echo = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let echo_port = echo.local_addr().unwrap().port();
        thread::spawn(move || {
            if let Ok((mut stream, _)) = echo.accept() {
                let mut buffer = [0u8; 64];
                if let Ok(count) = stream.read(&mut buffer) {
                    let _ = stream.write_all(&buffer[..count]);
                }
            }
        });

        let relay = Relay::start(SocketAddr::from(([127, 0, 0, 1], echo_port))).unwrap();
        let mut client = TcpStream::connect(("127.0.0.1", relay.port)).unwrap();
        client.write_all(b"hello").unwrap();
        let mut buffer = [0u8; 5];
        client.read_exact(&mut buffer).unwrap();
        assert_eq!(&buffer, b"hello");
    }
}
