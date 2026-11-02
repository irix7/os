// SPDX-License-Identifier: GPL-3.0-only
//! Interim M2 mesh listener and join handshake inside the MCP module.
//!
//! The M2 transport uses one module TCP slot for the HTTP control plane and the
//! other for a mesh listener on guest port 7000.  The guest has no TCP
//! connect primitive, so both nodes listen and the host rig's mesh relay
//! connects to both port forwards and pumps bytes between them.  This is an
//! explicit M2-1 deviation from the lower-`NodeId`-initiates rule in
//! M2-ADR-0002; it needs no kernel change and keeps the compressed PROM frozen.
//!
//! The listener is enabled only when NVRAM contains `mesh.peer`.  A normal
//! boot without that key keeps the original MCP two-slot behaviour.
//!
//! The wire is the M2-1 contract: a 32-bit little-endian envelope length, then
//! a `MeshEnvelope`.  `mesh-transport` owns the framing, bounds, deadlines,
//! send queue and request tags.  This file owns the socket state machine and
//! two exchanges:
//!
//! ```text
//! M2-1 connect: Request/Ping (dst = zero, src = self)
//!               Response/Ping (dst = requester, reply_to = request id)
//! M2-1 idle:    Control/Ping every 5 s
//!
//! M2-2 member:  Request/Hello    JOIN_REQUEST
//! M2-2 root:    Response/Hello   EPOCH_CONFIRM or EPOCH_REFUSED
//! M2-2 member:  Completion/Hello JOIN_ACK
//!
//! M2-3:         GET /mesh/homes   confirmed rendezvous table, bounded
//!
//! M2-4 invoke:  Request/Invoke     Capability + stat body
//! M2-4 home:    Response/Invoke    RESULT or bounded MOVED
//! M2-4 move:    Request/RouteQuery root-signed one-object home lease move
//! M2-4 ack:     Response/RouteQuery MOVE_ACK
//! M2-4 rig:     GET /mesh/route    bounded report; ?step=N drives one step
//! ```
//!
//! A Ping frame body must be the 16-byte source `NodeId`; a mismatch is a
//! protocol reject.  A join body is decoded by `mesh-join`, which owns the
//! bounds, the authority signature and the membership table.  A semantic join
//! rejection is logged and counted but does not tear down the M2-1 link: the
//! ping/heartbeat path remains valid while the membership view stays unchanged.
//!
//! When `mesh.peer` is absent the mesh is disabled and nothing here runs.  When
//! `mesh.peer` is present but `mesh.authority` (or its key) is missing, the
//! transport still runs and only the join is disabled; that is the M2-1
//! fallback, not a fake membership.

use mesh_core::{
    fixture_objects, shard_objects, HomeTable, HOME_FIXTURE_DEFAULT, HOME_FIXTURE_MAX,
    SHARD_FIXTURE_DEFAULT, SHARD_FIXTURE_MAX,
};
use mesh_join::{
    gossip_targets, EpochConfirmation, EpochRecord, JoinEvent, JoinMessage, JoinNode,
    MembershipSnapshot, Role, Tick, FANOUT as GOSSIP_FANOUT, MAX_BODY as JOIN_BODY_MAX,
};
use mesh_migrate::{
    Event as MigrateEvent, MigrateNode, Signer as MigrateSigner, Step as MigrateStep,
    Wire as MigrateWire, MAX_BODY as MIGRATE_BODY_MAX,
};
use mesh_route::{RouteNode, RouteSigner, Step as RouteStep, Wire as RouteWire};
use mesh_stream::{Class, Side, StreamMux};
use mesh_transport::{
    decode_record, ByteSink, Error as TransportError, Heartbeat, PendingTable, SendQueue, Stats,
    StreamDecoder, WriteOutcome, HEARTBEAT_INTERVAL_MS, RECEIVE_DEADLINE_MS,
};
use module_abi::Services;
use sys_primitives::{
    flags as frame_flags, mesh_flags, FrameHeader, MeshEnvelope, MeshHeader, MessageFrame,
    MessageKind, NodeId, NodeKey, ObjectId, OpCode, Status, StreamId, Tag, TraceContext,
};

// The authority write lease must expire before the membership failure window,
// so an isolated old home stops accepting commits before the root can re-home
// the object.  One heartbeat (5 s) plus the write lease (6 s) is 11 s, inside
// the 12 s failure window.
const _: () = assert!(mesh_route::WRITE_LEASE_MS < mesh_join::FAIL_AFTER_MS);
const _: () = assert!(
    mesh_transport::HEARTBEAT_INTERVAL_MS + mesh_route::WRITE_LEASE_MS < mesh_join::FAIL_AFTER_MS
);

/// Module TCP slot owned by the mesh listener when `mesh.peer` is present.
pub const MESH_SLOT: u32 = 1;
/// Guest port for the M2 mesh listener.
pub const MESH_PORT: u16 = 7000;
/// Module TCP slot kept for the MCP HTTP server when the mesh is enabled.
pub const HTTP_SLOT: u32 = 0;

/// Module read size and reads per tick.  8 x 4 KiB drains 32 KiB in one pass,
/// enough for one 16 KiB bulk chunk plus framing and a following control
/// record.  Four reads left a chunk just over the per-tick budget and cost an
/// extra 10 ms tick per chunk; the buffer is static because the module runs on
/// the kernel stack and a 32 KiB stack frame is not acceptable.
const READ_CHUNK: usize = 4096;
const READS_PER_TICK: usize = 8;

/// Largest body a single received record may carry.
///
/// Join bodies are 4 KiB, bulk chunks are one 16 KiB payload plus a 20-byte
/// header.  A larger body is rejected before dispatch.
const FRAME_BODY_MAX: usize = BULK_HEADER_LEN + BULK_CHUNK_MAX;

/// Bulk measurement path.
///
/// This is the reproducible two-kernel workload for #247: the rig starts a
/// `Read` request on one node and the peer streams fixed-size chunks back on
/// one logical record.  It is test/benchmark traffic, not a general data plane
/// or a second file service.  The same path is reused by the multiplexed
/// stream tests in #246.
const BULK_MAGIC: u32 = u32::from_le_bytes(*b"I7B1");
const BULK_HEADER_LEN: usize = 20;
const BULK_CHUNK_MAX: usize = 16 * 1024;
const BULK_CHUNK_DEFAULT: usize = 4 * 1024;
const BULK_RECORD_MAX: usize = 1024 * 1024;
const BULK_DEADLINE_MS: u64 = 20_000;
/// Initial chunk credit and the grant after two consumed chunks.
const BULK_INITIAL_CREDIT: u32 = 2;
const BULK_CREDIT_GRANT: u32 = 2;

/// Live MCP pending-tag capacity.
///
/// M2 keeps at most one route operation plus a few link-control frames in
/// flight per connection, so eight tags cover the live module. This is not
/// the fabric conformance capacity: `crates/fabric` proves the tag model at
/// 10,000 outstanding tags, and that result must not be read as a claim that
/// the live MCP module holds 10,000 tags.
const PENDING_CAPACITY: u32 = 8;
/// Live bound on distinct member links.
///
/// The wire membership allows 128 members, but the module holds one send
/// queue, pending table and heartbeat per source.  Eight is the largest real
/// kernel count the M3 rig drives; a ninth source is refused rather than
/// growing the module's fixed memory on the data path.
const MAX_MESH_LINKS: usize = 8;
/// Send-queue size for a non-authority link.
///
/// Join, route, migration and one aggregate are all far below 4 KiB; bulk and
/// latency stay on the authority link's 32 KiB queue.
const LINK_QUEUE_BYTES: usize = 4 * 1024;
/// Reply-tag capacity for a gossip link.
const LINK_PENDING_CAPACITY: u32 = 2;
/// Interval between anti-entropy re-gossip of the current aggregate.
///
/// The root re-sends the same signed object to a rotating bounded window, so
/// a lost push is repaired without another signature or an O(N) broadcast.
const GOSSIP_RESYNC_MS: u64 = 60_000;
/// A gossip record's header hop budget.
///
/// The live hard bound is "each node forwards one aggregate per epoch"; this
/// field records the intended bounded depth for diagnostics and forwarding
/// policy in a future hop-counting path.
const GOSSIP_TTL: u8 = 3;
const LINE_CAP: usize = 768;

/// Fixed control channels on every link.
///
/// The bulk data channel is allocated per transfer, so a transfer cannot
/// collide with a control ID and both sides keep the initiator/responder
/// parity rule.
const STREAM_CONTROL: u32 = 1;
const STREAM_JOIN: u32 = 2;
const STREAM_ROUTE: u32 = 3;
const STREAM_MIGRATE: u32 = 4;
const STREAM_LATENCY: u32 = 6;

/// Frames the module may stage for one tick before the class scheduler drains
/// them.  The stage is shared by every link.  One link produces at most one
/// frame per stream plus a retry in a tick; when several links fill the stage,
/// `stage_room` drains it into the link queues early.
const STAGE_FRAMES: usize = 8;
/// Body storage for the small control records that share a tick: route and
/// migration bodies are at most 512 bytes each.  Join bodies already live in
/// `JOIN_TX`, and a bulk chunk reuses `FRAME_RX` after the receive pass.
const STAGE_SMALL_MAX: usize = 1536;
/// Routine join-view and ack logging interval.
///
/// The 5 s member refresh is a protocol liveness check, not evidence: logging
/// every confirmation plus the full two-node home table (~1.5 KiB) saturates
/// the emulated serial line and starves the receive path.  Epoch changes,
/// first convergence, rejoin/duplicate and failures are still logged.
const JOIN_LOG_INTERVAL_MS: u64 = 30_000;

/// SCSI target reserved for the persistent epoch store.
///
/// Target 2 is the MCP module's EROFS fixture (`FS_TARGET` in `main.rs`), so
/// the store must not use it.  The M2 rig attaches a dedicated scratch volume
/// at target 3; target 1 is the boot disk in the lab and is never written by
/// this module.  A production image must provision the same reservation before
/// persistence is claimed.
const STORE_TARGET: u8 = 3;
/// First LBA of the store.  LBA 0 is the volume header, so the record starts
/// clear of anything the emulator or a volume tool writes there.
const STORE_LBA: u32 = 8;
/// Five sectors hold a 2,144-byte maximum epoch record plus the store header.
const STORE_SECTORS: usize = 5;
const STORE_BYTES: usize = STORE_SECTORS * 512;
const STORE_HEADER: usize = 16;
const STORE_MAGIC: &[u8; 8] = b"I7EPST01";
const STORE_VERSION: u8 = 1;
const STORE_KIND_RECORD: u8 = 1;
const STORE_KIND_CONFIRM: u8 = 2;

/// The last confirmed view, as recovered from or written to the epoch store.
struct EpochStore {
    /// Whether the scratch target answered.  A node without one logs the
    /// absence and falls back to its seed; it does not claim persistence.
    available: bool,
    /// Last epoch known to be on disk.
    epoch: u32,
    /// Digest of the last record on disk.
    digest: [u8; 16],
    /// Role that owns the record: a root writes signed records, a member
    /// writes the signed confirmation it installed.
    root: bool,
}

impl EpochStore {
    fn empty(root: bool, available: bool) -> Self {
        Self {
            available,
            epoch: 0,
            digest: [0u8; 16],
            root,
        }
    }

    fn matches(&self, epoch: u32, digest: &[u8; 16]) -> bool {
        self.epoch == epoch && self.digest == *digest
    }

    fn remember(&mut self, epoch: u32, digest: [u8; 16]) {
        self.epoch = epoch;
        self.digest = digest;
    }
}

enum Phase {
    Arm,
    Waiting,
    Connect,
    Connected,
}

struct Incoming {
    header: MeshHeader,
    trace: Option<TraceContext>,
    object: ObjectId,
    status: Status,
    kind: MessageKind,
    opcode: OpCode,
    stream: u32,
    body_len: usize,
}

/// Where a staged frame's body bytes live.
#[derive(Clone, Copy, PartialEq, Eq)]
enum BodyStore {
    /// The small control staging area.
    Small,
    /// `JOIN_TX`, written by `send_join` for this frame.
    Join,
    /// `FRAME_RX`, reused for one bulk body after the receive pass.
    FrameRx,
}

/// One encoded frame waiting for its class turn in the scheduler.
#[derive(Clone, Copy)]
struct StagedFrame {
    /// Link the frame leaves on: zero for the authority link, else the peer.
    link: NodeId,
    stream: u32,
    class: Class,
    header: MeshHeader,
    frame: FrameHeader,
    store: BodyStore,
    body_start: usize,
    body_len: usize,
}

/// Per-source membership and send state.
///
/// One TCP session carries every source.  The decoder is a property of that
/// session (a record's length prefix has no source field), but everything from
/// the envelope source onwards is per link: the send queue, reply tags,
/// heartbeat, last-seen time, summary counter, last aggregate epoch sent and
/// the stream mux, whose send ledger follows this link's FIFO queue.
/// That is what a hub-multiplexed root needs to give each member its own
/// bounded state instead of one shared queue and one global join view.
struct MeshLink {
    /// The source `NodeId`; zero only for the root's unbound listener link.
    peer: NodeId,
    /// A frame from this source has been accepted.
    confirmed: bool,
    queue: SendQueue,
    /// Version 1 stream mux: table, class scheduler and send ledger.
    streams: StreamMux,
    pending: PendingTable,
    heartbeat: Heartbeat,
    last_rx_ms: u64,
    /// Highest member-summary counter accepted from this peer.
    summary_counter: u64,
    /// Highest aggregate epoch queued to this peer.
    sent_epoch: u32,
    /// Bytes and whole frames written from this link's queue.
    bytes_sent: u64,
    frames_sent: u64,
}

impl MeshLink {
    fn new(peer: NodeId, primary: bool, heartbeat: Heartbeat, side: Side, now: u64) -> Self {
        let queue = if primary {
            SendQueue::new()
        } else {
            SendQueue::with_capacity(LINK_QUEUE_BYTES)
        };
        let pending = PendingTable::new(if primary {
            PENDING_CAPACITY
        } else {
            LINK_PENDING_CAPACITY
        });
        let mut heartbeat = heartbeat;
        heartbeat.reset(now);
        Self {
            peer,
            confirmed: false,
            queue,
            streams: live_streams(side),
            pending,
            heartbeat,
            last_rx_ms: now,
            summary_counter: 0,
            sent_epoch: 0,
            bytes_sent: 0,
            frames_sent: 0,
        }
    }
}

struct Mesh {
    phase: Phase,
    self_id: NodeId,
    /// The authority peer: the root on a member, the first member on a root.
    /// Route, migration and latency use this peer.
    peer: Option<NodeId>,
    /// Bounded per-source link table.  The M3 host hub multiplexes several
    /// member sessions onto one module TCP slot, so a root sees one source per
    /// member; each gets its own queue, tags and heartbeat.  The table lives on
    /// the module heap, not in the static image, so eight links do not consume
    /// the 512 KiB slot.
    links: alloc::vec::Vec<MeshLink>,
    /// Index of the authority link in `links`.
    primary_link: Option<usize>,
    peer_confirmed: bool,
    /// Record framer for the one TCP session.
    ///
    /// The length prefix is per session, not per source, so the decoder stays
    /// single.  It is reset only when the session is replaced; per-source
    /// processing begins after `decode_record` exposes `MeshHeader.src`.
    decoder: StreamDecoder,
    stats: Stats,
    /// Greedy-gossip round counter; rotates the bounded target window.
    gossip_round: u64,
    /// Highest aggregate epoch this node has forwarded or published.
    gossip_floor: u32,
    /// Last anti-entropy re-gossip time (root only).
    last_gossip_ms: u64,
    /// Member summaries emitted and accepted.
    gossip_summaries_sent: u64,
    gossip_summaries_received: u64,
    /// Aggregates queued and forwarded.
    gossip_aggregates_sent: u64,
    gossip_aggregates_forwarded: u64,
    /// Heartbeat policy copied into each link when the session is established.
    heartbeat: Heartbeat,
    join: Option<JoinNode>,
    /// Join requests this boot has queued.  A restarted member starts at zero,
    /// so a non-zero value proves it asked to rejoin.
    join_requests: u64,
    /// Signed confirmed-epoch record on the scratch volume, when present.
    store: Option<EpochStore>,
    route: Option<RouteNode>,
    migrate: Option<MigrateNode>,
    /// Whether the M2-6 failure detector is enabled (`mesh.fail=1`).
    failures: bool,
    home_count: u32,
    shard_count: u32,
    join_timeout_logged: bool,
    /// Last routine join-view log; rate-limits the 5 s refresh transcript.
    last_join_log_ms: u64,
    /// Last routine ack log; rate-limits the 5 s refresh transcript.
    last_ack_log_ms: u64,
    last_arm_log_ms: u64,
    pass_logged: bool,
    listen_logged: bool,
    last_rx_ms: u64,
    /// Outbound bulk measurement transfer, if one is active.
    bulk: Option<BulkTransfer>,
    /// Inbound bulk request being served, if one is active.
    bulk_reply: Option<BulkReply>,
    /// Epoch whose routine join confirmation was last logged.
    logged_join_epoch: u32,
    /// Epoch whose routine ack was last logged.
    logged_ack_epoch: u32,
    /// Incremented for every accepted link.
    link_generation: u64,
    /// Link generation whose confirmed home table was last logged.
    homes_logged_generation: u64,
    /// Optional direct peer target (`mesh.direct`).
    direct: Option<DirectPeer>,
    /// Whether this node is the lower NodeId and therefore initiates.
    direct_initiator: bool,
    /// Whether a failed direct connect has fallen back to the listener.
    direct_fallback: bool,
    /// Next direct connect attempt.
    connect_at_ms: u64,
    /// Direct connect attempts in this session.
    connect_attempts: u32,
    /// First direct connect attempt.
    connect_started_ms: u64,
    /// Last successful connect time, for the pass line.
    last_connect_ms: u64,
    /// Active latency measurement.
    latency: Option<LatencyProbe>,
    /// Frames encoded this tick for every link, in production order.
    staged: [Option<StagedFrame>; STAGE_FRAMES],
    /// Body bytes used in this tick.
    stage_len: usize,
    /// Set when a control frame could not be reserved or queued.  The link
    /// fails closed at the next flush instead of silently dropping it.
    stage_fault: Option<&'static str>,
    /// Data stream whose final record is queued; retired after the sink
    /// accepts it.
    retire_stream: Option<u32>,
}

/// Samples one probe can hold.  The array is in the module's static and slot A
/// is close to its 512 KiB limit, so this stays at the shipped 64: the gate
/// repeats the series and takes the median rather than growing the image.
const LATENCY_SAMPLES: usize = 64;

/// Spacing between `/mesh/latency` samples when the query names no `gap`.
///
/// 20 ms is the hot-link probe: it keeps the node inside the kernel's 50 ms
/// activity window.  A gap above that window (the rig uses 250 ms) lets the
/// node park and is the cold-path measurement: the round trip then includes
/// the interrupt wake.
const DEFAULT_LATENCY_GAP_MS: u32 = 20;
/// Longest accepted inter-sample gap.  Keeps a 64-sample probe under a minute
/// so the MCP HTTP slot is not held by a runaway query.
const MAX_LATENCY_GAP_MS: u32 = 1_000;

struct LatencyProbe {
    count: u32,
    sent: u32,
    received: u32,
    outstanding: bool,
    pending_send_ms: u32,
    next_ms: u64,
    /// Spacing between samples.  20 ms keeps the node in the hot window; a gap
    /// beyond the 50 ms activity window measures the cold path.
    gap_ms: u32,
    samples: [u64; LATENCY_SAMPLES],
    peak_ms: u64,
}

#[derive(Clone, Copy)]
struct DirectPeer {
    address: [u8; 4],
    port: u16,
}

