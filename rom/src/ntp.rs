//! NTP (Network Time Protocol) client.
//!
//! Sends an NTPv4 request to an NTP server and parses the response to obtain
//! the current date and time. Required before TLS/HTTPS can validate
//! certificate validity periods.
//!
//! NTP timestamp format: 64-bit fixed-point, seconds since 00:00 1 Jan 1900.
//! We convert to Unix epoch (1 Jan 1970) by subtracting
//! 2,208,988,800 seconds.

use crate::net::SeeqDevice;
use alloc::boxed::Box;
use alloc::format;
use smoltcp::iface::{Interface, SocketSet};
use smoltcp::socket::udp::{Socket as UdpSocket, UdpMetadata};
use smoltcp::storage::PacketBuffer;
use smoltcp::time::Duration;
use smoltcp::wire::{IpAddress, IpEndpoint, Ipv4Address};

/// NTP server port.
const NTP_PORT: u16 = 123;

/// NTP packet size (48 bytes).
const NTP_PACKET_SIZE: usize = 48;

/// Seconds between NTP epoch (1 Jan 1900) and Unix epoch (1 Jan 1970).
const NTP_TO_UNIX_DELTA: u64 = 2_208_988_800;

/// Local port used for the NTP client socket.
const NTP_LOCAL_PORT: u16 = 12345;

/// Build an NTPv4 client request packet (mode 3, version 4).
fn build_ntp_request() -> [u8; NTP_PACKET_SIZE] {
    let mut pkt = [0u8; NTP_PACKET_SIZE];
    // LI=0 (no warning), VN=4, Mode=3 (client)
    pkt[0] = 0b00_100_011;
    pkt
}

/// Parse NTP response: extract transmit timestamp (bytes 40..48).
/// Returns Unix timestamp (seconds since 1 Jan 1970) or None.
fn parse_ntp_response(pkt: &[u8]) -> Option<u64> {
    if pkt.len() < NTP_PACKET_SIZE {
        return None;
    }
    // Verify mode = 4 (server) in byte 0, low 3 bits.
    if (pkt[0] & 0x07) != 4 {
        return None;
    }
    // Transmit timestamp is at bytes 40..48.
    let secs = u32::from_be_bytes([pkt[40], pkt[41], pkt[42], pkt[43]]) as u64;
    if secs == 0 {
        return None;
    }
    secs.checked_sub(NTP_TO_UNIX_DELTA)
}

/// Query an NTP server and return the Unix timestamp (seconds since 1 Jan 1970).
///
/// Uses the caller's interface/socket set so the same stack carries DHCP,
/// NTP and the kernel fetch.
pub fn query_ntp(
    iface: &mut Interface,
    socket_set: &mut SocketSet,
    device: &mut SeeqDevice,
    server_ip: Ipv4Address,
) -> Option<u64> {
    // The socket set outlives this call, so the packet buffers must too.
    let rx_meta = Box::leak(Box::new(
        [smoltcp::storage::PacketMetadata::<UdpMetadata>::EMPTY; 1],
    ));
    let tx_meta = Box::leak(Box::new(
        [smoltcp::storage::PacketMetadata::<UdpMetadata>::EMPTY; 1],
    ));
    let rx_payload = Box::leak(Box::new([0u8; NTP_PACKET_SIZE]));
    let tx_payload = Box::leak(Box::new([0u8; NTP_PACKET_SIZE]));
    let udp_socket = UdpSocket::new(
        PacketBuffer::new(&mut rx_meta[..], &mut rx_payload[..]),
        PacketBuffer::new(&mut tx_meta[..], &mut tx_payload[..]),
    );
    let udp_handle = socket_set.add(udp_socket);

    // Bind to local port.
    socket_set
        .get_mut::<UdpSocket>(udp_handle)
        .bind(NTP_LOCAL_PORT)
        .ok()?;

    // Build and send request.
    let request = build_ntp_request();
    let remote = IpEndpoint::new(IpAddress::Ipv4(server_ip), NTP_PORT);
    socket_set
        .get_mut::<UdpSocket>(udp_handle)
        .send_slice(&request, remote)
        .ok()?;

    let start = crate::net::clock_now();
    let mut timestamp = start;
    let timeout = Duration::from_millis(10_000);

    loop {
        let _ = iface.poll(timestamp, device, socket_set);

        // Check for a received NTP response.
        let sock = socket_set.get_mut::<UdpSocket>(udp_handle);
        if let Ok((data, _meta)) = sock.recv() {
            if data.len() >= NTP_PACKET_SIZE {
                let mut pkt = [0u8; NTP_PACKET_SIZE];
                pkt.copy_from_slice(&data[..NTP_PACKET_SIZE]);
                return parse_ntp_response(&pkt);
            }
        }

        // Advance the interface clock by a fixed millisecond per poll.
        // poll_delay() can return zero and freeze the clock, which makes the
        // UDP retransmit logic fire on every iteration.
        crate::net::clock_advance(1);
        timestamp = crate::net::clock_now();
        if timestamp - start > timeout {
            return None;
        }
    }
}

/// Format a Unix timestamp as seconds since epoch.
pub fn format_timestamp(secs: u64) -> alloc::string::String {
    format!("{}", secs)
}
