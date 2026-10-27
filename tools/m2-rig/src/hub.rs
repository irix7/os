// SPDX-License-Identifier: GPL-3.0-only
//! M3 host mesh hub.
//!
//! The guest has one mesh TCP slot, so a real eight-node star cannot give the
//! root seven sockets.  This host-side hub connects to every guest's existing
//! mesh forward and forwards whole framed records by the `MeshHeader.dst`
//! field.  It is a **link switch**, not a directory: it never parses an object
//! ID, computes a home, confirms an epoch or rewrites a frame.  `dst == 0`
//! means the authority root for the M3 join and probe traffic.
//!
//! ## Bounded, lossless links
//!
//! Every destination has one queue and one writer thread.  A reader never
//! blocks another source's reader: it pushes a whole record into the
//! destination's bounded queue and waits there if the queue is full.  Waiting
//! is deliberate backpressure — the source guest's TCP send window closes and
//! the guest retries or paces itself.  The hub does not silently drop a
//! complete frame as the first hub revision did; the only drops are a record
//! that exceeded the queue deadline, a destination that has already stopped,
//! or a malformed/truncated record, and each has a counter.
//!
//! The queue is bounded by both frames and bytes, so eight 64 KiB frames
//! cannot grow without limit.  Writers use a socket write timeout and a total
//! write deadline, so a dead guest cannot pin a thread forever.  Readers use a
//! read timeout and a per-record receive deadline, so a half-written record is
//! discarded rather than forwarded.
//!
//! ## Sessions and ordering
//!
//! One guest is one session: one reader, one writer and one queue.  Adding a
//! member is explicit (`add_link`), removing one closes its socket and drops
//! its queue (`remove_link`).  Frames from one source keep their order because
//! a single reader pushes them in stream order.  Frames from different sources
//! are ordered by whichever reader reaches the destination queue first; the
//! hub does not invent a global order and does not need one.
//!
//! The hub is test harness only.  It is not linked into a kernel or module and
//! is not a production routing component.  The production invariant remains
//! "one logical link per peer" (M2-ADR-0002); the hub is the bounded M3 rig
//! workaround for the guest's single module TCP slot.