/// State of the origin side of the #247 bulk measurement.
struct BulkTransfer {
    nonce: u32,
    total: u32,
    chunk: u32,
    state: BulkState,
    started_ms: u64,
    request_queued_ms: u64,
    first_chunk_ms: u64,
    finished_ms: u64,
    received: u32,
    chunks: u32,
    expected_offset: u32,
    error: Option<&'static str>,
    /// Stream allocated for this transfer; reset when it retires.
    stream: u32,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum BulkState {
    /// The request has not been queued yet.
    Start,
    /// The request is queued or waiting for the first chunk.
    Running,
    Done,
    Failed,
}

/// State of the peer side of the #247 bulk measurement.
struct BulkReply {
    node: NodeId,
    nonce: u32,
    total: u32,
    chunk: u32,
    offset: u32,
    started_ms: u64,
    /// Chunks the origin has granted; zero means wait for WindowUpdate.
    credit: u32,
    /// The final chunk is queued; clear the reply once the sink accepts it.
    finishing: bool,
    /// Stream allocated by the origin for this reply.
    stream: u32,
}

/// Receive/transmit buffer for one frame body.
///
/// The control plane is single-threaded and never holds an inbound body across
/// a tick, so the frame receive copy and the outgoing bulk chunk share one
/// 16 KiB static.  Sharing is safe because `queue_bulk_chunk` runs from
/// `bulk_tick` after `feed` has returned.
static mut FRAME_RX: [u8; FRAME_BODY_MAX] = [0; FRAME_BODY_MAX];
/// Encode buffer for one outgoing join body.
static mut JOIN_TX: [u8; JOIN_BODY_MAX] = [0; JOIN_BODY_MAX];
/// Read/write buffer for the persistent epoch store.
///
/// The module is single-threaded and persistence runs outside the frame
/// receive path, so one buffer is enough and it keeps 2.5 KiB off the stack.
static mut STORE_BUF: [u8; STORE_BYTES] = [0; STORE_BYTES];
/// Encode buffer for the store payload (a signed record or confirmation).
static mut STORE_BODY: [u8; STORE_BYTES - STORE_HEADER] = [0; STORE_BYTES - STORE_HEADER];
/// Body storage for the small control records staged in one tick.
static mut STAGE_SMALL: [u8; STAGE_SMALL_MAX] = [0; STAGE_SMALL_MAX];
/// One receive scratch buffer reused by every socket read in a tick.
static mut READ_BUF: [u8; READ_CHUNK] = [0; READ_CHUNK];

static mut MESH: Option<Mesh> = None;
static mut ENABLED: bool = false;

/// The authority link's stream mux, borrowed from the link table alone so a
/// caller can hold another `Mesh` field at the same time.
fn primary_mux(links: &mut [MeshLink], primary: Option<usize>) -> Option<&mut StreamMux> {
    links.get_mut(primary?).map(|link| &mut link.streams)
}

/// Build the fixed stream set for one link side.
fn live_streams(side: Side) -> StreamMux {
    let mut streams = StreamMux::new(side);
    let _ = streams.add_channel(STREAM_CONTROL, true);
    let _ = streams.add_channel(STREAM_JOIN, true);
    let _ = streams.add_channel(STREAM_ROUTE, true);
    let _ = streams.add_channel(STREAM_MIGRATE, true);
    let _ = streams.add_channel(STREAM_LATENCY, true);
    streams
}

/// Initialise the listener and the join state from the service table.
///
/// Returns false when `mesh.peer` is absent or the node identity cannot be
/// read.  In that case the MCP module keeps both TCP slots and behaves exactly
/// as it did before M2-1.
///
/// # Safety
/// `services` must be the table supplied by the kernel to `_start`.
pub unsafe fn init(services: *const Services) -> bool {
    let Some(services) = (unsafe { services.as_ref() }) else {
        return false;
    };
    let mut peer = [0u8; 96];
    let relay_configured = config_any(services, &[b"mesh.peer", b"peer"], &mut peer).is_some();
    let mut direct_value = [0u8; 96];
    let direct = config_any(services, &[b"mesh.direct", b"d"], &mut direct_value)
        .and_then(|len| parse_direct(&direct_value[..len]));
    if !relay_configured && direct.is_none() {
        return false;
    }
    let Some(self_id) = identity(services) else {
        log(services, b"mesh-transport: identity unavailable\n");
        return false;
    };
    let mut peer_id_value = [0u8; 40];
    let direct_peer = config_any(services, &[b"mesh.peer_id", b"p"], &mut peer_id_value)
        .and_then(|len| parse_node_hex(&peer_id_value[..len]));
    let direct_initiator = direct.is_some()
        && direct_peer.is_some_and(|peer_id| self_id.as_bytes() < peer_id.as_bytes());
    let now = (services.clock_ms)();
    let home_count = home_object_count(services);
    let shard_count = shard_object_count(services);
    let (join, store) = match join_config(services, self_id, now) {
        Ok(Some((join, store))) => (Some(join), Some(store)),
        Ok(None) => (None, None),
        Err(label) => {
            let mut line = Line::new();
            line.str("mesh-join: disabled reason=");
            line.str(label);
            line.push(b'\n');
            log(services, line.as_bytes());
            (None, None)
        }
    };
    if join.is_some() {
        let mut line = Line::new();
        line.str("mesh-homes: configured set=m2-v1 count=");
        line.u64(home_count as u64);
        line.push(b'\n');
        log(services, line.as_bytes());
        if shard_count > 0 {
            let mut line = Line::new();
            line.str("mesh-shard: configured set=m3-v1 count=");
            line.u64(shard_count as u64);
            line.push(b'\n');
            log(services, line.as_bytes());
        }
    }
    let route = route_config(services, self_id, join.as_ref(), home_count);
    let migrate = migrate_config(services, self_id, join.as_ref());
    let failures = failure_config(services);
    // With failure detection enabled the transport timeout is a backstop, not
    // the failure path: a partition pauses the seam for longer than the M2-1
    // 30 s heartbeat timeout, and the membership detector (12 s) is what
    // declares the peer failed.  Keeping the session alive lets the same
    // connection carry the rejoin when the seam returns.
    let heartbeat = if failures {
        Heartbeat::new(HEARTBEAT_INTERVAL_MS, 300_000)
    } else if shard_count > 0 {
        // An M3 shard run can pause on the iris forward's guest-facing window
        // while the host TCP connection is healthy.  The M2 30 s liveness
        // timeout would tear down the authority link before the forward can
        // recover; the shard run does not enable the M2 failure detector, so a
        // longer bounded backstop is correct here.
        Heartbeat::new(HEARTBEAT_INTERVAL_MS, 120_000)
    } else {
        Heartbeat::m2()
    };
    let phase = if direct_initiator {
        Phase::Connect
    } else {
        Phase::Arm
    };
    let peer = match join.as_ref().map(|join| (join.role(), join.root())) {
        Some((Role::Member, root)) => Some(root),
        _ => None,
    };
    let mesh = Mesh {
        phase,
        self_id,
        peer,
        links: alloc::vec::Vec::new(),
        primary_link: None,
        peer_confirmed: false,
        decoder: StreamDecoder::new(),
        stats: Stats::default(),
        gossip_round: 0,
        gossip_floor: 0,
        last_gossip_ms: 0,
        gossip_summaries_sent: 0,
        gossip_summaries_received: 0,
        gossip_aggregates_sent: 0,
        gossip_aggregates_forwarded: 0,
        heartbeat,
        join,
        join_requests: 0,
        store,
        route,
        migrate,
        failures,
        home_count,
        shard_count,
        join_timeout_logged: false,
        last_join_log_ms: 0,
        last_ack_log_ms: 0,
        last_arm_log_ms: 0,
        pass_logged: false,
        listen_logged: false,
        last_rx_ms: now,
        bulk: None,
        bulk_reply: None,
        logged_join_epoch: 0,
        logged_ack_epoch: 0,
        link_generation: 0,
        homes_logged_generation: 0,
        direct,
        direct_initiator,
        direct_fallback: false,
        connect_at_ms: if direct_initiator {
            now.saturating_add(3_000)
        } else {
            now
        },
        connect_attempts: 0,
        connect_started_ms: now,
        last_connect_ms: 0,
        latency: None,
        staged: [None; STAGE_FRAMES],
        stage_len: 0,
        stage_fault: None,
        retire_stream: None,
    };
    unsafe {
        MESH = Some(mesh);
        ENABLED = true;
    }
    log(services, b"mesh-transport: enabled\n");
    if let Some(target) = direct {
        let mut line = Line::new();
        line.str("mesh-transport: direct role=");
        line.str(if direct_initiator {
            "initiator"
        } else {
            "listener"
        });
        line.str(" peer=");
        match direct_peer {
            Some(node) => line.node(&node),
            None => line.push(b'-'),
        }
        line.str(" target=");
        line.u64(target.address[0] as u64);
        line.push(b'.');
        line.u64(target.address[1] as u64);
        line.push(b'.');
        line.u64(target.address[2] as u64);
        line.push(b'.');
        line.u64(target.address[3] as u64);
        line.push(b':');
        line.u64(target.port as u64);
        line.push(b'\n');
        log(services, line.as_bytes());
    }
    true
}

/// Reset the mesh link and re-arm the listener.
///
/// This is the M2 restart harness control.  It is deliberately link-only: the
/// confirmed membership, epoch, route and migration state stay in memory, so
/// the node can rejoin without a process restart.  The old guest-side socket is
/// aborted and the listener re-arms on the next tick; that is what closes the
/// half-open connection the iris host forward can retain while a peer is down.
pub fn admin_reconnect(services: &Services) -> bool {
    // SAFETY: single-threaded module; the pointer is written once at entry.
    let slot = unsafe { &mut *core::ptr::addr_of_mut!(MESH) };
    let Some(mesh) = slot.as_mut() else {
        return false;
    };
    let now = (services.clock_ms)();
    mesh.disconnect(services, now, "admin-reconnect");
    true
}

/// Drive the listener from the module tick.
pub fn tick(services: &Services) {
    // SAFETY: the module tick is single-threaded.  The pointer is valid for
    // the module's lifetime and no other reference to MESH exists here.
    let Some(mesh) = (unsafe { &mut *core::ptr::addr_of_mut!(MESH) }).as_mut() else {
        return;
    };
    mesh.tick(services);
}

/// Render or start `GET /mesh/latency`, a bounded mesh round-trip probe.
///
/// `?start=1&count=N&gap=MS` sends `N` sequential `Request/Latency` records to
/// the current peer and records the mesh round trip.  `gap` defaults to 20 ms
/// (hot link); a gap above the kernel's 50 ms activity window measures the
/// cold path.  `?status=1` returns p50/p95/p99 from the recorded samples.  The
/// MCP HTTP phase machine is outside the measured interval.
#[inline(never)]
pub fn latency_response(services: &Services, path: &[u8], out: &mut [u8]) -> (u16, usize) {
    let mut start = false;
    let mut count = 32u32;
    let mut gap_ms = DEFAULT_LATENCY_GAP_MS;
    if let Some(question) = path.iter().position(|byte| *byte == b'?') {
        for part in path[question + 1..].split(|byte| *byte == b'&') {
            if part.is_empty() {
                continue;
            }
            let mut pieces = part.splitn(2, |byte| *byte == b'=');
            let key = pieces.next().unwrap_or(b"");
            let value = pieces.next().unwrap_or(b"");
            match key {
                b"start" => start = value == b"1",
                b"status" => {}
                b"count" => match parse_u32(value) {
                    Some(parsed) => count = parsed,
                    None => return bulk_line(out, 400, "bad-request reason=count"),
                },
                b"gap" => match parse_u32(value) {
                    Some(parsed) if parsed <= MAX_LATENCY_GAP_MS => gap_ms = parsed,
                    _ => return bulk_line(out, 400, "bad-request reason=gap"),
                },
                _ => return bulk_line(out, 400, "bad-request reason=query"),
            }
        }
    }
    let Some(mesh) = (unsafe { (&mut *core::ptr::addr_of_mut!(MESH)).as_mut() }) else {
        return bulk_line(out, 404, "disabled reason=peer");
    };
    let now = (services.clock_ms)();
    if start {
        if !mesh.start_latency(count, gap_ms, now) {
            return bulk_line(out, 409, "not-connected");
        }
        return bulk_line(out, 202, "started");
    }
    let mut body = Body::new(out);
    body.str("mesh-latency:");
    match mesh.latency.as_ref() {
        None => body.str(" state=idle"),
        Some(probe) => {
            body.str(" state=");
            if probe.received >= probe.count {
                body.str("done");
            } else {
                body.str("running");
            }
            body.str(" sent=");
            body.u64(probe.sent as u64);
            body.str(" received=");
            body.u64(probe.received as u64);
            body.str(" count=");
            body.u64(probe.count as u64);
            body.str(" gap=");
            body.u64(probe.gap_ms as u64);
            let mut samples = probe.samples;
            let used = (probe.received as usize).min(samples.len());
            samples[..used].sort_unstable();
            if used > 0 {
                body.str(" p50=");
                body.u64(samples[used / 2]);
                body.str(" p95=");
                body.u64(samples[(used * 95 / 100).min(used - 1)]);
                body.str(" p99=");
                body.u64(samples[(used * 99 / 100).min(used - 1)]);
                body.str(" min=");
                body.u64(samples[0]);
                body.str(" max=");
                body.u64(samples[used - 1]);
            }
        }
    }
    body.push(b'\n');
    let state = match mesh
        .latency
        .as_ref()
        .map(|probe| probe.received >= probe.count)
    {
        Some(true) => 200,
        Some(false) => 409,
        None => 200,
    };
    (state, body.len())
}

/// Render or start `GET /mesh/bulk`, the reproducible bulk measurement path.
///
/// `?start=1&bytes=N&chunk=M` queues one `Read` request to the current peer.
/// `?status=1` (or no query) returns the bounded state.  The peer streams
/// `M`-byte chunks back; the origin records the mesh start/finish timestamps,
/// so the reported elapsed time does not include the MCP HTTP phase machine.
#[inline(never)]
pub fn bulk_response(services: &Services, path: &[u8], out: &mut [u8]) -> (u16, usize) {
    let mut start = false;
    let mut cancel = false;
    let mut bytes = BULK_CHUNK_DEFAULT as u32;
    let mut chunk = BULK_CHUNK_DEFAULT as u32;
    let mut latency = 0u32;
    if let Some(question) = path.iter().position(|byte| *byte == b'?') {
        for part in path[question + 1..].split(|byte| *byte == b'&') {
            if part.is_empty() {
                continue;
            }
            let mut pieces = part.splitn(2, |byte| *byte == b'=');
            let key = pieces.next().unwrap_or(b"");
            let value = pieces.next().unwrap_or(b"");
            match key {
                b"start" => start = value == b"1",
                b"cancel" => cancel = value == b"1",
                b"status" => {}
                b"bytes" => match parse_u32(value) {
                    Some(parsed) => bytes = parsed,
                    None => return bulk_line(out, 400, "bad-request reason=bytes"),
                },
                b"chunk" => match parse_u32(value) {
                    Some(parsed) => chunk = parsed,
                    None => return bulk_line(out, 400, "bad-request reason=chunk"),
                },
                b"latency" => match parse_u32(value) {
                    Some(parsed) => latency = parsed.clamp(1, 64),
                    None => return bulk_line(out, 400, "bad-request reason=latency"),
                },
                _ => return bulk_line(out, 400, "bad-request reason=query"),
            }
        }
    }
    // SAFETY: the module is single-threaded and `MESH` is written once at
    // entry; no mutable reference to it is live here.
    let Some(mesh) = (unsafe { (&mut *core::ptr::addr_of_mut!(MESH)).as_mut() }) else {
        return bulk_line(out, 404, "disabled reason=peer");
    };
    if cancel {
        let now = (services.clock_ms)();
        let destination = mesh.peer.unwrap_or(NodeId::ZERO);
        let active = mesh.bulk.as_ref().is_some_and(|transfer| {
            matches!(transfer.state, BulkState::Start | BulkState::Running)
        }) || mesh.bulk_reply.is_some();
        if !active {
            return bulk_line(out, 409, "idle");
        }
        let stream = mesh
            .bulk
            .as_ref()
            .map(|transfer| transfer.stream)
            .or_else(|| mesh.bulk_reply.as_ref().map(|reply| reply.stream));
        let Some(stream) = stream else {
            return bulk_line(out, 409, "idle");
        };
        if let Some(transfer) = mesh.bulk.as_mut() {
            if transfer.stream == stream {
                transfer.state = BulkState::Failed;
                transfer.error = Some("cancel");
                transfer.finished_ms = now;
            }
        }
        if mesh
            .bulk_reply
            .as_ref()
            .is_some_and(|reply| reply.stream == stream)
        {
            mesh.bulk_reply = None;
        }
        let mut body = [0u8; 4];
        body[0..4].copy_from_slice(&stream.to_le_bytes());
        let mut sent = false;
        if let Ok(frame) = MessageFrame::inline(
            MessageKind::Cancel,
            OpCode::Cancel,
            StreamId(stream),
            Tag(0),
            &body,
        ) {
            let mut header = MeshHeader::new(mesh.self_id, destination, mesh.self_id);
            header.flags = mesh_flags::PROBE;
            header.ttl = MeshHeader::MAX_HOPS;
            let frame_header = frame.header;
            sent = mesh.stage_control(stream, header, frame_header, &body, now);
        }
        if sent {
            // The cancel record is queued; reset after the sink accepts it so
            // the peer sees the reset before the slot is reused.
            mesh.retire_stream = Some(stream);
            return bulk_line(out, 200, "cancelled");
        }
        return bulk_line(out, 503, "cancel-queue");
    } else if start {
        let now = (services.clock_ms)();
        if mesh
            .bulk
            .as_ref()
            .is_some_and(|transfer| matches!(transfer.state, BulkState::Start | BulkState::Running))
        {
            return bulk_line(out, 409, "busy");
        }
        if !matches!(mesh.phase, Phase::Connected) {
            return bulk_line(out, 409, "not-connected");
        }
        if mesh.peer.is_none() {
            return bulk_line(out, 409, "no-peer");
        }
        if latency > 0 && !mesh.start_latency(latency, DEFAULT_LATENCY_GAP_MS, now) {
            return bulk_line(out, 409, "latency");
        }
        if !mesh.start_bulk(services, bytes, chunk, now) {
            return bulk_line(out, 400, "bad-request reason=range");
        }
        return bulk_line(out, 202, "started");
    }
    let mut body = Body::new(out);
    body.str("mesh-bulk:");
    match mesh.bulk.as_ref() {
        None => body.str(" state=idle"),
        Some(transfer) => {
            body.str(" state=");
            body.str(match transfer.state {
                BulkState::Start | BulkState::Running => "running",
                BulkState::Done => "done",
                BulkState::Failed => "failed",
            });
            body.str(" bytes=");
            body.u64(transfer.total as u64);
            body.str(" chunk=");
            body.u64(transfer.chunk as u64);
            body.str(" stream=");
            body.u64(transfer.stream as u64);
            body.str(" received=");
            body.u64(transfer.received as u64);
            body.str(" chunks=");
            body.u64(transfer.chunks as u64);
            body.str(" elapsed_ms=");
            body.u64(transfer.finished_ms.saturating_sub(transfer.started_ms));
            body.str(" queue_ms=");
            body.u64(
                transfer
                    .request_queued_ms
                    .saturating_sub(transfer.started_ms),
            );
            body.str(" first_ms=");
            body.u64(
                transfer
                    .first_chunk_ms
                    .saturating_sub(transfer.request_queued_ms),
            );
            body.str(" body_ms=");
            body.u64(transfer.finished_ms.saturating_sub(transfer.first_chunk_ms));
            if let Some(error) = transfer.error {
                body.str(" error=");
                body.str(error);
            }
        }
    }
    body.str(" writes=");
    body.u64(mesh.stats.writes);
    body.str(" batched=");
    body.u64(mesh.stats.batched_flushes);
    body.str(" peak_queue=");
    body.u64(mesh.stats.peak_queued_bytes as u64);
    body.str(" peak_decoder=");
    body.u64(mesh.stats.peak_decoder_bytes as u64);
    body.str(" bulk_rx=");
    body.u64(mesh.stats.bulk_chunks_received);
    body.str(" bulk_tx=");
    body.u64(mesh.stats.bulk_chunks_sent);
    body.str(" bulk_abort=");
    body.u64(mesh.stats.bulk_aborts);
    // The stream counters are the authority link's; a node with no session
    // reports zero for each.
    let streams = mesh.primary_streams();
    body.str(" stream_open=");
    body.u64(streams.map_or(0, |streams| streams.len() as u64));
    body.str(" stream_opens=");
    body.u64(streams.map_or(0, |streams| streams.opens()));
    body.str(" stream_resets=");
    body.u64(streams.map_or(0, |streams| streams.resets()));
    body.str(" stream_credit_stall=");
    body.u64(streams.map_or(0, |streams| streams.credit_stalls()));
    body.str(" stream_control_stall=");
    body.u64(streams.map_or(0, |streams| streams.control_stalls()));
    body.str(" stream_tx_control=");
    body.u64(streams.map_or(0, |streams| streams.control_frames()));
    body.str(" stream_tx_data=");
    body.u64(streams.map_or(0, |streams| streams.data_frames()));
    body.str(" stream_queued=");
    body.u64(streams.map_or(0, |streams| streams.queued_bytes() as u64));
    body.push(b'\n');
    let state = match mesh.bulk.as_ref().map(|transfer| transfer.state) {
        Some(BulkState::Done) => 200,
        Some(BulkState::Failed) => 502,
        Some(_) => 409,
        None => 200,
    };
    (state, body.len())
}

fn bulk_line(out: &mut [u8], status: u16, state: &str) -> (u16, usize) {
    let mut body = Body::new(out);
    body.str("mesh-bulk: ");
    body.str(state);
    body.push(b'\n');
    (status, body.len())
}

/// Render `GET /mesh/homes` for the MCP HTTP server.
///
/// The body is the same bounded, line-oriented evidence the module logs on
/// convergence.  A table is served only from a confirmed snapshot and only
/// when the optional `epoch=` query equals that snapshot's epoch; an older
/// epoch is a `409` with no object lines.
#[inline(never)]
/// Render `GET /mesh/join`: whether this boot has sent a join request, and the
/// view it holds.  The two-node rig uses it to tell a quiet rejoin from a lost
/// one without depending on the serial transcript.
pub fn join_response(out: &mut [u8]) -> (u16, usize) {
    // SAFETY: single-threaded module; `MESH` is written once at entry and no
    // mutable reference is live here.
    let mesh = unsafe { (&*core::ptr::addr_of!(MESH)).as_ref() };
    let Some(mesh) = mesh else {
        return line(out, 404, "mesh-join: disabled reason=peer\n");
    };
    let (epoch, members, digest) = match mesh.join.as_ref() {
        Some(join) => {
            let local;
            let snapshot = match join.converged() {
                Some(snapshot) => snapshot,
                None => {
                    local = join.snapshot();
                    &local
                }
            };
            (snapshot.epoch, snapshot.len(), snapshot.digest())
        }
        None => (0, 0, [0u8; 16]),
    };
    let mut body = Body::new(out);
    body.str("mesh-join: requests=");
    body.u64(mesh.join_requests);
    body.str(" epoch=");
    body.u64(epoch as u64);
    body.str(" members=");
    body.u64(members as u64);
    body.str(" digest=");
    body.digest(&digest);
    body.push(b'\n');
    (200, body.len())
}

fn line(out: &mut [u8], status: u16, text: &str) -> (u16, usize) {
    let bytes = text.as_bytes();
    let count = bytes.len().min(out.len());
    out[..count].copy_from_slice(&bytes[..count]);
    (status, count)
}

pub fn homes_response(path: &[u8], out: &mut [u8]) -> (u16, usize) {
    // SAFETY: the module is single-threaded and `MESH` is written once at
    // entry; no mutable reference to it is live here.
    let mesh = unsafe { (&*core::ptr::addr_of!(MESH)).as_ref() };
    let Some(mesh) = mesh else {
        return homes_line(out, 404, "disabled reason=peer");
    };
    let requested = match query_epoch(path) {
        Ok(epoch) => epoch,
        Err(()) => return homes_line(out, 400, "bad-request reason=query"),
    };
    let Some(join) = mesh.join.as_ref() else {
        return homes_line(out, 404, "disabled reason=join");
    };
    let Some(snapshot) = join.converged() else {
        let local = join.snapshot();
        let mut body = Body::new(out);
        body.str("mesh-homes: state=pending epoch=");
        body.u64(local.epoch as u64);
        body.str(" members=");
        body.u64(local.len() as u64);
        body.str(" digest=");
        body.digest(&local.digest());
        body.str(" count=0\n");
        return (409, body.len());
    };
    let objects = fixture_objects(mesh.home_count);
    let Some(table) = HomeTable::new(&snapshot.view(), snapshot.digest(), &objects) else {
        return homes_line(out, 500, "error reason=membership");
    };
    if let Some(requested) = requested {
        if requested != table.epoch {
            let mut body = Body::new(out);
            body.str("mesh-homes: state=stale requested_epoch=");
            body.u64(requested as u64);
            body.str(" epoch=");
            body.u64(table.epoch as u64);
            body.str(" members=");
            body.u64(snapshot.len() as u64);
            body.str(" digest=");
            body.digest(&table.membership);
            body.str(" count=0\n");
            return (409, body.len());
        }
    }
    let mut body = Body::new(out);
    body.str("mesh-homes: state=current object_set=m2-v1 epoch=");
    body.u64(table.epoch as u64);
    body.str(" members=");
    body.u64(snapshot.len() as u64);
    body.str(" digest=");
    body.digest(&table.membership);
    body.str(" count=");
    body.u64(table.len() as u64);
    body.push(b'\n');
    for entry in &table.entries {
        body.str("mesh-home: object=");
        body.hex(entry.object.as_bytes());
        body.str(" home=");
        body.node(&entry.home);
        body.str(" epoch=");
        body.u64(table.epoch as u64);
        body.str(" digest=");
        body.digest(&table.membership);
        body.push(b'\n');
    }
    (200, body.len())
}

/// Render `GET /mesh/shard` for the M3 shard slice.
///
/// The node reports the confirmed epoch and membership digest, the per-member
/// owner counts and only its own home assignments.  Every row is derived by
/// `mesh_core::HomeTable` from the same confirmed snapshot; no directory is
/// queried and no owner is asked for its list.  A stale `epoch=` is refused
/// with no rows, exactly as `/mesh/homes` does.
#[inline(never)]
pub fn shard_response(path: &[u8], out: &mut [u8]) -> (u16, usize) {
    // SAFETY: the module is single-threaded and `MESH` is written once at
    // entry; no mutable reference to it is live here.
    let mesh = unsafe { (&*core::ptr::addr_of!(MESH)).as_ref() };
    let Some(mesh) = mesh else {
        return shard_line(out, 404, "state=disabled reason=peer");
    };
    if mesh.shard_count == 0 {
        return shard_line(out, 404, "state=disabled reason=config");
    }
    let requested = match query_epoch(path) {
        Ok(epoch) => epoch,
        Err(()) => return shard_line(out, 400, "state=bad-request reason=query"),
    };
    let Some(join) = mesh.join.as_ref() else {
        return shard_line(out, 404, "state=disabled reason=join");
    };
    let Some(snapshot) = join.converged() else {
        let local = join.snapshot();
        let mut body = Body::new(out);
        body.str("mesh-shard: state=pending epoch=");
        body.u64(local.epoch as u64);
        body.str(" members=");
        body.u64(local.len() as u64);
        body.str(" digest=");
        body.digest(&local.digest());
        body.str(" count=0 owned=0\n");
        return (409, body.len());
    };
    let objects = shard_objects(mesh.shard_count);
    let Some(table) = HomeTable::new(&snapshot.view(), snapshot.digest(), &objects) else {
        return shard_line(out, 500, "state=error reason=membership");
    };
    if let Some(requested) = requested {
        if requested != table.epoch {
            let mut body = Body::new(out);
            body.str("mesh-shard: state=stale requested_epoch=");
            body.u64(requested as u64);
            body.str(" epoch=");
            body.u64(table.epoch as u64);
            body.str(" members=");
            body.u64(snapshot.len() as u64);
            body.str(" digest=");
            body.digest(&table.membership);
            body.str(" count=0 owned=0\n");
            return (409, body.len());
        }
    }
    let balance = table.balance();
    let owned = table.owned_by(mesh.self_id);
    let mut body = Body::new(out);
    body.str("mesh-shard: state=current object_set=m3-v1 role=");
    body.str(join.role().as_str());
    body.str(" node=");
    body.node(&mesh.self_id);
    body.str(" epoch=");
    body.u64(table.epoch as u64);
    body.str(" members=");
    body.u64(snapshot.len() as u64);
    body.str(" digest=");
    body.digest(&table.membership);
    body.str(" count=");
    body.u64(table.len() as u64);
    body.str(" owners=");
    body.u64(balance.owners as u64);
    body.str(" owned=");
    body.u64(owned.len() as u64);
    body.str(" min=");
    body.u64(balance.min as u64);
    body.str(" max=");
    body.u64(balance.max as u64);
    body.push(b'\n');
    for member in &snapshot.members {
        body.str("mesh-shard-owner: node=");
        body.node(member);
        body.str(" count=");
        body.u64(table.owned_count(*member) as u64);
        body.push(b'\n');
    }
    for entry in &owned {
        body.str("mesh-shard-owned: object=");
        body.hex(entry.object.as_bytes());
        body.str(" home=");
        body.node(&entry.home);
        body.str(" epoch=");
        body.u64(table.epoch as u64);
        body.str(" digest=");
        body.digest(&table.membership);
        body.push(b'\n');
    }
    (200, body.len())
}

fn shard_line(out: &mut [u8], status: u16, state: &str) -> (u16, usize) {
    let mut body = Body::new(out);
    body.str("mesh-shard: ");
    body.str(state);
    body.push(b'\n');
    (status, body.len())
}

/// Render or drive `GET /mesh/migrate`.
///
/// A bare request returns the bounded migration report: the confirmed view,
/// one actor row (active, provisional or fenced) with the logical state hash,
/// the queued and unacknowledged message at the boundary, the serialised
/// capability handle, the last placement change and the counters.  `?step=N`
/// drives one rig step; the root chooses the order and the migration policy
/// chooses the actor, hosts and message ids.  It is not a directory and it is
/// not persisted.
#[inline(never)]
pub fn migrate_response(services: &Services, path: &[u8], out: &mut [u8]) -> (u16, usize) {
    let step = match migrate_step(path) {
        Ok(step) => step,
        Err(()) => return migrate_line(out, 400, "state=bad-request reason=query"),
    };
    if let Some(step) = step {
        return migrate_command(services, step, out);
    }
    // SAFETY: the module is single-threaded and `MESH` is written once at
    // entry; no other reference to it is live here.
    let mesh = unsafe { (&mut *core::ptr::addr_of_mut!(MESH)).as_mut() };
    let Some(mesh) = mesh else {
        return migrate_line(out, 404, "state=disabled reason=peer");
    };
    let Some(migrate) = mesh.migrate.as_mut() else {
        return migrate_line(out, 404, "state=disabled reason=config");
    };
    migrate.build_report();
    let report = migrate.report();
    let mut body = Body::new(out);
    body.str("mesh-migrate: state=enabled role=");
    body.str(report.role);
    body.str(" view_epoch=");
    body.u64(report.view_epoch as u64);
    body.str(" members=");
    body.u64(report.view_members as u64);
    body.str(" digest=");
    body.hex(&report.view_digest);
    body.push(b'\n');
    match &report.actor {
        Some(row) => {
            body.str("mesh-migrate: actor id=");
            body.hex(&row.actor.0);
            body.str(" home=");
            body.node(&row.home);
            body.str(" state=");
            body.str(row.state);
            body.str(" host=");
            body.node(&row.host);
            body.str(" epoch=");
            body.u64(row.epoch as u64);
            body.str(" applied=");
            body.u64(row.applied);
            body.str(" last=");
            body.u64(row.last);
            body.push(b'\n');
            body.str("mesh-migrate: actor hash=");
            body.hex(&row.hash);
            body.str(" logical=");
            body.hex(&row.logical);
            body.str(" queued=");
            match row.queued {
                Some(message) => {
                    body.u64(message.id);
                    body.push(b':');
                    body.hex(&message.payload);
                }
                None => body.push(b'-'),
            }
            body.str(" unacked=");
            match row.unacked {
                Some(ack) => {
                    body.u64(ack.id);
                    body.push(b':');
                    body.hex(&ack.state);
                }
                None => body.push(b'-'),
            }
            body.str(" handle=");
            match &row.handle {
                Some(handle) => {
                    body.hex(handle.object.as_bytes());
                    body.push(b':');
                    body.u64(handle.rights.0 as u64);
                    body.push(b':');
                    body.node(&handle.home);
                    body.push(b':');
                    body.u64(handle.home_epoch as u64);
                    body.push(b':');
                    body.u64(handle.object_epoch as u64);
                }
                None => body.push(b'-'),
            }
            body.push(b'\n');
        }
        None => body.str("mesh-migrate: actor state=none\n"),
    }
    if let Some(last) = report.last_move {
        body.str("mesh-migrate: move actor=");
        body.hex(&last.actor.0);
        body.str(" old=");
        body.node(&last.old_host);
        body.str(" new=");
        body.node(&last.new_host);
        body.str(" old_epoch=");
        body.u64(last.old_epoch as u64);
        body.str(" new_epoch=");
        body.u64(last.new_epoch as u64);
        body.str(" hash=");
        body.hex(&last.hash);
        body.push(b'\n');
    }
    body.str("mesh-migrate: counters migrations=");
    body.u64(report.counters.migrations as u64);
    body.str(" incoming=");
    body.u64(report.counters.incoming as u64);
    body.str(" outgoing=");
    body.u64(report.counters.outgoing as u64);
    body.str(" resumes=");
    body.u64(report.counters.resumes as u64);
    body.str(" duplicates=");
    body.u64(report.counters.duplicates as u64);
    body.str(" stale=");
    body.u64(report.counters.stale as u64);
    body.str(" capability=");
    body.u64(report.counters.capability_rejects as u64);
    body.str(" rejects=");
    body.u64(report.counters.rejects as u64);
    body.str(" aborted=");
    body.u64(report.counters.aborted as u64);
    body.push(b'\n');
    body.str("mesh-migrate: last stage=");
    body.str(report.stage);
    if let Some(reason) = report.reason {
        body.str(" reason=");
        body.str(reason.as_str());
    }
    body.push(b'\n');
    (200, body.len())
}

fn migrate_step(path: &[u8]) -> Result<Option<u8>, ()> {
    if path == b"/mesh/migrate" {
        return Ok(None);
    }
    let Some(query) = path.strip_prefix(b"/mesh/migrate?") else {
        return Err(());
    };
    let Some(value) = query.strip_prefix(b"step=") else {
        return Err(());
    };
    if value.is_empty() || value.len() > 2 || !value.iter().all(|byte| byte.is_ascii_digit()) {
        return Err(());
    }
    let mut step = 0u8;
    for byte in value {
        step = step.saturating_mul(10).saturating_add(*byte - b'0');
    }
    Ok(Some(step))
}

#[inline(never)]
fn migrate_command(services: &Services, step: u8, out: &mut [u8]) -> (u16, usize) {
    // SAFETY: the module tick is single-threaded and no other reference to
    // `MESH` is live while the HTTP handler runs.
    let Some(mesh) = (unsafe { (&mut *core::ptr::addr_of_mut!(MESH)).as_mut() }) else {
        return migrate_line(out, 404, "state=disabled reason=peer");
    };
    let Some(join) = mesh.join.as_ref() else {
        return migrate_line(out, 404, "state=disabled reason=join");
    };
    let Some(migrate) = mesh.migrate.as_mut() else {
        return migrate_line(out, 404, "state=disabled reason=config");
    };
    let now = (services.clock_ms)();
    let signer = JoinSigner(join);
    let command = migrate.command(step, now, &signer);
    // A stale or capability refusal is the intended result of the negative
    // steps; it is reported in the counters and the command is still a
    // successful control action.  Only a rejected command is a 409.
    let rejected = command.events.iter().find_map(|event| match event {
        MigrateEvent::Reject { reason, .. } => Some(*reason),
        MigrateEvent::Aborted { reason, .. } => Some(*reason),
        _ => None,
    });
    mesh.dispatch_migrate(services, command, now);
    let mut body = Body::new(out);
    match rejected {
        Some(reason) => {
            body.str("mesh-migrate: rejected step=");
            body.u64(step as u64);
            body.str(" reason=");
            body.str(reason.as_str());
            body.push(b'\n');
            (409, body.len())
        }
        None => {
            body.str("mesh-migrate: started step=");
            body.u64(step as u64);
            body.push(b'\n');
            (202, body.len())
        }
    }
}

fn migrate_line(out: &mut [u8], status: u16, state: &str) -> (u16, usize) {
    let mut body = Body::new(out);
    body.str("mesh-migrate: ");
    body.str(state);
    body.push(b'\n');
    (status, body.len())
}

/// Render or drive `GET /mesh/route`.
///
/// A bare request returns a bounded snapshot of the route policy's in-memory
/// report: the confirmed view, the object moved in each direction, every
/// completed operation (object, home, epoch, value, attempt count and whether
/// a `MOVED` was consumed), and the refusal/timeout counters.  `?step=N`
/// drives one rig step (`1` remote stat, `2` move peer->self, `3` stale stat,
/// `4` move self->peer, `5` stale stat).  The rig chooses the order; the route
/// policy chooses the object from the confirmed rendezvous view.
///
/// `?op=stat&object=<hex>` and `?op=move&object=<hex>` are the same policy
/// through a named object: the one-computer console uses them instead of the
/// fixed rig steps.  `move` always moves the object to the node that received
/// the request, which is the console's node.  It is not a directory and it is
/// not persisted.
#[inline(never)]
pub fn route_response(services: &Services, path: &[u8], out: &mut [u8]) -> (u16, usize) {
    let query = match route_query(path) {
        Ok(query) => query,
        Err(()) => return route_line(out, 400, "state=bad-request reason=query"),
    };
    match query {
        RouteQuery::Step(step) => return route_command(services, step, out),
        RouteQuery::Stat(object) => return route_object_command(services, object, false, out),
        RouteQuery::Move(object) => return route_object_command(services, object, true, out),
        RouteQuery::Report => {}
    }
    // SAFETY: the module is single-threaded and `MESH` is written once at
    // entry; no mutable reference to it is live here.
    let mesh = unsafe { (&*core::ptr::addr_of!(MESH)).as_ref() };
    let Some(mesh) = mesh else {
        return route_line(out, 404, "state=disabled reason=peer");
    };
    let Some(route) = mesh.route.as_ref() else {
        return route_line(out, 404, "state=disabled reason=config");
    };
    let report = route.report();
    let now = (services.clock_ms)();
    let mut body = Body::new(out);
    body.str("mesh-route: state=enabled view_epoch=");
    body.u64(report.view_epoch as u64);
    body.str(" members=");
    body.u64(report.view_members as u64);
    body.str(" digest=");
    body.digest(&report.view_digest);
    body.push(b'\n');
    body.str("mesh-route: authority state=");
    body.str(if route.authority_current() {
        "current"
    } else {
        "lost"
    });
    body.str(" epoch=");
    body.u64(route.epoch() as u64);
    body.str(" mutable=");
    body.u64(if route.mutable_available_at(now) {
        1
    } else {
        0
    });
    body.str(" write_lease_ms=");
    body.u64(route.write_lease_remaining(now) as u64);
    body.push(b'\n');
    for fence in &report.fence_rows {
        body.str("mesh-route: fence object=");
        body.hex(fence.object.as_bytes());
        body.str(" stale_home=");
        body.node(&fence.stale_home);
        body.str(" stale_home_epoch=");
        body.u64(fence.stale_home_epoch as u64);
        body.str(" home=");
        body.node(&fence.home);
        body.str(" home_epoch=");
        body.u64(fence.home_epoch as u64);
        body.str(" reason=");
        body.str(fence.reason);
        body.push(b'\n');
    }
    body.str("mesh-route: move old=");
    body.object_opt(&report.move_old);
    body.str(" new=");
    body.object_opt(&report.move_new);
    body.push(b'\n');
    for result in &report.results {
        body.str("mesh-route: result object=");
        body.hex(result.object.as_bytes());
        body.str(" home=");
        body.node(&result.home);
        body.str(" home_epoch=");
        body.u64(result.home_epoch as u64);
        body.str(" op=");
        body.str(mesh_route::operation_label(result.operation));
        body.str(" value=");
        body.hex(&result.value);
        body.str(" attempts=");
        body.u64(result.attempts as u64);
        body.str(" moved=");
        body.u64(if result.moved { 1 } else { 0 });
        body.push(b'\n');
    }
    body.str("mesh-route: counters completes=");
    body.u64(report.completes as u64);
    body.str(" retries=");
    body.u64(report.retries as u64);
    body.str(" rejects=");
    body.u64(report.rejects as u64);
    body.str(" timeouts=");
    body.u64(report.timeouts as u64);
    body.str(" results=");
    body.u64(report.results.len() as u64);
    body.str(" moved_local=");
    body.u64(report.moved_local as u64);
    body.str(" moved_remote=");
    body.u64(report.moved_remote as u64);
    body.str(" fences=");
    body.u64(report.fences as u64);
    body.str(" commits=");
    body.u64(report.commits as u64);
    body.str(" replays=");
    body.u64(report.replays as u64);
    if let Some(reason) = report.last_replay {
        body.str(" last_replay=");
        body.str(reason.as_str());
    }
    if let Some(reason) = report.last_reject {
        body.str(" last_reject=");
        body.str(reason.as_str());
    }
    body.push(b'\n');
    (200, body.len())
}

enum RouteQuery {
    Report,
    Step(u8),
    Stat(ObjectId),
    Move(ObjectId),
}

fn route_query(path: &[u8]) -> Result<RouteQuery, ()> {
    if path == b"/mesh/route" {
        return Ok(RouteQuery::Report);
    }
    let Some(query) = path.strip_prefix(b"/mesh/route?") else {
        return Err(());
    };
    if let Some(value) = query.strip_prefix(b"step=") {
        if value.is_empty() || value.len() > 2 || !value.iter().all(|byte| byte.is_ascii_digit()) {
            return Err(());
        }
        let mut step = 0u8;
        for byte in value {
            step = step.saturating_mul(10).saturating_add(*byte - b'0');
        }
        return Ok(RouteQuery::Step(step));
    }
    let Some(rest) = query.strip_prefix(b"op=") else {
        return Err(());
    };
    let Some(separator) = rest.iter().position(|byte| *byte == b'&') else {
        return Err(());
    };
    let (operation, rest) = rest.split_at(separator);
    let Some(value) = rest.get(1..).and_then(|rest| rest.strip_prefix(b"object=")) else {
        return Err(());
    };
    let object = parse_object_hex(value).ok_or(())?;
    match operation {
        b"stat" => Ok(RouteQuery::Stat(object)),
        b"move" => Ok(RouteQuery::Move(object)),
        _ => Err(()),
    }
}

fn parse_object_hex(bytes: &[u8]) -> Option<ObjectId> {
    if bytes.len() != 64 {
        return None;
    }
    let mut object = [0u8; 32];
    for (index, byte) in object.iter_mut().enumerate() {
        let high = hex_value(bytes[index * 2])?;
        let low = hex_value(bytes[index * 2 + 1])?;
        *byte = (high << 4) | low;
    }
    Some(ObjectId::from_bytes(object))
}

fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

#[inline(never)]
fn route_object_command(
    services: &Services,
    object: ObjectId,
    move_object: bool,
    out: &mut [u8],
) -> (u16, usize) {
    // SAFETY: the module tick is single-threaded and no other reference to
    // `MESH` is live while the HTTP handler runs.
    let Some(mesh) = (unsafe { (&mut *core::ptr::addr_of_mut!(MESH)).as_mut() }) else {
        return route_line(out, 404, "state=disabled reason=peer");
    };
    let now = (services.clock_ms)();
    let self_id = mesh.self_id;
    let Some(join) = mesh.join.as_ref() else {
        return route_line(out, 404, "state=disabled reason=join");
    };
    let signer = JoinSigner(join);
    let Some(route) = mesh.route.as_mut() else {
        return route_line(out, 404, "state=disabled reason=config");
    };
    let step_result = if move_object {
        route.move_object(object, self_id, now, &signer)
    } else {
        route.begin_operation(object, now, &signer)
    };
    let rejected = step_result.events.iter().find_map(|event| match event {
        mesh_route::Event::Reject { reason, .. } => Some(*reason),
        _ => None,
    });
    mesh.dispatch_route(services, step_result, now);
    let mut body = Body::new(out);
    body.str("mesh-route: ");
    if rejected.is_some() {
        body.str("rejected op=");
    } else {
        body.str("started op=");
    }
    body.str(if move_object { "move" } else { "stat" });
    body.str(" object=");
    body.hex(object.as_bytes());
    if let Some(reason) = rejected {
        body.str(" reason=");
        body.str(reason.as_str());
        body.push(b'\n');
        (409, body.len())
    } else {
        body.push(b'\n');
        (202, body.len())
    }
}

#[inline(never)]
fn route_command(services: &Services, step: u8, out: &mut [u8]) -> (u16, usize) {
    // SAFETY: the module tick is single-threaded and no other reference to
    // `MESH` is live while the HTTP handler runs.
    let Some(mesh) = (unsafe { (&mut *core::ptr::addr_of_mut!(MESH)).as_mut() }) else {
        return route_line(out, 404, "state=disabled reason=peer");
    };
    let now = (services.clock_ms)();
    let command = if let Some(join) = mesh.join.as_ref() {
        let signer = JoinSigner(join);
        mesh.route
            .as_mut()
            .map(|route| route.command(step, now, &signer))
    } else {
        None
    };
    let Some(step_result) = command else {
        return route_line(out, 404, "state=disabled reason=config");
    };
    let rejected = step_result.events.iter().find_map(|event| match event {
        mesh_route::Event::Reject { reason, .. } => Some(*reason),
        _ => None,
    });
    mesh.dispatch_route(services, step_result, now);
    let mut body = Body::new(out);
    match rejected {
        Some(reason) => {
            body.str("mesh-route: rejected step=");
            body.u64(step as u64);
            body.str(" reason=");
            body.str(reason.as_str());
            body.push(b'\n');
            (409, body.len())
        }
        None => {
            body.str("mesh-route: started step=");
            body.u64(step as u64);
            body.push(b'\n');
            (202, body.len())
        }
    }
}

fn route_line(out: &mut [u8], status: u16, state: &str) -> (u16, usize) {
    let mut body = Body::new(out);
    body.str("mesh-route: ");
    body.str(state);
    body.push(b'\n');
    (status, body.len())
}

fn homes_line(out: &mut [u8], status: u16, state: &str) -> (u16, usize) {
    let mut body = Body::new(out);
    body.str("mesh-homes: state=");
    body.str(state);
    body.push(b'\n');
    (status, body.len())
}

fn query_epoch(path: &[u8]) -> Result<Option<u32>, ()> {
    let Some(question) = path.iter().position(|byte| *byte == b'?') else {
        return Ok(None);
    };
    let query = &path[question + 1..];
    let mut epoch = None;
    for part in query.split(|byte| *byte == b'&') {
        if part.is_empty() {
            continue;
        }
        let mut pieces = part.splitn(2, |byte| *byte == b'=');
        let key = pieces.next().unwrap_or(b"");
        let value = pieces.next().ok_or(())?;
        if key != b"epoch" || epoch.is_some() {
            return Err(());
        }
        epoch = Some(parse_u32(value).ok_or(())?);
    }
    Ok(epoch)
}

struct Body<'a> {
    bytes: &'a mut [u8],
    len: usize,
}

