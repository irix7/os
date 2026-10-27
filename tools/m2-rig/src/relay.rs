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
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{mpsc, Arc, Barrier, Mutex};
use std::thread;
use std::time::{Duration, Instant};

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

// ---------------------------------------------------------------------------
// Mesh relay
// ---------------------------------------------------------------------------

/// A persistent mesh link may legitimately be idle between heartbeats.  The
/// probe relay's five-second quiet-period close is deliberately not reused.
pub const MESH_IDLE_TIMEOUT: Duration = Duration::from_secs(120);
/// How long the relay waits for both guests to send their first frame.
pub const MESH_FIRST_BYTE_TIMEOUT: Duration = Duration::from_secs(30);
/// First reconnect delay for the mesh relay.
pub const MESH_RETRY_MIN: Duration = Duration::from_millis(250);
/// Reconnect delay cap for the mesh relay.
pub const MESH_RETRY_MAX: Duration = Duration::from_secs(5);

/// A bidirectional mesh relay between the two guest port-7000 forwards.
///
/// The relay connects *to* both host forwards rather than listening for a
/// guest connection: the guest has no TCP connect service, so both modules
/// listen and the host is the only side that can initiate.  It is still a dumb
/// byte pump: it does not parse `MeshHeader`, choose a peer or confirm an
/// epoch.  The modules generate the heartbeats; any bytes count as activity.
pub struct MeshRelay {
    /// Connection and byte log.
    pub log: SharedLog,
    stop: Arc<AtomicBool>,
    paused: Arc<AtomicBool>,
}

impl MeshRelay {
    /// Start a relay between two guest mesh forwards.
    pub fn start(a: SocketAddr, b: SocketAddr) -> std::io::Result<Self> {
        Self::start_with_log(a, b, log())
    }

    /// Start a relay that appends to an existing connection log.
    ///
    /// The rig restarts the relay around a member restart so the live root
    /// does not accumulate half-open host connections while its peer is down.
    /// Reusing the log keeps the whole run's relay history in one file.
    pub fn start_with_log(a: SocketAddr, b: SocketAddr, log: SharedLog) -> std::io::Result<Self> {
        let stop = Arc::new(AtomicBool::new(false));
        let paused = Arc::new(AtomicBool::new(false));
        note(&log, format!("mesh-relay targets {a} <-> {b}"));
        let thread_log = log.clone();
        let thread_stop = stop.clone();
        let thread_paused = paused.clone();
        thread::Builder::new()
            .name("m2-mesh-relay".into())
            .spawn(move || supervisor(a, b, thread_log, thread_stop, thread_paused))?;
        Ok(Self { log, stop, paused })
    }

    /// Stop forwarding bytes without closing either guest session.
    ///
    /// This is the partition seam: the TCP sessions stay alive, the modules
    /// see no frames and the membership detector declares the peer suspect and
    /// failed.  The iris port forward cannot reliably reopen a half-open guest
    /// session, so a partition must not tear the session down at the start.
    pub fn pause(&self) {
        self.paused.store(true, Ordering::SeqCst);
        note(&self.log, "mesh-relay paused by the partition fault");
    }
}

impl Drop for MeshRelay {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
    }
}

fn supervisor(
    a: SocketAddr,
    b: SocketAddr,
    log: SharedLog,
    stop: Arc<AtomicBool>,
    paused: Arc<AtomicBool>,
) {
    let mut delay = MESH_RETRY_MIN;
    let mut warm = false;
    while !stop.load(Ordering::SeqCst) {
        match session(a, b, &log, &stop, &paused, warm) {
            Ok(()) => {
                note(&log, "mesh-relay session ended");
                delay = MESH_RETRY_MIN;
            }
            Err(error) => {
                note(&log, format!("mesh-relay session failed: {error}"));
                delay = (delay * 2).min(MESH_RETRY_MAX);
            }
        }
        // After the first attempt the guests are warm; later sessions must
        // not use the cold-start first-byte barrier.
        warm = true;
        if stop.load(Ordering::SeqCst) {
            break;
        }
        note(
            &log,
            format!("mesh-relay reconnecting in {} ms", delay.as_millis()),
        );
        sleep_interruptible(delay, &stop);
    }
    note(&log, "mesh-relay stopped");
}