use std::collections::{HashMap, VecDeque};
use std::io::{Read, Write};
use std::net::{Ipv4Addr, Shutdown, SocketAddr, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use crate::rig::Node;

/// Largest record the M2/M3 transport accepts.
pub const MAX_RECORD: usize = 64 * 1024;
/// Smallest envelope: a 128-byte `MeshHeader` plus the frame header.
pub const MIN_RECORD: usize = 128;
const HEADER_LEN: usize = 128;
const MAX_LINKS: usize = 128;

/// Hub pacing and bounds.
///
/// The defaults are the rig values.  Tests use shorter deadlines and smaller
/// queues so a blocked writer is observable without a slow guest.
#[derive(Clone, Debug)]
pub struct HubConfig {
    /// Time allowed for the guest's mesh forward to accept a connection.
    pub connect_timeout: Duration,
    /// Socket read timeout; bounds how often a reader notices `stop`.
    pub read_timeout: Duration,
    /// An idle link with no record for this long is closed.
    pub idle_timeout: Duration,
    /// Deadline for the rest of a record after its first byte arrived.
    pub receive_deadline: Duration,
    /// Socket write timeout; bounds one `write` call.
    pub write_timeout: Duration,
    /// Total time allowed to write one whole record.
    pub write_deadline: Duration,
    /// Total time a reader waits for room in a destination queue.
    pub backpressure_deadline: Duration,
    /// Maximum whole records buffered for one destination.
    pub queue_frames: usize,
    /// Maximum record bytes buffered for one destination.
    pub queue_bytes: usize,
    /// Largest accepted record.
    pub max_record: usize,
    /// Test-only pacing between whole-record writes.  Zero in the rig.
    pub write_delay: Duration,
}

impl Default for HubConfig {
    fn default() -> Self {
        Self {
            connect_timeout: Duration::from_secs(60),
            read_timeout: Duration::from_millis(200),
            idle_timeout: Duration::from_secs(300),
            // Matches `mesh_transport::RECEIVE_DEADLINE_MS`.
            receive_deadline: Duration::from_secs(10),
            write_timeout: Duration::from_millis(500),
            write_deadline: Duration::from_secs(60),
            backpressure_deadline: Duration::from_secs(60),
            queue_frames: 256,
            queue_bytes: 256 * 1024,
            max_record: MAX_RECORD,
            write_delay: Duration::ZERO,
        }
    }
}

/// One node's counters.
#[derive(Clone, Debug, Default)]
pub struct LinkStats {
    /// Frames read from this guest.
    pub received: u64,
    /// Bytes read from this guest.
    pub bytes: u64,
    /// Frames written to this guest.
    pub forwarded: u64,
    /// Bytes written to this guest.
    pub written: u64,
    /// Records refused because the writer queue was full past its deadline.
    pub queue_full: u64,
    /// Records refused because the writer had already stopped.
    pub queue_closed: u64,
    /// Records discarded after a partial read.
    pub truncated: u64,
    /// Partial records abandoned at the receive deadline.
    pub receive_deadlines: u64,
    /// Writer socket errors.
    pub write_errors: u64,
    /// Writer stopped (cleanly or after an error).
    pub writer_stopped: bool,
    /// Reader stopped.
    pub reader_stopped: bool,
    /// When the last complete frame was read from this guest.
    pub last_frame: Option<Instant>,
    /// When the last complete frame was written to this guest.
    pub last_written: Option<Instant>,
}

/// Aggregate hub evidence.
#[derive(Clone, Debug, Default)]
pub struct HubStats {
    /// Total complete records read.
    pub frames: u64,
    /// Total envelope bytes read.
    pub bytes: u64,
    /// Records written to a destination.
    pub forwarded: u64,
    /// Records dropped because the destination was this source.
    pub self_drops: u64,
    /// Records dropped because the destination was unknown.
    pub unknown: u64,
    /// Record or header bounds violations.
    pub malformed: u64,
    /// Records dropped by a full or closed destination queue.
    pub backpressure: u64,
    /// Complete records discarded for any reason (queue, malformed, truncated).
    pub dropped: u64,
    /// Partial records abandoned at the receive deadline.
    pub receive_deadlines: u64,
    /// Partial records whose source closed mid-record.
    pub truncated: u64,
    /// Per-source counters, in connection order.
    pub per_source: Vec<(String, LinkStats)>,
}

struct QueueState {
    frames: VecDeque<Vec<u8>>,
    bytes: usize,
    closed: bool,
}

/// A bounded whole-record queue with blocking backpressure.
struct FrameQueue {
    state: Mutex<QueueState>,
    ready: Condvar,
    space: Condvar,
    max_frames: usize,
    max_bytes: usize,
}

enum Pop {
    Frame(Vec<u8>),
    Empty,
    Closed,
}

#[derive(Debug, PartialEq, Eq)]
enum PushError {
    Closed,
    Stopped,
    Deadline,
}

impl FrameQueue {
    fn new(max_frames: usize, max_bytes: usize) -> Self {
        Self {
            state: Mutex::new(QueueState {
                frames: VecDeque::new(),
                bytes: 0,
                closed: false,
            }),
            ready: Condvar::new(),
            space: Condvar::new(),
            max_frames: max_frames.max(1),
            max_bytes: max_bytes.max(MIN_RECORD),
        }
    }

    /// Wait for room and enqueue one whole record.
    fn push(&self, frame: Vec<u8>, stop: &AtomicBool, deadline: Instant) -> Result<(), PushError> {
        let mut state = match self.state.lock() {
            Ok(state) => state,
            Err(_) => return Err(PushError::Closed),
        };
        loop {
            if state.closed {
                return Err(PushError::Closed);
            }
            if stop.load(Ordering::Relaxed) {
                return Err(PushError::Stopped);
            }
            if state.frames.len() < self.max_frames
                && state.bytes.saturating_add(frame.len()) <= self.max_bytes
            {
                state.bytes = state.bytes.saturating_add(frame.len());
                state.frames.push_back(frame);
                self.ready.notify_all();
                return Ok(());
            }
            let now = Instant::now();
            if now >= deadline {
                return Err(PushError::Deadline);
            }
            let wait = deadline
                .saturating_duration_since(now)
                .min(Duration::from_millis(50));
            state = match self.space.wait_timeout(state, wait) {
                Ok((state, _)) => state,
                Err(_) => return Err(PushError::Closed),
            };
        }
    }

    fn pop(&self, timeout: Duration) -> Pop {
        let deadline = Instant::now() + timeout;
        let mut state = match self.state.lock() {
            Ok(state) => state,
            Err(_) => return Pop::Closed,
        };
        loop {
            if let Some(frame) = state.frames.pop_front() {
                state.bytes = state.bytes.saturating_sub(frame.len());
                self.space.notify_all();
                return Pop::Frame(frame);
            }
            if state.closed {
                return Pop::Closed;
            }
            let now = Instant::now();
            if now >= deadline {
                return Pop::Empty;
            }
            let wait = deadline
                .saturating_duration_since(now)
                .min(Duration::from_millis(50));
            state = match self.ready.wait_timeout(state, wait) {
                Ok((state, _)) => state,
                Err(_) => return Pop::Closed,
            };
        }
    }

    fn close(&self) {
        if let Ok(mut state) = self.state.lock() {
            state.closed = true;
            state.frames.clear();
            state.bytes = 0;
        }
        self.ready.notify_all();
        self.space.notify_all();
    }

    #[cfg(test)]
    fn pending(&self) -> (usize, usize) {
        match self.state.lock() {
            Ok(state) => (state.frames.len(), state.bytes),
            Err(_) => (0, 0),
        }
    }
}

struct Link {
    node: Node,
    queue: Arc<FrameQueue>,
    stats: Arc<Mutex<LinkStats>>,
    /// A third descriptor used only to shut the session down and wake the
    /// reader; reads and writes have their own clones.
    shutdown: Mutex<Option<TcpStream>>,
}

impl Link {
    fn stop(&self) {
        self.queue.close();
        if let Ok(mut shutdown) = self.shutdown.lock() {
            if let Some(stream) = shutdown.take() {
                let _ = stream.shutdown(Shutdown::Both);
            }
        }
    }
}

struct LinkEntry {
    name: String,
    /// Kept for `remove_link`; the registry key is the same value.
    #[allow(dead_code)]
    node_id: [u8; 16],
    stats: Arc<Mutex<LinkStats>>,
    reader: Option<JoinHandle<()>>,
    writer: Option<JoinHandle<()>>,
}

/// A started hub.  Dropping it closes every session and joins its threads.
pub struct MeshHub {
    stop: Arc<AtomicBool>,
    stats: Arc<Mutex<HubStats>>,
    registry: Arc<Mutex<HashMap<[u8; 16], Arc<Link>>>>,
    links: Vec<LinkEntry>,
    root: [u8; 16],
    config: HubConfig,
    started: Instant,
}

impl MeshHub {
    /// Connect the root and `first_member`.
    ///
    /// The caller waits for the signed `epoch=2` confirmation before adding
    /// another member, so the root's first route peer is deterministic without
    /// a central placement decision: the rig only chooses the connection
    /// order.
    // The default constructor is kept for callers that do not need to
    // override the bounds; the shard runner uses `start_with` for its M3
    // receive/write deadlines.
    #[allow(dead_code)]
    pub fn start(nodes: &[Node], first_member: usize) -> Result<Self, String> {
        Self::start_with(nodes, first_member, HubConfig::default())
    }

    /// `start` with explicit bounds (used by the host tests).
    pub fn start_with(
        nodes: &[Node],
        first_member: usize,
        config: HubConfig,
    ) -> Result<Self, String> {
        if nodes.len() < 2 || first_member == 0 || first_member >= nodes.len() {
            return Err("hub needs a root and a non-root first member".into());
        }
        if nodes.len() > MAX_LINKS {
            return Err(format!("hub supports at most {MAX_LINKS} links"));
        }
        let root = nodes[0].node_id;
        let stop = Arc::new(AtomicBool::new(false));
        let stats = Arc::new(Mutex::new(HubStats::default()));
        let registry: Arc<Mutex<HashMap<[u8; 16], Arc<Link>>>> =
            Arc::new(Mutex::new(HashMap::new()));
        let started = Instant::now();
        let mut hub = Self {
            stop,
            stats,
            registry,
            links: Vec::new(),
            root,
            config,
            started,
        };

        for index in [0usize, first_member] {
            hub.add_link(&nodes[index])?;
        }
        Ok(hub)
    }

    /// Add one more member link.
    ///
    /// The caller adds members one at a time and waits for the root's epoch to
    /// advance before adding the next.  That is not a policy rule: it keeps the
    /// host test's frame burst small while preserving the production join
    /// handshake, signed epoch and single-writer authority.
    pub fn add_link(&mut self, node: &Node) -> Result<(), String> {
        if self
            .registry
            .lock()
            .map_err(poison)?
            .contains_key(&node.node_id)
        {
            return Err(format!("hub already has a link for {}", node.name));
        }
        if self.links.len() >= MAX_LINKS {
            return Err(format!("hub supports at most {MAX_LINKS} links"));
        }
        let (read, write, link) = connect(node, &self.config)?;
        let id = link.node.node_id;
        let stats = link.stats.clone();

        let writer = match spawn_writer(
            write,
            link.queue.clone(),
            stats.clone(),
            self.stop.clone(),
            self.config.clone(),
            link.node.name.clone(),
        ) {
            Ok(handle) => handle,
            Err(error) => {
                link.stop();
                return Err(error);
            }
        };
        if let Err(error) = self.registry.lock().map_err(poison).map(|mut registry| {
            registry.insert(id, link.clone());
        }) {
            link.stop();
            let _ = writer.join();
            return Err(error);
        }
        let reader = match spawn_reader(
            id,
            read,
            link.clone(),
            self.registry.clone(),
            self.stop.clone(),
            self.stats.clone(),
            self.root,
            self.config.clone(),
            self.started,
        ) {
            Ok(handle) => handle,
            Err(error) => {
                if let Ok(mut registry) = self.registry.lock() {
                    registry.remove(&id);
                }
                link.stop();
                let _ = writer.join();
                return Err(error);
            }
        };
        self.links.push(LinkEntry {
            name: link.node.name.clone(),
            node_id: id,
            stats,
            reader: Some(reader),
            writer: Some(writer),
        });
        Ok(())
    }

    /// Close one member session.  Frames addressed to it become `unknown`.
    ///
    /// The rig keeps this for a future leave/churn scenario; the host tests
    /// exercise it now, so it is part of the harness surface rather than dead
    /// code.
    #[allow(dead_code)]
    pub fn remove_link(&mut self, node_id: [u8; 16]) -> bool {
        let link = self
            .registry
            .lock()
            .ok()
            .and_then(|mut registry| registry.remove(&node_id));
        let Some(link) = link else {
            return false;
        };
        link.stop();
        let Some(index) = self.links.iter().position(|entry| entry.node_id == node_id) else {
            return true;
        };
        let mut entry = self.links.remove(index);
        if let Some(reader) = entry.reader.take() {
            let _ = reader.join();
        }
        if let Some(writer) = entry.writer.take() {
            let _ = writer.join();
        }
        true
    }

    /// Whether a live session has been silent for `threshold`.
    ///
    /// The iris host forward can hold a healthy host TCP connection while its
    /// guest-facing window is stalled.  The guest therefore sees no bytes and
    /// sends none; a host-side silence deadline is the only signal that the
    /// session needs replacing.  A converged node is never queried by the rig.
    pub fn node_stalled(&self, node_id: [u8; 16], threshold: Duration) -> bool {
        let link = match self.registry.lock() {
            Ok(registry) => registry.get(&node_id).cloned(),
            Err(_) => return false,
        };
        let Some(link) = link else {
            return false;
        };
        let stats = match link.stats.lock() {
            Ok(stats) => stats,
            Err(_) => return false,
        };
        if stats.writer_stopped || stats.reader_stopped {
            return false;
        }
        let inbound_silent = stats
            .last_frame
            .is_none_or(|last| last.elapsed() >= threshold);
        let outbound_silent = stats
            .last_written
            .is_none_or(|last| last.elapsed() >= threshold);
        inbound_silent || outbound_silent
    }

    /// A compact diagnostic for a failed convergence.
    pub fn summary(&self) -> String {
        let stats = self.stats();
        let writer_errors: u64 = stats.per_source.iter().map(|(_, s)| s.write_errors).sum();
        let queue_full: u64 = stats.per_source.iter().map(|(_, s)| s.queue_full).sum();
        let queue_closed: u64 = stats.per_source.iter().map(|(_, s)| s.queue_closed).sum();
        let stopped: Vec<&str> = stats
            .per_source
            .iter()
            .filter(|(_, s)| s.writer_stopped)
            .map(|(name, _)| name.as_str())
            .collect();
        format!(
            "hub frames={} forwarded={} self_drops={} unknown={} malformed={} \
             backpressure={} dropped={} receive_deadlines={} truncated={} \
             queue_full={queue_full} queue_closed={queue_closed} \
             write_errors={writer_errors} stopped=[{}] sources={}",
            stats.frames,
            stats.forwarded,
            stats.self_drops,
            stats.unknown,
            stats.malformed,
            stats.backpressure,
            stats.dropped,
            stats.receive_deadlines,
            stats.truncated,
            stopped.join(","),
            stats
                .per_source
                .iter()
                .map(|(name, _)| name.as_str())
                .collect::<Vec<_>>()
                .join(",")
        )
    }

    /// Snapshot the counters.
    pub fn stats(&self) -> HubStats {
        let mut stats = self
            .stats
            .lock()
            .map(|stats| stats.clone())
            .unwrap_or_default();
        stats.per_source = self
            .links
            .iter()
            .map(|entry| {
                (
                    entry.name.clone(),
                    entry
                        .stats
                        .lock()
                        .map(|stats| stats.clone())
                        .unwrap_or_default(),
                )
            })
            .collect();
        stats.forwarded = stats
            .per_source
            .iter()
            .map(|(_, stats)| stats.forwarded)
            .sum();
        stats.truncated = stats
            .per_source
            .iter()
            .map(|(_, stats)| stats.truncated)
            .sum();
        stats.receive_deadlines = stats
            .per_source
            .iter()
            .map(|(_, stats)| stats.receive_deadlines)
            .sum();
        stats
    }

    /// Close every session and wait for the reader and writer threads.
    ///
    /// `Drop` calls this.  Exposed so tests can prove the teardown is bounded
    /// rather than relying on process exit.
    pub fn shutdown(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        let links: Vec<Arc<Link>> = match self.registry.lock() {
            Ok(mut registry) => registry.drain().map(|(_, link)| link).collect(),
            Err(_) => Vec::new(),
        };
        for link in &links {
            link.stop();
        }
        for entry in &mut self.links {
            if let Some(reader) = entry.reader.take() {
                let _ = reader.join();
            }
            if let Some(writer) = entry.writer.take() {
                let _ = writer.join();
            }
        }
    }
}

impl Drop for MeshHub {
    fn drop(&mut self) {
        self.shutdown();
    }
}

fn poison<T>(_: T) -> String {
    "hub registry mutex poisoned".to_string()
}

fn connect(node: &Node, config: &HubConfig) -> Result<(TcpStream, TcpStream, Arc<Link>), String> {
    let address: SocketAddr = (Ipv4Addr::LOCALHOST, node.mesh_port).into();
    let deadline = Instant::now() + config.connect_timeout;
    loop {
        match TcpStream::connect_timeout(&address, Duration::from_secs(2)) {
            Ok(stream) => {
                let _ = stream.set_nodelay(true);
                let _ = stream.set_read_timeout(Some(config.read_timeout));
                let _ = stream.set_write_timeout(Some(config.write_timeout));
                let read = stream
                    .try_clone()
                    .map_err(|error| format!("clone {} read stream: {error}", node.name))?;
                let write = stream
                    .try_clone()
                    .map_err(|error| format!("clone {} write stream: {error}", node.name))?;
                let stats = Arc::new(Mutex::new(LinkStats {
                    last_frame: Some(Instant::now()),
                    last_written: Some(Instant::now()),
                    ..LinkStats::default()
                }));
                let queue = Arc::new(FrameQueue::new(config.queue_frames, config.queue_bytes));
                return Ok((
                    read,
                    write,
                    Arc::new(Link {
                        node: node.clone(),
                        queue,
                        stats,
                        shutdown: Mutex::new(Some(stream)),
                    }),
                ));
            }
            Err(error) => {
                if Instant::now() >= deadline {
                    return Err(format!(
                        "connect to {} mesh forward 127.0.0.1:{}: {error}",
                        node.name, node.mesh_port
                    ));
                }
                thread::sleep(Duration::from_millis(100));
            }
        }
    }
}

fn spawn_writer(
    mut stream: TcpStream,
    queue: Arc<FrameQueue>,
    stats: Arc<Mutex<LinkStats>>,
    stop: Arc<AtomicBool>,
    config: HubConfig,
    name: String,
) -> Result<JoinHandle<()>, String> {
    let error_name = name.clone();
    thread::Builder::new()
        .name(format!("m3-hub-writer-{name}"))
        .spawn(move || {
            let mut write_errors = 0u64;
            loop {
                match queue.pop(Duration::from_millis(200)) {
                    Pop::Frame(frame) => {
                        if !config.write_delay.is_zero() {
                            thread::sleep(config.write_delay);
                        }
                        let deadline = Instant::now() + config.write_deadline;
                        let mut offset = 0usize;
                        let mut failed = None;
                        while offset < frame.len() {
                            if stop.load(Ordering::Relaxed) {
                                failed = Some("stopped");
                                break;
                            }
                            match stream.write(&frame[offset..]) {
                                Ok(0) => {
                                    failed = Some("zero-write");
                                    break;
                                }
                                Ok(written) => offset += written,
                                Err(error)
                                    if error.kind() == std::io::ErrorKind::WouldBlock
                                        || error.kind() == std::io::ErrorKind::TimedOut =>
                                {
                                    if Instant::now() >= deadline {
                                        failed = Some("write-deadline");
                                        break;
                                    }
                                }
                                Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
                                Err(_) => {
                                    failed = Some("write-error");
                                    break;
                                }
                            }
                        }
                        if let Some(reason) = failed {
                            write_errors = write_errors.saturating_add(1);
                            if std::env::var_os("IRIX7_HUB_TRACE").is_some() {
                                eprintln!("hub: writer {name} stopped: {reason}");
                            }
                            let _ = stream.shutdown(Shutdown::Both);
                            break;
                        }
                        if let Ok(mut local) = stats.lock() {
                            local.forwarded = local.forwarded.saturating_add(1);
                            local.written = local.written.saturating_add(frame.len() as u64);
                            local.last_written = Some(Instant::now());
                        }
                    }
                    Pop::Empty => {
                        if stop.load(Ordering::Relaxed) {
                            break;
                        }
                    }
                    Pop::Closed => break,
                }
            }
            queue.close();
            if let Ok(mut local) = stats.lock() {
                local.write_errors = local.write_errors.saturating_add(write_errors);
                local.writer_stopped = true;
            }
        })
        .map_err(|error| format!("spawn hub writer for {error_name}: {error}"))
}

#[allow(clippy::too_many_arguments)]
fn spawn_reader(
    id: [u8; 16],
    mut read: TcpStream,
    link: Arc<Link>,
    registry: Arc<Mutex<HashMap<[u8; 16], Arc<Link>>>>,
    stop: Arc<AtomicBool>,
    stats: Arc<Mutex<HubStats>>,
    root: [u8; 16],
    config: HubConfig,
    started: Instant,
) -> Result<JoinHandle<()>, String> {
    let source_stats = link.stats.clone();
    let name = link.node.name.clone();
    let thread_name = name.clone();
    thread::Builder::new()
        .name(format!("m3-hub-reader-{name}"))
        .spawn(move || {
            read_loop(
                id,
                &mut read,
                &registry,
                &stop,
                &stats,
                &source_stats,
                &thread_name,
                root,
                &config,
                started,
            )
        })
        .map_err(|error| format!("spawn hub reader for {name}: {error}"))
}

enum ReadError {
    Closed,
    Stopped,
    Deadline,
}

fn read_exact_until(
    stream: &mut TcpStream,
    buffer: &mut [u8],
    stop: &AtomicBool,
    deadline: Instant,
) -> Result<(), ReadError> {
    let mut offset = 0usize;
    while offset < buffer.len() {
        if stop.load(Ordering::Relaxed) {
            return Err(ReadError::Stopped);
        }
        match stream.read(&mut buffer[offset..]) {
            Ok(0) => return Err(ReadError::Closed),
            Ok(count) => offset += count,
            Err(error)
                if error.kind() == std::io::ErrorKind::WouldBlock
                    || error.kind() == std::io::ErrorKind::TimedOut =>
            {
                if Instant::now() >= deadline {
                    return Err(ReadError::Deadline);
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
            Err(_) => return Err(ReadError::Closed),
        }
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn read_loop(
    source_id: [u8; 16],
    stream: &mut TcpStream,
    registry: &Arc<Mutex<HashMap<[u8; 16], Arc<Link>>>>,
    stop: &Arc<AtomicBool>,
    stats: &Arc<Mutex<HubStats>>,
    source_stats: &Arc<Mutex<LinkStats>>,
    source_name: &str,
    root: [u8; 16],
    config: &HubConfig,
    started: Instant,
) {
    let mut prefix = [0u8; 4];
    let mut record = vec![0u8; config.max_record.min(MAX_RECORD)];
    'session: loop {
        if stop.load(Ordering::Relaxed) {
            break;
        }
        let idle_deadline = Instant::now() + config.idle_timeout;
        match read_exact_until(stream, &mut prefix, stop, idle_deadline) {
            Ok(()) => {}
            Err(ReadError::Stopped) => break,
            Err(ReadError::Deadline) => {
                count(stats, source_stats, |total, local| {
                    total.malformed = total.malformed.saturating_add(1);
                    total.dropped = total.dropped.saturating_add(1);
                    local.truncated = local.truncated.saturating_add(1);
                });
                break;
            }
            Err(ReadError::Closed) => break,
        }
        let length = u32::from_le_bytes(prefix) as usize;
        if !(MIN_RECORD..=record.len()).contains(&length) {
            count(stats, source_stats, |total, local| {
                total.malformed = total.malformed.saturating_add(1);
                total.dropped = total.dropped.saturating_add(1);
                local.truncated = local.truncated.saturating_add(1);
            });
            break;
        }
        let body_deadline = Instant::now() + config.receive_deadline;
        match read_exact_until(stream, &mut record[..length], stop, body_deadline) {
            Ok(()) => {}
            Err(ReadError::Stopped) => break,
            Err(ReadError::Deadline) => {
                count(stats, source_stats, |total, local| {
                    total.receive_deadlines = total.receive_deadlines.saturating_add(1);
                    total.dropped = total.dropped.saturating_add(1);
                    local.receive_deadlines = local.receive_deadlines.saturating_add(1);
                });
                break;
            }
            Err(ReadError::Closed) => {
                count(stats, source_stats, |total, local| {
                    total.truncated = total.truncated.saturating_add(1);
                    total.dropped = total.dropped.saturating_add(1);
                    local.truncated = local.truncated.saturating_add(1);
                });
                break;
            }
        }
        count(stats, source_stats, |total, local| {
            total.frames = total.frames.saturating_add(1);
            total.bytes = total.bytes.saturating_add(length as u64);
            local.received = local.received.saturating_add(1);
            local.bytes = local.bytes.saturating_add(length as u64);
            local.last_frame = Some(Instant::now());
        });
        let record = &record[..length];
        if record.len() < HEADER_LEN || &record[0..4] != b"I7MH" || record[4] != 1 {
            count(stats, source_stats, |total, local| {
                total.malformed = total.malformed.saturating_add(1);
                total.dropped = total.dropped.saturating_add(1);
                local.truncated = local.truncated.saturating_add(1);
            });
            continue;
        }
        let mut src = [0u8; 16];
        src.copy_from_slice(&record[8..24]);
        let mut dst = [0u8; 16];
        dst.copy_from_slice(&record[24..40]);
        if src != source_id {
            count(stats, source_stats, |total, local| {
                total.malformed = total.malformed.saturating_add(1);
                total.dropped = total.dropped.saturating_add(1);
                local.truncated = local.truncated.saturating_add(1);
            });
            continue;
        }
        if std::env::var_os("IRIX7_HUB_TRACE").is_some() {
            eprintln!(
                "hub: {} -> {} len={length} t={}ms",
                hex16(&src),
                hex16(&dst),
                started.elapsed().as_millis()
            );
        }
        let target = if dst == [0u8; 16] { root } else { dst };
        if target == source_id {
            count(stats, source_stats, |total, _| {
                total.self_drops = total.self_drops.saturating_add(1);
                total.dropped = total.dropped.saturating_add(1);
            });
            continue;
        }
        let link = match registry.lock() {
            Ok(registry) => registry.get(&target).cloned(),
            Err(_) => break,
        };
        let Some(link) = link else {
            if std::env::var_os("IRIX7_HUB_TRACE").is_some() {
                eprintln!("hub: unknown target={}", hex16(&target));
            }
            count(stats, source_stats, |total, _| {
                total.unknown = total.unknown.saturating_add(1);
                total.dropped = total.dropped.saturating_add(1);
            });
            continue;
        };
        let mut frame = Vec::with_capacity(4 + length);
        frame.extend_from_slice(&(length as u32).to_le_bytes());
        frame.extend_from_slice(record);
        let deadline = Instant::now() + config.backpressure_deadline;
        match link.queue.push(frame, stop, deadline) {
            Ok(()) => {}
            Err(PushError::Deadline) => {
                if std::env::var_os("IRIX7_HUB_TRACE").is_some() {
                    eprintln!("hub: backpressure for {}", hex16(&target));
                }
                count(stats, source_stats, |total, local| {
                    total.backpressure = total.backpressure.saturating_add(1);
                    total.dropped = total.dropped.saturating_add(1);
                    local.queue_full = local.queue_full.saturating_add(1);
                });
            }
            Err(PushError::Closed) => {
                if std::env::var_os("IRIX7_HUB_TRACE").is_some() {
                    eprintln!("hub: writer gone for {}", hex16(&target));
                }
                count(stats, source_stats, |total, local| {
                    total.dropped = total.dropped.saturating_add(1);
                    local.queue_closed = local.queue_closed.saturating_add(1);
                });
            }
            Err(PushError::Stopped) => break 'session,
        }
    }
    if std::env::var_os("IRIX7_HUB_TRACE").is_some() {
        eprintln!(
            "hub: reader {} ({source_name}) exited t={}ms",
            hex16(&source_id),
            started.elapsed().as_millis()
        );
    }
    if let Ok(mut local) = source_stats.lock() {
        local.reader_stopped = true;
    }
}

fn count(
    stats: &Arc<Mutex<HubStats>>,
    source: &Arc<Mutex<LinkStats>>,
    update: impl FnOnce(&mut HubStats, &mut LinkStats),
) {
    if let Ok(mut source) = source.lock() {
        if let Ok(mut total) = stats.lock() {
            update(&mut total, &mut source);
        }
    }
}

fn hex16(bytes: &[u8; 16]) -> String {
    let mut out = String::with_capacity(32);
    for byte in bytes {
        out.push_str(&format!("{byte:02X}"));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;
    use std::path::PathBuf;
    use std::sync::mpsc;

    fn test_config() -> HubConfig {
        HubConfig {
            connect_timeout: Duration::from_secs(5),
            read_timeout: Duration::from_millis(50),
            idle_timeout: Duration::from_secs(5),
            receive_deadline: Duration::from_secs(2),
            write_timeout: Duration::from_millis(50),
            write_deadline: Duration::from_secs(5),
            backpressure_deadline: Duration::from_secs(5),
            queue_frames: 16,
            queue_bytes: 64 * 1024,
            max_record: MAX_RECORD,
            write_delay: Duration::ZERO,
        }
    }

    /// A fake guest: it listens on the mesh forward and reads/writes whole
    /// framed records.  It is deliberately simpler than the guest transport;
    /// the hub only needs a peer socket to pump bytes.
    struct FakeGuest {
        name: String,
        id: [u8; 16],
        port: u16,
        outbound: mpsc::Sender<Vec<u8>>,
        inbound: mpsc::Receiver<Vec<u8>>,
        reading: Arc<AtomicBool>,
        stop: Arc<AtomicBool>,
        thread: Option<JoinHandle<()>>,
    }

    impl FakeGuest {
        fn new(seed: u8) -> Self {
            let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).expect("bind fake guest");
            listener
                .set_nonblocking(true)
                .expect("nonblocking fake guest");
            let port = listener.local_addr().expect("fake guest address").port();
            let mut id = [0u8; 16];
            id[0] = seed;
            let name = format!("g{seed}");
            let (outbound, out_rx) = mpsc::channel::<Vec<u8>>();
            let (in_tx, inbound) = mpsc::channel::<Vec<u8>>();
            let reading = Arc::new(AtomicBool::new(true));
            let stop = Arc::new(AtomicBool::new(false));
            let thread_reading = reading.clone();
            let thread_stop = stop.clone();
            let thread = thread::Builder::new()
                .name(format!("fake-guest-{seed}"))
                .spawn(move || {
                    let mut stream: Option<TcpStream> = None;
                    let mut pending = Vec::new();
                    let mut incoming = Vec::new();
                    let mut buffer = [0u8; 4096];
                    while !thread_stop.load(Ordering::Relaxed) {
                        if stream.is_none() {
                            match listener.accept() {
                                Ok((accepted, _)) => {
                                    let _ = accepted.set_nonblocking(true);
                                    stream = Some(accepted);
                                }
                                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {}
                                Err(_) => break,
                            }
                        }
                        let Some(socket) = stream.as_mut() else {
                            thread::sleep(Duration::from_millis(1));
                            continue;
                        };
                        while let Ok(frame) = out_rx.try_recv() {
                            pending.extend_from_slice(&frame);
                        }
                        if !pending.is_empty() {
                            match socket.write(&pending) {
                                Ok(0) => break,
                                Ok(written) => {
                                    pending.drain(..written);
                                }
                                Err(error)
                                    if error.kind() == std::io::ErrorKind::WouldBlock
                                        || error.kind() == std::io::ErrorKind::Interrupted => {}
                                Err(_) => break,
                            }
                        }
                        if thread_reading.load(Ordering::Relaxed) {
                            match socket.read(&mut buffer) {
                                Ok(0) => break,
                                Ok(count) => incoming.extend_from_slice(&buffer[..count]),
                                Err(error)
                                    if error.kind() == std::io::ErrorKind::WouldBlock
                                        || error.kind() == std::io::ErrorKind::Interrupted => {}
                                Err(_) => break,
                            }
                        }
                        while incoming.len() >= 4 {
                            let length = u32::from_le_bytes([
                                incoming[0],
                                incoming[1],
                                incoming[2],
                                incoming[3],
                            ]) as usize;
                            if incoming.len() < 4 + length {
                                break;
                            }
                            let record = incoming[4..4 + length].to_vec();
                            incoming.drain(..4 + length);
                            let _ = in_tx.send(record);
                        }
                        thread::sleep(Duration::from_millis(1));
                    }
                })
                .expect("spawn fake guest");
            Self {
                name,
                id,
                port,
                outbound,
                inbound,
                reading,
                stop,
                thread: Some(thread),
            }
        }

        fn node(&self) -> Node {
            Node {
                name: self.name.clone(),
                instance: 0,
                state_dir: PathBuf::new(),
                config: PathBuf::new(),
                ci_socket: PathBuf::new(),
                serial_log: PathBuf::new(),
                monitor: 0,
                mac: [0u8; 6],
                node_id: self.id,
                nat_subnet: String::new(),
                gateway: Ipv4Addr::LOCALHOST,
                guest: Ipv4Addr::LOCALHOST,
                telnet_port: 0,
                http_port: 0,
                mesh_port: self.port,
                port_base: 0,
            }
        }

        fn send_record(&self, src: [u8; 16], dst: [u8; 16], payload: &[u8]) {
            let mut record = vec![0u8; HEADER_LEN + payload.len()];
            record[0..4].copy_from_slice(b"I7MH");
            record[4] = 1;
            record[8..24].copy_from_slice(&src);
            record[24..40].copy_from_slice(&dst);
            record[HEADER_LEN..].copy_from_slice(payload);
            let mut frame = Vec::with_capacity(4 + record.len());
            frame.extend_from_slice(&(record.len() as u32).to_le_bytes());
            frame.extend_from_slice(&record);
            self.outbound.send(frame).expect("fake guest send");
        }

        fn send_raw(&self, bytes: &[u8]) {
            self.outbound.send(bytes.to_vec()).expect("fake guest raw");
        }

        fn recv(&self) -> Vec<u8> {
            self.inbound
                .recv_timeout(Duration::from_secs(3))
                .expect("fake guest receive")
        }

        fn assert_empty(&self) {
            assert!(
                self.inbound
                    .recv_timeout(Duration::from_millis(200))
                    .is_err(),
                "{} received an unexpected frame",
                self.name
            );
        }

        fn set_reading(&self, reading: bool) {
            self.reading.store(reading, Ordering::Relaxed);
        }
    }

    impl Drop for FakeGuest {
        fn drop(&mut self) {
            self.stop.store(true, Ordering::Relaxed);
            if let Some(thread) = self.thread.take() {
                let _ = thread.join();
            }
        }
    }

    fn start(root: &FakeGuest, member: &FakeGuest) -> MeshHub {
        MeshHub::start_with(&[root.node(), member.node()], 1, test_config()).expect("start hub")
    }

    fn payload(record: &[u8]) -> &[u8] {
        &record[HEADER_LEN..]
    }

    #[test]
    fn routes_by_destination_and_counts_self_and_unknown() {
        let root = FakeGuest::new(1);
        let a = FakeGuest::new(2);
        let b = FakeGuest::new(3);
        let mut hub = start(&root, &a);
        hub.add_link(&b.node()).expect("add b");

        a.send_record(a.id, b.id, b"a-to-b");
        assert_eq!(payload(&b.recv()), b"a-to-b");

        root.send_record(root.id, a.id, b"root-to-a");
        assert_eq!(payload(&a.recv()), b"root-to-a");

        a.send_record(a.id, a.id, b"self");
        root.assert_empty();
        b.assert_empty();

        a.send_record(a.id, [9u8; 16], b"nobody");
        root.assert_empty();
        b.assert_empty();

        let stats = hub.stats();
        assert_eq!(stats.self_drops, 1, "{stats:?}");
        assert_eq!(stats.unknown, 1, "{stats:?}");
        assert_eq!(stats.malformed, 0, "{stats:?}");
        assert_eq!(stats.backpressure, 0, "{stats:?}");
    }

    #[test]
    fn forwards_duplicates_and_preserves_source_order() {
        let root = FakeGuest::new(10);
        let a = FakeGuest::new(11);
        let hub = start(&root, &a);
        for index in 0..32u32 {
            a.send_record(a.id, root.id, &index.to_be_bytes());
        }
        for index in 0..32u32 {
            assert_eq!(payload(&root.recv()), &index.to_be_bytes());
        }
        a.send_record(a.id, root.id, &7u32.to_be_bytes());
        assert_eq!(payload(&root.recv()), &7u32.to_be_bytes());
        let stats = hub.stats();
        assert_eq!(stats.frames, 33, "{stats:?}");
        assert_eq!(stats.forwarded, 33, "{stats:?}");
        assert_eq!(stats.dropped, 0, "{stats:?}");
    }

    #[test]
    fn truncated_record_is_never_forwarded() {
        let root = FakeGuest::new(20);
        let a = FakeGuest::new(21);
        let hub = start(&root, &a);
        // A prefix for 256 bytes, then a short body, then close.
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&256u32.to_le_bytes());
        bytes.extend_from_slice(&[0u8; 10]);
        a.send_raw(&bytes);
        thread::sleep(Duration::from_millis(100));
        drop(a);
        root.assert_empty();
        let stats = hub.stats();
        assert_eq!(stats.truncated, 1, "{stats:?}");
        assert_eq!(stats.forwarded, 0, "{stats:?}");
    }

    #[test]
    fn partial_record_hits_the_receive_deadline() {
        let root = FakeGuest::new(30);
        let a = FakeGuest::new(31);
        let mut config = test_config();
        config.receive_deadline = Duration::from_millis(300);
        let hub = MeshHub::start_with(&[root.node(), a.node()], 1, config).expect("start hub");
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&256u32.to_le_bytes());
        bytes.extend_from_slice(&[0u8; 10]);
        a.send_raw(&bytes);
        thread::sleep(Duration::from_millis(700));
        root.assert_empty();
        let stats = hub.stats();
        assert_eq!(stats.receive_deadlines, 1, "{stats:?}");
        assert_eq!(stats.forwarded, 0, "{stats:?}");
    }

    #[test]
    fn malformed_header_is_refused_but_the_link_continues() {
        let root = FakeGuest::new(40);
        let a = FakeGuest::new(41);
        let hub = start(&root, &a);
        let mut bad = vec![0u8; HEADER_LEN];
        bad[0..4].copy_from_slice(b"XXXX");
        bad[4] = 1;
        bad[8..24].copy_from_slice(&a.id);
        bad[24..40].copy_from_slice(&root.id);
        let mut frame = Vec::new();
        frame.extend_from_slice(&(bad.len() as u32).to_le_bytes());
        frame.extend_from_slice(&bad);
        a.send_raw(&frame);
        a.send_record(a.id, root.id, b"after");
        assert_eq!(payload(&root.recv()), b"after");
        let stats = hub.stats();
        assert_eq!(stats.malformed, 1, "{stats:?}");
        assert_eq!(stats.forwarded, 1, "{stats:?}");
    }

    #[test]
    fn late_joiner_can_send_and_receive() {
        let root = FakeGuest::new(50);
        let a = FakeGuest::new(51);
        let b = FakeGuest::new(52);
        let mut hub = start(&root, &a);
        a.send_record(a.id, root.id, b"warm");
        assert_eq!(payload(&root.recv()), b"warm");

        hub.add_link(&b.node()).expect("late joiner");
        root.send_record(root.id, b.id, b"to-late");
        assert_eq!(payload(&b.recv()), b"to-late");
        b.send_record(b.id, root.id, b"from-late");
        assert_eq!(payload(&root.recv()), b"from-late");
        assert_eq!(hub.stats().per_source.len(), 3);
    }

    #[test]
    fn leave_closes_the_session_and_makes_it_unknown() {
        let root = FakeGuest::new(60);
        let a = FakeGuest::new(61);
        let b = FakeGuest::new(62);
        let mut hub = start(&root, &a);
        hub.add_link(&b.node()).expect("add b");
        b.send_record(b.id, root.id, b"before-leave");
        assert_eq!(payload(&root.recv()), b"before-leave");

        assert!(hub.remove_link(b.id));
        assert!(!hub.remove_link(b.id));
        root.send_record(root.id, b.id, b"after-leave");
        root.assert_empty();
        assert_eq!(hub.stats().unknown, 1);
    }

    #[test]
    fn start_requires_root_and_first_member_and_rejects_duplicates() {
        let root = FakeGuest::new(70);
        let a = FakeGuest::new(71);
        assert!(MeshHub::start(&[], 1).is_err());
        assert!(MeshHub::start(&[root.node()], 1).is_err());
        assert!(MeshHub::start(&[root.node(), a.node()], 0).is_err());
        assert!(MeshHub::start(&[root.node(), a.node()], 2).is_err());

        let mut hub = start(&root, &a);
        assert!(hub.add_link(&root.node()).is_err());
        assert!(hub.add_link(&a.node()).is_err());
    }

    #[test]
    fn queue_bounds_wait_for_space_and_preserve_order() {
        let queue = FrameQueue::new(2, 128);
        let stop = AtomicBool::new(false);
        let now = Instant::now();
        queue
            .push(vec![1u8; 40], &stop, now + Duration::from_secs(1))
            .unwrap();
        queue
            .push(vec![2u8; 40], &stop, now + Duration::from_secs(1))
            .unwrap();
        assert!(matches!(
            queue.push(
                vec![3u8; 40],
                &stop,
                Instant::now() + Duration::from_millis(50)
            ),
            Err(PushError::Deadline)
        ));
        let Pop::Frame(first) = queue.pop(Duration::from_millis(10)) else {
            panic!("first frame");
        };
        assert_eq!(first, vec![1u8; 40]);
        queue
            .push(
                vec![3u8; 40],
                &stop,
                Instant::now() + Duration::from_secs(1),
            )
            .unwrap();
        let Pop::Frame(second) = queue.pop(Duration::from_millis(10)) else {
            panic!("second frame");
        };
        assert_eq!(second, vec![2u8; 40]);
        let Pop::Frame(third) = queue.pop(Duration::from_millis(10)) else {
            panic!("third frame");
        };
        assert_eq!(third, vec![3u8; 40]);
        assert_eq!(queue.pending(), (0, 0));
    }

    #[test]
    fn backpressure_preserves_every_frame() {
        let root = FakeGuest::new(80);
        let a = FakeGuest::new(81);
        root.set_reading(false);
        let mut config = test_config();
        config.queue_frames = 2;
        config.queue_bytes = 4096;
        config.write_delay = Duration::from_millis(50);
        let hub = MeshHub::start_with(&[root.node(), a.node()], 1, config).expect("start hub");

        for index in 0..8u32 {
            a.send_record(a.id, root.id, &index.to_be_bytes());
        }
        thread::sleep(Duration::from_millis(150));
        root.set_reading(true);
        for index in 0..8u32 {
            assert_eq!(payload(&root.recv()), &index.to_be_bytes());
        }
        let stats = hub.stats();
        assert_eq!(stats.dropped, 0, "{stats:?}");
        assert_eq!(stats.backpressure, 0, "{stats:?}");
        assert_eq!(stats.forwarded, 8, "{stats:?}");
    }

    #[test]
    fn shutdown_closes_every_session_and_stops_the_threads() {
        let root = FakeGuest::new(90);
        let a = FakeGuest::new(91);
        let mut hub = start(&root, &a);
        a.send_record(a.id, root.id, b"shutdown");
        assert_eq!(payload(&root.recv()), b"shutdown");
        hub.shutdown();
        let stats = hub.stats();
        assert!(
            stats.per_source.iter().all(|(_, link)| link.writer_stopped),
            "{stats:?}"
        );
        assert!(
            stats.per_source.iter().all(|(_, link)| link.reader_stopped),
            "{stats:?}"
        );
    }
}