impl<'a> Body<'a> {
    fn new(bytes: &'a mut [u8]) -> Self {
        Self { bytes, len: 0 }
    }

    fn len(&self) -> usize {
        self.len
    }

    fn push(&mut self, byte: u8) {
        if self.len < self.bytes.len() {
            self.bytes[self.len] = byte;
            self.len += 1;
        }
    }

    fn str(&mut self, text: &str) {
        for byte in text.as_bytes() {
            self.push(*byte);
        }
    }

    fn u64(&mut self, mut value: u64) {
        let mut digits = [0u8; 20];
        let mut count = 0;
        loop {
            digits[count] = b'0' + (value % 10) as u8;
            value /= 10;
            count += 1;
            if value == 0 {
                break;
            }
        }
        while count > 0 {
            count -= 1;
            self.push(digits[count]);
        }
    }

    fn hex(&mut self, bytes: &[u8]) {
        const HEX: &[u8; 16] = b"0123456789ABCDEF";
        for byte in bytes {
            self.push(HEX[(byte >> 4) as usize]);
            self.push(HEX[(byte & 0x0f) as usize]);
        }
    }

    fn node(&mut self, node: &NodeId) {
        self.hex(node.as_bytes());
    }

    fn digest(&mut self, digest: &[u8; 16]) {
        self.hex(digest);
    }

    fn object_opt(&mut self, object: &Option<ObjectId>) {
        match object {
            Some(object) => self.hex(object.as_bytes()),
            None => self.push(b'-'),
        }
    }
}

impl Mesh {
    fn link_count(&self) -> usize {
        self.links.len()
    }

    fn link_index(&self, peer: NodeId) -> Option<usize> {
        if peer.is_zero() {
            return self.primary_link;
        }
        self.links.iter().position(|link| link.peer == peer)
    }

    fn link(&self, peer: NodeId) -> Option<&MeshLink> {
        self.link_index(peer).map(|index| &self.links[index])
    }

    fn link_mut(&mut self, peer: NodeId) -> Option<&mut MeshLink> {
        let index = self.link_index(peer)?;
        self.links.get_mut(index)
    }

    /// Return the link for `peer`, creating a bounded gossip link if needed.
    ///
    /// The hub forwards by destination over the shared session, so a member can
    /// address another member without a separate TCP connection.  A full link
    /// table falls back to the primary queue: the frame is still routable, but
    /// it no longer has per-peer queue state, and the table never grows.
    fn ensure_link(&mut self, peer: NodeId, now: u64) -> Option<usize> {
        if peer.is_zero() {
            return self.primary_link;
        }
        if let Some(index) = self.link_index(peer) {
            return Some(index);
        }
        if self.links.len() >= MAX_MESH_LINKS {
            return self.primary_link;
        }
        let side = self.side_for(peer);
        self.links
            .push(MeshLink::new(peer, false, self.heartbeat, side, now));
        Some(self.links.len() - 1)
    }

    fn outbound_index(&mut self, destination: NodeId, now: u64) -> Option<usize> {
        if destination.is_zero() {
            self.primary_link
        } else {
            self.ensure_link(destination, now)
        }
    }

    /// The authority link's stream mux; bulk and latency use this one.
    fn primary_streams(&self) -> Option<&StreamMux> {
        let index = self.primary_link?;
        self.links.get(index).map(|link| &link.streams)
    }

    fn primary_streams_mut(&mut self) -> Option<&mut StreamMux> {
        primary_mux(&mut self.links, self.primary_link)
    }

    /// Start a fresh session: one unbound authority link, no gossip state.
    ///
    /// A member's authority link is addressed to the configured root from the
    /// start.  A root cannot know the first member's `NodeId` until a frame
    /// arrives, so its authority link starts unbound and is bound by
    /// `learn_peer` on the first accepted source.
    fn reset_links(&mut self, now: u64) {
        self.links.clear();
        let role = self.join.as_ref().map(|join| join.role());
        let root = self.join.as_ref().map(|join| join.root());
        let primary_peer = match (role, root) {
            (Some(Role::Member), Some(root)) => root,
            _ => NodeId::ZERO,
        };
        let side = self.side_for(primary_peer);
        self.links
            .push(MeshLink::new(primary_peer, true, self.heartbeat, side, now));
        self.primary_link = Some(0);
        self.stage_reset();
        self.stage_fault = None;
        self.retire_stream = None;
        self.peer = match role {
            Some(Role::Member) => Some(primary_peer),
            _ => None,
        };
        // A member's route and migration policy start bound to the authority
        // link.  Without this the first route frame from the root is refused as
        // `peer-mismatch`; the old single-link path set this on the first
        // learned peer, but a member now knows the root before it hears a frame.
        if role == Some(Role::Member) {
            if let Some(route) = self.route.as_mut() {
                let _ = route.set_peer(primary_peer, now);
            }
            if let Some(migrate) = self.migrate.as_mut() {
                let _ = migrate.set_peer(primary_peer);
            }
        }
        self.peer_confirmed = false;
        self.gossip_floor = 0;
        self.gossip_round = 0;
        self.last_gossip_ms = now;
    }

    /// Bind the root's unbound authority link to the first accepted source.
    fn bind_primary(&mut self, peer: NodeId) -> Option<usize> {
        let index = self.primary_link?;
        let link = self.links.get_mut(index)?;
        if link.peer.is_zero() {
            link.peer = peer;
            link.confirmed = true;
            if self.peer.is_none() {
                self.peer = Some(peer);
            }
        }
        Some(index)
    }

    /// Drop a link after the failure detector removed its member.
    fn forget_link(&mut self, peer: NodeId) {
        let Some(index) = self.link_index(peer) else {
            return;
        };
        if Some(index) == self.primary_link {
            // A member never removes its root.  A root keeps the primary slot
            // as its authority route target even if that member failed; the
            // slot is reused by the next confirmed member only if unbound.
            return;
        }
        self.links.remove(index);
    }

    fn mark_sent_epoch(&mut self, peer: NodeId, epoch: u32) {
        if let Some(link) = self.link_mut(peer) {
            if epoch > link.sent_epoch {
                link.sent_epoch = epoch;
            }
        }
    }

    fn link_sent_epoch(&self, peer: NodeId) -> u32 {
        self.link(peer).map(|link| link.sent_epoch).unwrap_or(0)
    }

    fn tick(&mut self, services: &Services) {
        let now = (services.clock_ms)();
        // Failure detection runs in every phase: a partition may leave the
        // link down, and a killed peer never reconnects.  The detector uses the
        // same membership/heartbeat path as the join.
        self.poll_membership(services, now);
        match self.phase {
            Phase::Arm => self.arm(services, now),
            Phase::Waiting => self.waiting(services, now),
            Phase::Connect => self.connecting(services, now),
            Phase::Connected => self.connected(services, now),
        }
    }

    /// Advance the bounded failure detector and log every transition.
    fn poll_membership(&mut self, services: &Services, now: u64) {
        if !self.failures {
            return;
        }
        let Some(join) = self.join.as_mut() else {
            return;
        };
        let events = join.poll_failures(now);
        if events.is_empty() {
            return;
        }
        let mut removed = None;
        let mut lost = false;
        for event in &events {
            match event {
                JoinEvent::Suspect { node, epoch } => {
                    let mut line = Line::new();
                    line.str("mesh-fail: suspect node=");
                    line.node(node);
                    line.str(" epoch=");
                    line.u64(*epoch as u64);
                    line.push(b'\n');
                    log(services, line.as_bytes());
                }
                JoinEvent::Failed { node, epoch } => {
                    let mut line = Line::new();
                    line.str("mesh-fail: failed node=");
                    line.node(node);
                    line.str(" epoch=");
                    line.u64(*epoch as u64);
                    line.push(b'\n');
                    log(services, line.as_bytes());
                }
                JoinEvent::Recovered { node, epoch } => {
                    let mut line = Line::new();
                    line.str("mesh-fail: recovered node=");
                    line.node(node);
                    line.str(" epoch=");
                    line.u64(*epoch as u64);
                    line.push(b'\n');
                    log(services, line.as_bytes());
                }
                JoinEvent::Removed { node, snapshot } => {
                    let mut line = Line::new();
                    line.str("mesh-fail: removed node=");
                    line.node(node);
                    line.str(" epoch=");
                    line.u64(snapshot.epoch as u64);
                    line.str(" members=");
                    line.u64(snapshot.len() as u64);
                    line.str(" digest=");
                    line.digest(&snapshot.digest());
                    line.str(" snapshot=");
                    line.nodes(&snapshot.members);
                    line.push(b'\n');
                    log(services, line.as_bytes());
                    removed = Some(snapshot.clone());
                }
                JoinEvent::RemovedBatch { nodes, snapshot } => {
                    let mut line = Line::new();
                    line.str("mesh-fail: removed batch");
                    for node in nodes.iter() {
                        line.str(" node=");
                        line.node(node);
                    }
                    line.str(" epoch=");
                    line.u64(snapshot.epoch as u64);
                    line.str(" members=");
                    line.u64(snapshot.len() as u64);
                    line.push(b'\n');
                    log(services, line.as_bytes());
                    removed = Some(snapshot.clone());
                }
                JoinEvent::AuthorityLost { node, epoch } => {
                    lost = true;
                    let mut line = Line::new();
                    line.str("mesh-fail: authority state=lost epoch=");
                    line.u64(*epoch as u64);
                    line.str(" root=");
                    line.node(node);
                    line.str(" mutable=0 reads=lease\n");
                    log(services, line.as_bytes());
                }
                _ => {}
            }
        }
        if lost {
            if let Some(route) = self.route.as_mut() {
                if let Some(event) = route.on_authority_lost() {
                    route.note_event(&event);
                    log_route_event(services, &event);
                }
            }
        }
        let had_removal = removed.is_some();
        if let Some(snapshot) = removed {
            self.log_homes(services, &snapshot, true);
            self.on_route_snapshot(services, &snapshot, now);
            // A removal advances the epoch without sending a confirmation to
            // the removed node.  Persist the root's new signed view so a root
            // restart resumes the post-removal epoch rather than the seed.
            self.persist_root(services);
        }
        if had_removal && self.join.as_ref().map(|join| join.role()) == Some(Role::Root) {
            // Publish the new view once and seed it to a bounded window; a
            // member that misses the push is repaired from its next summary.
            if let Some(aggregate) = self.join.as_mut().and_then(|join| join.publish_aggregate()) {
                self.gossip_epoch(services, &aggregate, None, now, false);
            }
        }
    }

    fn arm(&mut self, services: &Services, now: u64) {
        let state = (services.tcp_state)(MESH_SLOT);
        let code = unsafe { (services.tcp_listen)(MESH_SLOT, MESH_PORT) };
        if code == 0 {
            self.phase = Phase::Waiting;
            if !self.listen_logged {
                self.listen_logged = true;
                log(services, b"mesh-transport: listening on port 7000\n");
            }
        } else if now.saturating_sub(self.last_arm_log_ms) >= 5_000 {
            self.last_arm_log_ms = now;
            let mut line = Line::new();
            line.str("mesh-transport: listen retry state=");
            line.u64(state as u64);
            line.str(" code=");
            line.i32(code);
            line.push(b'\n');
            log(services, line.as_bytes());
        }
    }

