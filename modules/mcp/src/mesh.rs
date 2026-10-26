// SPDX-License-Identifier: GPL-3.0-only
//! Interim M2-1 mesh listener inside the MCP module.
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
//! the M2-1 probe exchange:
//!
//! ```text
//! connect:  Request/Ping (dst = zero, src = self)
//!           Response/Ping (dst = requester, reply_to = request id)
//! idle:     Control/Ping every 5 s
//! ```
//!
//! A frame whose body is not the 16-byte source `NodeId` is rejected.  The
//! first decoded peer request logs `mesh-transport: peer=...`; a completed
//! response logs one `mesh-transport: pass ...` line for the rig evidence.

use mesh_transport::{
    decode_record, ByteSink, Error as TransportError, Heartbeat, PendingTable, SendQueue, Stats,
    StreamDecoder, WriteOutcome, RECEIVE_DEADLINE_MS,
};
use module_abi::Services;
use sys_primitives::{
    mesh_flags, MeshEnvelope, MeshHeader, MessageFrame, MessageKind, NodeId, OpCode, StreamId, Tag,
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
const LINE_CAP: usize = 192;

enum Phase {
    Arm,
    Waiting,
    Connected,
}

struct Incoming {
    header: MeshHeader,
    kind: MessageKind,
    opcode: OpCode,
    body: [u8; 16],
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
    pass_logged: bool,
    listen_logged: bool,
    last_rx_ms: u64,
}

static mut MESH: Option<Mesh> = None;
static mut ENABLED: bool = false;

/// Whether the mesh listener is configured for this boot.
pub fn enabled() -> bool {
    // SAFETY: single-threaded kernel; the flag is written once at entry.
    unsafe { ENABLED }
}

/// Initialise the listener from the service table.
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
    let n = unsafe {
        (services.config_get)(
            b"mesh.peer".as_ptr(),
            b"mesh.peer".len(),
            peer.as_mut_ptr(),
            peer.len(),
        )
    };
    if n <= 0 {
        return false;
    }
    let Some(self_id) = identity(services) else {
        log(services, b"mesh-transport: identity unavailable\n");
        return false;
    };
    let now = (services.clock_ms)();
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
            Phase::Arm => self.arm(services),
            Phase::Waiting => self.waiting(services, now),
            Phase::Connected => self.connected(services, now),
        }
    }

    fn arm(&mut self, services: &Services) {
        if unsafe { (services.tcp_listen)(MESH_SLOT, MESH_PORT) } == 0 {
            self.phase = Phase::Waiting;
            if !self.listen_logged {
                self.listen_logged = true;
                log(services, b"mesh-transport: listening on port 7000\n");
            }
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
                self.last_rx_ms = now;
                self.heartbeat.reset(now);
                self.phase = Phase::Connected;
                log(services, b"mesh-transport: connected\n");
                self.send_handshake(now);
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
                    if envelope.frame.body.len() > 16 {
                        return Err(TransportError::Envelope(sys_primitives::Error::BadLength));
                    }
                    let mut body = [0u8; 16];
                    body[..envelope.frame.body.len()].copy_from_slice(envelope.frame.body);
                    Incoming {
                        header: envelope.header,
                        kind: envelope.frame.header.kind,
                        opcode: envelope.frame.header.opcode,
                        body,
                        body_len: envelope.frame.body.len(),
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
        if incoming.body_len != 16 || incoming.body != *incoming.header.src.as_bytes() {
            self.stats.protocol_rejects = self.stats.protocol_rejects.saturating_add(1);
            return Err(TransportError::Envelope(sys_primitives::Error::Protocol));
        }
        self.learn_peer(services, incoming.header.src)?;

        match (incoming.kind, incoming.opcode) {
            (MessageKind::Request, OpCode::Ping) => {
                self.queue_ping(
                    now,
                    MessageKind::Response,
                    OpCode::Ping,
                    incoming.header.src,
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
            _ => {
                self.stats.protocol_rejects = self.stats.protocol_rejects.saturating_add(1);
            }
        }
        Ok(())
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
        self.last_rx_ms = now;
        self.phase = Phase::Arm;

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

fn identity(services: &Services) -> Option<NodeId> {
    let mut text = [0u8; 256];
    let n = unsafe { (services.info)(0, text.as_mut_ptr(), text.len()) };
    if n <= 0 {
        return None;
    }
    parse_mac(&text[..n as usize])
}

fn parse_mac(text: &[u8]) -> Option<NodeId> {
    let marker = b"mac: ";
    let start = text
        .windows(marker.len())
        .position(|window| window == marker)?
        + marker.len();
    let mut id = [0u8; 16];
    let mut index = start;
    for byte in id.iter_mut().take(6) {
        let high = hex(text.get(index)?)?;
        let low = hex(text.get(index + 1)?)?;
        *byte = (high << 4) | low;
        index += 2;
        if index < start + 17 {
            if text.get(index) != Some(&b':') {
                return None;
            }
            index += 1;
        }
    }
    Some(NodeId(id))
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

    fn node(&mut self, node: &NodeId) {
        const HEX: &[u8; 16] = b"0123456789ABCDEF";
        for byte in node.as_bytes() {
            self.push(HEX[(byte >> 4) as usize]);
            self.push(HEX[(byte & 0x0f) as usize]);
        }
    }
}
