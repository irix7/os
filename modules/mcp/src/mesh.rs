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

use mesh_join::{JoinEvent, JoinMessage, JoinNode, Role, Tick, MAX_BODY as JOIN_BODY_MAX};
use mesh_transport::{
    decode_record, ByteSink, Error as TransportError, Heartbeat, PendingTable, SendQueue, Stats,
    StreamDecoder, WriteOutcome, RECEIVE_DEADLINE_MS,
};
use module_abi::Services;
use sys_primitives::{
    mesh_flags, MeshEnvelope, MeshHeader, MessageFrame, MessageKind, NodeId, NodeKey, OpCode,
    StreamId, Tag,
};

/// Module TCP slot owned by the mesh listener when `mesh.peer` is present.
pub const MESH_SLOT: u32 = 1;
/// Guest port for the M2 mesh listener.
pub const MESH_PORT: u16 = 7000;
/// Module TCP slot kept for the MCP HTTP server when the mesh is enabled.
pub const HTTP_SLOT: u32 = 0;

const READ_CHUNK: usize = 1024;
const READS_PER_TICK: usize = 8;
const PENDING_CAPACITY: u32 = 8;
const LINE_CAP: usize = 256;

enum Phase {
    Arm,
    Waiting,
    Connected,
}

struct Incoming {
    header: MeshHeader,
    kind: MessageKind,
    opcode: OpCode,
    body_len: usize,
}

struct Mesh {
    phase: Phase,
    self_id: NodeId,
    peer: Option<NodeId>,
    peer_confirmed: bool,
    decoder: StreamDecoder,
    queue: SendQueue,
    pending: PendingTable,
    stats: Stats,
    heartbeat: Heartbeat,
    join: Option<JoinNode>,
    join_timeout_logged: bool,
    last_arm_log_ms: u64,
    pass_logged: bool,
    listen_logged: bool,
    last_rx_ms: u64,
}

/// Receive buffer for one join body.  A static keeps the 4 KiB bound out of
/// the frame loop's stack frame; the module is single-threaded.
static mut JOIN_RX: [u8; JOIN_BODY_MAX] = [0; JOIN_BODY_MAX];
/// Encode buffer for one outgoing join body.
static mut JOIN_TX: [u8; JOIN_BODY_MAX] = [0; JOIN_BODY_MAX];

static mut MESH: Option<Mesh> = None;
static mut ENABLED: bool = false;

