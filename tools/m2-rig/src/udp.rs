// SPDX-License-Identifier: GPL-3.0-only
//! A two-flow UDP relay for the M3-ADR-0006 datagram gate.
//!
//! Each guest NAT maps a datagram addressed to its own gateway onto host
//! loopback (`iris` `nat_udp::gateway_to_host`), so both kernels can reach the
//! same host port even though their emulated subnets are isolated and neither
//! node has an inbound forward for it.  The relay binds one socket on that
//! loopback port, pairs the first two source endpoints it sees (one per guest)
//! and forwards every datagram to the other side's endpoint.
//!
//! That pairing is what turns two one-way guest-to-gateway sends into a
//! two-kernel exchange: the peer's reply goes back to the socket the relay
//! received the request from, and the guest's NAT still maps that socket to its
//! own datagram socket.  The relay is a switch, not a protocol endpoint: it
//! does not parse, reorder, retransmit or acknowledge anything.
//!
//! The first datagram from each side is consumed while pairing, so the probe's
//! 500 ms resend is what actually crosses.  The counters make that visible
//! instead of hiding it.

use std::net::{SocketAddr, UdpSocket};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

/// Guest UDP port the MCP module's probe binds (`netprobe.port`).
pub const GUEST_PORT: u16 = 7001;

/// A grow-only log shared with the orchestrator.
pub type SharedLog = Arc<Mutex<Vec<String>>>;

/// Create an empty relay log.
pub fn log() -> SharedLog {
    Arc::new(Mutex::new(Vec::new()))
}

/// Append one line to a relay log.
pub fn note(log: &SharedLog, line: impl Into<String>) {
    if let Ok(mut entries) = log.lock() {
        entries.push(line.into());
    }
}

/// Snapshot a relay log.
pub fn lines(log: &SharedLog) -> Vec<String> {
    log.lock()
        .map(|entries| entries.clone())
        .unwrap_or_default()
}

/// What to do with one received datagram.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Route {
    /// Send the datagram to the other paired endpoint.
    ForwardTo(SocketAddr),
    /// The sender became the first or second flow; there is no peer yet.
    Learned,
    /// A known sender has no pair yet, or a third flow arrived.
    Drop,
}

/// Pair the first two source endpoints and forward between them.
///
/// The pairing is deliberately blind: the gate runs exactly two guests, and a
/// third source means the scenario is not the one under test, so it is dropped
/// rather than merged into the pair.
#[derive(Debug, Default)]
pub struct Pairing {
    peers: [Option<SocketAddr>; 2],
}

impl Pairing {
    /// Classify one datagram source.
    pub fn route(&mut self, from: SocketAddr) -> Route {
        if self.peers[0] == Some(from) {
            return match self.peers[1] {
                Some(peer) => Route::ForwardTo(peer),
                None => Route::Drop,
            };
        }
        if self.peers[1] == Some(from) {
            return match self.peers[0] {
                Some(peer) => Route::ForwardTo(peer),
                None => Route::Drop,
            };
        }
        if self.peers[0].is_none() {
            self.peers[0] = Some(from);
            return Route::Learned;
        }
        if self.peers[1].is_none() {
            self.peers[1] = Some(from);
            return Route::Learned;
        }
        Route::Drop
    }

    /// The paired endpoints, for evidence.
    pub fn peers(&self) -> [Option<SocketAddr>; 2] {
        self.peers
    }
}

/// Relay counters, shared with the orchestrator.
#[derive(Debug, Default)]
pub struct Counters {
    /// Datagrams received from either guest.
    pub received: AtomicU64,
    /// Datagrams consumed while learning a flow's endpoint.
    pub learned: AtomicU64,
    /// Datagrams sent to the opposite flow.
    pub forwarded: AtomicU64,
    /// Datagrams dropped: no pair yet, a third flow, or a failed send.
    pub dropped: AtomicU64,
}

impl Counters {
    /// Read the counter set.
    pub fn snapshot(&self) -> CounterSnapshot {
        CounterSnapshot {
            received: self.received.load(Ordering::Relaxed),
            learned: self.learned.load(Ordering::Relaxed),
            forwarded: self.forwarded.load(Ordering::Relaxed),
            dropped: self.dropped.load(Ordering::Relaxed),
        }
    }
}

/// A point-in-time copy of [`Counters`].
#[derive(Clone, Copy, Debug, Default)]
pub struct CounterSnapshot {
    pub received: u64,
    pub learned: u64,
    pub forwarded: u64,
    pub dropped: u64,
}

/// A running relay.
pub struct UdpRelay {
    /// Host loopback port the guests reach through their own gateways.
    pub port: u16,
    /// The relay's log.
    pub log: SharedLog,
    counters: Arc<Counters>,
    stop: Arc<AtomicBool>,
    thread: Option<thread::JoinHandle<()>>,
}

impl UdpRelay {
    /// Bind a relay on host loopback and start forwarding.
    ///
    /// The port is chosen by the kernel, so two rig runs on one host never
    /// collide and the probe's peer value carries the real port.
    pub fn start() -> std::io::Result<Self> {
        let socket = UdpSocket::bind(("127.0.0.1", 0))?;
        socket.set_read_timeout(Some(Duration::from_millis(50)))?;
        let port = socket.local_addr()?.port();
        let log = log();
        note(&log, format!("udp relay listening on 127.0.0.1:{port}"));
        let counters = Arc::new(Counters::default());
        let stop = Arc::new(AtomicBool::new(false));
        let service_counters = counters.clone();
        let service_stop = stop.clone();
        let service_log = log.clone();
        let thread = thread::Builder::new()
            .name(format!("m2-udp-relay-{port}"))
            .spawn(move || serve(socket, service_counters, service_stop, service_log))?;
        Ok(Self {
            port,
            log,
            counters,
            stop,
            thread: Some(thread),
        })
    }