    fn waiting(&mut self, services: &Services, now: u64) {
        match (services.tcp_state)(MESH_SLOT) {
            2 => self.on_connected(services, now),
            3 => {
                (services.tcp_close)(MESH_SLOT);
                self.phase = Phase::Arm;
            }
            0 => self.phase = Phase::Arm,
            _ => {}
        }
    }

    /// Drive the lower-NodeId side of a direct peer link.
    fn connecting(&mut self, services: &Services, now: u64) {
        if (services.tcp_state)(MESH_SLOT) == 2 {
            self.on_connected(services, now);
            return;
        }
        let Some(target) = self.direct else {
            self.phase = Phase::Arm;
            return;
        };
        if now >= self.connect_at_ms && (services.tcp_state)(MESH_SLOT) == 0 {
            if self.connect_attempts == 0 {
                self.connect_started_ms = now;
            }
            let address = u32::from_be_bytes(target.address);
            let code = unsafe { (services.tcp_connect)(MESH_SLOT, address, target.port) };
            self.connect_attempts = self.connect_attempts.saturating_add(1);
            self.connect_at_ms = now.saturating_add(500);
            if code != 0 {
                let mut line = Line::new();
                line.str("mesh-transport: direct connect failed attempt=");
                line.u64(self.connect_attempts as u64);
                line.push(b'\n');
                log(services, line.as_bytes());
            }
        }
        if self.connect_attempts >= 5 || now.saturating_sub(self.connect_started_ms) >= 5_000 {
            self.direct_fallback = true;
            self.phase = Phase::Arm;
            log(services, b"mesh-transport: direct fallback listen\n");
        }
    }

    /// Enter the connected state after either a listener accept or a direct
    /// connect succeeds.
    fn on_connected(&mut self, services: &Services, now: u64) {
        self.stats.connects = self.stats.connects.saturating_add(1);
        self.link_generation = self.link_generation.wrapping_add(1);
        self.decoder.reset();
        self.reset_links(now);
        self.pass_logged = false;
        self.join_timeout_logged = false;
        self.last_rx_ms = now;
        self.phase = Phase::Connected;
        self.connect_attempts = 0;
        self.last_connect_ms = if self.direct_initiator {
            now.saturating_sub(self.connect_started_ms)
        } else {
            0
        };
        let mut line = Line::new();
        line.str("mesh-transport: connected connect_ms=");
        line.u64(self.last_connect_ms);
        line.str(" links=");
        line.u64(self.link_count() as u64);
        line.push(b'\n');
        log(services, line.as_bytes());
        self.send_handshake(now);
        let request = self.join.as_mut().and_then(|join| join.on_link_up(now));
        if let Some(request) = request {
            self.send_join(
                services,
                JoinMessage::Request(request),
                NodeId::ZERO,
                now,
                self.verbose_join_log(),
            );
        }
        self.flush(services, now);
    }

    fn connected(&mut self, services: &Services, now: u64) {
        if (services.tcp_state)(MESH_SLOT) != 2 {
            self.disconnect(services, now, "closed");
            return;
        }

        // A TCP read can contain several records, and `feed` processes one
        // complete record per iteration before it pushes the rest into the
        // decoder.  Drain the decoder every tick as well, otherwise a burst
        // (for example seven join requests arriving while the root is signing)
        // leaves complete records queued with no further `tcp_recv` bytes to
        // trigger them, and the 10 s receive deadline eventually closes a
        // healthy link.  The M2 two-node path usually read one record at a
        // time and did not expose this.
        if let Err(error) = self.feed(services, &[], now) {
            self.reject(services, now, error);
            return;
        }

        // Read before evaluating the receive and heartbeat deadlines.  A
        // bounded serial burst (the root logs its confirmed home table) can
        // keep this tick busy for longer than the deadline; bytes for the
        // next member are already queued in the socket when it returns.  A
        // deadline is evidence of silence, not of time spent processing the
        // previous record, so the socket must be drained first.  This is the
        // multi-member convergence defect the eight-node host hub exposed.
        let buffer = unsafe { &mut *core::ptr::addr_of_mut!(READ_BUF) };
        for _ in 0..READS_PER_TICK {
            let read = unsafe { (services.tcp_recv)(MESH_SLOT, buffer.as_mut_ptr(), buffer.len()) };
            if read <= 0 {
                break;
            }
            let count = read as usize;
            self.stats.bytes_received = self.stats.bytes_received.saturating_add(count as u64);
            self.last_rx_ms = now;
            if let Err(error) = self.feed(services, &buffer[..count], now) {
                self.reject(services, now, error);
                return;
            }
        }

        if self.decoder.buffered_len() > 0
            && now.saturating_sub(self.last_rx_ms) >= self.receive_deadline_ms()
        {
            self.stats.receive_deadlines = self.stats.receive_deadlines.saturating_add(1);
            self.disconnect(services, now, "receive-deadline");
            return;
        }
        if self
            .links
            .iter()
            .any(|link| link.queue.send_deadline_expired(now))
        {
            self.stats.send_deadlines = self.stats.send_deadlines.saturating_add(1);
            self.disconnect(services, now, "send-deadline");
            return;
        }
        // A silent gossip link is dropped; the authority link is kept until
        // every link is silent, because under the hub one quiet member must
        // not tear down the root's session with the others.
        self.expire_silent_links(services, now);
        if !self.links.iter().any(|link| !link.heartbeat.timed_out(now)) {
            self.stats.heartbeat_timeouts = self.stats.heartbeat_timeouts.saturating_add(1);
            self.disconnect(services, now, "heartbeat-timeout");
            return;
        }
        let tick = self
            .join
            .as_mut()
            .map(|join| (join.poll(now), join.attempts()));
        match tick {
            // Do not start or answer a routine epoch refresh while a bulk
            // measurement is in flight.  The M2 refresh signs a full member
            // list with Ed25519; on the emulated guest that work lands in the
            // middle of the transfer and was the 200-600 ms tail in the first
            // #247 runs.  Retries and timeouts are not suppressed.
            Some((Tick::Refresh(_), _)) if self.bulk_active() => {}
            Some((Tick::Send(request), attempts)) => {
                self.send_join(
                    services,
                    JoinMessage::Request(request),
                    NodeId::ZERO,
                    now,
                    self.verbose_join_log(),
                );
                let mut line = Line::new();
                line.str("mesh-join: retry attempt=");
                line.u64(attempts as u64);
                line.push(b'\n');
                log(services, line.as_bytes());
            }
            Some((Tick::Refresh(request), _)) => {
                // Routine refresh: the signed request is still sent, but the
                // 5 s transcript line is omitted; the rate-limited
                // `refreshed` event records the installed view.
                self.send_join(
                    services,
                    JoinMessage::Request(request),
                    NodeId::ZERO,
                    now,
                    false,
                );
            }
            Some((Tick::Summary, _)) => {
                // Routine M3 refresh: the member reports liveness with a small
                // signed summary.  The root is always a target; a bounded
                // rotating window of other members receives the same summary
                // so membership evidence spreads without a central directory
                // and without one record per peer per interval.
                if let Some((summary, root, snapshot)) = self.join.as_mut().and_then(|join| {
                    let summary = join.member_summary()?;
                    Some((summary, join.root(), join.snapshot()))
                }) {
                    let mut targets: alloc::vec::Vec<NodeId> =
                        alloc::vec::Vec::with_capacity(GOSSIP_FANOUT);
                    targets.push(root);
                    let peers = gossip_targets(
                        self.self_id,
                        &snapshot.members,
                        &[root],
                        self.gossip_round,
                        GOSSIP_FANOUT.saturating_sub(1),
                    );
                    self.gossip_round = self.gossip_round.wrapping_add(1);
                    targets.extend(peers);
                    targets.sort_unstable();
                    targets.dedup();
                    for target in targets {
                        if self.send_join(
                            services,
                            JoinMessage::Summary(summary),
                            target,
                            now,
                            false,
                        ) {
                            self.gossip_summaries_sent =
                                self.gossip_summaries_sent.saturating_add(1);
                        }
                    }
                }
            }
            Some((Tick::TimedOut, attempts)) if !self.join_timeout_logged => {
                self.join_timeout_logged = true;
                let mut line = Line::new();
                line.str("mesh-join: timeout attempts=");
                line.u64(attempts as u64);
                line.str(" state=pending\n");
                log(services, line.as_bytes());
            }
            _ => {}
        }

        let route_step = if let Some(join) = self.join.as_ref() {
            let signer = JoinSigner(join);
            self.route.as_mut().map(|route| route.tick(now, &signer))
        } else {
            None
        };
        if let Some(step) = route_step {
            self.dispatch_route(services, step, now);
        }

        let migrate_step = self.migrate.as_mut().map(|migrate| migrate.tick(now));
        if let Some(step) = migrate_step {
            self.dispatch_migrate(services, step, now);
        }

        self.latency_tick(services, now);

        // Bulk measurement traffic is data-class: it can use the send-queue
        // space above the control reserve, one chunk per tick.  Control work
        // above has already been queued, so a chunk cannot delay it.
        self.bulk_tick(services, now);

        // Root anti-entropy: re-send the current signed aggregate to a
        // rotating bounded window.  The object is not re-signed, so a lost
        // push costs bytes, not another Ed25519 signature.
        //
        // The interval restarts whether or not a resync is due.  A two-member
        // view has nobody to resync, and testing `snapshot()` on every tick
        // would clone the member list each time; the x86 kernel's bump heap
        // never frees those copies.
        if self.join.as_ref().map(|join| join.role()) == Some(Role::Root)
            && now.saturating_sub(self.last_gossip_ms) >= GOSSIP_RESYNC_MS
        {
            self.last_gossip_ms = now;
            if self
                .join
                .as_ref()
                .is_some_and(|join| join.snapshot().len() > 2)
            {
                if let Some(summary) = self
                    .join
                    .as_ref()
                    .and_then(|join| join.current_aggregate())
                    .cloned()
                {
                    self.gossip_epoch(services, &summary, None, now, true);
                }
            }
        }

        // One heartbeat per link.  The authority link and each gossip peer
        // have independent clocks, so a quiet member cannot suppress the
        // root's heartbeat to a healthy one.
        let mut pings: alloc::vec::Vec<NodeId> = alloc::vec::Vec::new();
        for link in self.links.iter() {
            if link.heartbeat.due(now) {
                pings.push(if link.peer.is_zero() {
                    NodeId::ZERO
                } else {
                    link.peer
                });
            }
        }
        for destination in pings {
            if self.queue_ping(
                now,
                MessageKind::Control,
                OpCode::Ping,
                STREAM_CONTROL,
                destination,
                0,
                0,
                Tag(0),
            ) {
                self.stats.heartbeats_sent = self.stats.heartbeats_sent.saturating_add(1);
                if let Some(link) = self.link_mut(destination) {
                    link.heartbeat.on_transmit(now);
                }
            }
        }
        let mut expired = 0usize;
        for link in self.links.iter_mut() {
            expired += link.pending.expire(now);
        }
        self.stats.expired_tags = self.stats.expired_tags.saturating_add(expired as u64);
        self.flush(services, now);
        self.maybe_log_pass(services);
    }

    /// Drop silent gossip links while at least one link remains.
    fn expire_silent_links(&mut self, services: &Services, now: u64) {
        let mut silent: alloc::vec::Vec<NodeId> = alloc::vec::Vec::new();
        for (index, link) in self.links.iter().enumerate() {
            if Some(index) == self.primary_link || link.peer.is_zero() {
                continue;
            }
            if link.heartbeat.timed_out(now) {
                silent.push(link.peer);
            }
        }
        if silent.len() >= self.link_count().saturating_sub(1) {
            // Keep the last link; the session heartbeat below decides whether
            // the whole session is dead.
            return;
        }
        for peer in silent {
            let mut line = Line::new();
            line.str("mesh-link: expired peer=");
            line.node(&peer);
            line.push(b'\n');
            log(services, line.as_bytes());
            self.forget_link(peer);
        }
    }

    /// Queue one root-signed aggregate to a bounded deterministic window.
    ///
    /// `resync` re-sends an already-forwarded epoch to a rotating window for
    /// anti-entropy.  Otherwise each node forwards a given epoch once, so the
    /// work per change set is `O(N * FANOUT)` records, not an `O(N)` broadcast
    /// of an `O(N)` object by the root.
    fn gossip_epoch(
        &mut self,
        services: &Services,
        summary: &mesh_join::EpochSummary,
        exclude: Option<NodeId>,
        now: u64,
        resync: bool,
    ) {
        let Some(snapshot) = self.join.as_ref().map(|join| join.snapshot()) else {
            return;
        };
        if !snapshot.members.contains(&self.self_id) {
            return;
        }
        if !resync && summary.epoch <= self.gossip_floor {
            return;
        }
        let mut excludes: alloc::vec::Vec<NodeId> = alloc::vec::Vec::new();
        if let Some(source) = exclude {
            excludes.push(source);
        }
        // The root is the writer; a member forwards to peers only.  Sending an
        // aggregate back to the root would be an unexpected message there.
        let member = self.join.as_ref().map(|join| join.role()) == Some(Role::Member);
        let root = self
            .join
            .as_ref()
            .map(|join| join.root())
            .unwrap_or(NodeId::ZERO);
        if member && !root.is_zero() {
            excludes.push(root);
        }
        let targets = gossip_targets(
            self.self_id,
            &snapshot.members,
            &excludes,
            self.gossip_round,
            GOSSIP_FANOUT,
        );
        self.gossip_round = self.gossip_round.wrapping_add(1);
        let mut sent = 0usize;
        for target in targets {
            if !resync && self.link_sent_epoch(target) >= summary.epoch {
                continue;
            }
            if self.send_join(
                services,
                JoinMessage::Epoch(summary.clone()),
                target,
                now,
                false,
            ) {
                self.mark_sent_epoch(target, summary.epoch);
                sent += 1;
            }
        }
        if sent == 0 {
            return;
        }
        if summary.epoch > self.gossip_floor {
            self.gossip_floor = summary.epoch;
        }
        self.gossip_aggregates_sent = self.gossip_aggregates_sent.saturating_add(sent as u64);
        let forwarded = member;
        if forwarded {
            self.gossip_aggregates_forwarded =
                self.gossip_aggregates_forwarded.saturating_add(sent as u64);
        }
        let mut line = Line::new();
        line.str("mesh-gossip: aggregate epoch=");
        line.u64(summary.epoch as u64);
        line.str(" members=");
        line.u64(summary.grants.len() as u64);
        line.str(" targets=");
        line.u64(sent as u64);
        line.str(match forwarded {
            true => " forward=1",
            false => " forward=0",
        });
        line.push(b'\n');
        log(services, line.as_bytes());
    }

    /// Forward the aggregate this member installed, once per epoch.
    fn maybe_forward_aggregate(&mut self, services: &Services, source: NodeId, now: u64) {
        if self.join.as_ref().map(|join| join.role()) != Some(Role::Member) {
            return;
        }
        let Some(summary) = self
            .join
            .as_ref()
            .and_then(|join| join.current_aggregate())
            .cloned()
        else {
            return;
        };
        self.gossip_epoch(services, &summary, Some(source), now, false);
    }

    fn send_handshake(&mut self, now: u64) {
        let Some(index) = self.primary_link else {
            return;
        };
        let Ok(tag) = self
            .links
            .get_mut(index)
            .map(|link| {
                link.pending
                    .begin_at(now.saturating_add(RECEIVE_DEADLINE_MS))
            })
            .unwrap_or(Err(TransportError::NoTag))
        else {
            return;
        };
        if self.queue_ping(
            now,
            MessageKind::Request,
            OpCode::Ping,
            STREAM_CONTROL,
            NodeId::ZERO,
            tag.0 as u64,
            0,
            tag,
        ) {
            self.stats.heartbeats_sent = self.stats.heartbeats_sent.saturating_add(1);
            if let Some(link) = self.links.get_mut(index) {
                link.heartbeat.on_transmit(now);
            }
        } else if let Some(link) = self.links.get_mut(index) {
            let _ = link.pending.complete(tag);
        }
    }

    fn feed(&mut self, services: &Services, bytes: &[u8], now: u64) -> Result<(), TransportError> {
        let mut offset = 0usize;
        loop {
            if let Some(record) = self.decoder.next_record()? {
                let incoming = {
                    let envelope = decode_record(record)?;
                    let body = envelope.frame.body;
                    let limit =
                        body_limit(envelope.frame.header.kind, envelope.frame.header.opcode);
                    if body.len() > limit {
                        return Err(TransportError::Envelope(sys_primitives::Error::BadLength));
                    }
                    // SAFETY: single-threaded module; FRAME_RX is the only
                    // frame-body buffer and the copied slice is read before
                    // the next record is decoded.
                    let storage = unsafe { &mut *core::ptr::addr_of_mut!(FRAME_RX) };
                    storage[..body.len()].copy_from_slice(body);
                    let frame_header = envelope.frame.header;
                    let trace = if frame_header.flags & sys_primitives::flags::TRACE != 0 {
                        Some(TraceContext {
                            trace_id: frame_header.trace_id,
                            span_id: frame_header.span_id,
                            flags: frame_header.trace_flags,
                        })
                    } else {
                        None
                    };
                    Incoming {
                        header: envelope.header,
                        trace,
                        object: frame_header.object,
                        status: frame_header.status,
                        kind: frame_header.kind,
                        opcode: frame_header.opcode,
                        stream: frame_header.stream.0,
                        body_len: body.len(),
                    }
                };
                self.decoder.consume_record()?;
                self.stats.frames_received = self.stats.frames_received.saturating_add(1);
                self.handle(services, incoming, now)?;
                // A socket read can contain several records.  Process every
                // complete record before pushing more bytes, so a burst cannot
                // strand a queued record behind the next read.
                continue;
            }
            if offset >= bytes.len() {
                return Ok(());
            }
            let used = self.decoder.push(&bytes[offset..])?;
            if used == 0 {
                return Ok(());
            }
            offset += used;
        }
    }

    fn handle(
        &mut self,
        services: &Services,
        incoming: Incoming,
        now: u64,
    ) -> Result<(), TransportError> {
        if incoming.header.src.is_zero() || incoming.header.src == self.self_id {
            self.stats.protocol_rejects = self.stats.protocol_rejects.saturating_add(1);
            return Err(TransportError::Envelope(sys_primitives::Error::Protocol));
        }
        if !incoming.header.dst.is_zero() && incoming.header.dst != self.self_id {
            self.stats.protocol_rejects = self.stats.protocol_rejects.saturating_add(1);
            return Err(TransportError::Envelope(sys_primitives::Error::Protocol));
        }
        let source = incoming.header.src;
        let body = body_bytes(incoming.body_len);
        match (incoming.kind, incoming.opcode) {
            (MessageKind::Request, OpCode::Ping)
            | (MessageKind::Response, OpCode::Ping)
            | (MessageKind::Control, OpCode::Ping) => {
                if body.len() != 16 || body != source.as_bytes() {
                    self.stats.protocol_rejects = self.stats.protocol_rejects.saturating_add(1);
                    return Err(TransportError::Envelope(sys_primitives::Error::Protocol));
                }
            }
            (MessageKind::Request, OpCode::Hello)
            | (MessageKind::Response, OpCode::Hello)
            | (MessageKind::Completion, OpCode::Hello)
            | (MessageKind::Control, OpCode::Hello) => {
                if self.join.is_none() {
                    self.stats.protocol_rejects = self.stats.protocol_rejects.saturating_add(1);
                    return Err(TransportError::Envelope(sys_primitives::Error::Protocol));
                }
            }
            (MessageKind::Request, OpCode::Invoke)
            | (MessageKind::Response, OpCode::Invoke)
            | (MessageKind::Request, OpCode::RouteQuery)
            | (MessageKind::Response, OpCode::RouteQuery) => {
                if self.route.is_none() {
                    self.stats.protocol_rejects = self.stats.protocol_rejects.saturating_add(1);
                    return Err(TransportError::Envelope(sys_primitives::Error::Protocol));
                }
            }
            (MessageKind::Request, OpCode::Migrate)
            | (MessageKind::Response, OpCode::Migrate)
            | (MessageKind::Completion, OpCode::Migrate) => {
                if self.migrate.is_none() {
                    self.stats.protocol_rejects = self.stats.protocol_rejects.saturating_add(1);
                    return Err(TransportError::Envelope(sys_primitives::Error::Protocol));
                }
            }
            (MessageKind::Request, OpCode::Read)
            | (MessageKind::Response, OpCode::Read)
            | (MessageKind::Control, OpCode::WindowUpdate)
            | (MessageKind::Cancel, OpCode::Cancel) => {}
            _ => {
                self.stats.protocol_rejects = self.stats.protocol_rejects.saturating_add(1);
                return Err(TransportError::Envelope(sys_primitives::Error::Protocol));
            }
        }
        let stream_ok = match incoming.opcode {
            OpCode::Ping => matches!(incoming.stream, STREAM_CONTROL | STREAM_LATENCY),
            OpCode::Hello => incoming.stream == STREAM_JOIN,
            OpCode::Invoke | OpCode::RouteQuery => incoming.stream == STREAM_ROUTE,
            OpCode::Migrate => incoming.stream == STREAM_MIGRATE,
            OpCode::Read => {
                incoming.stream != 0
                    && !self
                        .primary_streams()
                        .and_then(|streams| streams.stream(incoming.stream))
                        .is_some_and(|stream| stream.control)
            }
            OpCode::WindowUpdate | OpCode::Cancel => incoming.stream != 0,
            _ => false,
        };
        if !stream_ok {
            self.stats.protocol_rejects = self.stats.protocol_rejects.saturating_add(1);
            return Err(TransportError::Envelope(sys_primitives::Error::Protocol));
        }
        self.learn_peer(services, source, now, incoming.kind, incoming.opcode)?;
        if let Some(link) = self.link_mut(source) {
            link.confirmed = true;
            link.last_rx_ms = now;
            link.heartbeat.on_receive(now);
        }
        // Only the configured authority renews a member's write lease.  A
        // frame from another member proves that member is alive, not that the
        // authority still holds the view.
        let authority = self.join.as_ref().map(|join| join.root());
        if authority == Some(source) {
            if let Some(route) = self.route.as_mut() {
                route.on_peer_frame(now);
            }
        }
        let rejoin = if let Some(join) = self.join.as_mut() {
            let was_current = join.authority_current();
            if let Some(JoinEvent::Recovered { node, epoch }) = join.observe_peer(source, now) {
                let mut line = Line::new();
                line.str("mesh-fail: recovered node=");
                line.node(&node);
                line.str(" epoch=");
                line.u64(epoch as u64);
                line.push(b'\n');
                log(services, line.as_bytes());
            }
            // Only a non-join frame triggers a fresh request.  A join
            // confirmation must be processed against the request it answers;
            // creating a new request first would make its nonce mismatch.
            if !was_current && source == join.root() && incoming.opcode != OpCode::Hello {
                join.request_rejoin(now)
            } else {
                None
            }
        } else {
            None
        };
        if let Some(request) = rejoin {
            self.send_join(
                services,
                JoinMessage::Request(request),
                NodeId::ZERO,
                now,
                self.verbose_join_log(),
            );
        }

        match (incoming.kind, incoming.opcode) {
            (MessageKind::Request, OpCode::Ping) => {
                self.queue_ping(
                    now,
                    MessageKind::Response,
                    OpCode::Ping,
                    incoming.stream,
                    source,
                    0,
                    incoming.header.request_id,
                    Tag(0),
                );
            }
            (MessageKind::Response, OpCode::Ping) => {
                let reply_to = incoming.header.reply_to as u32;
                let tag = Tag(reply_to);
                if let Some(link) = self.link_mut(source) {
                    let _ = link.pending.complete(tag);
                }
                self.peer_confirmed = true;
                self.on_latency(reply_to, now);
            }
            (MessageKind::Control, OpCode::Ping) => {}
            (MessageKind::Request, OpCode::Hello)
            | (MessageKind::Response, OpCode::Hello)
            | (MessageKind::Completion, OpCode::Hello)
            | (MessageKind::Control, OpCode::Hello) => {
                let message = match JoinMessage::decode(body) {
                    Ok(message) => message,
                    Err(error) => {
                        self.stats.protocol_rejects = self.stats.protocol_rejects.saturating_add(1);
                        let mut line = Line::new();
                        line.str("mesh-join: reject reason=");
                        line.str(error.as_str());
                        line.push(b'\n');
                        log(services, line.as_bytes());
                        return Ok(());
                    }
                };
                if message.expected_kind() != incoming.kind {
                    let mut line = Line::new();
                    line.str("mesh-join: reject reason=kind-mismatch body=");
                    line.str(message.kind());
                    line.push(b'\n');
                    log(services, line.as_bytes());
                    return Ok(());
                }
                let converged_before = self
                    .join
                    .as_ref()
                    .map(|join| join.converged().is_some())
                    .unwrap_or(false);
                let before = self
                    .join
                    .as_ref()
                    .map(|join| join.authority_current())
                    .unwrap_or(false);
                let epoch_before = self.join.as_ref().map(|join| join.epoch()).unwrap_or(0);
                let stored_confirmation = match &message {
                    JoinMessage::Confirm(confirmation) => Some(confirmation.clone()),
                    _ => None,
                };
                let summary = match &message {
                    JoinMessage::Summary(summary) => Some(summary.counter),
                    _ => None,
                };
                let outcome = match &mut self.join {
                    Some(join) => join.on_message(source, message, now),
                    None => return Ok(()),
                };
                if let Some(counter) = summary {
                    if !matches!(outcome.event, JoinEvent::Rejected(_)) {
                        if let Some(link) = self.link_mut(source) {
                            if counter > link.summary_counter {
                                link.summary_counter = counter;
                            }
                        }
                        self.gossip_summaries_received =
                            self.gossip_summaries_received.saturating_add(1);
                    }
                }
                if let Some(confirmation) = stored_confirmation {
                    let accepted = match &outcome.event {
                        JoinEvent::Converged(snapshot)
                        | JoinEvent::Duplicate(snapshot)
                        | JoinEvent::Refreshed(snapshot) => {
                            snapshot.epoch == confirmation.epoch
                                && snapshot.members == confirmation.members
                        }
                        _ => false,
                    };
                    if accepted {
                        self.persist_confirmation(services, &confirmation);
                    }
                }
                let full_view = self
                    .join
                    .as_ref()
                    .map(|join| join.epoch() > epoch_before)
                    .unwrap_or(false);
                let after = self
                    .join
                    .as_ref()
                    .map(|join| join.authority_current())
                    .unwrap_or(false);
                if converged_before && !before && after {
                    if let Some(join) = self.join.as_ref() {
                        let mut line = Line::new();
                        line.str("mesh-fail: authority state=restored epoch=");
                        line.u64(join.epoch() as u64);
                        line.str(" mutable=1\n");
                        log(services, line.as_bytes());
                    }
                    if let Some(route) = self.route.as_mut() {
                        if let Some(event) = route.on_authority_restored() {
                            route.note_event(&event);
                            log_route_event(services, &event);
                        }
                    }
                }
                self.after_join(services, outcome, source, now, full_view);
            }
            (MessageKind::Request, OpCode::Invoke)
            | (MessageKind::Response, OpCode::Invoke)
            | (MessageKind::Request, OpCode::RouteQuery)
            | (MessageKind::Response, OpCode::RouteQuery) => {
                let step = if let Some(join) = self.join.as_ref() {
                    let signer = JoinSigner(join);
                    self.route.as_mut().map(|route| {
                        route.on_wire(
                            source,
                            &incoming.header,
                            incoming.trace,
                            incoming.object,
                            incoming.kind,
                            incoming.opcode,
                            incoming.status,
                            body,
                            now,
                            &signer,
                        )
                    })
                } else {
                    None
                };
                if let Some(step) = step {
                    self.dispatch_route(services, step, now);
                }
            }
            (MessageKind::Request, OpCode::Migrate)
            | (MessageKind::Response, OpCode::Migrate)
            | (MessageKind::Completion, OpCode::Migrate) => {
                let step = if let Some(join) = self.join.as_ref() {
                    let signer = JoinSigner(join);
                    self.migrate.as_mut().map(|migrate| {
                        migrate.on_wire(
                            source,
                            &incoming.header,
                            incoming.kind,
                            incoming.status,
                            body,
                            now,
                            &signer,
                        )
                    })
                } else {
                    None
                };
                if let Some(step) = step {
                    self.dispatch_migrate(services, step, now);
                }
            }
            (MessageKind::Request, OpCode::Read) => {
                self.on_bulk_request(services, source, incoming.stream, body, now)?;
            }
            (MessageKind::Response, OpCode::Read) => {
                self.on_bulk_chunk(services, source, incoming.stream, body, now)?;
            }
            (MessageKind::Control, OpCode::WindowUpdate) => {
                self.on_window_update(body)?;
            }
            (MessageKind::Cancel, OpCode::Cancel) => {
                self.on_cancel(services, body, now)?;
            }
            _ => {}
        }
        Ok(())
    }

