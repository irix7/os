// SPDX-License-Identifier: GPL-3.0-only
//! Minimal DNS A-record resolver over the shared UDP stack.
//!
//! Sends one recursive query for an IPv4 address and walks the answer
//! section, following compression pointers in names.  Names are resolved
//! per request; the console is not a hot path.

use smoltcp::socket::udp::{Socket as UdpSocket, UdpMetadata};
use smoltcp::storage::{PacketBuffer, PacketMetadata};
use smoltcp::wire::{IpAddress, IpEndpoint, Ipv4Address};

use crate::net;

const DNS_PORT: u16 = 53;
const DNS_LOCAL_PORT: u16 = 49153;
const MAX_PACKET: usize = 512;

static mut DNS_RX_META: [PacketMetadata<UdpMetadata>; 1] = [PacketMetadata::EMPTY; 1];
static mut DNS_TX_META: [PacketMetadata<UdpMetadata>; 1] = [PacketMetadata::EMPTY; 1];
static mut DNS_RX_BUF: [u8; MAX_PACKET] = [0; MAX_PACKET];
static mut DNS_TX_BUF: [u8; MAX_PACKET] = [0; MAX_PACKET];

/// Resolve `name` to an IPv4 address using `server`.
pub fn resolve(name: &str, server: Ipv4Address) -> Option<Ipv4Address> {
    let mut query = [0u8; MAX_PACKET];
    let query_len = build_query(name, &mut query)?;

    let handle = net::sockets_with(|sockets| {
        // SAFETY: the buffers are static and used by one query at a time.
        let rx = PacketBuffer::new(unsafe { &mut DNS_RX_META[..] }, unsafe {
            &mut DNS_RX_BUF[..]
        });
        let tx = PacketBuffer::new(unsafe { &mut DNS_TX_META[..] }, unsafe {
            &mut DNS_TX_BUF[..]
        });
        sockets.add(UdpSocket::new(rx, tx))
    })?;

    let bound = net::sockets_with(|sockets| {
        sockets
            .get_mut::<UdpSocket>(handle)
            .bind(DNS_LOCAL_PORT)
            .is_ok()
    });
    if bound != Some(true) {
        cleanup(handle);
        return None;
    }

    let remote = IpEndpoint::new(IpAddress::Ipv4(server), DNS_PORT);
    let sent = net::sockets_with(|sockets| {
        sockets
            .get_mut::<UdpSocket>(handle)
            .send_slice(&query[..query_len], remote)
            .is_ok()
    });
    if sent != Some(true) {
        cleanup(handle);
        return None;
    }

    let deadline = net::clock_now().total_millis() + 5_000;
    loop {
        net::poll();

        let reply = net::sockets_with(|sockets| {
            let sock = sockets.get_mut::<UdpSocket>(handle);
            match sock.recv() {
                Ok((data, _meta)) => {
                    let mut packet = [0u8; MAX_PACKET];
                    let n = data.len().min(MAX_PACKET);
                    packet[..n].copy_from_slice(&data[..n]);
                    Some((n, packet))
                }
                Err(_) => None,
            }
        });

        if let Some(Some((n, packet))) = reply {
            cleanup(handle);
            return parse_answer(&packet[..n]);
        }

        if net::clock_now().total_millis() > deadline {
            cleanup(handle);
            return None;
        }
    }
}

fn cleanup(handle: smoltcp::iface::SocketHandle) {
    net::sockets_with(|sockets| {
        let _ = sockets.remove(handle);
    });
}

/// Build a recursive A-record query.  Returns the packet length.
fn build_query(name: &str, packet: &mut [u8; MAX_PACKET]) -> Option<usize> {
    // Header: ID, flags RD, QDCOUNT=1.
    packet[0] = 0x49;
    packet[1] = 0x52;
    packet[2] = 0x01; // standard query, recursion desired
    packet[3] = 0x00;
    packet[4] = 0x00;
    packet[5] = 0x01;
    let mut len = 12;

    for label in name.split('.') {
        if label.is_empty() || label.len() > 63 {
            return None;
        }
        if len + 1 + label.len() + 5 > MAX_PACKET {
            return None;
        }
        packet[len] = label.len() as u8;
        len += 1;
        packet[len..len + label.len()].copy_from_slice(label.as_bytes());
        len += label.len();
    }
    packet[len] = 0; // root label
    len += 1;

    packet[len..len + 2].copy_from_slice(&1u16.to_be_bytes()); // QTYPE A
    packet[len + 2..len + 4].copy_from_slice(&1u16.to_be_bytes()); // QCLASS IN
    len += 4;
    Some(len)
}

/// Walk the answer section and return the first A record.
fn parse_answer(packet: &[u8]) -> Option<Ipv4Address> {
    if packet.len() < 12 {
        return None;
    }
    let qdcount = u16::from_be_bytes([packet[4], packet[5]]) as usize;
    let ancount = u16::from_be_bytes([packet[6], packet[7]]) as usize;

    let mut pos = 12;
    for _ in 0..qdcount {
        pos = skip_name(packet, pos)?;
        pos = pos.checked_add(4)?; // QTYPE + QCLASS
        if pos > packet.len() {
            return None;
        }
    }

    for _ in 0..ancount {
        pos = skip_name(packet, pos)?;
        if pos + 10 > packet.len() {
            return None;
        }
        let rtype = u16::from_be_bytes([packet[pos], packet[pos + 1]]);
        let rclass = u16::from_be_bytes([packet[pos + 2], packet[pos + 3]]);
        let rdlength = u16::from_be_bytes([packet[pos + 8], packet[pos + 9]]) as usize;
        pos += 10;
        if pos + rdlength > packet.len() {
            return None;
        }
        if rtype == 1 && rclass == 1 && rdlength == 4 {
            return Some(Ipv4Address::new(
                packet[pos],
                packet[pos + 1],
                packet[pos + 2],
                packet[pos + 3],
            ));
        }
        pos += rdlength;
    }
    None
}

/// Advance past a (possibly compressed) DNS name.
fn skip_name(packet: &[u8], mut pos: usize) -> Option<usize> {
    loop {
        let len = *packet.get(pos)?;
        if len & 0xC0 == 0xC0 {
            // Compression pointer: two bytes and the name ends here.
            return pos.checked_add(2).filter(|p| *p <= packet.len());
        }
        if len == 0 {
            return Some(pos + 1);
        }
        pos = pos.checked_add(1 + len as usize)?;
        if pos > packet.len() {
            return None;
        }
    }
}
