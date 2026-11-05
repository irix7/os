// SPDX-License-Identifier: GPL-3.0-only
//! Socket slots for the in-kernel services.
//!
//! MCP, the mesh listener and the ports self-test use a fixed set of TCP and
//! UDP slots through the service table (`kernel_core::service::SERVICES`).  The
//! table keeps one smoltcp socket per slot.  The kernel has no module
//! multiplexer: the slots are plain kernel state.  The idle loop asks
//! [`note_ready`] whether a slot needs service before it parks.

use smoltcp::iface::{Interface, SocketHandle, SocketSet};
use smoltcp::socket::udp::{PacketBuffer, RecvError, SendError, Socket as UdpSocket, UdpMetadata};
use smoltcp::storage::PacketMetadata;

use crate::net_irq;

/// Whether any TCP slot has a readable, writable or state-change event.
pub fn any_ready() -> bool {
    (0..MODULE_TCP_SLOTS as u32).any(|slot| net_irq::slot_ready(tcp_events(slot)))
}

/// Clock time (ms) of the last idle pass that found a ready slot.
static mut LAST_ACTIVITY_MS: u64 = 0;

/// Report whether a slot is ready, and remember when it was.  The idle loop
/// calls this once per pass, before the services run.
pub fn note_ready() -> bool {
    let ready = any_ready();
    if ready {
        let now = crate::services::clock_ms();
        // SAFETY: single-threaded kernel.
        unsafe { LAST_ACTIVITY_MS = now };
    }
    ready
}

/// Whether a slot was ready recently enough that a short wait can catch the
/// peer's next record.  This is a wake hint, not a work queue.
pub fn recently_active() -> bool {
    let now = crate::services::clock_ms();
    // SAFETY: single-threaded kernel.
    unsafe { now.saturating_sub(LAST_ACTIVITY_MS) < 50 }
}

/// The Indy's stack, for the shared module TCP slot table.
struct IndyStack;