    fn dispatch_route(&mut self, services: &Services, step: RouteStep, now: u64) {
        for event in &step.events {
            if let Some(route) = self.route.as_mut() {
                route.note_event(event);
            }
            log_route_event(services, event);
        }
        if let Some(wire) = step.send {
            if !self.queue_route(wire, now) {
                log(services, b"mesh-route: send-failed\n");
            }
        }
    }

    fn queue_route(&mut self, wire: RouteWire, now: u64) -> bool {
        let Ok(mut frame) = MessageFrame::inline(
            wire.kind,
            wire.opcode,
            StreamId(STREAM_ROUTE),
            Tag(0),
            &wire.body,
        ) else {
            return false;
        };
        frame.header.object = wire.object;
        frame.header.status = wire.status;
        wire.apply_trace(&mut frame.header);
        let header = wire.header(self.self_id);
        let frame_header = frame.header;
        self.stage_control(STREAM_ROUTE, header, frame_header, &wire.body, now)
    }

    fn dispatch_migrate(&mut self, services: &Services, step: MigrateStep, now: u64) {
        for event in &step.events {
            log_migrate_event(services, event);
        }
        if let Some(migrate) = self.migrate.as_mut() {
            migrate.build_report();
        }
        if let Some(wire) = step.send {
            if !self.queue_migrate(wire, now) {
                log(services, b"mesh-migrate: send-failed\n");
            }
        }
    }

    fn queue_migrate(&mut self, wire: MigrateWire, now: u64) -> bool {
        let Ok(mut frame) = MessageFrame::inline(
            wire.kind,
            OpCode::Migrate,
            StreamId(STREAM_MIGRATE),
            Tag(0),
            &wire.body,
        ) else {
            return false;
        };
        frame.header.object = mesh_migrate::actor_object(&wire.actor);
        frame.header.status = wire.status;
        let header = wire.header(self.self_id);
        let frame_header = frame.header;
        self.stage_control(STREAM_MIGRATE, header, frame_header, &wire.body, now)
    }

    fn after_join(
        &mut self,
        services: &Services,
        outcome: mesh_join::Outcome,
        source: NodeId,
        now: u64,
        full_view: bool,
    ) {
        // Reply before writing the bounded serial evidence.  The root confirms
        // each member before it logs the resulting home table; otherwise the
        // confirm waits behind a serial burst, and a late member can exhaust
        // its retry budget before the root has finished logging the previous
        // epoch.  That ordering was the multi-member join defect the host hub
        // exposed.  The wire message and the log lines are unchanged.
        if let Some(reply) = outcome.reply {
            self.send_join(services, reply, source, now, self.verbose_join_log());
        }
        match &outcome.event {
            JoinEvent::Confirmed(snapshot) => {
                // A routine refresh from an existing member does not advance
                // the epoch.  The M2 transcript keeps the signed confirmation
                // line; tables are logged on a new link, an epoch change or
                // the 30 s rate-limited refresh so the ~50 byte/s serial line
                // cannot starve the receive path.
                let new_link = self.link_generation != self.homes_logged_generation;
                if full_view {
                    self.log_snapshot(services, "mesh-join: converged role=root", snapshot);
                    self.last_join_log_ms = now;
                    self.logged_join_epoch = snapshot.epoch;
                    self.homes_logged_generation = self.link_generation;
                    if !self.m3_quiet() && snapshot.len() <= 2 {
                        self.log_snapshot(services, "mesh-join: confirm", snapshot);
                        self.log_homes(services, snapshot, true);
                        self.log_shard(services, snapshot, false);
                    }
                } else if self.bulk_active() {
                    // Keep the serial console out of a bulk measurement; the
                    // confirmation itself is still sent by the join machine.
                } else if new_link {
                    self.last_join_log_ms = now;
                    self.logged_join_epoch = snapshot.epoch;
                    self.homes_logged_generation = self.link_generation;
                    if !self.m3_quiet() && snapshot.len() <= 2 {
                        self.log_snapshot(services, "mesh-join: confirm", snapshot);
                        self.log_homes(services, snapshot, true);
                        self.log_shard(services, snapshot, false);
                    }
                } else if !self.m3_quiet()
                    && snapshot.len() <= 2
                    && (snapshot.epoch != self.logged_join_epoch
                        || now.saturating_sub(self.last_join_log_ms) >= JOIN_LOG_INTERVAL_MS)
                {
                    self.last_join_log_ms = now;
                    self.logged_join_epoch = snapshot.epoch;
                    self.log_snapshot(services, "mesh-join: confirm", snapshot);
                    self.log_snapshot(services, "mesh-join: duplicate", snapshot);
                }
                self.on_route_snapshot(services, snapshot, now);
            }
            JoinEvent::Converged(snapshot) => {
                self.join_timeout_logged = false;
                self.log_snapshot(services, "mesh-join: converged role=member", snapshot);
                self.last_join_log_ms = now;
                if !self.m3_quiet() && snapshot.len() <= 2 {
                    self.log_homes(services, snapshot, true);
                    self.log_shard(services, snapshot, false);
                }
                self.logged_join_epoch = snapshot.epoch;
                self.homes_logged_generation = self.link_generation;
                self.on_route_snapshot(services, snapshot, now);
            }
            JoinEvent::Duplicate(snapshot) => {
                if !self.m3_quiet() && snapshot.len() <= 2 {
                    self.last_join_log_ms = now;
                    self.log_snapshot(services, "mesh-join: duplicate", snapshot);
                    self.logged_join_epoch = snapshot.epoch;
                    // After a restart the member gets Duplicate (not Converged)
                    // because the epoch already matches. Log homes so the
                    // restart gate can verify the home table survived.
                    if self.link_generation != self.homes_logged_generation {
                        self.log_homes(services, snapshot, true);
                        self.log_shard(services, snapshot, false);
                        self.homes_logged_generation = self.link_generation;
                    }
                }
                self.on_route_snapshot(services, snapshot, now);
            }
            JoinEvent::Refreshed(snapshot) => {
                // Routine M3 refresh: another member joined and advanced the
                // epoch.  Above two members the serial line is compact and
                // the table is read over HTTP.
                if now.saturating_sub(self.last_join_log_ms) >= JOIN_LOG_INTERVAL_MS {
                    self.last_join_log_ms = now;
                    let mut line = Line::new();
                    line.str("mesh-join: refreshed epoch=");
                    line.u64(snapshot.epoch as u64);
                    line.str(" members=");
                    line.u64(snapshot.len() as u64);
                    line.str(" digest=");
                    line.digest(&snapshot.digest());
                    line.push(b'\n');
                    log(services, line.as_bytes());
                    if !self.m3_quiet() && snapshot.len() <= 2 {
                        self.log_homes(services, snapshot, true);
                        self.log_shard(services, snapshot, false);
                    }
                }
                self.on_route_snapshot(services, snapshot, now);
            }
            JoinEvent::Refused { reason, epoch } => {
                let mut line = Line::new();
                line.str("mesh-join: refused reason=");
                line.str(reason.as_str());
                line.str(" root_epoch=");
                line.u64(*epoch as u64);
                line.push(b'\n');
                log(services, line.as_bytes());
            }
            JoinEvent::Acked {
                node,
                epoch,
                digest,
            } if self.verbose_join_log() => {
                self.last_ack_log_ms = now;
                let mut line = Line::new();
                line.str("mesh-join: ack from=");
                line.node(node);
                line.str(" epoch=");
                line.u64(*epoch as u64);
                line.str(" digest=");
                line.digest(digest);
                line.push(b'\n');
                log(services, line.as_bytes());
                self.logged_ack_epoch = *epoch;
                self.last_ack_log_ms = now;
            }
            JoinEvent::Rejected(label) if self.verbose_join_log() => {
                // The envelope and frame were well formed; the membership
                // policy rejected the body.  Keep this out of the transport
                // reject counter, which the M2-1 pass line reports.
                let mut line = Line::new();
                line.str("mesh-join: reject reason=");
                line.str(label);
                line.push(b'\n');
                log(services, line.as_bytes());
            }
            // A compact multi-member view suppresses per-ack and per-reject
            // lines; the protocol handling above is unchanged.
            JoinEvent::Acked { .. } | JoinEvent::Rejected(_) => {}
            JoinEvent::Removed { node, snapshot } => {
                let mut line = Line::new();
                line.str("mesh-join: removed node=");
                line.node(node);
                line.str(" epoch=");
                line.u64(snapshot.epoch as u64);
                line.push(b'\n');
                log(services, line.as_bytes());
                self.persist_root(services);
                self.on_route_snapshot(services, snapshot, now);
            }
            JoinEvent::RemovedBatch { nodes, snapshot } => {
                for node in nodes {
                    let mut line = Line::new();
                    line.str("mesh-join: removed node=");
                    line.node(node);
                    line.str(" epoch=");
                    line.u64(snapshot.epoch as u64);
                    line.push(b'\n');
                    log(services, line.as_bytes());
                }
                self.persist_root(services);
                self.on_route_snapshot(services, snapshot, now);
            }
            JoinEvent::Suspect { .. }
            | JoinEvent::Failed { .. }
            | JoinEvent::Recovered { .. }
            | JoinEvent::AuthorityLost { .. }
            | JoinEvent::None => {}
        }

        // Root: sign at most one aggregate for a changed view and seed it to a
        // bounded deterministic window.  Members forward the same signed
        // object once per epoch.  `publish_aggregate` returns owned data, so
        // the mutable borrow on `self.join` ends before gossip needs `self`.
        let is_root = self.join.as_ref().map(|join| join.role()) == Some(Role::Root);
        if is_root {
            match &outcome.event {
                JoinEvent::Confirmed { .. } | JoinEvent::RemovedBatch { .. } => {
                    if let Some(aggregate) =
                        self.join.as_mut().and_then(|join| join.publish_aggregate())
                    {
                        self.gossip_epoch(services, &aggregate, None, now, false);
                    }
                }
                _ => {}
            }
            match &outcome.event {
                JoinEvent::Removed { node, .. } => self.forget_link(*node),
                JoinEvent::RemovedBatch { nodes, .. } => {
                    for node in nodes {
                        self.forget_link(*node);
                    }
                }
                _ => {}
            }
        } else {
            // A member that installed an aggregate forwards it once.  The
            // source is excluded so the record does not echo to its sender.
            self.maybe_forward_aggregate(services, source, now);
        }
    }

    /// Whether the detailed M2 transcript is affordable for this view.
    ///
    /// Two members keep every line the M2 runbook reads.  Above two members
    /// the emulated serial line is the bottleneck, so only compact epoch
    /// lines are written and the tables are read from the MCP HTTP surface.
    /// The receive deadline for a partial record.
    ///
    /// The iris host forward can trickle the tail of a record through its
    /// zero-window probe when the guest's receive window shrinks to a few
    /// bytes.  The M2 10 s deadline is correct for a direct two-node relay;
    /// an M3 shard run uses a longer bounded deadline so an 18-byte trickle
    /// can finish instead of tearing down a healthy link.
    fn receive_deadline_ms(&self) -> u64 {
        if self.shard_count > 0 {
            120_000
        } else {
            RECEIVE_DEADLINE_MS
        }
    }

    /// Persist the root's confirmed view when it changes.
    ///
    /// The record is signed by the authority key, so a restart can verify it
    /// before installing it.  Routine duplicate confirmations (the 5 s
    /// refresh) are skipped: only an epoch or member-list change reaches the
    /// disk.
    fn persist_root(&mut self, services: &Services) {
        let (Some(join), Some(store)) = (self.join.as_ref(), self.store.as_mut()) else {
            return;
        };
        if !store.available || !store.root {
            return;
        }
        let record = match join.epoch_record() {
            Ok(record) => record,
            Err(error) => {
                let mut line = Line::new();
                line.str("epoch-store: sign-failed reason=");
                line.str(error.as_str());
                line.push(b'\n');
                log(services, line.as_bytes());
                return;
            }
        };
        let snapshot = record.snapshot();
        let digest = snapshot.digest();
        if store.matches(record.epoch, &digest) {
            return;
        }
        // Never overwrite a confirmed epoch with a same-epoch view that lost a
        // failed-but-not-yet-removed member.  That transient view would make a
        // restart equivocate with a member's stored confirmation.  A removal
        // advances the epoch and is written normally.
        if store.epoch != 0
            && (record.epoch < store.epoch
                || (record.epoch == store.epoch && digest != store.digest))
        {
            log(services, b"epoch-store: skip unstable same-epoch view\n");
            return;
        }
        // SAFETY: single-threaded module; STORE_BODY is only used here and in
        // the member persistence path, never concurrently.
        let body = unsafe { &mut *core::ptr::addr_of_mut!(STORE_BODY) };
        let length = match record.encode(body) {
            Ok(length) => length,
            Err(error) => {
                let mut line = Line::new();
                line.str("epoch-store: encode-failed reason=");
                line.str(error.as_str());
                line.push(b'\n');
                log(services, line.as_bytes());
                return;
            }
        };
        match write_epoch_store(services, STORE_KIND_RECORD, &body[..length]) {
            Ok(()) => {
                store.remember(record.epoch, digest);
                let mut line = Line::new();
                line.str("epoch-store: stored epoch=");
                line.u64(record.epoch as u64);
                line.str(" members=");
                line.u64(snapshot.len() as u64);
                line.str(" digest=");
                line.digest(&digest);
                line.str(" source=root-record\n");
                log(services, line.as_bytes());
            }
            Err(reason) => {
                let mut line = Line::new();
                line.str("epoch-store: write-failed reason=");
                line.str(reason);
                line.push(b'\n');
                log(services, line.as_bytes());
            }
        }
    }

    /// Persist the signed confirmation a member installed.
    ///
    /// A member cannot sign its own epoch, so it stores the root's signed
    /// confirmation body.  A restart verifies it with `mesh.pub` before
    /// installing it and then presents the confirmed `last_epoch` to the
    /// root.  Routine duplicate refreshes are skipped.
    fn persist_confirmation(&mut self, services: &Services, confirmation: &EpochConfirmation) {
        let Some(store) = self.store.as_mut() else {
            return;
        };
        if !store.available || store.root {
            return;
        }
        let snapshot = confirmation.snapshot();
        let digest = snapshot.digest();
        if store.matches(confirmation.epoch, &digest) {
            return;
        }
        // The verifier already rejects a same-epoch different list; keep the
        // disk guard anyway so a bug in the caller cannot overwrite a stable
        // record with a transient view.
        if store.epoch != 0
            && (confirmation.epoch < store.epoch
                || (confirmation.epoch == store.epoch && digest != store.digest))
        {
            log(services, b"epoch-store: skip unstable same-epoch view\n");
            return;
        }
        // SAFETY: single-threaded module; STORE_BODY is only used here and in
        // the root persistence path, never concurrently.
        let body = unsafe { &mut *core::ptr::addr_of_mut!(STORE_BODY) };
        let length = match confirmation.encode(body) {
            Ok(length) => length,
            Err(error) => {
                let mut line = Line::new();
                line.str("epoch-store: encode-failed reason=");
                line.str(error.as_str());
                line.push(b'\n');
                log(services, line.as_bytes());
                return;
            }
        };
        match write_epoch_store(services, STORE_KIND_CONFIRM, &body[..length]) {
            Ok(()) => {
                store.remember(confirmation.epoch, digest);
                let mut line = Line::new();
                line.str("epoch-store: stored epoch=");
                line.u64(confirmation.epoch as u64);
                line.str(" members=");
                line.u64(snapshot.len() as u64);
                line.str(" digest=");
                line.digest(&digest);
                line.str(" source=confirmation\n");
                log(services, line.as_bytes());
            }
            Err(reason) => {
                let mut line = Line::new();
                line.str("epoch-store: write-failed reason=");
                line.str(reason);
                line.push(b'\n');
                log(services, line.as_bytes());
            }
        }
    }

    fn verbose_join_log(&self) -> bool {
        if self.m3_quiet() {
            return false;
        }
        self.join
            .as_ref()
            .and_then(|join| join.converged())
            .map(|snapshot| snapshot.len() <= 2)
            .unwrap_or(true)
    }

    /// Whether this boot is the opt-in M3 shard surface.
    ///
    /// An M3 shard run asks for a 32-object `/mesh/shard` view and fetches the
    /// full tables over HTTP.  Its serial transcript is compact so the
    /// emulated line cannot starve the receive path.  The M2 run (no
    /// `mesh.shard` key) keeps the detailed transcript.
    fn m3_quiet(&self) -> bool {
        self.shard_count > 0
    }

    fn log_snapshot(
        &self,
        services: &Services,
        prefix: &str,
        snapshot: &mesh_join::MembershipSnapshot,
    ) {
        let mut line = Line::new();
        line.str(prefix);
        line.str(" epoch=");
        line.u64(snapshot.epoch as u64);
        line.str(" members=");
        line.u64(snapshot.len() as u64);
        line.str(" digest=");
        line.digest(&snapshot.digest());
        if self.m3_quiet() || snapshot.len() > 2 {
            line.str(" compact=1");
        } else {
            line.str(" snapshot=");
            line.nodes(&snapshot.members);
        }
        line.push(b'\n');
        log(services, line.as_bytes());
    }

    fn on_route_snapshot(&mut self, services: &Services, snapshot: &MembershipSnapshot, now: u64) {
        let verbose = self.verbose_join_log();
        if let Some(route) = self.route.as_mut() {
            if let Some(step) =
                route.on_snapshot(snapshot.epoch, &snapshot.members, snapshot.digest(), now)
            {
                for event in &step.events {
                    route.note_event(event);
                    // The route report is served by `/mesh/route`; above two
                    // members the serial fence rows would compete with the
                    // receive path for the same slow line.
                    if verbose {
                        log_route_event(services, event);
                    }
                }
            }
        }
        if let Some(migrate) = self.migrate.as_mut() {
            if let Some(step) =
                migrate.on_snapshot(snapshot.epoch, &snapshot.members, snapshot.digest())
            {
                for event in &step.events {
                    if verbose {
                        log_migrate_event(services, event);
                    }
                }
                migrate.build_report();
            }
        }
    }

    fn log_homes(&self, services: &Services, snapshot: &MembershipSnapshot, rows: bool) {
        let objects = fixture_objects(self.home_count);
        let Some(table) = HomeTable::new(&snapshot.view(), snapshot.digest(), &objects) else {
            return;
        };
        let mut header = Line::new();
        header.str("mesh-homes: state=current object_set=m2-v1 epoch=");
        header.u64(table.epoch as u64);
        header.str(" members=");
        header.u64(snapshot.len() as u64);
        header.str(" digest=");
        header.digest(&table.membership);
        header.str(" count=");
        header.u64(table.len() as u64);
        header.push(b'\n');
        log(services, header.as_bytes());
        if !rows {
            return;
        }
        for entry in &table.entries {
            let mut line = Line::new();
            line.str("mesh-home: object=");
            line.hex(entry.object.as_bytes());
            line.str(" home=");
            line.node(&entry.home);
            line.str(" epoch=");
            line.u64(table.epoch as u64);
            line.str(" digest=");
            line.digest(&table.membership);
            line.push(b'\n');
            log(services, line.as_bytes());
        }
    }

    /// Log the bounded M3 shard view from the same confirmed snapshot.
    ///
    /// `rows` is false for a routine refresh: the header still proves the node
    /// is on the current epoch and digest, but the per-object rows do not
    /// repeat every five seconds in the serial transcript.
    fn log_shard(&self, services: &Services, snapshot: &MembershipSnapshot, rows: bool) {
        if self.shard_count == 0 {
            return;
        }
        let objects = shard_objects(self.shard_count);
        let Some(table) = HomeTable::new(&snapshot.view(), snapshot.digest(), &objects) else {
            return;
        };
        let balance = table.balance();
        let owned = table.owned_by(self.self_id);
        let mut header = Line::new();
        header.str("mesh-shard: state=current object_set=m3-v1 epoch=");
        header.u64(table.epoch as u64);
        header.str(" members=");
        header.u64(snapshot.len() as u64);
        header.str(" digest=");
        header.digest(&table.membership);
        header.str(" count=");
        header.u64(table.len() as u64);
        header.str(" owners=");
        header.u64(balance.owners as u64);
        header.str(" owned=");
        header.u64(owned.len() as u64);
        header.str(" min=");
        header.u64(balance.min as u64);
        header.str(" max=");
        header.u64(balance.max as u64);
        header.str(" node=");
        header.node(&self.self_id);
        header.push(b'\n');
        log(services, header.as_bytes());
        if !rows {
            return;
        }
        for member in &snapshot.members {
            let mut line = Line::new();
            line.str("mesh-shard-owner: node=");
            line.node(member);
            line.str(" count=");
            line.u64(table.owned_count(*member) as u64);
            line.push(b'\n');
            log(services, line.as_bytes());
        }
        for entry in &owned {
            let mut line = Line::new();
            line.str("mesh-shard-owned: object=");
            line.hex(entry.object.as_bytes());
            line.str(" home=");
            line.node(&entry.home);
            line.str(" epoch=");
            line.u64(table.epoch as u64);
            line.str(" digest=");
            line.digest(&table.membership);
            line.push(b'\n');
            log(services, line.as_bytes());
        }
    }

    fn send_join(
        &mut self,
        services: &Services,
        message: JoinMessage,
        reply_to_source: NodeId,
        now: u64,
        verbose: bool,
    ) -> bool {
        if matches!(message, JoinMessage::Request(_)) {
            self.join_requests = self.join_requests.saturating_add(1);
        }
        // JOIN_TX holds one body.  If a join frame from earlier in this tick is
        // still staged, drain it into the transport queue before overwriting
        // the buffer; control frames stay ahead of any data in the scheduler.
        if self
            .staged
            .iter()
            .any(|frame| frame.is_some_and(|frame| frame.store == BodyStore::Join))
        {
            self.drain_stage(now);
            if self.stage_fault.is_some() {
                return false;
            }
        }
        // SAFETY: single-threaded module; JOIN_TX is the only encode buffer and
        // the bytes are copied into the send queue before the next message.
        let buffer = unsafe { &mut *core::ptr::addr_of_mut!(JOIN_TX) };
        let len = match message.encode(buffer) {
            Ok(len) => len,
            Err(error) => {
                let mut line = Line::new();
                line.str("mesh-join: encode-failed reason=");
                line.str(error.as_str());
                line.push(b'\n');
                log(services, line.as_bytes());
                return false;
            }
        };
        // The ack is sent by the member to the root, not to the member's own
        // `node` field.
        let ack_root = self
            .join
            .as_ref()
            .map(|join| join.root())
            .unwrap_or(NodeId::ZERO);
        let (kind, destination, request_id, reply_to) = match &message {
            JoinMessage::Request(request) => (MessageKind::Request, NodeId::ZERO, request.nonce, 0),
            JoinMessage::Confirm(confirmation) => (
                MessageKind::Response,
                confirmation.subject,
                confirmation.request_nonce,
                confirmation.request_nonce,
            ),
            JoinMessage::Refused(refusal) => (
                MessageKind::Response,
                reply_to_source,
                refusal.request_nonce,
                refusal.request_nonce,
            ),
            JoinMessage::Ack(ack) => (
                MessageKind::Completion,
                ack_root,
                ack.request_nonce,
                ack.request_nonce,
            ),
            JoinMessage::Summary(_) => (MessageKind::Control, reply_to_source, 0, 0),
            JoinMessage::Epoch(_) => (MessageKind::Control, reply_to_source, 0, 0),
        };
        if verbose {
            match &message {
                JoinMessage::Request(request) => {
                    let attempts = self.join.as_ref().map(|join| join.attempts()).unwrap_or(0);
                    let mut line = Line::new();
                    line.str("mesh-join: request from=");
                    line.node(&self.self_id);
                    line.str(" incarnation=");
                    line.u64(request.incarnation);
                    line.str(" last_epoch=");
                    line.u64(request.last_epoch as u64);
                    line.str(" last_digest=");
                    line.digest(&request.last_digest);
                    line.str(" nonce=");
                    line.u64(request.nonce);
                    line.str(" attempt=");
                    line.u64(attempts as u64);
                    line.push(b'\n');
                    log(services, line.as_bytes());
                }
                JoinMessage::Ack(ack) => {
                    self.last_ack_log_ms = now;
                    let mut line = Line::new();
                    line.str("mesh-join: ack-sent epoch=");
                    line.u64(ack.epoch as u64);
                    line.str(" digest=");
                    line.digest(&ack.digest);
                    line.push(b'\n');
                    log(services, line.as_bytes());
                }
                JoinMessage::Summary(summary) => {
                    let mut line = Line::new();
                    line.str("mesh-join: summary-sent epoch=");
                    line.u64(summary.epoch as u64);
                    line.str(" digest=");
                    line.digest(&summary.digest);
                    line.str(" counter=");
                    line.u64(summary.counter);
                    line.push(b'\n');
                    log(services, line.as_bytes());
                }
                JoinMessage::Epoch(summary) => {
                    let mut line = Line::new();
                    line.str("mesh-join: epoch-sent epoch=");
                    line.u64(summary.epoch as u64);
                    line.str(" members=");
                    line.u64(summary.grants.len() as u64);
                    line.push(b'\n');
                    log(services, line.as_bytes());
                }
                _ => {}
            }
        }
        let body = &buffer[..len];
        // A confirmation is the root's epoch decision.  Persist the signed
        // view **before** it reaches the wire, so a crash after the member
        // installs it cannot leave the root one epoch behind.  A duplicate
        // refresh is skipped by the epoch/digest comparison.
        if matches!(&message, JoinMessage::Confirm(_)) {
            self.persist_root(services);
        }
        if !self.stage_hello(kind, destination, request_id, reply_to, body, now) {
            let mut line = Line::new();
            line.str("mesh-join: send-failed kind=");
            line.str(message.kind());
            line.push(b'\n');
            log(services, line.as_bytes());
            return false;
        }
        true
    }

