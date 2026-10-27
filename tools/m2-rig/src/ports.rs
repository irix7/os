// SPDX-License-Identifier: GPL-3.0-only
//! Eight echo listeners for the module TCP slot probe.
//!
//! The probe module opens one socket per kernel slot and sends `p=<slot>\n`
//! to `gateway:(base + slot)`.  iris maps gateway-addressed traffic to host
//! loopback, so these listeners see eight independent connections at the same
//! time.  The fixture records every line it echoes; the rig compares the set
//! with the slot range instead of trusting the guest's own log.

use std::io::{Read, Write};
use std::net::{Shutdown, TcpListener, TcpStream};
use std::sync::{Arc, Mutex};
use std::thread;

/// A running set of echo listeners.
pub struct PortsFixture {
    /// First host port.
    pub base: u16,
    /// Number of listeners.
    pub count: u16,
    /// Lines received, with the listener port, in arrival order.
    received: Arc<Mutex<Vec<(u16, String)>>>,
}

impl PortsFixture {
    /// Bind `count` listeners on `0.0.0.0:base..base+count`.
    ///
    /// Binding all listeners before returning makes a port collision a
    /// startup error rather than a half-started run.
    pub fn start(base: u16, count: u16) -> std::io::Result<Self> {
        let received = Arc::new(Mutex::new(Vec::new()));
        let mut listeners = Vec::with_capacity(count as usize);
        for index in 0..count {
            let listener = TcpListener::bind(("0.0.0.0", base + index))?;
            listeners.push(listener);
        }
        for (index, listener) in listeners.into_iter().enumerate() {
            let received = received.clone();
            thread::Builder::new()
                .name(format!("ports-echo-{index}"))
                .spawn(move || accept_loop(listener, received, base + index as u16))?;
        }
        Ok(Self {
            base,
            count,
            received,
        })
    }

    /// Snapshot the received lines.
    pub fn lines(&self) -> Vec<String> {
        self.received
            .lock()
            .map(|entries| entries.iter().map(|(_, line)| line.clone()).collect())
            .unwrap_or_default()
    }

    /// Snapshot the `(listener port, line)` pairs.
    pub fn entries(&self) -> Vec<(u16, String)> {
        self.received
            .lock()
            .map(|entries| entries.clone())
            .unwrap_or_default()
    }

    /// Whether every slot line arrived on its own listener exactly once.
    pub fn complete(&self) -> bool {
        let entries = self.entries();
        (0..self.count).all(|slot| {
            let want = format!("p={slot}");
            entries
                .iter()
                .filter(|(port, line)| *port == self.base + slot && line == &want)
                .count()
                == 1
        })
    }
}

fn accept_loop(listener: TcpListener, received: Arc<Mutex<Vec<(u16, String)>>>, port: u16) {
    for incoming in listener.incoming() {
        let Ok(mut stream) = incoming else {
            continue;
        };
        let received = received.clone();
        let _ = thread::Builder::new()
            .name("ports-echo-connection".into())
            .spawn(move || echo(&mut stream, &received, port));
    }
}

fn echo(stream: &mut TcpStream, received: &Mutex<Vec<(u16, String)>>, port: u16) {
    let mut buffer = [0u8; 64];
    let mut line = Vec::new();
    loop {
        match stream.read(&mut buffer) {
            Ok(0) => break,
            Ok(count) => {
                line.extend_from_slice(&buffer[..count]);
                if line.contains(&b'\n') {
                    if let Ok(mut entries) = received.lock() {
                        entries.push((port, String::from_utf8_lossy(&line).trim().to_string()));
                    }
                    if let Err(error) = stream.write_all(&line) {
                        eprintln!("ports fixture {port}: echo write failed: {error}");
                    }
                    let _ = stream.flush();
                    line.clear();
                }
            }
            Err(_) => break,
        }
    }
    let _ = stream.shutdown(Shutdown::Both);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpStream;

    fn connect(base: u16, slot: u16) -> TcpStream {
        let stream = TcpStream::connect(("127.0.0.1", base + slot)).unwrap();
        stream
            .set_read_timeout(Some(std::time::Duration::from_secs(5)))
            .unwrap();
        stream
    }

    #[test]
    fn every_listener_echoes_its_own_line() {
        let fixture = PortsFixture::start(19400, 8).unwrap();
        let mut clients = Vec::new();
        for slot in 0..8u16 {
            let mut client = connect(fixture.base, slot);
            let line = format!("p={slot}\n");
            client.write_all(line.as_bytes()).unwrap();
            clients.push((client, line));
        }
        for (client, line) in &mut clients {
            let mut echo = [0u8; 16];
            let read = client.read(&mut echo).unwrap();
            assert_eq!(&echo[..read], line.as_bytes());
        }
        assert!(fixture.complete());
        assert_eq!(fixture.entries().len(), 8);
    }

    #[test]
    fn a_missing_slot_is_not_complete() {
        let fixture = PortsFixture::start(19410, 8).unwrap();
        let mut client = connect(fixture.base, 3);
        client.write_all(b"p=3\n").unwrap();
        let mut echo = [0u8; 16];
        let _ = client.read(&mut echo).unwrap();
        assert!(!fixture.complete());
    }
}