impl module_tcp::Stack for IndyStack {
    fn with_sockets<R>(f: impl FnOnce(&mut SocketSet<'static>) -> R) -> Option<R> {
        crate::net::sockets_with(f)
    }

    fn with_stack<R>(f: impl FnOnce(&mut Interface, &mut SocketSet<'static>) -> R) -> Option<R> {
        crate::net::with_stack(f)
    }
}

const MODULE_TCP_SLOTS: usize = 8;
/// The receive ring is deliberately larger than the two-node case needs.
///
/// Under the M3 host hub several member records share the root's single mesh
/// slot while the module writes a bounded serial burst.  A 2 KiB buffer fills
/// during that burst, advertises a tiny window, and then trickles the rest of
/// a 388-byte record through the host forward's zero-window probe; the 10 s
/// receive deadline fires on a healthy link.  16 KiB holds a full eight-member
/// join/refresh burst, so the window never collapses during logging.
const MODULE_TCP_BUFFER: usize = 16384;

static mut MODULE_TCP: module_tcp::Slots<MODULE_TCP_SLOTS, MODULE_TCP_BUFFER> =
    module_tcp::Slots::new();

fn module_tcp() -> &'static mut module_tcp::Slots<MODULE_TCP_SLOTS, MODULE_TCP_BUFFER> {
    // SAFETY: single-threaded kernel; the slot table is used only here.
    unsafe { &mut *core::ptr::addr_of_mut!(MODULE_TCP) }
}

pub(crate) fn tcp_listen(slot: u32, port: u16) -> i32 {
    let result = module_tcp().listen::<IndyStack>(slot, port);
    result
}

pub(crate) fn tcp_connect(slot: u32, address: u32, port: u16) -> i32 {
    let result = module_tcp().connect::<IndyStack>(slot, address, port);
    result
}

/// Level-triggered readiness for one module TCP slot.  The bits are recomputed
/// from the live socket; there is no event queue and no lost edge.
pub(crate) fn tcp_events(slot: u32) -> u32 {
    module_tcp().events::<IndyStack>(slot)
}

/// Number of module TCP slots the kernel provides.
pub(crate) fn tcp_slots() -> u32 {
    module_tcp().slots()
}

pub(crate) fn tcp_state(slot: u32) -> u32 {
    module_tcp().state::<IndyStack>(slot)
}

pub(crate) fn tcp_recv(slot: u32, out: &mut [u8]) -> isize {
    module_tcp().recv_into::<IndyStack>(slot, out)
}

pub(crate) fn tcp_send(slot: u32, bytes: &[u8]) -> isize {
    module_tcp().send_from::<IndyStack>(slot, bytes)
}

pub(crate) fn tcp_close(slot: u32) {
    module_tcp().close::<IndyStack>(slot);
}

pub(crate) fn tcp_abort(slot: u32) {
    module_tcp().abort::<IndyStack>(slot);
}

/// Module UDP slots.  A datagram transport needs one slot per peer link plus
/// one for a listener; eight matches the TCP policy and fits the socket set.
const MODULE_UDP_SLOTS: usize = 8;

/// One datagram buffer per direction per slot.  A 2 KiB buffer holds the
/// largest QUIC datagram that fits an Ethernet path (1500-byte MTU after IP
/// and UDP headers) with room for a coalesced short-header packet.
const MODULE_UDP_BUFFER: usize = 2048;

/// Datagrams queued per direction per slot.  The payload ring is the larger
/// bound; this caps how many small datagrams can wait in one poll.
const MODULE_UDP_PACKETS: usize = 8;

static mut MODULE_UDP: [Option<SocketHandle>; MODULE_UDP_SLOTS] = [None; MODULE_UDP_SLOTS];
static mut MODULE_UDP_RX: [[u8; MODULE_UDP_BUFFER]; MODULE_UDP_SLOTS] =
    [[0; MODULE_UDP_BUFFER]; MODULE_UDP_SLOTS];
static mut MODULE_UDP_TX: [[u8; MODULE_UDP_BUFFER]; MODULE_UDP_SLOTS] =
    [[0; MODULE_UDP_BUFFER]; MODULE_UDP_SLOTS];
static mut MODULE_UDP_RX_META: [[PacketMetadata<UdpMetadata>; MODULE_UDP_PACKETS];
    MODULE_UDP_SLOTS] = [[PacketMetadata::EMPTY; MODULE_UDP_PACKETS]; MODULE_UDP_SLOTS];
static mut MODULE_UDP_TX_META: [[PacketMetadata<UdpMetadata>; MODULE_UDP_PACKETS];
    MODULE_UDP_SLOTS] = [[PacketMetadata::EMPTY; MODULE_UDP_PACKETS]; MODULE_UDP_SLOTS];

fn udp_handle(slot: u32) -> Option<SocketHandle> {
    let slot = slot as usize;
    if slot >= MODULE_UDP_SLOTS {
        return None;
    }
    // SAFETY: single-threaded kernel; the table is only written by the
    // service functions below.
    unsafe { MODULE_UDP[slot] }
}

/// Remove any UDP socket previously owned by `slot`.
fn clear_udp_slot(slot: usize) {
    if let Some(old) = unsafe { MODULE_UDP[slot] } {
        let _ = crate::net::sockets_with(|sockets| sockets.remove(old));
        // SAFETY: single-threaded; the old socket has been removed.
        unsafe { MODULE_UDP[slot] = None };
    }
}

pub(crate) fn udp_open(slot: u32, port: u16) -> i32 {
    let slot_usize = slot as usize;
    if slot_usize >= MODULE_UDP_SLOTS || port == 0 {
        return -1;
    }
    clear_udp_slot(slot_usize);
    // SAFETY: each slot's buffers are owned by at most one socket at a time.
    let rx = unsafe { &mut *core::ptr::addr_of_mut!(MODULE_UDP_RX[slot_usize]) };
    let tx = unsafe { &mut *core::ptr::addr_of_mut!(MODULE_UDP_TX[slot_usize]) };
    let rx_meta = unsafe { &mut *core::ptr::addr_of_mut!(MODULE_UDP_RX_META[slot_usize]) };
    let tx_meta = unsafe { &mut *core::ptr::addr_of_mut!(MODULE_UDP_TX_META[slot_usize]) };
    let mut socket = UdpSocket::new(
        PacketBuffer::new(&mut rx_meta[..], &mut rx[..]),
        PacketBuffer::new(&mut tx_meta[..], &mut tx[..]),
    );
    if socket.bind(port).is_err() {
        return -1;
    }
    let Some(handle) = crate::net::sockets_with(|sockets| sockets.add(socket)) else {
        return -1;
    };
    // SAFETY: single-threaded.
    unsafe { MODULE_UDP[slot_usize] = Some(handle) };
    0
}

pub(crate) fn udp_send(slot: u32, address: u32, port: u16, bytes: &[u8]) -> isize {
    if bytes.is_empty() || address == 0 || port == 0 {
        return -1;
    }
    let Some(handle) = udp_handle(slot) else {
        return -1;
    };
    // `address` is the IPv4 address in network byte order.
    let octets = address.to_be_bytes();
    let remote = smoltcp::wire::Ipv4Address::new(octets[0], octets[1], octets[2], octets[3]);
    let result = crate::net::sockets_with(|sockets| {
        sockets
            .get_mut::<UdpSocket>(handle)
            .send_slice(bytes, (smoltcp::wire::IpAddress::Ipv4(remote), port))
    });
    match result {
        Some(Ok(())) => bytes.len() as isize,
        // The socket buffer is full: the module retries after the next poll.
        Some(Err(SendError::BufferFull)) => 0,
        _ => -1,
    }
}

pub(crate) fn udp_recv(
    slot: u32,
    out: &mut [u8],
    src_address: Option<&mut u32>,
    src_port: Option<&mut u16>,
) -> isize {
    if out.is_empty() {
        return -1;
    }
    let Some(handle) = udp_handle(slot) else {
        return -1;
    };
    let result =
        crate::net::sockets_with(|sockets| sockets.get_mut::<UdpSocket>(handle).recv_slice(out));
    match result {
        Some(Ok((n, meta))) => {
            if let smoltcp::wire::IpAddress::Ipv4(address) = meta.endpoint.addr {
                let octets = address.octets();
                if let Some(src_address) = src_address {
                    *src_address = u32::from_be_bytes(octets);
                }
                if let Some(src_port) = src_port {
                    *src_port = meta.endpoint.port;
                }
            }
            n as isize
        }
        Some(Err(RecvError::Exhausted)) => 0,
        // The datagram did not fit the caller's buffer and was dropped.
        Some(Err(RecvError::Truncated)) => -2,
        None => -1,
    }
}

pub(crate) fn udp_close(slot: u32) {
    let slot = slot as usize;
    if slot < MODULE_UDP_SLOTS {
        clear_udp_slot(slot);
    }
}

/// Number of module UDP slots the kernel provides.
pub(crate) fn udp_slots() -> u32 {
    MODULE_UDP_SLOTS as u32
}

pub(crate) fn udp_state(slot: u32, address: Option<&mut u32>, port: Option<&mut u16>) -> i32 {
    let Some(handle) = udp_handle(slot) else {
        return -1;
    };
    let endpoint = crate::net::sockets_with(|sockets| sockets.get::<UdpSocket>(handle).endpoint());
    let Some(endpoint) = endpoint else {
        return -1;
    };
    if let Some(port) = port {
        *port = endpoint.port;
    }
    if let Some(address) = address {
        let local = crate::net::ip_addr().map(|ip| u32::from_be_bytes(ip.octets()));
        *address = local.unwrap_or(0);
    }
    0
}