    #[allow(clippy::too_many_arguments)]
    fn stage_hello(
        &mut self,
        kind: MessageKind,
        destination: NodeId,
        request_id: u64,
        reply_to: u64,
        body: &[u8],
        now: u64,
    ) -> bool {
        let Ok(frame) =
            MessageFrame::inline(kind, OpCode::Hello, StreamId(STREAM_JOIN), Tag(0), body)
        else {
            return false;
        };
        let mut header = MeshHeader::new(self.self_id, destination, self.self_id);
        header.flags = mesh_flags::PROBE;
        if kind != MessageKind::Request {
            header.flags |= mesh_flags::REPLY;
        }
        header.request_id = request_id;
        header.reply_to = reply_to;
        header.ttl = if kind == MessageKind::Control {
            GOSSIP_TTL
        } else {
            MeshHeader::MAX_HOPS
        };
        let frame_header = frame.header;
        self.stage_control_join(STREAM_JOIN, header, frame_header, body, now)
    }

    /// The stream-allocator side for the link to `peer`.
    ///
    /// Dynamic stream IDs follow the lower `NodeId` as initiator.  An unbound
    /// link (the root's listener before the first frame) uses the direct
    /// connect role until `learn_peer` binds it.
    fn side_for(&self, peer: NodeId) -> Side {
        if peer.is_zero() {
            if self.direct_initiator {
                Side::Initiator
            } else {
                Side::Responder
            }
        } else if self.self_id < peer {
            Side::Initiator
        } else {
            Side::Responder
        }
    }

