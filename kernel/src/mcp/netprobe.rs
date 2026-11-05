// SPDX-License-Identifier: GPL-3.0-only
//! Module UDP probe for the M3-ADR-0006 datagram slice.
//!
//! This is a gate, not a product: it exists so two real kernels can exchange a
//! datagram over the ABI 16 `udp_*` services and prove send, receive, the
//! sender address and the reply path in one run.  It is inert unless the
//! DS1386 environment carries `netprobe.peer` (`a.b.c.d:port`), so a normal
//! boot keeps the MCP module's behaviour exactly as it was.
//!
//! The probe sends `PING <name> <seq>` to its peer and answers a received
//! `PING` with `PONG <name> <seq>` back to the sender reported by `udp_recv`.
//! The host relay in `tools/m2-rig` forwards each side's datagrams to the other
//! side, so a `PONG` arriving at the sender proves a full two-kernel round trip
//! through the kernel's UDP service and the non-blocking Seeq transmit path.
//!
//! The kernel owns the socket and the interface queue; the probe owns framing,
//! its own sequence numbers and its retry policy on a full ring.

use module_abi::Services;

/// Longest accepted `netprobe.peer` value.
const PEER_MAX: usize = 32;
/// Longest accepted `netprobe.name` value.
const NAME_MAX: usize = 16;
/// Datagram payload cap.  `PING <name> <seq>` fits with room to spare.
const PAYLOAD_MAX: usize = 64;
/// Module UDP slot the probe owns.
const SLOT: u32 = 0;
/// Guest port used when `netprobe.port` is absent.
const DEFAULT_PORT: u16 = 7001;
/// Name used when `netprobe.name` is absent.
const DEFAULT_NAME: &[u8] = b"node";
/// Send period in milliseconds.
const PERIOD_MS: u64 = 500;
/// Delay before retrying a datagram the socket ring could not take.
const RETRY_MS: u64 = 20;
/// Datagrams drained per tick.  The socket holds eight; four is enough to keep
/// the serial log bounded while never leaving a reply queued for long.
const DRAIN_PER_TICK: usize = 4;
/// Log the first events of each kind, then every `LOG_EVERY`th.
const LOG_FIRST: u32 = 4;
const LOG_EVERY: u32 = 20;
/// Longest log line the probe builds.
const LINE_CAP: usize = 128;

/// One peer endpoint parsed from `netprobe.peer`.
#[derive(Clone, Copy)]
struct Peer {
    address: [u8; 4],
    port: u16,
}

/// Probe state.  A module is single-threaded and its tick is not re-entrant,
/// so plain statics behind `unsafe` follow the MCP module's existing pattern.
struct Probe {
    active: bool,
    port: u16,
    name: [u8; NAME_MAX],
    name_len: usize,
    peer: Peer,
    seq: u32,
    next_ms: u64,
    logged_tx: u32,
    logged_rx: u32,
    logged_pong: u32,
    logged_full: bool,
}

impl Probe {
    const EMPTY: Self = Self {
        active: false,
        port: 0,
        name: [0; NAME_MAX],
        name_len: 0,
        peer: Peer {
            address: [0; 4],
            port: 0,
        },
        seq: 0,
        next_ms: 0,
        logged_tx: 0,
        logged_rx: 0,
        logged_pong: 0,
        logged_full: false,
    };
}

static mut PROBE: Probe = Probe::EMPTY;

/// Arm the probe from the DS1386 environment.
///
/// Returns `true` when `netprobe.peer` is present and a socket was opened.
/// Absence or a malformed value leaves the probe inert; a malformed value logs
/// one line so a misconfigured gate is visible rather than silent.
///
/// # Safety
/// `services` must be the kernel's live service table.
pub unsafe fn init(services: &Services) -> bool {
    let mut text = [0u8; PEER_MAX];
    let Some(len) = config(services, b"netprobe.peer", &mut text) else {
        return false;
    };
    let Some(peer) = parse_endpoint(&text[..len]) else {
        log(services, b"netprobe: bad peer\n");
        return false;
    };

    let port = config_port(services, b"netprobe.port", DEFAULT_PORT);
    let mut name = [0u8; NAME_MAX];
    let name_len = match config(services, b"netprobe.name", &mut name) {
        Some(len) if len > 0 => len,
        _ => {
            name[..DEFAULT_NAME.len()].copy_from_slice(DEFAULT_NAME);
            DEFAULT_NAME.len()
        }
    };

    // SAFETY: single-threaded module; the probe table is only touched here and
    // in `tick`.
    let probe = unsafe { &mut *core::ptr::addr_of_mut!(PROBE) };
    probe.peer = peer;
    probe.port = port;
    probe.name[..name_len].copy_from_slice(&name[..name_len]);
    probe.name_len = name_len;
    probe.next_ms = (services.clock_ms)();

    // SAFETY: the service table is live and `SLOT` is in range.
    if unsafe { (services.udp_open)(SLOT, port) } != 0 {
        let mut buf = [0u8; LINE_CAP];
        let mut line = Line::new(&mut buf);
        line.str("netprobe: open failed port=");
        line.u32(port as u32);
        line.byte(b'\n');
        line.flush(services);
        return false;
    }
    probe.active = true;

    let mut buf = [0u8; LINE_CAP];
    let mut line = Line::new(&mut buf);
    line.str("netprobe: up name=");
    line.bytes(&probe.name[..probe.name_len]);
    line.str(" port=");
    line.u32(port as u32);
    line.str(" peer=");
    line.address(peer.address);
    line.byte(b':');
    line.u32(peer.port as u32);
    line.byte(b'\n');
    line.flush(services);
    true
}

