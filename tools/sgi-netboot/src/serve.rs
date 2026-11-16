// SPDX-License-Identifier: GPL-3.0-only
//! BOOTP and TFTP server for one SGI PROM client.
//!
//! Three PROM limits shape it:
//!
//! * The PROM reads the TFTP server's port as a signed 16-bit number, so data
//!   sockets use ports 20000 to 32000.
//! * The PROM drops datagrams with the Don't Fragment bit, so every socket
//!   turns path-MTU discovery off for itself.
//! * The PROM speaks plain BOOTP.  A request with a DHCP message type comes
//!   from another client (the IRIX 7 kernel uses DHCP with the same MAC) and
//!   is ignored.

use std::fs;
use std::io;
use std::net::{Ipv4Addr, SocketAddrV4, UdpSocket};
use std::os::fd::FromRawFd;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU16, Ordering};
use std::thread;
use std::time::{Duration, Instant};

const BOOTP_SERVER_PORT: u16 = 67;
const BOOTP_CLIENT_PORT: u16 = 68;
const TFTP_PORT: u16 = 69;
const DATA_PORT_FIRST: u16 = 20000;
const DATA_PORT_LAST: u16 = 32000;
const BLOCK: usize = 512;
const ACK_TIMEOUT: Duration = Duration::from_secs(1);
const RETRIES: u32 = 8;

/// Fixed BOOTP header length, before the vendor area.
const BOOTP_HEADER: usize = 236;
const BOOTP_REPLY_LEN: usize = 300;
const DHCP_COOKIE: [u8; 4] = [99, 130, 83, 99];
const DHCP_MESSAGE_TYPE: u8 = 53;

const AF_INET: i32 = 2;
const SOCK_DGRAM: i32 = 2;
const SOL_SOCKET: i32 = 1;
const SO_REUSEADDR: i32 = 2;
const SO_BROADCAST: i32 = 6;
const SO_BINDTODEVICE: i32 = 25;
const IPPROTO_IP: i32 = 0;
const IP_MTU_DISCOVER: i32 = 10;
const IP_PMTUDISC_DONT: i32 = 0;

#[repr(C)]
struct SockaddrIn {
    family: u16,
    port: u16,
    addr: u32,
    zero: [u8; 8],
}

extern "C" {
    fn socket(domain: i32, kind: i32, protocol: i32) -> i32;
    fn setsockopt(fd: i32, level: i32, name: i32, value: *const u8, len: u32) -> i32;
    fn bind(fd: i32, addr: *const SockaddrIn, len: u32) -> i32;
    fn close(fd: i32) -> i32;
}

/// Server settings from the command line.
pub struct Config {
    pub iface: String,
    pub server: Ipv4Addr,
    pub mac: [u8; 6],
    pub ip: Ipv4Addr,
    pub dir: PathBuf,
}

pub fn parse_mac(text: &str) -> Result<[u8; 6], String> {
    let mut mac = [0u8; 6];
    let mut parts = text.split(':');
    for slot in &mut mac {
        let part = parts.next().ok_or("MAC needs six bytes")?;
        *slot = u8::from_str_radix(part, 16).map_err(|_| format!("bad MAC byte {part:?}"))?;
    }
    if parts.next().is_some() {
        return Err("MAC needs six bytes".into());
    }
    Ok(mac)
}

impl Config {
    pub fn parse(args: &[String]) -> Result<Config, String> {
        let mut iface = None;
        let mut server = None;
        let mut mac = None;
        let mut ip = None;
        let mut dir = None;
        let mut args = args.iter();
        while let Some(flag) = args.next() {
            let value = args.next().ok_or(format!("{flag} needs a value"))?;
            let address = |text: &String| {
                text.parse::<Ipv4Addr>()
                    .map_err(|_| format!("bad IPv4 address {text:?}"))
            };
            match flag.as_str() {
                "--iface" => iface = Some(value.clone()),
                "--server" => server = Some(address(value)?),
                "--mac" => mac = Some(parse_mac(value)?),
                "--ip" => ip = Some(address(value)?),
                "--dir" => dir = Some(PathBuf::from(value)),
                other => return Err(format!("unknown option {other}")),
            }
        }
        Ok(Config {
            iface: iface.ok_or("--iface is required")?,
            server: server.ok_or("--server is required")?,
            mac: mac.ok_or("--mac is required")?,
            ip: ip.ok_or("--ip is required")?,
            dir: dir.ok_or("--dir is required")?,
        })
    }
}