    fn learn_peer(
        &mut self,
        services: &Services,
        source: NodeId,
        now: u64,
        kind: MessageKind,
        opcode: OpCode,
    ) -> Result<(), TransportError> {
        if self.link_index(source).is_some() {
            return Ok(());
        }
        let role = self.join.as_ref().map(|join| join.role());
        let root = self.join.as_ref().map(|join| join.root());
        // A member accepts the authority plus confirmed members carrying
        // membership gossip.  Route, migration and bulk frames from a third
        // node are not a member link and are refused before they reach policy.
        if role == Some(Role::Member) && root != Some(source) {
            let gossip = matches!((kind, opcode), (MessageKind::Control, OpCode::Hello));
            let member = self
                .join
                .as_ref()
                .map(|join| join.snapshot().members.contains(&source))
                .unwrap_or(false);
            if !gossip || !member {
                self.stats.protocol_rejects = self.stats.protocol_rejects.saturating_add(1);
                return Err(TransportError::Envelope(sys_primitives::Error::Protocol));
            }
        }
        // The root's authority link is unbound until the first member frame.
        if self
            .primary_link
            .and_then(|index| self.links.get(index))
            .is_some_and(|link| link.peer.is_zero())
        {
            self.bind_primary(source);
            if let Some(route) = self.route.as_mut() {
                let _ = route.set_peer(source, now);
            }
            if let Some(migrate) = self.migrate.as_mut() {
                let _ = migrate.set_peer(source);
            }
            // Dynamic stream IDs follow the lower NodeId as initiator, so
            // both ends agree without a negotiation round trip.
            let side = self.side_for(source);
            if let Some(streams) = self.primary_streams_mut() {
                if streams.side() != side {
                    *streams = live_streams(side);
                    self.staged
                        .iter_mut()
                        .filter(|frame| frame.is_some_and(|frame| frame.link.is_zero()))
                        .for_each(|frame| *frame = None);
                    self.retire_stream = None;
                }
            }
        }
        if self.link_index(source).is_some() {
            return Ok(());
        }
        if self.link_count() >= MAX_MESH_LINKS {
            self.stats.protocol_rejects = self.stats.protocol_rejects.saturating_add(1);
            return Err(TransportError::Envelope(sys_primitives::Error::Protocol));
        }
        if self.ensure_link(source, now).is_none() {
            self.stats.protocol_rejects = self.stats.protocol_rejects.saturating_add(1);
            return Err(TransportError::Envelope(sys_primitives::Error::Protocol));
        }
        if let Some(link) = self.link_mut(source) {
            link.confirmed = true;
            link.last_rx_ms = now;
        }
        let mut line = Line::new();
        line.str("mesh-link: peer=");
        line.node(&source);
        line.str(" role=");
        line.str(match (role, root == Some(source)) {
            (Some(Role::Root), _) => "member",
            (Some(Role::Member), true) => "authority",
            (Some(Role::Member), false) => "gossip",
            _ => "peer",
        });
        line.str(" links=");
        line.u64(self.link_count() as u64);
        line.push(b'\n');
        log(services, line.as_bytes());
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    fn queue_ping(
        &mut self,
        now: u64,
        kind: MessageKind,
        opcode: OpCode,
        stream: u32,
        destination: NodeId,
        request_id: u64,
        reply_to: u64,
        tag: Tag,
    ) -> bool {
        let body = *self.self_id.as_bytes();
        let Ok(frame) = MessageFrame::inline(kind, opcode, StreamId(stream), tag, &body) else {
            return false;
        };
        let mut header = MeshHeader::new(self.self_id, destination, self.self_id);
        header.flags = mesh_flags::PROBE;
        if kind == MessageKind::Response {
            header.flags |= mesh_flags::REPLY;
        }
        header.request_id = request_id;
        header.reply_to = reply_to;
        header.ttl = MeshHeader::MAX_HOPS;
        let frame_header = frame.header;
        self.stage_control(stream, header, frame_header, &body, now)
    }

    fn start_latency(&mut self, count: u32, gap_ms: u32, now: u64) -> bool {
        if !matches!(self.phase, Phase::Connected) || self.peer.is_none() {
            return false;
        }
        let count = count.clamp(1, LATENCY_SAMPLES as u32);
        self.latency = Some(LatencyProbe {
            count,
            sent: 0,
            received: 0,
            outstanding: false,
            pending_send_ms: 0,
            next_ms: now,
            gap_ms: gap_ms.min(MAX_LATENCY_GAP_MS),
            samples: [0; LATENCY_SAMPLES],
            peak_ms: 0,
        });
        true
    }

    fn latency_tick(&mut self, _services: &Services, now: u64) {
        let Some(probe) = self.latency.as_ref() else {
            return;
        };
        if probe.received >= probe.count || probe.outstanding || now < probe.next_ms {
            return;
        }
        let send_ms = now as u32;
        let Some(destination) = self.peer else {
            return;
        };
        if self.queue_ping(
            now,
            MessageKind::Request,
            OpCode::Ping,
            STREAM_LATENCY,
            destination,
            u64::from(send_ms),
            0,
            Tag(0),
        ) {
            if let Some(probe) = self.latency.as_mut() {
                probe.sent = probe.sent.saturating_add(1);
                probe.outstanding = true;
                probe.pending_send_ms = send_ms;
            }
        }
    }

    fn on_latency(&mut self, reply_to: u32, now: u64) {
        let Some(probe) = self.latency.as_mut() else {
            return;
        };
        if !probe.outstanding || reply_to != probe.pending_send_ms {
            return;
        }
        let elapsed = (now as u32).wrapping_sub(probe.pending_send_ms) as u64;
        if probe.received < 64 {
            probe.samples[probe.received as usize] = elapsed;
        }
        probe.received = probe.received.saturating_add(1);
        probe.outstanding = false;
        probe.peak_ms = probe.peak_ms.max(elapsed);
        probe.next_ms = now.saturating_add(u64::from(probe.gap_ms));
    }

    fn queue_window_update(
        &mut self,
        destination: NodeId,
        stream: u32,
        nonce: u32,
        credit: u32,
        now: u64,
    ) {
        let mut body = [0u8; 16];
        body[0..4].copy_from_slice(&stream.to_le_bytes());
        body[4..8].copy_from_slice(&credit.to_le_bytes());
        body[8..12].copy_from_slice(&nonce.to_le_bytes());
        let Ok(frame) = MessageFrame::inline(
            MessageKind::Control,
            OpCode::WindowUpdate,
            StreamId(stream),
            Tag(0),
            &body,
        ) else {
            return;
        };
        let mut header = MeshHeader::new(self.self_id, destination, self.self_id);
        header.flags = mesh_flags::PROBE;
        header.ttl = MeshHeader::MAX_HOPS;
        let frame_header = frame.header;
        self.stage_control(stream, header, frame_header, &body, now);
    }

    fn on_window_update(&mut self, body: &[u8]) -> Result<(), TransportError> {
        if body.len() < 8 {
            return Err(TransportError::Envelope(sys_primitives::Error::BadLength));
        }
        let stream = u32::from_le_bytes([body[0], body[1], body[2], body[3]]);
        // Window updates carry bytes, not chunks, so the receiver does not need
        // to know the sender's chunk size.
        let credit = u32::from_le_bytes([body[4], body[5], body[6], body[7]])
            .clamp(1, mesh_stream::MAX_CREDIT);
        // A window update can cross a cancel in flight.  A grant for a stream
        // this side has already reset is stale, not a protocol failure.
        if let Some(streams) = primary_mux(&mut self.links, self.primary_link) {
            let _ = streams.grant_send(stream, credit);
        }
        if let Some(reply) = self.bulk_reply.as_mut() {
            if reply.stream == stream {
                let chunks = (credit / reply.chunk.max(1)).max(1);
                reply.credit = reply.credit.saturating_add(chunks).min(8);
            }
        }
        Ok(())
    }

    fn on_cancel(
        &mut self,
        services: &Services,
        body: &[u8],
        now: u64,
    ) -> Result<(), TransportError> {
        if body.len() < 4 {
            return Err(TransportError::Envelope(sys_primitives::Error::BadLength));
        }
        let stream = u32::from_le_bytes([body[0], body[1], body[2], body[3]]);
        if stream == 0 {
            return Err(TransportError::Envelope(sys_primitives::Error::Protocol));
        }
        if !primary_mux(&mut self.links, self.primary_link)
            .is_some_and(|streams| streams.reset(stream).is_ok())
        {
            // A cancel for a stream this side already retired is stale, not a
            // protocol failure; there is nothing left to reclaim.
            return Ok(());
        }
        if let Some(transfer) = self.bulk.as_mut() {
            if transfer.stream == stream {
                transfer.state = BulkState::Failed;
                transfer.error = Some("cancel");
                transfer.finished_ms = now;
            }
        }
        if self
            .bulk_reply
            .as_ref()
            .is_some_and(|reply| reply.stream == stream)
        {
            self.bulk_reply = None;
        }
        if self.retire_stream == Some(stream) {
            self.retire_stream = None;
        }
        self.stats.stream_resets = self.stats.stream_resets.saturating_add(1);
        let mut line = Line::new();
        line.str("mesh-stream: cancel stream=");
        line.u64(stream as u64);
        line.str(" reset=");
        line.u64(self.primary_streams().map_or(0, |streams| streams.resets()));
        line.push(b'\n');
        log(services, line.as_bytes());
        Ok(())
    }

    /// Whether a bulk transfer is queued or in flight in either direction.
    fn bulk_active(&self) -> bool {
        self.bulk
            .as_ref()
            .is_some_and(|transfer| matches!(transfer.state, BulkState::Start | BulkState::Running))
            || self.bulk_reply.is_some()
    }

    /// Start one bulk measurement transfer to the current peer.
    fn start_bulk(&mut self, services: &Services, total: u32, chunk: u32, now: u64) -> bool {
        if !matches!(self.phase, Phase::Connected) || self.peer.is_none() {
            return false;
        }
        if total == 0
            || total as usize > BULK_RECORD_MAX
            || chunk == 0
            || chunk as usize > BULK_CHUNK_MAX
            || !total.is_multiple_of(chunk)
        {
            return false;
        }
        let nonce = (now as u32) ^ self.stats.frames_sent as u32 ^ 0x4952_4237;
        let Some(stream) = primary_mux(&mut self.links, self.primary_link)
            .and_then(|streams| streams.open_channel(false).ok())
        else {
            return false;
        };
        self.bulk = Some(BulkTransfer {
            nonce,
            total,
            chunk,
            state: BulkState::Start,
            started_ms: now,
            request_queued_ms: 0,
            first_chunk_ms: 0,
            finished_ms: 0,
            received: 0,
            chunks: 0,
            expected_offset: 0,
            error: None,
            stream,
        });
        // No serial line here: the emulated console is synchronous inside the
        // module tick, and one 70-byte line delayed the bulk request by ~600 ms
        // in the first #247 measurement.  `/mesh/bulk` is the evidence surface.
        let _ = services;
        true
    }

    fn on_bulk_request(
        &mut self,
        services: &Services,
        source: NodeId,
        stream: u32,
        body: &[u8],
        now: u64,
    ) -> Result<(), TransportError> {
        if body.len() != BULK_HEADER_LEN {
            self.stats.protocol_rejects = self.stats.protocol_rejects.saturating_add(1);
            return Ok(());
        }
        let magic = u32::from_le_bytes([body[0], body[1], body[2], body[3]]);
        let total = u32::from_le_bytes([body[4], body[5], body[6], body[7]]);
        let chunk = u32::from_le_bytes([body[8], body[9], body[10], body[11]]);
        let nonce = u32::from_le_bytes([body[12], body[13], body[14], body[15]]);
        let credit = u32::from_le_bytes([body[16], body[17], body[18], body[19]]).clamp(1, 8);
        if magic != BULK_MAGIC
            || total == 0
            || total as usize > BULK_RECORD_MAX
            || chunk == 0
            || chunk as usize > BULK_CHUNK_MAX
            || !total.is_multiple_of(chunk)
        {
            self.stats.protocol_rejects = self.stats.protocol_rejects.saturating_add(1);
            return Ok(());
        }
        if let Some(reply) = self.bulk_reply.as_ref() {
            if reply.stream != stream {
                let old = reply.stream;
                if let Some(streams) = primary_mux(&mut self.links, self.primary_link) {
                    let _ = streams.reset(old);
                }
                self.bulk_reply = None;
            }
        }
        let accepted = match primary_mux(&mut self.links, self.primary_link) {
            Some(streams) => streams.accept_channel(stream, false),
            None => Err(mesh_stream::Error::BadStream),
        };
        if let Err(error) = accepted {
            let mut line = Line::new();
            line.str("mesh-stream: reject reason=");
            line.str(error.as_str());
            line.str(" stream=");
            line.u64(stream as u64);
            line.push(b'\n');
            log(services, line.as_bytes());
            return Err(TransportError::Envelope(sys_primitives::Error::Protocol));
        }
        if !primary_mux(&mut self.links, self.primary_link).is_some_and(|streams| {
            streams
                .set_send_credit(stream, credit.saturating_mul(chunk))
                .is_ok()
        }) {
            self.stats.protocol_rejects = self.stats.protocol_rejects.saturating_add(1);
            return Ok(());
        }
        self.bulk_reply = Some(BulkReply {
            node: source,
            nonce,
            total,
            chunk,
            offset: 0,
            started_ms: now,
            credit,
            finishing: false,
            stream,
        });
        // The peer-side counters are reported by the origin's `/mesh/bulk`
        // raw line; do not block this tick on the emulated serial console.
        let _ = services;
        Ok(())
    }

    fn on_bulk_chunk(
        &mut self,
        services: &Services,
        source: NodeId,
        stream: u32,
        body: &[u8],
        now: u64,
    ) -> Result<(), TransportError> {
        if body.len() < BULK_HEADER_LEN {
            self.stats.protocol_rejects = self.stats.protocol_rejects.saturating_add(1);
            return Ok(());
        }
        let magic = u32::from_le_bytes([body[0], body[1], body[2], body[3]]);
        let nonce = u32::from_le_bytes([body[4], body[5], body[6], body[7]]);
        let total = u32::from_le_bytes([body[8], body[9], body[10], body[11]]);
        let offset = u32::from_le_bytes([body[12], body[13], body[14], body[15]]);
        let payload = u32::from_le_bytes([body[16], body[17], body[18], body[19]]) as usize;
        let Some(transfer) = self.bulk.as_mut() else {
            return Ok(());
        };
        if stream != transfer.stream {
            return Err(TransportError::Envelope(sys_primitives::Error::Protocol));
        }
        if transfer.state != BulkState::Running {
            // A chunk that arrives after a cancel or a terminal state is
            // stale; the stream is already reset.
            return Ok(());
        }
        if magic != BULK_MAGIC
            || nonce != transfer.nonce
            || total != transfer.total
            || payload != body.len() - BULK_HEADER_LEN
            || offset != transfer.expected_offset
            || transfer.received.saturating_add(payload as u32) > transfer.total
            || Some(source) != self.peer
        {
            transfer.state = BulkState::Failed;
            transfer.error = Some("chunk");
            transfer.finished_ms = now;
            self.stats.bulk_aborts = self.stats.bulk_aborts.saturating_add(1);
            return Ok(());
        }
        if !primary_mux(&mut self.links, self.primary_link)
            .is_some_and(|streams| streams.consume_receive(stream, payload as u32).is_ok())
        {
            transfer.state = BulkState::Failed;
            transfer.error = Some("credit");
            transfer.finished_ms = now;
            self.stats.protocol_rejects = self.stats.protocol_rejects.saturating_add(1);
            return Err(TransportError::Envelope(sys_primitives::Error::Protocol));
        }
        if transfer.received == 0 {
            transfer.first_chunk_ms = now;
        }
        transfer.received = transfer.received.saturating_add(payload as u32);
        transfer.expected_offset = transfer.expected_offset.saturating_add(payload as u32);
        transfer.chunks = transfer.chunks.saturating_add(1);
        self.stats.bulk_chunks_received = self.stats.bulk_chunks_received.saturating_add(1);
        let (received, total, chunk, nonce, started_ms) = (
            transfer.received,
            transfer.total,
            transfer.chunk,
            transfer.nonce,
            transfer.started_ms,
        );
        let grant = received % (BULK_CREDIT_GRANT * chunk.max(1)) == 0 && received != total;
        if received == total {
            transfer.state = BulkState::Done;
            transfer.finished_ms = now;
            if let Some(streams) = primary_mux(&mut self.links, self.primary_link) {
                let _ = streams.close_remote(stream);
            }
            self.retire_stream = Some(stream);
        }
        let elapsed = now.saturating_sub(started_ms);
        let chunks = transfer.chunks;
        if grant {
            // The origin grants the next pair of chunks only after it has
            // consumed the previous pair.  The wire value is bytes.
            let grant_bytes = BULK_CREDIT_GRANT * chunk;
            if let Some(streams) = primary_mux(&mut self.links, self.primary_link) {
                let _ = streams.grant_receive(stream, grant_bytes);
            }
            self.queue_window_update(source, stream, nonce, grant_bytes, now);
            self.stats.credit_grants = self.stats.credit_grants.saturating_add(1);
        }
        if received == total {
            let mut line = Line::new();
            line.str("mesh-bulk: complete bytes=");
            line.u64(total as u64);
            line.str(" chunks=");
            line.u64(chunks as u64);
            line.str(" elapsed_ms=");
            line.u64(elapsed);
            line.push(b'\n');
            log(services, line.as_bytes());
        }
        Ok(())
    }

    /// Queue one outbound bulk request or reply chunk.  Called once per tick,
    /// after control work and before the single flush.
    fn bulk_tick(&mut self, services: &Services, now: u64) {
        let start = match self.bulk.as_ref() {
            Some(transfer) if transfer.state == BulkState::Start => Some((
                transfer.total,
                transfer.chunk,
                transfer.nonce,
                transfer.stream,
            )),
            Some(transfer)
                if transfer.state == BulkState::Running
                    && now.saturating_sub(transfer.started_ms) >= BULK_DEADLINE_MS =>
            {
                let received = transfer.received;
                let total = transfer.total;
                let stream = transfer.stream;
                if let Some(transfer) = self.bulk.as_mut() {
                    transfer.state = BulkState::Failed;
                    transfer.error = Some("timeout");
                    transfer.finished_ms = now;
                }
                if let Some(streams) = primary_mux(&mut self.links, self.primary_link) {
                    let _ = streams.reset(stream);
                }
                self.stats.stream_resets = self.stats.stream_resets.saturating_add(1);
                self.stats.bulk_aborts = self.stats.bulk_aborts.saturating_add(1);
                let mut line = Line::new();
                line.str("mesh-bulk: failed reason=timeout received=");
                line.u64(received as u64);
                line.str("/");
                line.u64(total as u64);
                line.push(b'\n');
                log(services, line.as_bytes());
                None
            }
            _ => None,
        };
        if let Some((total, chunk, nonce, stream)) = start {
            if self.queue_bulk_request(stream, total, chunk, nonce, now) {
                if let Some(transfer) = self.bulk.as_mut() {
                    transfer.state = BulkState::Running;
                    transfer.request_queued_ms = now;
                }
            } else {
                if let Some(transfer) = self.bulk.as_mut() {
                    transfer.state = BulkState::Failed;
                    transfer.error = Some("queue");
                    transfer.finished_ms = now;
                }
                if let Some(streams) = primary_mux(&mut self.links, self.primary_link) {
                    let _ = streams.reset(stream);
                }
                self.stats.stream_resets = self.stats.stream_resets.saturating_add(1);
            }
        }
        let Some(reply) = self.bulk_reply.as_ref() else {
            return;
        };
        if reply.finishing || reply.credit == 0 {
            return;
        }
        if now.saturating_sub(reply.started_ms) >= BULK_DEADLINE_MS {
            let stream = reply.stream;
            self.bulk_reply = None;
            if let Some(streams) = primary_mux(&mut self.links, self.primary_link) {
                let _ = streams.reset(stream);
            }
            self.stats.stream_resets = self.stats.stream_resets.saturating_add(1);
            self.stats.bulk_aborts = self.stats.bulk_aborts.saturating_add(1);
            return;
        }
        let (node, nonce, total, chunk, offset, stream) = (
            reply.node,
            reply.nonce,
            reply.total,
            reply.chunk,
            reply.offset,
            reply.stream,
        );
        if self.queue_bulk_chunk(node, stream, nonce, total, chunk, offset, now) {
            self.stats.bulk_chunks_sent = self.stats.bulk_chunks_sent.saturating_add(1);
            if let Some(reply) = self.bulk_reply.as_mut() {
                reply.offset = reply.offset.saturating_add(chunk);
                reply.credit = reply.credit.saturating_sub(1);
                if reply.offset >= reply.total {
                    // Keep the reply until the sink accepts the final record;
                    // the flush path resets the stream and clears it.
                    reply.finishing = true;
                    self.retire_stream = Some(reply.stream);
                }
            }
        }
    }

    fn queue_bulk_request(
        &mut self,
        stream: u32,
        total: u32,
        chunk: u32,
        nonce: u32,
        now: u64,
    ) -> bool {
        let Some(destination) = self.peer else {
            return false;
        };
        if self
            .staged
            .iter()
            .any(|frame| frame.is_some_and(|frame| frame.store == BodyStore::FrameRx))
        {
            self.drain_stage(now);
            if self.stage_fault.is_some() {
                return false;
            }
        }
        // SAFETY: single-threaded module; FRAME_RX is reused after the receive
        // pass and the staged record is copied into the transport queue before
        // the next `feed`.
        let body = unsafe { &mut (&mut *core::ptr::addr_of_mut!(FRAME_RX))[..BULK_HEADER_LEN] };
        body[0..4].copy_from_slice(&BULK_MAGIC.to_le_bytes());
        body[4..8].copy_from_slice(&total.to_le_bytes());
        body[8..12].copy_from_slice(&chunk.to_le_bytes());
        body[12..16].copy_from_slice(&nonce.to_le_bytes());
        body[16..20].copy_from_slice(&BULK_INITIAL_CREDIT.to_le_bytes());
        let Ok(frame) = MessageFrame::inline(
            MessageKind::Request,
            OpCode::Read,
            StreamId(stream),
            Tag(0),
            body,
        ) else {
            return false;
        };
        let mut header = MeshHeader::new(self.self_id, destination, self.self_id);
        header.flags = mesh_flags::PROBE;
        header.ttl = MeshHeader::MAX_HOPS;
        header.request_id = u64::from(nonce);
        // The request carries the initial window; the receiver must enforce it
        // before the first chunk arrives.
        if let Some(streams) = self.primary_streams_mut() {
            let _ = streams.set_receive_credit(stream, BULK_INITIAL_CREDIT * chunk);
        }
        let frame_header = frame.header;
        self.stage_data_rx(
            stream,
            header,
            frame_header,
            BULK_HEADER_LEN,
            BULK_HEADER_LEN as u32,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn queue_bulk_chunk(
        &mut self,
        destination: NodeId,
        stream: u32,
        nonce: u32,
        total: u32,
        chunk: u32,
        offset: u32,
        now: u64,
    ) -> bool {
        let payload = chunk as usize;
        let body_len = BULK_HEADER_LEN + payload;
        if body_len > FRAME_BODY_MAX {
            return false;
        }
        if self
            .staged
            .iter()
            .any(|frame| frame.is_some_and(|frame| frame.store == BodyStore::FrameRx))
        {
            self.drain_stage(now);
            if self.stage_fault.is_some() {
                return false;
            }
        }
        let bytes = chunk;
        // Bulk is measurement traffic on the authority link.
        let Some(index) = self.primary_link else {
            return false;
        };
        let Some(class) = self
            .links
            .get_mut(index)
            .and_then(|link| link.streams.begin(stream, bytes).ok())
        else {
            self.stats.credit_stalls = self.stats.credit_stalls.saturating_add(1);
            return false;
        };
        // SAFETY: single-threaded module; FRAME_RX is reused after the receive
        // pass and the staged record is copied into the transport queue before
        // the next `feed`.
        let body = unsafe { &mut (&mut *core::ptr::addr_of_mut!(FRAME_RX))[..body_len] };
        body[0..4].copy_from_slice(&BULK_MAGIC.to_le_bytes());
        body[4..8].copy_from_slice(&nonce.to_le_bytes());
        body[8..12].copy_from_slice(&total.to_le_bytes());
        body[12..16].copy_from_slice(&offset.to_le_bytes());
        body[16..20].copy_from_slice(&(chunk).to_le_bytes());
        for byte in &mut body[BULK_HEADER_LEN..] {
            *byte = 0;
        }
        let final_chunk = offset.saturating_add(chunk) >= total;
        let Ok(mut frame) = MessageFrame::inline(
            MessageKind::Response,
            OpCode::Read,
            StreamId(stream),
            Tag(0),
            body,
        ) else {
            if let Some(link) = self.links.get_mut(index) {
                link.streams.abort(stream, bytes);
            }
            return false;
        };
        frame.header.flags |= if final_chunk {
            frame_flags::EOS
        } else {
            frame_flags::MORE
        };
        frame.header.offset = u64::from(offset);
        frame.header.length = total;
        let mut header = MeshHeader::new(self.self_id, destination, self.self_id);
        header.flags = mesh_flags::PROBE | mesh_flags::REPLY;
        header.ttl = MeshHeader::MAX_HOPS;
        header.request_id = u64::from(nonce);
        let frame_header = frame.header;
        self.stage_record(
            index,
            stream,
            class,
            header,
            frame_header,
            BodyStore::FrameRx,
            0,
            body_len,
            chunk,
        )
    }

    /// Drop staged frames and their bodies.
    fn stage_reset(&mut self) {
        self.staged = [None; STAGE_FRAMES];
        self.stage_len = 0;
    }

    /// The key a staged frame records for the link at `index`.
    ///
    /// The authority link is keyed as zero because the root binds its peer
    /// after frames may already be staged; every other link is keyed by peer.
    fn link_key(&self, index: usize) -> NodeId {
        if Some(index) == self.primary_link {
            return NodeId::ZERO;
        }
        self.links.get(index).map_or(NodeId::ZERO, |link| link.peer)
    }

    /// Record a control frame that could not be staged or queued.
    ///
    /// The authority link fails closed at the next flush, so a join, route or
    /// migration record is never skipped.  A gossip link drops the frame: its
    /// summaries and aggregates are repaired by the next interval or resync,
    /// and one full 4 KiB queue must not close the session for every member.
    fn stage_failed(&mut self, index: usize, reason: &'static str) {
        if Some(index) == self.primary_link {
            self.stage_fault = Some(reason);
        }
    }

    /// Make room for one more staged frame with a `small` byte body.
    ///
    /// The stage is shared by every link, and a root with eight links can
    /// owe eight heartbeats in the tick that also carries join and route
    /// records.  A full stage is drained into the link queues early; it is
    /// never a reason to fail a control record.
    fn stage_room(&mut self, small: usize, now: u64) {
        let slots_full = self.staged.iter().all(Option::is_some);
        if slots_full || self.stage_len.saturating_add(small) > STAGE_SMALL_MAX {
            self.drain_stage(now);
        }
    }

    /// Copy one small control body into the tick's staging area.
    fn stage_body_bytes(&mut self, body: &[u8]) -> Option<(usize, usize)> {
        let start = self.stage_len;
        let end = start.checked_add(body.len())?;
        if end > STAGE_SMALL_MAX {
            return None;
        }
        // SAFETY: single-threaded module; STAGE_SMALL is only written here and
        // read by `drain_stage` before the next tick clears `stage_len`.
        let storage = unsafe { &mut *core::ptr::addr_of_mut!(STAGE_SMALL) };
        storage[start..end].copy_from_slice(body);
        self.stage_len = end;
        Some((start, body.len()))
    }

    /// Stage one frame for the link at `index`; its body already sits at
    /// `body_start`.
    #[allow(clippy::too_many_arguments)]
    fn stage_record(
        &mut self,
        index: usize,
        id: u32,
        class: Class,
        header: MeshHeader,
        frame: FrameHeader,
        store: BodyStore,
        body_start: usize,
        body_len: usize,
        bytes: u32,
    ) -> bool {
        let link = self.link_key(index);
        let slot = self.staged.iter().position(Option::is_none);
        let scheduled = slot.is_some()
            && self
                .links
                .get_mut(index)
                .is_some_and(|target| target.streams.schedule(id, class).is_ok());
        let Some(slot) = slot.filter(|_| scheduled) else {
            if store == BodyStore::Small {
                self.stage_len = body_start;
            }
            if let Some(target) = self.links.get_mut(index) {
                target.streams.abort(id, bytes);
            }
            return false;
        };
        self.staged[slot] = Some(StagedFrame {
            link,
            stream: id,
            class,
            header,
            frame,
            store,
            body_start,
            body_len,
        });
        true
    }

    /// Stage one frame after copying its small body into the staging area.
    #[allow(clippy::too_many_arguments)]
    fn stage_commit(
        &mut self,
        index: usize,
        id: u32,
        class: Class,
        header: MeshHeader,
        frame: FrameHeader,
        body: &[u8],
        bytes: u32,
    ) -> bool {
        let Some((body_start, body_len)) = self.stage_body_bytes(body) else {
            if let Some(target) = self.links.get_mut(index) {
                target.streams.abort(id, bytes);
            }
            return false;
        };
        self.stage_record(
            index,
            id,
            class,
            header,
            frame,
            BodyStore::Small,
            body_start,
            body_len,
            bytes,
        )
    }

    /// Stage a control-class frame on the link for `header.dst`.
    fn stage_control(
        &mut self,
        id: u32,
        header: MeshHeader,
        frame: FrameHeader,
        body: &[u8],
        now: u64,
    ) -> bool {
        let Some(index) = self.outbound_index(header.dst, now) else {
            return false;
        };
        self.stage_room(body.len(), now);
        let bytes = u32::try_from(body.len()).unwrap_or(u32::MAX);
        let class = self
            .links
            .get_mut(index)
            .and_then(|link| link.streams.begin_control(id, bytes).ok());
        let staged = match class {
            Some(class) => self.stage_commit(index, id, class, header, frame, body, bytes),
            None => false,
        };
        if !staged {
            self.stage_failed(index, "stream-control");
        }
        staged
    }

    /// Stage a join frame whose body is the current `JOIN_TX` contents.
    fn stage_control_join(
        &mut self,
        id: u32,
        header: MeshHeader,
        frame: FrameHeader,
        body: &[u8],
        now: u64,
    ) -> bool {
        let Some(index) = self.outbound_index(header.dst, now) else {
            return false;
        };
        self.stage_room(0, now);
        let bytes = u32::try_from(body.len()).unwrap_or(u32::MAX);
        let class = self
            .links
            .get_mut(index)
            .and_then(|link| link.streams.begin_control(id, bytes).ok());
        let staged = match class {
            Some(class) => self.stage_record(
                index,
                id,
                class,
                header,
                frame,
                BodyStore::Join,
                0,
                body.len(),
                bytes,
            ),
            None => false,
        };
        if !staged {
            self.stage_failed(index, "stream-control");
        }
        staged
    }

    /// Stage a data-class frame whose body is the current `FRAME_RX` contents.
    ///
    /// Data frames use the authority link.  `credit_bytes` is the payload size
    /// the stream window counts.  A bulk chunk's 20-byte body header is
    /// framing, not window credit, so the window stays in payload units.
    fn stage_data_rx(
        &mut self,
        id: u32,
        header: MeshHeader,
        frame: FrameHeader,
        body_len: usize,
        credit_bytes: u32,
    ) -> bool {
        let Some(index) = self.primary_link else {
            return false;
        };
        let Some(class) = self
            .links
            .get_mut(index)
            .and_then(|link| link.streams.begin(id, credit_bytes).ok())
        else {
            self.stats.credit_stalls = self.stats.credit_stalls.saturating_add(1);
            return false;
        };
        self.stage_record(
            index,
            id,
            class,
            header,
            frame,
            BodyStore::FrameRx,
            0,
            body_len,
            credit_bytes,
        )
    }

    /// Drain staged frames into each link's transport queue.
    ///
    /// The authority link drains first; within a link the scheduler puts the
    /// control class ahead of data.
    fn drain_stage(&mut self, now: u64) {
        for pass in 0..2 {
            for index in 0..self.links.len() {
                let primary = Some(index) == self.primary_link;
                if (pass == 0) == primary {
                    self.drain_link(index, now);
                }
            }
        }
        self.staged = [None; STAGE_FRAMES];
        self.stage_len = 0;
    }

    /// Release a staged frame that never reached its transport queue.
    fn drain_failed(&mut self, index: usize, frame: &StagedFrame, reason: &'static str) {
        let bytes = u32::try_from(frame.body_len).unwrap_or(u32::MAX);
        if let Some(link) = self.links.get_mut(index) {
            link.streams.abort(frame.stream, bytes);
        }
        if frame.class == Class::Control {
            self.stage_failed(index, reason);
        } else {
            self.rollback_data(frame);
        }
    }

    /// Move the staged frames of one link into its queue, in scheduler order.
    fn drain_link(&mut self, index: usize, now: u64) {
        let key = self.link_key(index);
        while let Some((id, class)) = self
            .links
            .get_mut(index)
            .and_then(|link| link.streams.next_frame())
        {
            let Some(slot) = self.staged.iter().position(|frame| {
                frame.is_some_and(|frame| {
                    frame.link == key && frame.stream == id && frame.class == class
                })
            }) else {
                continue;
            };
            let Some(frame) = self.staged[slot].take() else {
                continue;
            };
            let bytes = u32::try_from(frame.body_len).unwrap_or(u32::MAX);
            if !self
                .links
                .get(index)
                .is_some_and(|link| link.streams.can_commit())
            {
                self.drain_failed(index, &frame, "stream-ledger");
                continue;
            }
            let body = unsafe {
                // SAFETY: single-threaded module; the staged body is copied
                // into the transport queue before this slice is released.
                match frame.store {
                    BodyStore::Small => {
                        &(&*core::ptr::addr_of!(STAGE_SMALL))
                            [frame.body_start..frame.body_start + frame.body_len]
                    }
                    BodyStore::Join => {
                        &(&*core::ptr::addr_of!(JOIN_TX))
                            [frame.body_start..frame.body_start + frame.body_len]
                    }
                    BodyStore::FrameRx => {
                        &(&*core::ptr::addr_of!(FRAME_RX))
                            [frame.body_start..frame.body_start + frame.body_len]
                    }
                }
            };
            let Ok(mut message) = MessageFrame::inline(
                frame.frame.kind,
                frame.frame.opcode,
                StreamId(id),
                frame.frame.tag,
                body,
            ) else {
                self.drain_failed(index, &frame, "stream-encode");
                continue;
            };
            message.header = frame.frame;
            let envelope = MeshEnvelope {
                header: frame.header,
                frame: message,
            };
            let Some(link) = self.links.get_mut(index) else {
                continue;
            };
            let queued = match class {
                Class::Control => link.queue.push_record(&envelope, now),
                Class::Data => link.queue.push_data_record(&envelope, now),
            };
            match queued {
                Ok(()) => {
                    if link.streams.commit(id, bytes).is_err() {
                        self.drain_failed(index, &frame, "stream-ledger");
                    }
                }
                Err(_) => {
                    // Never skip an authority control record.  Close the link
                    // and let the rejoin path retry it on a fresh queue.
                    self.drain_failed(index, &frame, "stream-queue");
                    if class == Class::Data {
                        self.stats.credit_stalls = self.stats.credit_stalls.saturating_add(1);
                    }
                }
            }
        }
    }

    /// Put a data frame that never reached the transport queue back at the
    /// head of its stream so the next tick retries it.
    fn rollback_data(&mut self, frame: &StagedFrame) {
        if let Some(reply) = self.bulk_reply.as_mut() {
            if reply.stream == frame.stream {
                reply.offset = frame.frame.offset as u32;
                reply.finishing = false;
                reply.credit = reply.credit.saturating_add(1);
            }
        }
        if let Some(transfer) = self.bulk.as_mut() {
            if transfer.stream == frame.stream
                && transfer.state == BulkState::Running
                && frame.frame.kind == MessageKind::Request
            {
                // The request never reached the transport queue; retry it on
                // the next tick.
                transfer.state = BulkState::Start;
                transfer.request_queued_ms = 0;
            }
        }
        if self.retire_stream == Some(frame.stream) {
            self.retire_stream = None;
        }
    }

    fn flush(&mut self, services: &Services, now: u64) {
        self.drain_stage(now);
        if let Some(reason) = self.stage_fault.take() {
            self.disconnect(services, now, reason);
            return;
        }
        let mut sink = ServiceSink {
            services,
            slot: MESH_SLOT,
        };
        // Flush the authority link first, then every gossip link.  The sink is
        // one TCP session, so a blocked queue stops later queues too; the
        // order keeps join/route control ahead of gossip.
        let mut bytes = 0usize;
        let mut frames = 0usize;
        let mut writes = 0usize;
        let mut blocked = false;
        let mut closed = false;
        let mut batching = 0u64;
        let mut peak = 0usize;
        for pass in 0..2 {
            for index in 0..self.links.len() {
                let primary = Some(index) == self.primary_link;
                if (pass == 0) != primary {
                    continue;
                }
                let Some(link) = self.links.get_mut(index) else {
                    continue;
                };
                let report = link.queue.flush(&mut sink);
                // Each link's ledger follows its own FIFO queue.
                link.streams.settle(report.frames_written);
                bytes += report.bytes_written;
                frames += report.frames_written;
                writes += report.writes;
                blocked |= report.blocked;
                closed |= report.closed;
                if report.frames_written > 1 && report.writes == 1 {
                    batching += 1;
                }
                peak = peak.max(link.queue.peak_pending());
                link.bytes_sent = link.bytes_sent.saturating_add(report.bytes_written as u64);
                link.frames_sent = link
                    .frames_sent
                    .saturating_add(report.frames_written as u64);
            }
        }
        if let Some(stream) = self.retire_stream {
            let queued = self
                .primary_streams()
                .and_then(|streams| streams.stream(stream))
                .map_or(0, |state| state.queued);
            if queued == 0 {
                if let Some(streams) = self.primary_streams_mut() {
                    let _ = streams.reset(stream);
                }
                self.stats.stream_resets = self.stats.stream_resets.saturating_add(1);
                self.retire_stream = None;
                if self
                    .bulk_reply
                    .as_ref()
                    .is_some_and(|reply| reply.stream == stream)
                {
                    self.bulk_reply = None;
                }
            }
        }
        self.stats.bytes_sent = self.stats.bytes_sent.saturating_add(bytes as u64);
        self.stats.frames_sent = self.stats.frames_sent.saturating_add(frames as u64);
        self.stats.writes = self.stats.writes.saturating_add(writes as u64);
        self.stats.batched_flushes = self.stats.batched_flushes.saturating_add(batching);
        self.stats.peak_queued_bytes = self.stats.peak_queued_bytes.max(peak);
        self.stats.peak_decoder_bytes = self
            .stats
            .peak_decoder_bytes
            .max(self.decoder.buffered_len());
        if blocked {
            self.stats.short_writes = self.stats.short_writes.saturating_add(1);
        }
        if closed {
            self.disconnect(services, now, "send-closed");
        }
    }

    fn maybe_log_pass(&mut self, services: &Services) {
        if !matches!(self.phase, Phase::Connected)
            || self.pass_logged
            || !self.peer_confirmed
            || self.stats.frames_sent == 0
        {
            return;
        }
        let Some(peer) = self.peer else {
            return;
        };
        let mut line = Line::new();
        line.str("mesh-transport: pass peer=");
        line.node(&peer);
        line.str(" rx_frames=");
        line.u64(self.stats.frames_received);
        line.str(" tx_frames=");
        line.u64(self.stats.frames_sent);
        line.str(" rx_bytes=");
        line.u64(self.stats.bytes_received);
        line.str(" tx_bytes=");
        line.u64(self.stats.bytes_sent);
        line.str(" rejects=");
        line.u64(
            self.stats
                .oversize_rejects
                .saturating_add(self.stats.short_frame_rejects)
                .saturating_add(self.stats.header_rejects)
                .saturating_add(self.stats.protocol_rejects),
        );
        line.str(" connects=");
        line.u64(self.stats.connects);
        line.str(" connect_ms=");
        line.u64(self.last_connect_ms);
        line.str(" links=");
        line.u64(self.link_count() as u64);
        line.str(" gossip_sum=");
        line.u64(self.gossip_summaries_sent);
        line.str(" gossip_sum_rx=");
        line.u64(self.gossip_summaries_received);
        line.str(" gossip_agg=");
        line.u64(self.gossip_aggregates_sent);
        line.str(" gossip_fwd=");
        line.u64(self.gossip_aggregates_forwarded);
        line.str(" writes=");
        line.u64(self.stats.writes);
        line.str(" batched=");
        line.u64(self.stats.batched_flushes);
        line.str(" peak_queue=");
        line.u64(self.stats.peak_queued_bytes as u64);
        line.str(" peak_decoder=");
        line.u64(self.stats.peak_decoder_bytes as u64);
        line.str(" bulk_rx=");
        line.u64(self.stats.bulk_chunks_received);
        line.str(" bulk_tx=");
        line.u64(self.stats.bulk_chunks_sent);
        line.str(" bulk_abort=");
        line.u64(self.stats.bulk_aborts);
        line.str(" credit_stall=");
        line.u64(self.stats.credit_stalls);
        line.str(" credit_grant=");
        line.u64(self.stats.credit_grants);
        line.str(" stream_reset=");
        line.u64(self.stats.stream_resets);
        if let Some(streams) = self.primary_streams() {
            line.str(" stream_open=");
            line.u64(streams.len() as u64);
            line.str(" stream_opens=");
            line.u64(streams.opens());
            line.str(" stream_credit_stall=");
            line.u64(streams.credit_stalls());
            line.str(" stream_control_stall=");
            line.u64(streams.control_stalls());
            line.str(" stream_tx_control=");
            line.u64(streams.control_frames());
            line.str(" stream_tx_data=");
            line.u64(streams.data_frames());
            line.str(" stream_inflight=");
            line.u64(streams.inflight() as u64);
        }
        line.push(b'\n');
        log(services, line.as_bytes());
        self.pass_logged = true;
    }

    fn reject(&mut self, services: &Services, now: u64, error: TransportError) {
        match error {
            TransportError::Oversize => {
                self.stats.oversize_rejects = self.stats.oversize_rejects.saturating_add(1);
            }
            TransportError::ShortPrefix
            | TransportError::TooShort
            | TransportError::ShortEnvelope
            | TransportError::LengthMismatch => {
                self.stats.short_frame_rejects = self.stats.short_frame_rejects.saturating_add(1);
            }
            TransportError::Envelope(_) => {
                self.stats.header_rejects = self.stats.header_rejects.saturating_add(1);
            }
            _ => {
                self.stats.protocol_rejects = self.stats.protocol_rejects.saturating_add(1);
            }
        }
        let mut line = Line::new();
        line.str("mesh-transport: reject ");
        line.str(error.as_str());
        line.push(b'\n');
        log(services, line.as_bytes());
        self.disconnect(services, now, "reject");
    }

    fn disconnect(&mut self, services: &Services, now: u64, reason: &str) {
        if matches!(self.phase, Phase::Connected) {
            self.stats.disconnects = self.stats.disconnects.saturating_add(1);
        }
        let buffered = self.decoder.buffered_len();
        let expected = self.decoder.expected_record_len();
        let poisoned = self.decoder.is_poisoned();
        (services.tcp_abort)(MESH_SLOT);
        self.decoder.reset();
        self.links.clear();
        self.primary_link = None;
        self.peer = match self.join.as_ref().map(|join| (join.role(), join.root())) {
            Some((Role::Member, root)) => Some(root),
            _ => None,
        };
        self.stage_reset();
        self.stage_fault = None;
        self.retire_stream = None;
        self.peer_confirmed = false;
        self.pass_logged = false;
        self.join_timeout_logged = false;
        self.last_rx_ms = now;
        if self.direct_initiator && self.direct.is_some() {
            self.phase = Phase::Connect;
            self.connect_attempts = 0;
            self.connect_started_ms = now;
            self.connect_at_ms = now.saturating_add(500);
            self.last_connect_ms = 0;
        } else {
            self.phase = Phase::Arm;
        }
        if let Some(join) = &mut self.join {
            join.on_link_down();
        }
        if let Some(route) = self.route.as_mut() {
            let _ = route.on_link_down();
        }
        if let Some(migrate) = self.migrate.as_mut() {
            let _ = migrate.on_link_down();
        }

        let mut line = Line::new();
        line.str("mesh-transport: disconnected reason=");
        line.str(reason);
        line.str(" buffered=");
        line.u64(buffered as u64);
        line.str(" expected=");
        match expected {
            Some(expected) => line.u64(expected as u64),
            None => line.push(b'-'),
        }
        line.str(" poisoned=");
        line.u64(poisoned as u64);
        line.str(" rx_frames=");
        line.u64(self.stats.frames_received);
        line.str(" tx_frames=");
        line.u64(self.stats.frames_sent);
        line.push(b'\n');
        log(services, line.as_bytes());
    }
}

struct ServiceSink<'a> {
    services: &'a Services,
    slot: u32,
}

impl ByteSink for ServiceSink<'_> {
    fn write(&mut self, data: &[u8]) -> WriteOutcome {
        let written = unsafe { (self.services.tcp_send)(self.slot, data.as_ptr(), data.len()) };
        if written > 0 {
            WriteOutcome::Wrote(written as usize)
        } else if written == 0 {
            WriteOutcome::WouldBlock
        } else {
            WriteOutcome::Closed
        }
    }
}

/// Log the route state changes a failure or partition produces.
///
/// The full bounded report is still served by `GET /mesh/route`; these lines
/// make the fence and authority transitions visible in the serial transcript
/// as well.
fn log_route_event(services: &Services, event: &mesh_route::Event) {
    match event {
        mesh_route::Event::Fence { .. }
        | mesh_route::Event::Authority { .. }
        | mesh_route::Event::Replay { .. }
        | mesh_route::Event::Reject { .. } => {
            let mut line = Line::new();
            line.str(&alloc_format(event));
            line.push(b'\n');
            log(services, line.as_bytes());
        }
        _ => {}
    }
}

/// Format one route event without `alloc`.
fn alloc_format(event: &mesh_route::Event) -> alloc::string::String {
    use core::fmt::Write as _;
    let mut text = alloc::string::String::new();
    let _ = write!(&mut text, "{event}");
    text
}

fn body_limit(kind: MessageKind, opcode: OpCode) -> usize {
    match (kind, opcode) {
        (MessageKind::Request, OpCode::Hello)
        | (MessageKind::Response, OpCode::Hello)
        | (MessageKind::Completion, OpCode::Hello) => JOIN_BODY_MAX,
        (MessageKind::Request, OpCode::Invoke)
        | (MessageKind::Response, OpCode::Invoke)
        | (MessageKind::Request, OpCode::RouteQuery)
        | (MessageKind::Response, OpCode::RouteQuery) => mesh_route::MAX_BODY,
        (MessageKind::Request, OpCode::Migrate)
        | (MessageKind::Response, OpCode::Migrate)
        | (MessageKind::Completion, OpCode::Migrate) => MIGRATE_BODY_MAX,
        (MessageKind::Request, OpCode::Read)
        | (MessageKind::Response, OpCode::Read)
        | (MessageKind::Request, OpCode::Write)
        | (MessageKind::Response, OpCode::Write) => FRAME_BODY_MAX,
        (MessageKind::Control, OpCode::WindowUpdate) => 16,
        (MessageKind::Control, OpCode::Hello) => JOIN_BODY_MAX,
        (MessageKind::Cancel, OpCode::Cancel) => 8,
        _ => 16,
    }
}

fn body_bytes(len: usize) -> &'static [u8] {
    // SAFETY: single-threaded module; FRAME_BODY holds the current frame body and
    // the slice is consumed before the next record is decoded.
    unsafe { core::slice::from_raw_parts(core::ptr::addr_of!(FRAME_RX) as *const u8, len) }
}

/// Read the M2-6 failure-detection opt-in.
///
/// Failure detection is opt-in so the M2-2 same-epoch rejoin check (a member
/// restart shorter than the failure window) keeps its original behaviour when
/// the key is absent.  The failure fault modes set `mesh.fail=1`.
fn failure_config(services: &Services) -> bool {
    let mut text = [0u8; 8];
    let enabled = config(services, b"mesh.fail", &mut text).is_some_and(|len| &text[..len] == b"1");
    log(
        services,
        if enabled {
            b"mesh-fail: enabled\n"
        } else {
            b"mesh-fail: disabled reason=config\n"
        },
    );
    enabled
}