    /// The counters so far.
    pub fn counters(&self) -> CounterSnapshot {
        self.counters.snapshot()
    }

    /// Stop the relay and join its thread.
    pub fn stop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

impl Drop for UdpRelay {
    fn drop(&mut self) {
        self.stop();
    }
}

fn serve(socket: UdpSocket, counters: Arc<Counters>, stop: Arc<AtomicBool>, log: SharedLog) {
    let mut pairing = Pairing::default();
    let mut buf = [0u8; 2048];
    while !stop.load(Ordering::Relaxed) {
        match socket.recv_from(&mut buf) {
            Ok((n, from)) => {
                counters.received.fetch_add(1, Ordering::Relaxed);
                match pairing.route(from) {
                    Route::ForwardTo(peer) => match socket.send_to(&buf[..n], peer) {
                        Ok(_) => {
                            counters.forwarded.fetch_add(1, Ordering::Relaxed);
                        }
                        Err(error) => {
                            counters.dropped.fetch_add(1, Ordering::Relaxed);
                            note(&log, format!("udp relay send to {peer} failed: {error}"));
                        }
                    },
                    Route::Learned => {
                        counters.learned.fetch_add(1, Ordering::Relaxed);
                        match pairing.peers() {
                            [Some(first), Some(second)] => {
                                note(&log, format!("udp relay paired {first} <-> {second}"))
                            }
                            _ => note(&log, format!("udp relay learned flow {from}")),
                        }
                    }
                    Route::Drop => {
                        counters.dropped.fetch_add(1, Ordering::Relaxed);
                    }
                }
            }
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) => {}
            Err(error) => {
                note(&log, format!("udp relay receive failed: {error}"));
                break;
            }
        }
    }
}

/// One node's datagram-probe markers.
#[derive(Clone, Debug)]
pub struct UdpNodeEvidence {
    /// `n0` or `n1`.
    pub name: String,
    /// The first `netprobe: up` line.
    pub up: Option<String>,
    /// The first `netprobe: ping` line.
    pub ping: Option<String>,
    /// The first `netprobe: ping-rx` line.
    pub ping_rx: Option<String>,
    /// The first `netprobe: pong` line.
    pub pong: Option<String>,
}

/// The whole gate's result.
#[derive(Clone, Debug)]
pub struct UdpEvidence {
    /// Host loopback port the guests reached.
    pub port: u16,
    /// Relay counters at the end of the run.
    pub counters: CounterSnapshot,
    /// Per-node markers.
    pub nodes: Vec<UdpNodeEvidence>,
    /// Whether every node saw every marker.
    pub ok: bool,
    /// Human-readable result.
    pub message: String,
}

/// Extract the probe markers from one serial transcript.
pub fn markers(name: &str, text: &str) -> UdpNodeEvidence {
    let first = |needle: &str| {
        text.lines()
            .find(|line| line.contains(needle))
            .map(|line| line.trim().to_string())
    };
    UdpNodeEvidence {
        name: name.to_string(),
        up: first("netprobe: up"),
        ping: first("netprobe: ping seq="),
        ping_rx: first("netprobe: ping-rx"),
        pong: first("netprobe: pong"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::SocketAddr;

    fn addr(port: u16) -> SocketAddr {
        SocketAddr::from(([127, 0, 0, 1], port))
    }

    #[test]
    fn pairs_two_flows_and_forwards_between_them() {
        let mut pairing = Pairing::default();
        assert_eq!(pairing.route(addr(1000)), Route::Learned);
        assert_eq!(pairing.route(addr(1000)), Route::Drop);
        assert_eq!(pairing.route(addr(2000)), Route::Learned);
        assert_eq!(pairing.route(addr(1000)), Route::ForwardTo(addr(2000)));
        assert_eq!(pairing.route(addr(2000)), Route::ForwardTo(addr(1000)));
    }

    #[test]
    fn drops_a_third_flow() {
        let mut pairing = Pairing::default();
        pairing.route(addr(1000));
        pairing.route(addr(2000));
        assert_eq!(pairing.route(addr(3000)), Route::Drop);
        // The original pair still forwards.
        assert_eq!(pairing.route(addr(1000)), Route::ForwardTo(addr(2000)));
    }

    #[test]
    fn peers_are_reported_for_evidence() {
        let mut pairing = Pairing::default();
        pairing.route(addr(1000));
        pairing.route(addr(2000));
        assert_eq!(pairing.peers(), [Some(addr(1000)), Some(addr(2000))]);
    }

    #[test]
    fn markers_read_the_first_of_each_kind() {
        let text = "boot\nnetprobe: up name=n0 port=7001 peer=192.168.0.1:7002\nnetprobe: ping seq=1\nnetprobe: ping seq=20\nnetprobe: ping-rx from=192.168.0.1:7002 peer=n1 seq=1\nnetprobe: pong from=192.168.0.1:7002 peer=n1 seq=1\n";
        let markers = markers("n0", text);
        assert_eq!(
            markers.up.as_deref(),
            Some("netprobe: up name=n0 port=7001 peer=192.168.0.1:7002")
        );
        assert_eq!(markers.ping.as_deref(), Some("netprobe: ping seq=1"));
        assert!(markers.ping_rx.is_some());
        assert!(markers.pong.is_some());
    }
}