fn set_option(fd: i32, level: i32, name: i32, value: &[u8]) -> io::Result<()> {
    // SAFETY: `value` is a live buffer of the stated length.
    let status = unsafe { setsockopt(fd, level, name, value.as_ptr(), value.len() as u32) };
    if status == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

/// A UDP socket bound to `iface`, with address reuse and no Don't Fragment bit.
///
/// Binding to the device lets the wildcard address share port 67 with another
/// interface's DHCP server and sends broadcasts out of `iface`.
fn udp_socket(iface: &str, addr: Ipv4Addr, port: u16) -> io::Result<UdpSocket> {
    // SAFETY: plain socket creation; the descriptor is owned below.
    let fd = unsafe { socket(AF_INET, SOCK_DGRAM, 0) };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    let one = 1i32.to_ne_bytes();
    let configure = || -> io::Result<()> {
        set_option(fd, SOL_SOCKET, SO_REUSEADDR, &one)?;
        set_option(fd, SOL_SOCKET, SO_BROADCAST, &one)?;
        set_option(fd, SOL_SOCKET, SO_BINDTODEVICE, iface.as_bytes())?;
        set_option(
            fd,
            IPPROTO_IP,
            IP_MTU_DISCOVER,
            &IP_PMTUDISC_DONT.to_ne_bytes(),
        )?;
        let sockaddr = SockaddrIn {
            family: AF_INET as u16,
            port: port.to_be(),
            addr: u32::from(addr).to_be(),
            zero: [0; 8],
        };
        // SAFETY: `sockaddr` is a valid `sockaddr_in` of the stated size.
        let status = unsafe { bind(fd, &sockaddr, core::mem::size_of::<SockaddrIn>() as u32) };
        if status == 0 {
            Ok(())
        } else {
            Err(io::Error::last_os_error())
        }
    };
    match configure() {
        // SAFETY: `fd` is an open socket this function owns.
        Ok(()) => Ok(unsafe { UdpSocket::from_raw_fd(fd) }),
        Err(error) => {
            // SAFETY: `fd` is open and not yet owned by a `UdpSocket`.
            unsafe { close(fd) };
            Err(error)
        }
    }
}

/// Build the BOOTP reply for `request`, or `None` when it is not a plain
/// BOOTP request from `mac`.
pub fn bootp_reply(
    request: &[u8],
    mac: &[u8; 6],
    client: Ipv4Addr,
    server: Ipv4Addr,
) -> Option<Vec<u8>> {
    if request.len() < BOOTP_HEADER {
        return None;
    }
    // op = BOOTREQUEST, htype = Ethernet, hlen = 6.
    if request[0] != 1 || request[1] != 1 || request[2] != 6 {
        return None;
    }
    if &request[28..34] != mac {
        return None;
    }
    if is_dhcp(&request[BOOTP_HEADER..]) {
        return None;
    }
    let mut reply = vec![0u8; BOOTP_REPLY_LEN];
    reply[0] = 2; // BOOTREPLY
    reply[1] = 1;
    reply[2] = 6;
    reply[4..8].copy_from_slice(&request[4..8]); // xid
    reply[10..12].copy_from_slice(&request[10..12]); // flags
    reply[12..16].copy_from_slice(&request[12..16]); // ciaddr
    reply[16..20].copy_from_slice(&client.octets()); // yiaddr
    reply[20..24].copy_from_slice(&server.octets()); // siaddr
    reply[24..28].copy_from_slice(&request[24..28]); // giaddr
    reply[28..44].copy_from_slice(&request[28..44]); // chaddr
    reply[108..236].copy_from_slice(&request[108..236]); // file, as requested
    reply[236..240].copy_from_slice(&DHCP_COOKIE);
    reply[240] = 255; // end of the vendor area
    Some(reply)
}

/// True when the vendor area carries a DHCP message type option.
fn is_dhcp(vendor: &[u8]) -> bool {
    if vendor.len() < 4 || vendor[..4] != DHCP_COOKIE {
        return false;
    }
    let mut at = 4;
    while at < vendor.len() {
        match vendor[at] {
            0 => at += 1,
            255 => return false,
            DHCP_MESSAGE_TYPE => return true,
            _ => {
                let Some(&len) = vendor.get(at + 1) else {
                    return false;
                };
                at += 2 + len as usize;
            }
        }
    }
    false
}

fn bootp_loop(socket: UdpSocket, config: &Config) {
    let mut buffer = [0u8; 1500];
    loop {
        let Ok((len, from)) = socket.recv_from(&mut buffer) else {
            continue;
        };
        let request = &buffer[..len];
        let Some(reply) = bootp_reply(request, &config.mac, config.ip, config.server) else {
            continue;
        };
        let ciaddr = Ipv4Addr::new(request[12], request[13], request[14], request[15]);
        let target = if ciaddr.is_unspecified() {
            Ipv4Addr::BROADCAST
        } else {
            ciaddr
        };
        let file = String::from_utf8_lossy(&request[108..236]);
        println!(
            "bootp: request from {from}, file {:?}; reply {} -> {target}",
            file.trim_end_matches('\0'),
            config.ip
        );
        if let Err(error) = socket.send_to(&reply, SocketAddrV4::new(target, BOOTP_CLIENT_PORT)) {
            eprintln!("bootp: send failed: {error}");
        }
    }
}

/// The file name and mode of a TFTP read request.
pub fn parse_rrq(packet: &[u8]) -> Option<(String, String)> {
    if packet.len() < 4 || packet[0] != 0 || packet[1] != 1 {
        return None;
    }
    let mut fields = packet[2..].split(|&byte| byte == 0);
    let name = String::from_utf8(fields.next()?.to_vec()).ok()?;
    let mode = String::from_utf8(fields.next()?.to_vec()).ok()?;
    Some((name, mode.to_ascii_lowercase()))
}

/// The last path component of a requested name, or `None` when there is none.
/// Requests cannot name a file outside the served directory.
pub fn served_name(requested: &str) -> Option<&str> {
    let name = requested.rsplit('/').next()?;
    if name.is_empty() || name == "." || name == ".." {
        return None;
    }
    Some(name)
}

fn tftp_error(socket: &UdpSocket, code: u16, message: &str) {
    let mut packet = vec![0, 5];
    packet.extend_from_slice(&code.to_be_bytes());
    packet.extend_from_slice(message.as_bytes());
    packet.push(0);
    let _ = socket.send(&packet);
}

static NEXT_PORT: AtomicU16 = AtomicU16::new(DATA_PORT_FIRST);

fn data_socket(config_iface: &str, server: Ipv4Addr) -> io::Result<UdpSocket> {
    let span = DATA_PORT_LAST - DATA_PORT_FIRST + 1;
    let mut last = io::Error::other("no data port tried");
    for _ in 0..64 {
        let port = DATA_PORT_FIRST + NEXT_PORT.fetch_add(1, Ordering::Relaxed) % span;
        match udp_socket(config_iface, server, port) {
            Ok(socket) => return Ok(socket),
            Err(error) => last = error,
        }
    }
    Err(last)
}

fn transfer(socket: UdpSocket, name: String, data: Vec<u8>) {
    let started = Instant::now();
    let _ = socket.set_read_timeout(Some(ACK_TIMEOUT));
    let mut block: u16 = 1;
    let mut offset = 0usize;
    loop {
        let end = (offset + BLOCK).min(data.len());
        let mut packet = Vec::with_capacity(4 + BLOCK);
        packet.extend_from_slice(&[0, 3]);
        packet.extend_from_slice(&block.to_be_bytes());
        packet.extend_from_slice(&data[offset..end]);

        let mut acked = false;
        for _ in 0..RETRIES {
            if socket.send(&packet).is_err() {
                break;
            }
            let deadline = Instant::now() + ACK_TIMEOUT;
            let mut reply = [0u8; 516];
            // A duplicate ACK for an earlier block is skipped, not answered,
            // so one late ACK cannot double every later block.
            while Instant::now() < deadline {
                match socket.recv(&mut reply) {
                    Ok(len) if len >= 4 && reply[1] == 4 => {
                        if u16::from_be_bytes([reply[2], reply[3]]) == block {
                            acked = true;
                            break;
                        }
                    }
                    Ok(len) if len >= 4 && reply[1] == 5 => {
                        eprintln!("tftp: {name}: client reported an error");
                        return;
                    }
                    Ok(_) => {}
                    Err(_) => break,
                }
            }
            if acked {
                break;
            }
        }
        if !acked {
            eprintln!("tftp: {name}: no ACK for block {block}; gave up at byte {offset}");
            return;
        }
        // A final block shorter than 512 bytes (possibly empty) ends the file.
        if end - offset < BLOCK {
            println!(
                "tftp: {name}: sent {} bytes in {:.1} s",
                data.len(),
                started.elapsed().as_secs_f32()
            );
            return;
        }
        offset = end;
        block = block.wrapping_add(1);
    }
}

fn tftp_loop(socket: UdpSocket, config: &Config) {
    let mut buffer = [0u8; 1500];
    loop {
        let Ok((len, from)) = socket.recv_from(&mut buffer) else {
            continue;
        };
        let Some((requested, mode)) = parse_rrq(&buffer[..len]) else {
            continue;
        };
        let data = match data_socket(&config.iface, config.server) {
            Ok(data) => data,
            Err(error) => {
                eprintln!("tftp: no data socket: {error}");
                continue;
            }
        };
        if data.connect(from).is_err() {
            continue;
        }
        let Some(name) = served_name(&requested) else {
            tftp_error(&data, 2, "bad file name");
            continue;
        };
        match fs::read(config.dir.join(name)) {
            Ok(bytes) => {
                println!(
                    "tftp: {from} reads {requested:?} ({mode}): {name}, {} bytes",
                    bytes.len()
                );
                let name = name.to_string();
                thread::spawn(move || transfer(data, name, bytes));
            }
            Err(error) => {
                eprintln!("tftp: {from} asked for {requested:?}: {error}");
                tftp_error(&data, 1, "file not found");
            }
        }
    }
}

/// Bind both ports and serve until killed.
pub fn run(config: &Config) -> Result<(), String> {
    let bootp = udp_socket(&config.iface, Ipv4Addr::UNSPECIFIED, BOOTP_SERVER_PORT)
        .map_err(|error| format!("UDP port {BOOTP_SERVER_PORT}: {error}"))?;
    let tftp = udp_socket(&config.iface, Ipv4Addr::UNSPECIFIED, TFTP_PORT)
        .map_err(|error| format!("UDP port {TFTP_PORT}: {error}"))?;
    println!(
        "sgi-netboot: BOOTP for {} -> {}, TFTP from {} on {} ({})",
        config
            .mac
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<Vec<_>>()
            .join(":"),
        config.ip,
        config.dir.display(),
        config.iface,
        config.server
    );
    thread::scope(|scope| {
        scope.spawn(|| bootp_loop(bootp, config));
        tftp_loop(tftp, config);
    });
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const MAC: [u8; 6] = [0x08, 0x00, 0x69, 0x12, 0x12, 0x12];
    const CLIENT: Ipv4Addr = Ipv4Addr::new(10, 1, 1, 112);
    const SERVER: Ipv4Addr = Ipv4Addr::new(10, 1, 1, 3);

    fn request() -> Vec<u8> {
        let mut packet = vec![0u8; 300];
        packet[0] = 1;
        packet[1] = 1;
        packet[2] = 6;
        packet[4..8].copy_from_slice(&[1, 2, 3, 4]);
        packet[28..34].copy_from_slice(&MAC);
        packet[108..113].copy_from_slice(b"irix7");
        packet
    }

    #[test]
    fn plain_bootp_from_the_client_is_answered() {
        let reply = bootp_reply(&request(), &MAC, CLIENT, SERVER).unwrap();
        assert_eq!(reply[0], 2);
        assert_eq!(&reply[4..8], &[1, 2, 3, 4]);
        assert_eq!(&reply[16..20], &CLIENT.octets());
        assert_eq!(&reply[20..24], &SERVER.octets());
        assert_eq!(&reply[108..113], b"irix7");
    }

    #[test]
    fn another_mac_is_ignored() {
        let mut packet = request();
        packet[33] = 0x13;
        assert!(bootp_reply(&packet, &MAC, CLIENT, SERVER).is_none());
    }

    #[test]
    fn dhcp_from_the_same_mac_is_ignored() {
        let mut packet = request();
        packet[236..240].copy_from_slice(&DHCP_COOKIE);
        packet[240..243].copy_from_slice(&[DHCP_MESSAGE_TYPE, 1, 1]);
        packet[243] = 255;
        assert!(bootp_reply(&packet, &MAC, CLIENT, SERVER).is_none());
    }

    #[test]
    fn a_cookie_without_a_message_type_is_bootp() {
        let mut packet = request();
        packet[236..240].copy_from_slice(&DHCP_COOKIE);
        packet[240] = 255;
        assert!(bootp_reply(&packet, &MAC, CLIENT, SERVER).is_some());
    }

    #[test]
    fn rrq_fields_are_parsed() {
        let packet = b"\0\x01/tftpboot/irix7\0OCTET\0";
        assert_eq!(
            parse_rrq(packet),
            Some(("/tftpboot/irix7".to_string(), "octet".to_string()))
        );
        assert!(parse_rrq(b"\0\x02irix7\0octet\0").is_none());
    }

    #[test]
    fn only_the_last_path_component_is_served() {
        assert_eq!(served_name("/tftpboot/irix7"), Some("irix7"));
        assert_eq!(served_name("../../etc/passwd"), Some("passwd"));
        assert_eq!(served_name("irix7/"), None);
        assert_eq!(served_name(".."), None);
    }

    #[test]
    fn mac_text_is_parsed() {
        assert_eq!(parse_mac("08:00:69:12:12:12"), Ok(MAC));
        assert!(parse_mac("08:00:69:12:12").is_err());
    }
}