/// Build the M2 route policy when `mesh.route` is present.
///
/// Route policy is opt-in so a normal M2-3 boot keeps exactly the
/// transport/join/homes behaviour.  The route node is still fail-closed: it
/// has no view until the confirmed snapshot arrives, and a member never signs
/// a capability or a home move.
fn route_config(
    services: &Services,
    self_id: NodeId,
    join: Option<&JoinNode>,
    home_count: u32,
) -> Option<RouteNode> {
    let mut text = [0u8; 32];
    let Some(len) = config_any(services, &[b"mesh.route", b"r"], &mut text) else {
        log(services, b"mesh-route: disabled reason=config\n");
        return None;
    };
    let enabled = &text[..len] == b"1" || &text[..len] == b"2" || &text[..len] == b"probe";
    if !enabled {
        log(services, b"mesh-route: disabled reason=config\n");
        return None;
    }
    let Some(join) = join else {
        log(services, b"mesh-route: disabled reason=join\n");
        return None;
    };
    let Some(route) = RouteNode::new(
        self_id,
        join.root(),
        join.authority_key(),
        matches!(join.role(), Role::Root),
        home_count,
    ) else {
        log(services, b"mesh-route: disabled reason=config\n");
        return None;
    };
    let mut line = Line::new();
    line.str("mesh-route: enabled role=");
    line.str(join.role().as_str());
    line.str(" objects=");
    line.u64(home_count as u64);
    line.push(b'\n');
    log(services, line.as_bytes());
    Some(route)
}

/// Log one migration state change without allocation.
fn log_migrate_event(services: &Services, event: &MigrateEvent) {
    let mut line = Line::new();
    match event {
        MigrateEvent::Enabled { role } => {
            line.str("mesh-migrate: enabled role=");
            line.str(role);
        }
        MigrateEvent::Snapshot {
            epoch,
            members,
            digest,
        } => {
            line.str("mesh-migrate: view epoch=");
            line.u64(*epoch as u64);
            line.str(" members=");
            line.u64(*members as u64);
            line.str(" digest=");
            line.hex(digest);
        }
        MigrateEvent::Seed {
            actor,
            host,
            epoch,
            hash,
        } => {
            line.str("mesh-migrate: seed actor=");
            line.hex(&actor.0);
            line.str(" host=");
            line.node(host);
            line.str(" epoch=");
            line.u64(*epoch as u64);
            line.str(" hash=");
            line.hex(hash);
        }
        MigrateEvent::Deliver {
            actor,
            message,
            applied,
            duplicate,
            hash,
        } => {
            line.str("mesh-migrate: deliver actor=");
            line.hex(&actor.0);
            line.str(" message=");
            line.u64(*message);
            line.str(" applied=");
            line.u64(*applied);
            line.str(" duplicate=");
            line.u64(if *duplicate { 1 } else { 0 });
            line.str(" hash=");
            line.hex(hash);
        }
        MigrateEvent::Queued { actor, message } => {
            line.str("mesh-migrate: queue actor=");
            line.hex(&actor.0);
            line.str(" message=");
            line.u64(*message);
        }
        MigrateEvent::Resume {
            actor,
            message,
            applied,
            hash,
        } => {
            line.str("mesh-migrate: resume actor=");
            line.hex(&actor.0);
            line.str(" message=");
            line.u64(*message);
            line.str(" applied=");
            line.u64(*applied);
            line.str(" hash=");
            line.hex(hash);
        }
        MigrateEvent::Start {
            actor,
            from,
            to,
            old_epoch,
            new_epoch,
        } => {
            line.str("mesh-migrate: migrate actor=");
            line.hex(&actor.0);
            line.str(" from=");
            line.node(from);
            line.str(" to=");
            line.node(to);
            line.str(" old_epoch=");
            line.u64(*old_epoch as u64);
            line.str(" new_epoch=");
            line.u64(*new_epoch as u64);
        }
        MigrateEvent::Moved {
            actor,
            from,
            to,
            old_epoch,
            new_epoch,
            hash,
            role,
        } => {
            line.str("mesh-migrate: moved actor=");
            line.hex(&actor.0);
            line.str(" from=");
            line.node(from);
            line.str(" to=");
            line.node(to);
            line.str(" old_epoch=");
            line.u64(*old_epoch as u64);
            line.str(" new_epoch=");
            line.u64(*new_epoch as u64);
            line.str(" hash=");
            line.hex(hash);
            line.str(" role=");
            line.str(role);
        }
        MigrateEvent::Fenced {
            actor,
            old_host,
            new_host,
            old_epoch,
            new_epoch,
            hash,
        } => {
            line.str("mesh-migrate: fence actor=");
            line.hex(&actor.0);
            line.str(" old_host=");
            line.node(old_host);
            line.str(" new_host=");
            line.node(new_host);
            line.str(" old_epoch=");
            line.u64(*old_epoch as u64);
            line.str(" new_epoch=");
            line.u64(*new_epoch as u64);
            line.str(" hash=");
            line.hex(hash);
        }
        MigrateEvent::Stale {
            actor,
            reason,
            expected_epoch,
            got_epoch,
        } => {
            line.str("mesh-migrate: stale actor=");
            line.hex(&actor.0);
            line.str(" reason=");
            line.str(reason.as_str());
            line.str(" expected_epoch=");
            line.u64(*expected_epoch as u64);
            line.str(" got_epoch=");
            line.u64(*got_epoch as u64);
        }
        MigrateEvent::Duplicate {
            actor,
            message,
            applied,
        } => {
            line.str("mesh-migrate: duplicate actor=");
            line.hex(&actor.0);
            line.str(" message=");
            line.u64(*message);
            line.str(" applied=");
            line.u64(*applied);
        }
        MigrateEvent::Capability { actor, reason } => {
            line.str("mesh-migrate: capability actor=");
            line.hex(&actor.0);
            line.str(" reason=");
            line.str(reason.as_str());
        }
        MigrateEvent::Reject { actor, reason } => {
            line.str("mesh-migrate: reject actor=");
            match actor {
                Some(actor) => line.hex(&actor.0),
                None => line.push(b'-'),
            }
            line.str(" reason=");
            line.str(reason.as_str());
        }
        MigrateEvent::Aborted { actor, reason } => {
            line.str("mesh-migrate: abort actor=");
            match actor {
                Some(actor) => line.hex(&actor.0),
                None => line.push(b'-'),
            }
            line.str(" reason=");
            line.str(reason.as_str());
        }
    }
    line.push(b'\n');
    log(services, line.as_bytes());
}

/// Build the M2-5 L0 migration policy when `mesh.route=2` requests it.
///
/// The migration policy is opt-in on top of the route.  It reuses the same
/// confirmed membership view and authority key; it does not create a second
/// directory or a second authority.
fn migrate_config(
    services: &Services,
    self_id: NodeId,
    join: Option<&JoinNode>,
) -> Option<MigrateNode> {
    let mut text = [0u8; 32];
    let Some(len) = config_any(services, &[b"mesh.route", b"r"], &mut text) else {
        log(services, b"mesh-migrate: disabled reason=config\n");
        return None;
    };
    if &text[..len] != b"2" {
        log(services, b"mesh-migrate: disabled reason=config\n");
        return None;
    }
    let Some(join) = join else {
        log(services, b"mesh-migrate: disabled reason=join\n");
        return None;
    };
    let Some(migrate) = MigrateNode::new(
        self_id,
        join.root(),
        join.authority_key(),
        matches!(join.role(), Role::Root),
    ) else {
        log(services, b"mesh-migrate: disabled reason=config\n");
        return None;
    };
    let mut line = Line::new();
    line.str("mesh-migrate: enabled role=");
    line.str(join.role().as_str());
    line.push(b'\n');
    log(services, line.as_bytes());
    Some(migrate)
}

/// The root-only signing seam for capability issuance and home moves.
struct JoinSigner<'a>(&'a JoinNode);

impl RouteSigner for JoinSigner<'_> {
    fn key(&self) -> NodeKey {
        self.0.authority_key()
    }

    fn sign(&self, message: &[u8]) -> Option<[u8; 64]> {
        self.0.sign_authority(message)
    }
}

impl MigrateSigner for JoinSigner<'_> {
    fn key(&self) -> NodeKey {
        self.0.authority_key()
    }

    fn sign(&self, message: &[u8]) -> Option<[u8; 64]> {
        self.0.sign_authority(message)
    }
}

/// Read the join configuration and build the state machine.
///
/// The authority root is identified by `mesh.authority` (the MAC-derived
/// `NodeId`).  The member needs `mesh.pub`; only the root has `mesh.key`.
/// A missing or malformed key disables the join but
/// leaves the M2-1 transport running, so an M2-1-only configuration is still
/// valid.
fn join_config(
    services: &Services,
    self_id: NodeId,
    now: u64,
) -> Result<Option<(JoinNode, EpochStore)>, &'static str> {
    let mut text = [0u8; 128];
    let Some(n) = config_any(services, &[b"mesh.authority", b"a"], &mut text) else {
        return Ok(None);
    };
    let Some(authority) = parse_node_id(&text[..n]) else {
        return Err("authority");
    };
    let seed_epoch = match config(services, b"mesh.epoch", &mut text) {
        Some(n) => parse_u32(&text[..n]).ok_or("epoch")?,
        None => 1,
    };
    let configured_key =
        config(services, b"mesh.pub", &mut text).and_then(|n| parse_key(&text[..n]));
    let mut public_key = configured_key;
    let join = if self_id == authority {
        let Some(n) = config(services, b"mesh.key", &mut text) else {
            return Err("authority-seed");
        };
        let Some(seed) = parse_seed(&text[..n]) else {
            return Err("authority-seed");
        };
        let signing = mesh_join::SigningAuthority::from_bytes(&seed);
        public_key = Some(signing.public());
        // The root derives its public key; `mesh.pub` is optional here and is
        // only a provisioning check when present.
        if let Some(key) = configured_key {
            if signing.public() != key {
                return Err("key-mismatch");
            }
        }
        JoinNode::new_root(self_id, signing, seed_epoch, now)
    } else {
        let Some(key) = configured_key else {
            return Err("authority-key");
        };
        JoinNode::new_member(self_id, authority, key, seed_epoch, now)
    };
    let mut join = match join {
        Ok(join) => join,
        Err(error) => return Err(error.as_str()),
    };

    // Restore the last confirmed view before logging the local state, so the
    // `mesh-join: local` line and the first join request use the persisted
    // epoch rather than the seed.  A corrupt record disables the join instead
    // of silently lowering the node back to the seed.
    let store = restore_epoch_store(services, &mut join, now)?;

    let role = join.role();
    let mut line = Line::new();
    line.str("mesh-join: enabled role=");
    line.str(role.as_str());
    line.str(" seed_epoch=");
    line.u64(seed_epoch as u64);
    line.str(" authority=");
    line.node(&authority);
    line.str(" key=");
    if let Some(key) = public_key {
        line.hex(&key.0);
    } else {
        line.str("-");
    }
    line.push(b'\n');
    log(services, line.as_bytes());

    let snapshot = join.snapshot();
    let mut line = Line::new();
    line.str("mesh-join: local role=");
    line.str(role.as_str());
    line.str(" epoch=");
    line.u64(snapshot.epoch as u64);
    line.str(" members=");
    line.u64(snapshot.len() as u64);
    line.str(" digest=");
    line.digest(&snapshot.digest());
    line.str(" snapshot=");
    line.nodes(&snapshot.members);
    line.str(match role {
        Role::Root => " state=confirmed\n",
        Role::Member => " state=pending\n",
    });
    log(services, line.as_bytes());
    Ok(Some((join, store)))
}

/// A record read back from the scratch volume.
enum StoredEpoch {
    /// The volume exists but carries no epoch record yet.
    Empty,
    /// A root-signed record.
    Record(EpochRecord),
    /// A root-signed confirmation accepted by a member.
    Confirmation(EpochConfirmation),
}

/// Why the store could not be read.
enum StoreReadError {
    /// No target answered.  Persistence is not configured; the caller logs
    /// this and continues from the seed, but never claims a restored epoch.
    Unavailable,
    /// The target answered with bytes that are not a valid record.
    Corrupt(&'static str),
}

/// Read and validate the epoch store without trusting it.
fn read_epoch_store(services: &Services) -> Result<StoredEpoch, StoreReadError> {
    // SAFETY: single-threaded module; STORE_BUF is the only store buffer and
    // the sectors are contiguous.
    let buffer = unsafe { &mut *core::ptr::addr_of_mut!(STORE_BUF) };
    for sector in 0..STORE_SECTORS {
        let offset = sector * 512;
        let rc = unsafe {
            (services.block_read)(
                STORE_TARGET,
                STORE_LBA + sector as u32,
                buffer[offset..].as_mut_ptr(),
            )
        };
        if rc != 0 {
            return Err(StoreReadError::Unavailable);
        }
    }
    if &buffer[..8] != STORE_MAGIC {
        return Ok(StoredEpoch::Empty);
    }
    if buffer[8] != STORE_VERSION {
        return Err(StoreReadError::Corrupt("version"));
    }
    if u16::from_le_bytes([buffer[10], buffer[11]]) != 0 {
        return Err(StoreReadError::Corrupt("flags"));
    }
    let length = u32::from_le_bytes([buffer[12], buffer[13], buffer[14], buffer[15]]) as usize;
    if length == 0 || length > STORE_BYTES - STORE_HEADER {
        return Err(StoreReadError::Corrupt("length"));
    }
    let body = &buffer[STORE_HEADER..STORE_HEADER + length];
    match buffer[9] {
        STORE_KIND_RECORD => EpochRecord::decode(body)
            .map(StoredEpoch::Record)
            .map_err(|error| StoreReadError::Corrupt(error.as_str())),
        STORE_KIND_CONFIRM => match JoinMessage::decode(body) {
            Ok(JoinMessage::Confirm(confirmation)) => Ok(StoredEpoch::Confirmation(confirmation)),
            Ok(_) => Err(StoreReadError::Corrupt("confirmation-kind")),
            Err(error) => Err(StoreReadError::Corrupt(error.as_str())),
        },
        _ => Err(StoreReadError::Corrupt("kind")),
    }
}

/// Write one record to the epoch store.  The caller has already bounded it.
fn write_epoch_store(services: &Services, kind: u8, body: &[u8]) -> Result<(), &'static str> {
    if body.is_empty() || body.len() > STORE_BYTES - STORE_HEADER {
        return Err("length");
    }
    // SAFETY: single-threaded module; STORE_BUF is the only store buffer.
    let buffer = unsafe { &mut *core::ptr::addr_of_mut!(STORE_BUF) };
    buffer.fill(0);
    buffer[..8].copy_from_slice(STORE_MAGIC);
    buffer[8] = STORE_VERSION;
    buffer[9] = kind;
    buffer[12..16].copy_from_slice(&(body.len() as u32).to_le_bytes());
    buffer[STORE_HEADER..STORE_HEADER + body.len()].copy_from_slice(body);
    for sector in 0..STORE_SECTORS {
        let offset = sector * 512;
        let rc = unsafe {
            (services.block_write)(
                STORE_TARGET,
                STORE_LBA + sector as u32,
                buffer[offset..].as_ptr(),
            )
        };
        if rc != 0 {
            return Err("write");
        }
    }
    // Read the sectors back through the same service.  A store that reports
    // success but cannot be restored must fail here, not at the next boot.
    for sector in 0..STORE_SECTORS {
        let offset = sector * 512;
        let mut check = [0u8; 512];
        let rc = unsafe {
            (services.block_read)(STORE_TARGET, STORE_LBA + sector as u32, check.as_mut_ptr())
        };
        if rc != 0 {
            return Err("verify-read");
        }
        if check != buffer[offset..offset + 512] {
            return Err("verify");
        }
    }
    Ok(())
}

/// Restore a signed confirmed view from the scratch volume.
fn restore_epoch_store(
    services: &Services,
    join: &mut JoinNode,
    now: u64,
) -> Result<EpochStore, &'static str> {
    let mut store = EpochStore::empty(join.role() == Role::Root, false);
    if services.version < 11 {
        log(
            services,
            b"epoch-store: unavailable reason=abi (block_write needs ABI 11)\n",
        );
        return Ok(store);
    }
    let stored = match read_epoch_store(services) {
        Ok(stored) => stored,
        Err(StoreReadError::Unavailable) => {
            log(
                services,
                b"epoch-store: unavailable target=3 lba=8 (persistence not configured)\n",
            );
            return Ok(store);
        }
        Err(StoreReadError::Corrupt(reason)) => {
            let mut line = Line::new();
            line.str("epoch-store: corrupt reason=");
            line.str(reason);
            line.push(b'\n');
            log(services, line.as_bytes());
            return Err("epoch-store");
        }
    };
    store.available = true;
    match stored {
        StoredEpoch::Empty => {
            log(services, b"epoch-store: empty target=3 lba=8\n");
            Ok(store)
        }
        StoredEpoch::Record(record) => {
            if join.role() != Role::Root {
                log(services, b"epoch-store: corrupt reason=member-record\n");
                return Err("epoch-store");
            }
            match join.restore_record(&record, now) {
                Ok(snapshot) => {
                    let mut line = Line::new();
                    line.str("epoch-store: restored epoch=");
                    line.u64(snapshot.epoch as u64);
                    line.str(" members=");
                    line.u64(snapshot.len() as u64);
                    line.str(" digest=");
                    line.digest(&snapshot.digest());
                    line.str(" source=root-record\n");
                    log(services, line.as_bytes());
                    store.remember(snapshot.epoch, snapshot.digest());
                    Ok(store)
                }
                Err(error) => {
                    let mut line = Line::new();
                    line.str("epoch-store: corrupt reason=");
                    line.str(error.as_str());
                    line.push(b'\n');
                    log(services, line.as_bytes());
                    Err("epoch-store")
                }
            }
        }
        StoredEpoch::Confirmation(confirmation) => {
            if join.role() != Role::Member {
                log(services, b"epoch-store: corrupt reason=root-confirmation\n");
                return Err("epoch-store");
            }
            match join.restore_confirmation(&confirmation, now) {
                Ok(snapshot) => {
                    let mut line = Line::new();
                    line.str("epoch-store: restored epoch=");
                    line.u64(snapshot.epoch as u64);
                    line.str(" members=");
                    line.u64(snapshot.len() as u64);
                    line.str(" digest=");
                    line.digest(&snapshot.digest());
                    line.str(" source=confirmation\n");
                    log(services, line.as_bytes());
                    store.remember(snapshot.epoch, snapshot.digest());
                    Ok(store)
                }
                Err(error) => {
                    let mut line = Line::new();
                    line.str("epoch-store: corrupt reason=");
                    line.str(error.as_str());
                    line.push(b'\n');
                    log(services, line.as_bytes());
                    Err("epoch-store")
                }
            }
        }
    }
}

/// Read the configured M2 fixture object count.
///
/// `mesh.obj` is optional, bounded to [`HOME_FIXTURE_MAX`] and defaults to
/// [`HOME_FIXTURE_DEFAULT`].  An invalid value falls back to the default
/// rather than disabling the join: the count is evidence configuration, not
/// membership authority.
fn home_object_count(services: &Services) -> u32 {
    let mut text = [0u8; 32];
    let Some(len) = config(services, b"mesh.obj", &mut text) else {
        return HOME_FIXTURE_DEFAULT;
    };
    match parse_u32(&text[..len]) {
        Some(value) if (1..=HOME_FIXTURE_MAX).contains(&value) => value,
        _ => HOME_FIXTURE_DEFAULT,
    }
}

/// Read the optional M3 shard-evidence object count.
///
/// `mesh.shard=<count>` enables `/mesh/shard`.  The value is bounded to
/// [`SHARD_FIXTURE_MAX`]; an invalid or out-of-range value falls back to
/// [`SHARD_FIXTURE_DEFAULT`] rather than disabling the endpoint.  Absence
/// keeps the M2 evidence surface exactly as it was.
fn shard_object_count(services: &Services) -> u32 {
    let mut text = [0u8; 32];
    let Some(len) = config(services, b"mesh.shard", &mut text) else {
        return 0;
    };
    match parse_u32(&text[..len]) {
        Some(value) if (1..=SHARD_FIXTURE_MAX).contains(&value) => value,
        _ => SHARD_FIXTURE_DEFAULT,
    }
}

fn config(services: &Services, key: &[u8], out: &mut [u8]) -> Option<usize> {
    let n = unsafe { (services.config_get)(key.as_ptr(), key.len(), out.as_mut_ptr(), out.len()) };
    if n <= 0 {
        None
    } else {
        Some((n as usize).min(out.len()))
    }
}

/// Read the first present key.  Direct-transport boots use the short `a`,
/// `d`, `p` and `r` names so the DS1386 298-byte environment fits.
fn config_any(services: &Services, keys: &[&[u8]], out: &mut [u8]) -> Option<usize> {
    keys.iter().find_map(|key| config(services, key, out))
}

fn identity(services: &Services) -> Option<NodeId> {
    let mut text = [0u8; 256];
    let n = unsafe { (services.info)(0, text.as_mut_ptr(), text.len()) };
    if n <= 0 {
        return None;
    }
    let text = &text[..n as usize];
    let marker = b"mac: ";
    let start = text
        .windows(marker.len())
        .position(|window| window == marker)?
        + marker.len();
    parse_mac_address(&text[start..])
}

fn parse_node_id(text: &[u8]) -> Option<NodeId> {
    if text.contains(&b':') {
        return parse_mac_address(text);
    }
    let mut id = [0u8; 16];
    parse_hex(text, &mut id)?;
    Some(NodeId(id))
}

fn parse_mac_address(text: &[u8]) -> Option<NodeId> {
    let mut id = [0u8; 16];
    let mut index = 0;
    for byte in id.iter_mut().take(6) {
        let high = hex(text.get(index)?)?;
        let low = hex(text.get(index + 1)?)?;
        *byte = (high << 4) | low;
        index += 2;
        if index < 17 {
            if text.get(index) != Some(&b':') {
                return None;
            }
            index += 1;
        }
    }
    Some(NodeId(id))
}

fn parse_key(text: &[u8]) -> Option<NodeKey> {
    let mut key = [0u8; 32];
    parse_hex(text, &mut key)?;
    Some(NodeKey(key))
}

fn parse_seed(text: &[u8]) -> Option<[u8; 32]> {
    let mut seed = [0u8; 32];
    parse_hex(text, &mut seed)?;
    Some(seed)
}

fn parse_hex(text: &[u8], out: &mut [u8]) -> Option<()> {
    if text.len() != out.len() * 2 {
        return None;
    }
    for (index, byte) in out.iter_mut().enumerate() {
        let high = hex(&text[index * 2])?;
        let low = hex(&text[index * 2 + 1])?;
        *byte = (high << 4) | low;
    }
    Some(())
}

/// Parse `a.b.c.d:port` from the `mesh.direct` value.
fn parse_direct(text: &[u8]) -> Option<DirectPeer> {
    let colon = text.iter().position(|byte| *byte == b':')?;
    let (ip, port) = (&text[..colon], &text[colon + 1..]);
    let mut address = [0u8; 4];
    let mut parts = ip.split(|byte| *byte == b'.');
    for slot in &mut address {
        *slot = parse_u32(parts.next()?).filter(|value| *value <= 255)? as u8;
    }
    if parts.next().is_some() {
        return None;
    }
    let port = parse_u32(port).filter(|value| *value > 0 && *value <= 65535)? as u16;
    Some(DirectPeer { address, port })
}

/// Parse a 32-character upper- or lower-case NodeId.
fn parse_node_hex(text: &[u8]) -> Option<NodeId> {
    if text.len() != 32 {
        return None;
    }
    let mut node = [0u8; 16];
    for (index, slot) in node.iter_mut().enumerate() {
        *slot = hex(&text[index * 2])? << 4 | hex(&text[index * 2 + 1])?;
    }
    Some(NodeId(node))
}

fn parse_u32(text: &[u8]) -> Option<u32> {
    if text.is_empty() {
        return None;
    }
    let mut value = 0u32;
    for byte in text {
        if !byte.is_ascii_digit() {
            return None;
        }
        value = value.checked_mul(10)?.checked_add((byte - b'0') as u32)?;
    }
    Some(value)
}

fn hex(byte: &u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

fn log(services: &Services, bytes: &[u8]) {
    (services.write)(bytes.as_ptr(), bytes.len());
}

struct Line {
    bytes: [u8; LINE_CAP],
    len: usize,
}

impl Line {
    fn new() -> Self {
        Self {
            bytes: [0; LINE_CAP],
            len: 0,
        }
    }

    fn as_bytes(&self) -> &[u8] {
        &self.bytes[..self.len]
    }

    fn push(&mut self, byte: u8) {
        if self.len < self.bytes.len() {
            self.bytes[self.len] = byte;
            self.len += 1;
        }
    }

    fn str(&mut self, text: &str) {
        for byte in text.as_bytes() {
            self.push(*byte);
        }
    }

    fn u64(&mut self, mut value: u64) {
        let mut digits = [0u8; 20];
        let mut count = 0;
        loop {
            digits[count] = b'0' + (value % 10) as u8;
            value /= 10;
            count += 1;
            if value == 0 {
                break;
            }
        }
        while count > 0 {
            count -= 1;
            self.push(digits[count]);
        }
    }

    fn i32(&mut self, value: i32) {
        if value < 0 {
            self.push(b'-');
            self.u64(value.unsigned_abs() as u64);
        } else {
            self.u64(value as u64);
        }
    }

    fn node(&mut self, node: &NodeId) {
        self.hex(node.as_bytes());
    }

    fn nodes(&mut self, nodes: &[NodeId]) {
        for (index, node) in nodes.iter().enumerate() {
            if index > 0 {
                self.push(b',');
            }
            self.node(node);
        }
    }

    fn digest(&mut self, digest: &[u8; 16]) {
        self.hex(digest);
    }

    fn hex(&mut self, bytes: &[u8]) {
        const HEX: &[u8; 16] = b"0123456789ABCDEF";
        for byte in bytes {
            self.push(HEX[(byte >> 4) as usize]);
            self.push(HEX[(byte & 0x0f) as usize]);
        }
    }
}