/// One probe tick: drain received datagrams, then send a `PING` when due.
pub fn tick(services: &Services) {
    // SAFETY: single-threaded module; `init` published the state.
    let probe = unsafe { &mut *core::ptr::addr_of_mut!(PROBE) };
    if !probe.active {
        return;
    }

    let mut buf = [0u8; PAYLOAD_MAX];
    for _ in 0..DRAIN_PER_TICK {
        let mut src_address = 0u32;
        let mut src_port = 0u16;
        // SAFETY: the buffers outlive the call and the kernel writes at most
        // `buf.len()` bytes.
        let received = unsafe {
            (services.udp_recv)(
                SLOT,
                buf.as_mut_ptr(),
                buf.len(),
                &mut src_address,
                &mut src_port,
            )
        };
        if received <= 0 {
            break;
        }
        handle(
            services,
            probe,
            &buf[..received as usize],
            src_address,
            src_port,
        );
    }

    let now = (services.clock_ms)();
    if now < probe.next_ms {
        return;
    }

    let mut payload = [0u8; PAYLOAD_MAX];
    let mut line = Line::new(&mut payload);
    line.str("PING ");
    line.bytes(&probe.name[..probe.name_len]);
    line.byte(b' ');
    line.u32(probe.seq);
    let len = line.len;

    // SAFETY: the payload outlives the call and `peer` was validated in
    // `init`.
    let sent = unsafe {
        (services.udp_send)(
            SLOT,
            u32::from_be_bytes(probe.peer.address),
            probe.peer.port,
            payload.as_ptr(),
            len,
        )
    };
    if sent > 0 {
        probe.next_ms = now + PERIOD_MS;
        if should_log(probe.logged_tx) {
            let mut out = [0u8; LINE_CAP];
            let mut line = Line::new(&mut out);
            line.str("netprobe: ping seq=");
            line.u32(probe.seq);
            line.byte(b'\n');
            line.flush(services);
        }
        probe.logged_tx = probe.logged_tx.saturating_add(1);
        probe.seq = probe.seq.wrapping_add(1);
    } else {
        // 0 is a full socket ring: keep the sequence number and retry shortly.
        // -1 is a missing socket; the retry simply keeps failing.
        probe.next_ms = now + RETRY_MS;
        if !probe.logged_full {
            probe.logged_full = true;
            log(
                services,
                if sent == 0 {
                    b"netprobe: tx full\n"
                } else {
                    b"netprobe: no socket\n"
                },
            );
        }
    }
}

