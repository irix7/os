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
//! Every link has a bounded writer queue and its own writer thread.  A reader
//! therefore never blocks on a slow destination, which would otherwise let two
//! guests fill each other's receive windows while both are trying to write.
//! The queue drops with a counter when full; the mesh checksums and deadlines
//! decide whether the dropped frame matters.
//!
//! The hub is test harness only.  It is not linked into a kernel or module and
//! is not a production routing component.  The production invariant remains
//! "one logical link per peer" (M2-ADR-0002); the hub is the bounded M3 rig
//! workaround for the guest's single module TCP slot.

use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{sync_channel, SyncSender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use crate::rig::Node;

/// Largest record the M2/M3 transport accepts.
pub const MAX_RECORD: usize = 64 * 1024;
/// Smallest envelope: a 128-byte `MeshHeader` plus the frame header.
pub const MIN_RECORD: usize = 128;
const HEADER_LEN: usize = 128;
const LINK_QUEUE: usize = 256;

/// One node's counters.
#[derive(Clone, Debug, Default)]
pub struct LinkStats {
    /// Frames read from this guest.
    pub received: u64,
    /// Bytes read from this guest.
    pub bytes: u64,
    /// Frames written to this guest.
    pub forwarded: u64,
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
    /// Per-source counters.
    pub per_source: Vec<(String, LinkStats)>,
}

struct Link {
    node: Node,
    tx: SyncSender<Vec<u8>>,
    stats: Arc<Mutex<LinkStats>>,
}

/// A started hub.  Dropping it asks the readers to stop.
pub struct MeshHub {
    stop: Arc<AtomicBool>,
    stats: Arc<Mutex<HubStats>>,
    registry: Arc<Mutex<HashMap<[u8; 16], Arc<Link>>>>,
    links: Vec<(String, Arc<Mutex<LinkStats>>)>,
    root: [u8; 16],
}

impl MeshHub {
    /// Connect the root and `first_member`.
    ///
    /// The caller waits for the signed `epoch=2` confirmation before adding
    /// another member, so the root's first route peer is deterministic without
    /// a central placement decision: the rig only chooses the connection
    /// order.
    pub fn start(nodes: &[Node], first_member: usize) -> Result<Self, String> {
        if nodes.len() < 2 || first_member == 0 || first_member >= nodes.len() {
            return Err("hub needs a root and a non-root first member".into());
        }
        let root = nodes[0].node_id;
        let stop = Arc::new(AtomicBool::new(false));
        let stats = Arc::new(Mutex::new(HubStats::default()));
        let registry: Arc<Mutex<HashMap<[u8; 16], Arc<Link>>>> =
            Arc::new(Mutex::new(HashMap::new()));
        let mut link_stats = Vec::new();

        for index in [0usize, first_member] {
            add_link_inner(
                &nodes[index],
                &registry,
                &stop,
                &stats,
                &mut link_stats,
                root,
            )?;
        }

        Ok(Self {
            stop,
            stats,
            registry,
            links: link_stats,
            root,
        })
    }

    /// Add one more member link.
    ///
    /// The caller adds members one at a time and waits for the root's epoch to
    /// advance before adding the next.  That is not a policy rule: it keeps the
    /// host test's frame burst small while preserving the production join
    /// handshake, signed epoch and single-writer authority.
    pub fn add_link(&mut self, node: &Node) -> Result<(), String> {
        add_link_inner(
            node,
            &self.registry,
            &self.stop,
            &self.stats,
            &mut self.links,
            self.root,
        )
    }

    /// A compact diagnostic for a failed convergence.
    pub fn summary(&self) -> String {
        let stats = self.stats();
        format!(
            "hub frames={} forwarded={} self_drops={} unknown={} malformed={} backpressure={} sources={}",
            stats.frames,
            stats.forwarded,
            stats.self_drops,
            stats.unknown,
            stats.malformed,
            stats.backpressure,
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
            .map(|(name, counters)| {
                (
                    name.clone(),
                    counters.lock().map(|counters| counters.clone()).unwrap_or_default(),
                )
            })
            .collect();
        stats.forwarded = stats
            .per_source
            .iter()
            .map(|(_, counters)| counters.forwarded)
            .sum();
        stats
    }
}

impl Drop for MeshHub {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

fn add_link_inner(
    node: &Node,
    registry: &Arc<Mutex<HashMap<[u8; 16], Arc<Link>>>>,
    stop: &Arc<AtomicBool>,
    stats: &Arc<Mutex<HubStats>>,
    link_stats: &mut Vec<(String, Arc<Mutex<LinkStats>>)>,
    root: [u8; 16],
) -> Result<(), String> {
    let (read, link) = connect(node)?;
    link_stats.push((link.node.name.clone(), link.stats.clone()));
    let id = link.node.node_id;
    registry
        .lock()
        .map_err(|_| "hub registry mutex poisoned".to_string())?
        .insert(id, link.clone());
    spawn_reader(id, read, link, registry, stop, stats, root)
}

fn connect(node: &Node) -> Result<(TcpStream, Arc<Link>), String> {
    let address: SocketAddr = (Ipv4Addr::LOCALHOST, node.mesh_port).into();
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        match TcpStream::connect_timeout(&address, Duration::from_secs(2)) {
            Ok(stream) => {
                let _ = stream.set_nodelay(true);
                let mut writer = stream
                    .try_clone()
                    .map_err(|error| format!("clone {} stream: {error}", node.name))?;
                let stats = Arc::new(Mutex::new(LinkStats::default()));
                let (tx, rx) = sync_channel::<Vec<u8>>(LINK_QUEUE);
                let thread_stats = stats.clone();
                let name = node.name.clone();
                thread::Builder::new()
                    .name(format!("m3-hub-writer-{name}"))
                    .spawn(move || {
                        while let Ok(frame) = rx.recv() {
                            // The guest's emulated TCP receive path is happiest
                            // with small writes.  This is still a byte pump:
                            // the framing is unchanged, and the guest decoder
                            // reassembles the record.
                            let mut failed = false;
                            for chunk in frame.chunks(128) {
                                if writer.write_all(chunk).is_err() {
                                    failed = true;
                                    break;
                                }
                                thread::sleep(Duration::from_millis(2));
                            }
                            if failed {
                                break;
                            }
                            if let Ok(mut local) = thread_stats.lock() {
                                local.forwarded = local.forwarded.saturating_add(1);
                            }
                        }
                    })
                    .map_err(|error| format!("spawn hub writer for {name}: {error}"))?;
                return Ok((
                    stream,
                    Arc::new(Link {
                        node: node.clone(),
                        tx,
                        stats,
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
                thread::sleep(Duration::from_millis(200));
            }
        }
    }
}

fn spawn_reader(
    id: [u8; 16],
    mut read: TcpStream,
    link: Arc<Link>,
    registry: &Arc<Mutex<HashMap<[u8; 16], Arc<Link>>>>,
    stop: &Arc<AtomicBool>,
    stats: &Arc<Mutex<HubStats>>,
    root: [u8; 16],
) -> Result<(), String> {
    let registry = registry.clone();
    let stop = stop.clone();
    let stats = stats.clone();
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
            )
        })
        .map_err(|error| format!("spawn hub reader for {name}: {error}"))?;
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
) {
    let mut prefix = [0u8; 4];
    let mut record = vec![0u8; MAX_RECORD];
    while !stop.load(Ordering::Relaxed) {
        if read_exact(stream, &mut prefix).is_err() {
            break;
        }
        let length = u32::from_le_bytes(prefix) as usize;
        if !(MIN_RECORD..=MAX_RECORD).contains(&length) {
            count(stats, source_stats, |total, local| {
                total.malformed += 1;
                local.received += 1;
            });
            break;
        }
        if read_exact(stream, &mut record[..length]).is_err() {
            break;
        }
        count(stats, source_stats, |total, local| {
            total.frames += 1;
            total.bytes += length as u64;
            local.received += 1;
            local.bytes += length as u64;
        });
        let record = &record[..length];
        if record.len() < HEADER_LEN || &record[0..4] != b"I7MH" || record[4] != 1 {
            count(stats, source_stats, |total, _| total.malformed += 1);
            continue;
        }
        let mut src = [0u8; 16];
        src.copy_from_slice(&record[8..24]);
        let mut dst = [0u8; 16];
        dst.copy_from_slice(&record[24..40]);
        if src != source_id {
            count(stats, source_stats, |total, _| total.malformed += 1);
            continue;
        }
        if std::env::var_os("IRIX7_HUB_TRACE").is_some() {
            eprintln!("hub: {} -> {} len={length}", hex16(&src), hex16(&dst));
        }
        let target = if dst == [0u8; 16] { root } else { dst };
        if target == source_id {
            count(stats, source_stats, |total, _| total.self_drops += 1);
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
            count(stats, source_stats, |total, _| total.unknown += 1);
            continue;
        };
        let mut frame = Vec::with_capacity(4 + length);
        frame.extend_from_slice(&(length as u32).to_le_bytes());
        frame.extend_from_slice(record);
        match link.tx.try_send(frame) {
            Ok(()) => {}
            Err(_) => {
                if std::env::var_os("IRIX7_HUB_TRACE").is_some() {
                    eprintln!("hub: backpressure for {}", hex16(&target));
                }
                count(stats, source_stats, |total, _| total.backpressure += 1);
            }
        }
    }
    if std::env::var_os("IRIX7_HUB_TRACE").is_some() {
        eprintln!("hub: reader {} ({source_name}) exited", hex16(&source_id));
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

fn read_exact(stream: &mut TcpStream, buffer: &mut [u8]) -> std::io::Result<()> {
    let mut offset = 0;
    while offset < buffer.len() {
        match stream.read(&mut buffer[offset..]) {
            Ok(0) => {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::UnexpectedEof,
                    "hub link closed",
                ))
            }
            Ok(count) => offset += count,
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
            Err(error) => return Err(error),
        }
    }
    Ok(())
}