fn sleep_interruptible(delay: Duration, stop: &AtomicBool) {
    let deadline = Instant::now() + delay;
    while Instant::now() < deadline {
        if stop.load(Ordering::SeqCst) {
            return;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn session(
    a: SocketAddr,
    b: SocketAddr,
    log: &SharedLog,
    stop: &Arc<AtomicBool>,
    paused: &Arc<AtomicBool>,
    warm: bool,
) -> Result<(), String> {
    let a = connect_retry(a, stop)?;
    let b = connect_retry(b, stop)?;
    let _ = a.set_nodelay(true);
    let _ = b.set_nodelay(true);
    let _ = a.set_write_timeout(Some(Duration::from_secs(5)));
    let _ = b.set_write_timeout(Some(Duration::from_secs(5)));
    note(log, "mesh-relay connected both forwards");

    let a_control = a
        .try_clone()
        .map_err(|error| format!("clone n0: {error}"))?;
    let b_control = b
        .try_clone()
        .map_err(|error| format!("clone n1: {error}"))?;
    let a_read = a
        .try_clone()
        .map_err(|error| format!("clone n0 read: {error}"))?;
    let b_read = b
        .try_clone()
        .map_err(|error| format!("clone n1 read: {error}"))?;
    // The first session uses the first-byte guard for the iris forward's
    // early-data race.  A later session (for example after one node restarts)
    // must not deadlock: the live guest can send a join retry before the other
    // guest's listener is armed, and the relay has to carry it so the peer can
    // answer.  Warm sessions pump each direction as soon as it has bytes.
    let barrier = Arc::new(Barrier::new(if warm { 1 } else { 2 }));
    let fail = Arc::new(AtomicBool::new(false));
    let activity = Arc::new(AtomicU64::new(0));
    let started = Instant::now();
    let (ready_tx, ready_rx) = mpsc::channel();

    let up = pump_mesh(
        a_read,
        b,
        "n0->n1",
        log.clone(),
        barrier.clone(),
        ready_tx.clone(),
        stop.clone(),
        paused.clone(),
        fail.clone(),
        activity.clone(),
        started,
        warm,
    );
    let down = pump_mesh(
        b_read,
        a,
        "n1->n0",
        log.clone(),
        barrier,
        ready_tx,
        stop.clone(),
        paused.clone(),
        fail.clone(),
        activity,
        started,
        warm,
    );

    if warm {
        note(
            log,
            "mesh-relay warm session: pumping without the first-byte guard",
        );
        let _ = up.join();
        let _ = down.join();
        let _ = a_control.shutdown(Shutdown::Both);
        let _ = b_control.shutdown(Shutdown::Both);
        return Ok(());
    }

    let mut first = 0usize;
    let deadline = Instant::now() + MESH_FIRST_BYTE_TIMEOUT + Duration::from_secs(5);
    while first < 2 {
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            break;
        }
        match ready_rx.recv_timeout(left) {
            Ok(true) => first += 1,
            Ok(false) => break,
            Err(_) => break,
        }
    }
    if first != 2 {
        fail.store(true, Ordering::SeqCst);
        let _ = a_control.shutdown(Shutdown::Both);
        let _ = b_control.shutdown(Shutdown::Both);
        let _ = up.join();
        let _ = down.join();
        return Err("both guests did not send their first mesh bytes".into());
    }
    note(log, "mesh-relay established after first bytes both ways");

    let _ = up.join();
    let _ = down.join();
    let _ = a_control.shutdown(Shutdown::Both);
    let _ = b_control.shutdown(Shutdown::Both);
    Ok(())
}

fn connect_retry(addr: SocketAddr, stop: &AtomicBool) -> Result<TcpStream, String> {
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        if stop.load(Ordering::SeqCst) {
            return Err("stopped".into());
        }
        match TcpStream::connect_timeout(&addr, Duration::from_millis(500)) {
            Ok(stream) => return Ok(stream),
            Err(error) => {
                if Instant::now() >= deadline {
                    return Err(format!("connect {addr}: {error}"));
                }
                std::thread::sleep(Duration::from_millis(200));
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn pump_mesh(
    mut from: TcpStream,
    mut to: TcpStream,
    direction: &'static str,
    log: SharedLog,
    barrier: Arc<Barrier>,
    ready: mpsc::Sender<bool>,
    stop: Arc<AtomicBool>,
    paused: Arc<AtomicBool>,
    fail: Arc<AtomicBool>,
    activity: Arc<AtomicU64>,
    started: Instant,
    warm: bool,
) -> thread::JoinHandle<()> {
    thread::Builder::new()
        .name(format!("m2-mesh-{direction}"))
        .spawn(move || {
            let _ = from.set_read_timeout(Some(Duration::from_millis(100)));
            let mut first = vec![0u8; 8192];
            let mut first_len = 0usize;
            let mut opened = false;
            let deadline = Instant::now() + MESH_FIRST_BYTE_TIMEOUT;
            loop {
                if stop.load(Ordering::SeqCst) || fail.load(Ordering::SeqCst) {
                    break;
                }
                match from.read(&mut first) {
                    Ok(0) => break,
                    Ok(count) => {
                        first_len = count;
                        opened = true;
                        break;
                    }
                    Err(error)
                        if error.kind() == std::io::ErrorKind::WouldBlock
                            || error.kind() == std::io::ErrorKind::TimedOut =>
                    {
                        if Instant::now() >= deadline {
                            break;
                        }
                    }
                    Err(_) => break,
                }
            }
            if !warm {
                let _ = ready.send(opened);
                // Both directions must have their first chunk before either
                // is written.  This is the early-data guard for the iris port
                // forward, which may accept the host connection before the
                // guest listener is ready.
                barrier.wait();
            }
            if !opened || stop.load(Ordering::SeqCst) || fail.load(Ordering::SeqCst) {
                let _ = to.shutdown(Shutdown::Both);
                return;
            }
            if let Err(error) = to.write_all(&first[..first_len]) {
                note(&log, format!("mesh-relay {direction} first write: {error}"));
                fail.store(true, Ordering::SeqCst);
                let _ = from.shutdown(Shutdown::Both);
                let _ = to.shutdown(Shutdown::Both);
                return;
            }
            let mut total = first_len as u64;
            activity.store(
                started.elapsed().as_millis().min(u64::MAX as u128) as u64,
                Ordering::SeqCst,
            );
            note(
                &log,
                format!("mesh-relay {direction} first {first_len} bytes"),
            );

            let mut buffer = [0u8; 8192];
            loop {
                if stop.load(Ordering::SeqCst) || fail.load(Ordering::SeqCst) {
                    break;
                }
                if paused.load(Ordering::SeqCst) {
                    // A partition drops frames, not the session.  Keep the
                    // idle timer from closing the link while the seam is cut.
                    activity.store(
                        started.elapsed().as_millis().min(u64::MAX as u128) as u64,
                        Ordering::SeqCst,
                    );
                    std::thread::sleep(Duration::from_millis(20));
                    continue;
                }
                match from.read(&mut buffer) {
                    Ok(0) => {
                        break;
                    }
                    Ok(count) => {
                        total += count as u64;
                        activity.store(
                            started.elapsed().as_millis().min(u64::MAX as u128) as u64,
                            Ordering::SeqCst,
                        );
                        if let Err(error) = to.write_all(&buffer[..count]) {
                            note(&log, format!("mesh-relay {direction} write: {error}"));
                            fail.store(true, Ordering::SeqCst);
                            break;
                        }
                    }
                    Err(error)
                        if error.kind() == std::io::ErrorKind::WouldBlock
                            || error.kind() == std::io::ErrorKind::TimedOut =>
                    {
                        let now = started.elapsed().as_millis().min(u64::MAX as u128) as u64;
                        let last = activity.load(Ordering::SeqCst);
                        if now.saturating_sub(last) >= MESH_IDLE_TIMEOUT.as_millis() as u64 {
                            note(
                                &log,
                                format!("mesh-relay {direction} idle after {total} bytes"),
                            );
                            break;
                        }
                    }
                    Err(error) => {
                        note(&log, format!("mesh-relay {direction} read: {error}"));
                        break;
                    }
                }
            }
            fail.store(true, Ordering::SeqCst);
            let _ = from.shutdown(Shutdown::Both);
            let _ = to.shutdown(Shutdown::Both);
            note(
                &log,
                format!("mesh-relay {direction} closed after {total} bytes"),
            );
        })
        .expect("spawn mesh relay pump")
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

    #[test]
    fn mesh_relay_waits_for_both_first_bytes() {
        let left = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let right = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let left_port = left.local_addr().unwrap().port();
        let right_port = right.local_addr().unwrap().port();
        let (left_tx, left_rx) = mpsc::channel();
        let (right_tx, right_rx) = mpsc::channel();
        thread::spawn(move || {
            let (mut stream, _) = left.accept().unwrap();
            let _ = stream.write_all(b"L");
            let mut byte = [0u8; 1];
            stream.read_exact(&mut byte).unwrap();
            let _ = left_tx.send(byte[0]);
        });
        thread::spawn(move || {
            let (mut stream, _) = right.accept().unwrap();
            let _ = stream.write_all(b"R");
            let mut byte = [0u8; 1];
            stream.read_exact(&mut byte).unwrap();
            let _ = right_tx.send(byte[0]);
        });

        let relay = MeshRelay::start(
            SocketAddr::from(([127, 0, 0, 1], left_port)),
            SocketAddr::from(([127, 0, 0, 1], right_port)),
        )
        .unwrap();
        // The left guest must receive the right guest's first byte, and the
        // right guest the left one.  Neither write happens before both reads,
        // which is the early-data guard the iris forward needs.
        assert_eq!(
            left_rx.recv_timeout(std::time::Duration::from_secs(5)),
            Ok(b'R')
        );
        assert_eq!(
            right_rx.recv_timeout(std::time::Duration::from_secs(5)),
            Ok(b'L')
        );
        drop(relay);
    }
}
