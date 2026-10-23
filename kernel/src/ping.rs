// SPDX-License-Identifier: GPL-3.0-only
//! ICMP echo client ("ping") over the shared smoltcp stack.

use smoltcp::socket::icmp::{
    Endpoint as IcmpEndpoint, PacketBuffer as IcmpPacketBuffer,
    PacketMetadata as IcmpPacketMetadata, Socket as IcmpSocket,
};
use smoltcp::wire::{IpAddress, Ipv4Address};

use crate::net;

const COUNT: u16 = 3;
const TIMEOUT_MS: i64 = 1_500;
const IDENT: u16 = 0x1D17;

static mut PING_RX_META: [IcmpPacketMetadata; 4] = [IcmpPacketMetadata::EMPTY; 4];
static mut PING_TX_META: [IcmpPacketMetadata; 4] = [IcmpPacketMetadata::EMPTY; 4];
static mut PING_RX_BUF: [u8; 512] = [0; 512];
static mut PING_TX_BUF: [u8; 512] = [0; 512];

/// Result of a ping run.
pub struct Summary {
    pub sent: u16,
    pub received: u16,
    pub avg_ms: u64,
}

/// Send `COUNT` echo requests and collect the replies.
pub fn run(ip: Ipv4Address) -> Option<Summary> {
    let handle = net::sockets_with(|sockets| {
        // SAFETY: the buffers are static and used by one run at a time.
        let rx = IcmpPacketBuffer::new(unsafe { &mut PING_RX_META[..] }, unsafe {
            &mut PING_RX_BUF[..]
        });
        let tx = IcmpPacketBuffer::new(unsafe { &mut PING_TX_META[..] }, unsafe {
            &mut PING_TX_BUF[..]
        });
        sockets.add(IcmpSocket::new(rx, tx))
    })?;

    let bound = net::sockets_with(|sockets| {
        sockets
            .get_mut::<IcmpSocket>(handle)
            .bind(IcmpEndpoint::Ident(IDENT))
            .is_ok()
    });
    if bound != Some(true) {
        cleanup(handle);
        return None;
    }

    let mut summary = Summary {
        sent: 0,
        received: 0,
        avg_ms: 0,
    };
    let mut total_ms = 0u64;

    for seq in 0..COUNT {
        let packet = build_echo(IDENT, seq);
        let started = net::clock_now().total_millis();
        let sent = net::sockets_with(|sockets| {
            sockets
                .get_mut::<IcmpSocket>(handle)
                .send_slice(&packet, IpAddress::Ipv4(ip))
                .is_ok()
        });
        if sent != Some(true) {
            break;
        }
        summary.sent += 1;

        let deadline = started + TIMEOUT_MS;
        loop {
            net::poll();

            let replied = net::sockets_with(|sockets| {
                let addr = IpAddress::Ipv4(ip);
                let sock = sockets.get_mut::<IcmpSocket>(handle);
                let mut buf = [0u8; 128];
                match sock.recv_slice(&mut buf) {
                    Ok((n, _src)) => {
                        let _ = addr;
                        n >= 8
                            && buf[0] == 0 // echo reply
                            && u16::from_be_bytes([buf[4], buf[5]]) == IDENT
                    }
                    Err(_) => false,
                }
            });
            if replied == Some(true) {
                summary.received += 1;
                total_ms += (net::clock_now().total_millis() - started).max(0) as u64;
                break;
            }
            if net::clock_now().total_millis() > deadline {
                break;
            }
        }
    }

    cleanup(handle);
    if summary.received > 0 {
        summary.avg_ms = total_ms / summary.received as u64;
    }
    Some(summary)
}

fn cleanup(handle: smoltcp::iface::SocketHandle) {
    net::sockets_with(|sockets| {
        let _ = sockets.remove(handle);
    });
}

/// Build an echo request: 8-byte ICMP header plus a 32-byte pattern.
fn build_echo(ident: u16, seq: u16) -> [u8; 40] {
    let mut packet = [0u8; 40];
    packet[0] = 8; // echo request
    packet[1] = 0; // code
    packet[4..6].copy_from_slice(&ident.to_be_bytes());
    packet[6..8].copy_from_slice(&seq.to_be_bytes());
    for (i, b) in packet.iter_mut().enumerate().skip(8) {
        *b = (i as u8) ^ 0x5A;
    }
    let sum = checksum(&packet);
    packet[2..4].copy_from_slice(&sum.to_be_bytes());
    packet
}

fn checksum(data: &[u8]) -> u16 {
    let mut sum = 0u32;
    let mut i = 0;
    while i + 1 < data.len() {
        sum += u16::from_be_bytes([data[i], data[i + 1]]) as u32;
        i += 2;
    }
    if i < data.len() {
        sum += (data[i] as u32) << 8;
    }
    while (sum >> 16) != 0 {
        sum = (sum & 0xFFFF) + (sum >> 16);
    }
    !(sum as u16)
}
