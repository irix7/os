// SPDX-License-Identifier: GPL-3.0-only
//! NTP (Network Time Protocol) client over the shared smoltcp stack.
//!
//! Sends an NTPv3 request to a server and stores the transmit timestamp as
//! the kernel's wall clock.  The wall clock is advanced afterwards from the
//! same monotonic CP0 Count clock the network stack uses.
//!
//! NTP timestamps count seconds from 00:00 1 January 1900; Unix time counts
//! from 1 January 1970, so the difference (2,208,988,800 s) is subtracted.

use core::sync::atomic::{AtomicU64, Ordering};

use smoltcp::socket::udp::{Socket as UdpSocket, UdpMetadata};
use smoltcp::storage::{PacketBuffer, PacketMetadata};
use smoltcp::wire::{IpAddress, IpEndpoint, Ipv4Address};

use crate::net;

const NTP_PACKET_SIZE: usize = 48;
const NTP_TO_UNIX_DELTA: u64 = 2_208_988_800;
const NTP_LOCAL_PORT: u16 = 12345;

/// Socket buffers for the single in-flight query.
static mut NTP_RX_META: [PacketMetadata<UdpMetadata>; 1] = [PacketMetadata::EMPTY; 1];
static mut NTP_TX_META: [PacketMetadata<UdpMetadata>; 1] = [PacketMetadata::EMPTY; 1];
static mut NTP_RX_BUF: [u8; NTP_PACKET_SIZE] = [0; NTP_PACKET_SIZE];
static mut NTP_TX_BUF: [u8; NTP_PACKET_SIZE] = [0; NTP_PACKET_SIZE];

/// Wall clock at the last successful sync (Unix seconds, 0 = unset).
static WALL_SECS: AtomicU64 = AtomicU64::new(0);
/// Network clock reading at the last sync (milliseconds).
static WALL_SYNC_MS: AtomicU64 = AtomicU64::new(0);

/// Current wall-clock time in Unix seconds, if it has been set.
pub fn now_unix() -> Option<u64> {
    let base = WALL_SECS.load(Ordering::SeqCst);
    if base == 0 {
        return None;
    }
    let sync = WALL_SYNC_MS.load(Ordering::SeqCst);
    let now = net::clock_now().total_millis() as u64;
    Some(base + now.saturating_sub(sync) / 1000)
}

/// Set the wall clock from an already-known Unix timestamp.
pub fn set_unix(unix: u64) {
    WALL_SECS.store(unix, Ordering::SeqCst);
    WALL_SYNC_MS.store(net::clock_now().total_millis() as u64, Ordering::SeqCst);
}

/// Query `ip:port` and set the wall clock.  Returns the Unix timestamp.
pub fn query(ip: Ipv4Address, port: u16) -> Option<u64> {
    let handle = net::sockets_with(|sockets| {
        // SAFETY: the buffers are static and used by one query at a time.
        let rx = PacketBuffer::new(unsafe { &mut NTP_RX_META[..] }, unsafe {
            &mut NTP_RX_BUF[..]
        });
        let tx = PacketBuffer::new(unsafe { &mut NTP_TX_META[..] }, unsafe {
            &mut NTP_TX_BUF[..]
        });
        sockets.add(UdpSocket::new(rx, tx))
    })?;

    let bound = net::sockets_with(|sockets| {
        sockets
            .get_mut::<UdpSocket>(handle)
            .bind(NTP_LOCAL_PORT)
            .is_ok()
    });
    if bound != Some(true) {
        cleanup(handle);
        return None;
    }

    let mut request = [0u8; NTP_PACKET_SIZE];
    // LI = 0 (no warning), VN = 3, mode = 3 (client).
    request[0] = 0b00_011_011;
    let remote = IpEndpoint::new(IpAddress::Ipv4(ip), port);
    let sent = net::sockets_with(|sockets| {
        sockets
            .get_mut::<UdpSocket>(handle)
            .send_slice(&request, remote)
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
                    let mut packet = [0u8; NTP_PACKET_SIZE];
                    let n = data.len().min(NTP_PACKET_SIZE);
                    packet[..n].copy_from_slice(&data[..n]);
                    Some((n, packet))
                }
                Err(_) => None,
            }
        });

        if let Some(Some((n, packet))) = reply {
            cleanup(handle);
            if n < NTP_PACKET_SIZE {
                return None;
            }
            let unix = parse_response(&packet)?;
            set_unix(unix);
            return Some(unix);
        }

        if net::clock_now().total_millis() > deadline {
            cleanup(handle);
            return None;
        }
    }
}

/// Remove the socket so repeated queries do not exhaust the set.
fn cleanup(handle: smoltcp::iface::SocketHandle) {
    net::sockets_with(|sockets| {
        let _ = sockets.remove(handle);
    });
}

/// Extract the transmit timestamp (bytes 40..48) and verify the reply mode.
fn parse_response(packet: &[u8; NTP_PACKET_SIZE]) -> Option<u64> {
    if packet[0] & 0x07 != 4 {
        return None;
    }
    let secs = u32::from_be_bytes([packet[40], packet[41], packet[42], packet[43]]) as u64;
    if secs == 0 {
        return None;
    }
    secs.checked_sub(NTP_TO_UNIX_DELTA)
}

/// Format a Unix timestamp as `YYYY-MM-DD HH:MM:SS` (UTC).  Returns the
/// number of bytes written.
pub fn format_utc(unix: u64, out: &mut [u8; 20]) -> usize {
    let days = (unix / 86_400) as i64;
    let secs_of_day = unix % 86_400;
    let (year, month, day) = civil_from_days(days);
    let hour = secs_of_day / 3600;
    let minute = (secs_of_day % 3600) / 60;
    let second = secs_of_day % 60;

    let mut n = 0;
    n += write_padded(out, n, year as u64, 4);
    out[n] = b'-';
    n += 1;
    n += write_padded(out, n, month as u64, 2);
    out[n] = b'-';
    n += 1;
    n += write_padded(out, n, day as u64, 2);
    out[n] = b' ';
    n += 1;
    n += write_padded(out, n, hour, 2);
    out[n] = b':';
    n += 1;
    n += write_padded(out, n, minute, 2);
    out[n] = b':';
    n += 1;
    n += write_padded(out, n, second, 2);
    n
}

/// Write `value` as exactly `width` decimal digits, zero padded.
fn write_padded(out: &mut [u8; 20], start: usize, mut value: u64, width: usize) -> usize {
    let mut i = start + width;
    while i > start {
        i -= 1;
        out[i] = b'0' + (value % 10) as u8;
        value /= 10;
    }
    width
}

/// Convert days since the Unix epoch to a civil date (Howard Hinnant's
/// algorithm), avoiding a table of leap years.
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}