/// Whether the mesh listener is configured for this boot.
pub fn enabled() -> bool {
    // SAFETY: single-threaded kernel; the flag is written once at entry.
    unsafe { ENABLED }
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
    if config(services, b"mesh.peer", &mut peer).is_none() {
        return false;
    }
    let Some(self_id) = identity(services) else {
        log(services, b"mesh-transport: identity unavailable\n");
        return false;
    };
    let now = (services.clock_ms)();
    let join = match join_config(services, self_id, now) {
        Ok(join) => join,
        Err(label) => {
            let mut line = Line::new();
            line.str("mesh-join: disabled reason=");
            line.str(label);
            line.push(b'\n');
            log(services, line.as_bytes());
            None
        }
    };
    let mesh = Mesh {
        phase: Phase::Arm,
        self_id,
        peer: None,
        peer_confirmed: false,
        decoder: StreamDecoder::new(),
        queue: SendQueue::new(),
        pending: PendingTable::new(PENDING_CAPACITY),
        stats: Stats::default(),
        heartbeat: Heartbeat::m2(),
        join,
        join_timeout_logged: false,
        last_arm_log_ms: 0,
        pass_logged: false,
        listen_logged: false,
        last_rx_ms: now,
    };
    unsafe {
        MESH = Some(mesh);
        ENABLED = true;
    }
    log(services, b"mesh-transport: enabled\n");
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

impl Mesh {
    fn tick(&mut self, services: &Services) {
        let now = (services.clock_ms)();
        match self.phase {
            Phase::Arm => self.arm(services, now),
            Phase::Waiting => self.waiting(services, now),
            Phase::Connected => self.connected(services, now),
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
            2 => {
                self.stats.connects = self.stats.connects.saturating_add(1);
                self.decoder.reset();
                self.queue.clear();
                self.pending = PendingTable::new(PENDING_CAPACITY);
                self.peer = None;
                self.peer_confirmed = false;
                self.pass_logged = false;
                self.join_timeout_logged = false;
                self.last_rx_ms = now;
                self.heartbeat.reset(now);
                self.phase = Phase::Connected;
                log(services, b"mesh-transport: connected\n");
                self.send_handshake(now);
                let request = self.join.as_mut().and_then(|join| join.on_link_up(now));
                if let Some(request) = request {
                    self.send_join(services, JoinMessage::Request(request), NodeId::ZERO, now);
                }
                self.flush(services, now);
            }
            3 => {
                (services.tcp_close)(MESH_SLOT);
                self.phase = Phase::Arm;
            }
            0 => self.phase = Phase::Arm,
            _ => {}
        }
    }

    fn connected(&mut self, services: &Services, now: u64) {
        if (services.tcp_state)(MESH_SLOT) != 2 {
            self.disconnect(services, now, "closed");
            return;
        }

        if self.decoder.buffered_len() > 0
            && now.saturating_sub(self.last_rx_ms) >= RECEIVE_DEADLINE_MS
        {
            self.stats.receive_deadlines = self.stats.receive_deadlines.saturating_add(1);
            self.disconnect(services, now, "receive-deadline");
            return;
        }
        if self.queue.send_deadline_expired(now) {
            self.stats.send_deadlines = self.stats.send_deadlines.saturating_add(1);
            self.disconnect(services, now, "send-deadline");
            return;
        }
        if self.heartbeat.timed_out(now) {
            self.stats.heartbeat_timeouts = self.stats.heartbeat_timeouts.saturating_add(1);
            self.disconnect(services, now, "heartbeat-timeout");
            return;
        }

        let mut buffer = [0u8; READ_CHUNK];
        for _ in 0..READS_PER_TICK {
            let read = unsafe { (services.tcp_recv)(MESH_SLOT, buffer.as_mut_ptr(), buffer.len()) };
            if read <= 0 {
                break;
            }
            let count = read as usize;
            self.stats.bytes_received = self.stats.bytes_received.saturating_add(count as u64);
            self.last_rx_ms = now;
            self.heartbeat.on_receive(now);
            if let Err(error) = self.feed(services, &buffer[..count], now) {
                self.reject(services, now, error);
                return;
            }
        }

        let tick = self
            .join
            .as_mut()
            .map(|join| (join.poll(now), join.attempts()));
        match tick {
            Some((Tick::Send(request), attempts)) => {
                self.send_join(services, JoinMessage::Request(request), NodeId::ZERO, now);
                let mut line = Line::new();
                line.str("mesh-join: retry attempt=");
                line.u64(attempts as u64);
                line.push(b'\n');
                log(services, line.as_bytes());
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

        if self.heartbeat.due(now) {
            let destination = self.peer.unwrap_or(NodeId::ZERO);
            if self.queue_ping(
                now,
                MessageKind::Control,
                OpCode::Ping,
                destination,
                0,
                0,
                Tag(0),
            ) {
                self.stats.heartbeats_sent = self.stats.heartbeats_sent.saturating_add(1);
                self.heartbeat.on_transmit(now);
            }
        }
        let expired = self.pending.expire(now);
        self.stats.expired_tags = self.stats.expired_tags.saturating_add(expired as u64);
        self.flush(services, now);
        self.maybe_log_pass(services);
    }

    fn send_handshake(&mut self, now: u64) {
        let Ok(tag) = self
            .pending
            .begin_at(now.saturating_add(RECEIVE_DEADLINE_MS))
        else {
            return;
        };
        if self.queue_ping(
            now,
            MessageKind::Request,
            OpCode::Ping,
            NodeId::ZERO,
            tag.0 as u64,
            0,
            tag,
        ) {
            self.stats.heartbeats_sent = self.stats.heartbeats_sent.saturating_add(1);
            self.heartbeat.on_transmit(now);
        } else {
            let _ = self.pending.complete(tag);
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
                    // SAFETY: single-threaded module; JOIN_RX is the only
                    // frame-body buffer and the copied slice is read before
                    // the next record is decoded.
                    let storage = unsafe { &mut *core::ptr::addr_of_mut!(JOIN_RX) };
                    storage[..body.len()].copy_from_slice(body);
                    Incoming {
                        header: envelope.header,
                        kind: envelope.frame.header.kind,
                        opcode: envelope.frame.header.opcode,
                        body_len: body.len(),
                    }
                };
                self.decoder.consume_record()?;
                self.stats.frames_received = self.stats.frames_received.saturating_add(1);
                self.handle(services, incoming, now)?;
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
            | (MessageKind::Completion, OpCode::Hello) => {
                if self.join.is_none() {
                    self.stats.protocol_rejects = self.stats.protocol_rejects.saturating_add(1);
                    return Err(TransportError::Envelope(sys_primitives::Error::Protocol));
                }
            }
            _ => {
                self.stats.protocol_rejects = self.stats.protocol_rejects.saturating_add(1);
                return Err(TransportError::Envelope(sys_primitives::Error::Protocol));
            }
        }
        self.learn_peer(services, source)?;

        match (incoming.kind, incoming.opcode) {
            (MessageKind::Request, OpCode::Ping) => {
                self.queue_ping(
                    now,
                    MessageKind::Response,
                    OpCode::Ping,
                    source,
                    0,
                    incoming.header.request_id,
                    Tag(0),
                );
            }
            (MessageKind::Response, OpCode::Ping) => {
                let tag = Tag(incoming.header.reply_to as u32);
                let _ = self.pending.complete(tag);
                self.peer_confirmed = true;
            }
            (MessageKind::Control, OpCode::Ping) => {}
            (MessageKind::Request, OpCode::Hello)
            | (MessageKind::Response, OpCode::Hello)
            | (MessageKind::Completion, OpCode::Hello) => {
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
                let outcome = match &mut self.join {
                    Some(join) => join.on_message(source, message, now),
                    None => return Ok(()),
                };
                self.after_join(services, outcome, source, now);
            }
            _ => {}
        }
        Ok(())
    }

    fn after_join(
        &mut self,
        services: &Services,
        outcome: mesh_join::Outcome,
        source: NodeId,
        now: u64,
    ) {
        match &outcome.event {
            JoinEvent::Confirmed(snapshot) => {
                self.log_snapshot(services, "mesh-join: confirm", snapshot);
                self.log_snapshot(services, "mesh-join: converged role=root", snapshot);
            }
            JoinEvent::Converged(snapshot) => {
                self.join_timeout_logged = false;
                self.log_snapshot(services, "mesh-join: converged role=member", snapshot);
            }
            JoinEvent::Duplicate(snapshot) => {
                self.log_snapshot(services, "mesh-join: duplicate", snapshot);
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
            } => {
                let mut line = Line::new();
                line.str("mesh-join: ack from=");
                line.node(node);
                line.str(" epoch=");
                line.u64(*epoch as u64);
                line.str(" digest=");
                line.digest(digest);
                line.push(b'\n');
                log(services, line.as_bytes());
            }
            JoinEvent::Rejected(label) => {
                // The envelope and frame were well formed; the membership
                // policy rejected the body.  Keep this out of the transport
                // reject counter, which the M2-1 pass line reports.
                let mut line = Line::new();
                line.str("mesh-join: reject reason=");
                line.str(label);
                line.push(b'\n');
                log(services, line.as_bytes());
            }
            JoinEvent::None => {}
        }
        if let Some(reply) = outcome.reply {
            self.send_join(services, reply, source, now);
        }
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
        line.str(" snapshot=");
        line.nodes(&snapshot.members);
        line.push(b'\n');
        log(services, line.as_bytes());
    }

    fn send_join(
        &mut self,
        services: &Services,
        message: JoinMessage,
        reply_to_source: NodeId,
        now: u64,
    ) {
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
                return;
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
        };
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
                let mut line = Line::new();
                line.str("mesh-join: ack-sent epoch=");
                line.u64(ack.epoch as u64);
                line.str(" digest=");
                line.digest(&ack.digest);
                line.push(b'\n');
                log(services, line.as_bytes());
            }
            _ => {}
        }
        let body = &buffer[..len];
        if !self.queue_hello(kind, destination, request_id, reply_to, body, now) {
            let mut line = Line::new();
            line.str("mesh-join: send-failed kind=");
            line.str(message.kind());
            line.push(b'\n');
            log(services, line.as_bytes());
        }
        self.flush(services, now);
    }

    fn queue_hello(
        &mut self,
        kind: MessageKind,
        destination: NodeId,
        request_id: u64,
        reply_to: u64,
        body: &[u8],
        now: u64,
    ) -> bool {
        let Ok(frame) = MessageFrame::inline(kind, OpCode::Hello, StreamId(2), Tag(0), body) else {
            return false;
        };
        let mut header = MeshHeader::new(self.self_id, destination, self.self_id);
        header.flags = mesh_flags::PROBE;
        if kind != MessageKind::Request {
            header.flags |= mesh_flags::REPLY;
        }
        header.request_id = request_id;
        header.reply_to = reply_to;
        header.ttl = MeshHeader::MAX_HOPS;
        let envelope = MeshEnvelope { header, frame };
        self.queue.push_record(&envelope, now).is_ok()
    }

    fn learn_peer(&mut self, services: &Services, source: NodeId) -> Result<(), TransportError> {
        match self.peer {
            None => {
                self.peer = Some(source);
                let mut line = Line::new();
                line.str("mesh-transport: peer=");
                line.node(&source);
                line.push(b'\n');
                log(services, line.as_bytes());
                Ok(())
            }
            Some(peer) if peer == source => Ok(()),
            Some(_) => {
                self.stats.protocol_rejects = self.stats.protocol_rejects.saturating_add(1);
                Err(TransportError::Envelope(sys_primitives::Error::Protocol))
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn queue_ping(
        &mut self,
        now: u64,
        kind: MessageKind,
        opcode: OpCode,
        destination: NodeId,
        request_id: u64,
        reply_to: u64,
        tag: Tag,
    ) -> bool {
        let body = self.self_id.as_bytes();
        let Ok(frame) = MessageFrame::inline(kind, opcode, StreamId(1), tag, body) else {
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
        let envelope = MeshEnvelope { header, frame };
        self.queue.push_record(&envelope, now).is_ok()
    }

    fn flush(&mut self, services: &Services, now: u64) {
        let mut sink = ServiceSink {
            services,
            slot: MESH_SLOT,
        };
        let report = self.queue.flush(&mut sink);
        self.stats.bytes_sent = self
            .stats
            .bytes_sent
            .saturating_add(report.bytes_written as u64);
        self.stats.frames_sent = self
            .stats
            .frames_sent
            .saturating_add(report.frames_written as u64);
        if report.blocked {
            self.stats.short_writes = self.stats.short_writes.saturating_add(1);
        }
        if report.closed {
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
        (services.tcp_abort)(MESH_SLOT);
        self.decoder.reset();
        self.queue.clear();
        self.pending = PendingTable::new(PENDING_CAPACITY);
        self.peer = None;
        self.peer_confirmed = false;
        self.pass_logged = false;
        self.join_timeout_logged = false;
        self.last_rx_ms = now;
        self.phase = Phase::Arm;
        if let Some(join) = &mut self.join {
            join.on_link_down();
        }

        let mut line = Line::new();
        line.str("mesh-transport: disconnected reason=");
        line.str(reason);
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

fn body_limit(kind: MessageKind, opcode: OpCode) -> usize {
    match (kind, opcode) {
        (MessageKind::Request, OpCode::Hello)
        | (MessageKind::Response, OpCode::Hello)
        | (MessageKind::Completion, OpCode::Hello) => JOIN_BODY_MAX,
        _ => 16,
    }
}

fn body_bytes(len: usize) -> &'static [u8] {
    // SAFETY: single-threaded module; JOIN_RX holds the current frame body and
    // the slice is consumed before the next record is decoded.
    unsafe { core::slice::from_raw_parts(core::ptr::addr_of!(JOIN_RX) as *const u8, len) }
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
) -> Result<Option<JoinNode>, &'static str> {
    let mut text = [0u8; 128];
    let Some(n) = config(services, b"mesh.authority", &mut text) else {
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
    let join = match join {
        Ok(join) => join,
        Err(error) => return Err(error.as_str()),
    };

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
    Ok(Some(join))
}

fn config(services: &Services, key: &[u8], out: &mut [u8]) -> Option<usize> {
    let n = unsafe { (services.config_get)(key.as_ptr(), key.len(), out.as_mut_ptr(), out.len()) };
    if n <= 0 {
        None
    } else {
        Some((n as usize).min(out.len()))
    }
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