/// Answer a `PING` and record a `PONG`.
fn handle(services: &Services, probe: &mut Probe, data: &[u8], src_address: u32, src_port: u16) {
    let mut parts = data.split(|byte| *byte == b' ');
    let kind = parts.next().unwrap_or(b"");
    let peer_name = parts.next().unwrap_or(b"");
    let seq = parts.next().and_then(parse_u32).unwrap_or(0);
    let octets = src_address.to_be_bytes();

    if kind == b"PING" {
        let mut reply = [0u8; PAYLOAD_MAX];
        let mut line = Line::new(&mut reply);
        line.str("PONG ");
        line.bytes(&probe.name[..probe.name_len]);
        line.byte(b' ');
        line.u32(seq);
        let len = line.len;
        // SAFETY: the reply outlives the call; the sender was just reported by
        // `udp_recv`.
        let _ = unsafe { (services.udp_send)(SLOT, src_address, src_port, reply.as_ptr(), len) };
        if should_log(probe.logged_rx) {
            let mut out = [0u8; LINE_CAP];
            let mut line = Line::new(&mut out);
            line.str("netprobe: ping-rx from=");
            line.address(octets);
            line.byte(b':');
            line.u32(src_port as u32);
            line.str(" peer=");
            line.bytes(peer_name);
            line.str(" seq=");
            line.u32(seq);
            line.byte(b'\n');
            line.flush(services);
        }
        probe.logged_rx = probe.logged_rx.saturating_add(1);
    } else if kind == b"PONG" {
        if should_log(probe.logged_pong) {
            let mut out = [0u8; LINE_CAP];
            let mut line = Line::new(&mut out);
            line.str("netprobe: pong from=");
            line.address(octets);
            line.byte(b':');
            line.u32(src_port as u32);
            line.str(" peer=");
            line.bytes(peer_name);
            line.str(" seq=");
            line.u32(seq);
            line.byte(b'\n');
            line.flush(services);
        }
        probe.logged_pong = probe.logged_pong.saturating_add(1);
    }
}

/// Log the first few events of a kind, then a sample.
fn should_log(count: u32) -> bool {
    count < LOG_FIRST || count.is_multiple_of(LOG_EVERY)
}

/// Read a PROM environment value.
fn config(services: &Services, key: &[u8], out: &mut [u8]) -> Option<usize> {
    // SAFETY: the kernel writes at most `out.len()` bytes into `out`.
    let n = unsafe { (services.config_get)(key.as_ptr(), key.len(), out.as_mut_ptr(), out.len()) };
    if n <= 0 {
        None
    } else {
        Some((n as usize).min(out.len()))
    }
}

/// Read a decimal environment value, falling back to `default`.
fn config_port(services: &Services, key: &[u8], default: u16) -> u16 {
    let mut text = [0u8; 8];
    match config(services, key, &mut text).and_then(|len| parse_u32(&text[..len])) {
        Some(value) if value > 0 && value <= 65535 => value as u16,
        _ => default,
    }
}

/// Parse `a.b.c.d:port`.
fn parse_endpoint(text: &[u8]) -> Option<Peer> {
    let colon = text.iter().position(|byte| *byte == b':')?;
    let (ip, port) = (&text[..colon], &text[colon + 1..]);
    let mut address = [0u8; 4];
    let mut parts = ip.split(|byte| *byte == b'.');
    for slot in &mut address {
        let value = parse_u32(parts.next()?)?;
        if value > 255 {
            return None;
        }
        *slot = value as u8;
    }
    if parts.next().is_some() {
        return None;
    }
    let port = parse_u32(port)?;
    if port == 0 || port > 65535 {
        return None;
    }
    Some(Peer {
        address,
        port: port as u16,
    })
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

fn log(services: &Services, bytes: &[u8]) {
    (services.write)(bytes.as_ptr(), bytes.len());
}

/// A fixed-buffer line writer.  The module has an allocator, but a tick must
/// not depend on it, so every log line is built on the stack.
struct Line<'a> {
    buf: &'a mut [u8],
    len: usize,
}

impl<'a> Line<'a> {
    fn new(buf: &'a mut [u8]) -> Self {
        Self { buf, len: 0 }
    }

    fn push(&mut self, byte: u8) {
        if self.len < self.buf.len() {
            self.buf[self.len] = byte;
            self.len += 1;
        }
    }

    fn byte(&mut self, byte: u8) {
        self.push(byte);
    }

    fn str(&mut self, text: &str) {
        self.bytes(text.as_bytes());
    }

    fn bytes(&mut self, text: &[u8]) {
        for byte in text {
            self.push(*byte);
        }
    }

    /// Append an IPv4 address in dotted-quad form.
    fn address(&mut self, octets: [u8; 4]) {
        for (index, octet) in octets.iter().enumerate() {
            if index > 0 {
                self.push(b'.');
            }
            self.u32(*octet as u32);
        }
    }

    fn u32(&mut self, value: u32) {
        let mut digits = [0u8; 10];
        let mut count = 0;
        let mut value = value;
        loop {
            digits[count] = b'0' + (value % 10) as u8;
            count += 1;
            value /= 10;
            if value == 0 {
                break;
            }
        }
        while count > 0 {
            count -= 1;
            self.push(digits[count]);
        }
    }

    /// Write the line to the console.
    fn flush(&self, services: &Services) {
        (services.write)(self.buf.as_ptr(), self.len);
    }
}
