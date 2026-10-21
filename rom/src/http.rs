//! Minimal HTTP/1.1 client for the ROM's netboot path.
//!
//! Performs GET requests with Range support over smoltcp TCP sockets,
//! connecting directly to an IPv4 address (DNS resolution is a follow-up).
//! The caller owns the interface, socket set and Seeq device, so the same
//! network stack carries DHCP, NTP and the kernel fetch.

use crate::net::SeeqDevice;
use alloc::boxed::Box;
use alloc::format;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;
use smoltcp::iface::{Interface, SocketSet};
use smoltcp::socket::tcp;
use smoltcp::socket::tcp::Socket as TcpSocket;
use smoltcp::time::{Duration, Instant};
use smoltcp::wire::{IpAddress, Ipv4Address};

const HTTP_TIMEOUT_MS: u64 = 30_000;

/// Result of an HTTP fetch: the body bytes and the server IP address.
pub struct FetchResult {
    pub body: Vec<u8>,
    pub server_ip: Ipv4Address,
}

/// Perform an HTTP/1.1 GET request and return the body bytes.
///
/// `server_ip`/`server_port` identify the server. `host` is the value for the
/// `Host:` header (without the port); `range` is an optional `Range:` value.
#[allow(clippy::result_unit_err)]
#[allow(clippy::too_many_arguments)]
pub fn http_get(
    iface: &mut Interface,
    socket_set: &mut SocketSet,
    device: &mut SeeqDevice,
    server_ip: Ipv4Address,
    server_port: u16,
    host: &str,
    path: &str,
    range: Option<&str>,
) -> Result<FetchResult, ()> {
    // Create a TCP socket with leaked buffers (live for the duration of the
    // fetch; the bump allocator never frees).
    let rx_buf = Box::leak(Box::new(vec![0u8; 4096]));
    let tx_buf = Box::leak(Box::new(vec![0u8; 4096]));
    let rx_buffer = tcp::SocketBuffer::new(&mut rx_buf[..]);
    let tx_buffer = tcp::SocketBuffer::new(&mut tx_buf[..]);
    let tcp_socket = TcpSocket::new(rx_buffer, tx_buffer);
    let tcp_handle = socket_set.add(tcp_socket);

    let cx = iface.context();
    socket_set
        .get_mut::<TcpSocket>(tcp_handle)
        .connect(cx, (IpAddress::Ipv4(server_ip), server_port), 49152)
        .map_err(|_| ())?;

    let mut timestamp = Instant::from_millis(0);
    let deadline = timestamp + Duration::from_millis(HTTP_TIMEOUT_MS);

    // Wait for connection establishment.
    loop {
        let _ = iface.poll(timestamp, device, socket_set);
        let state = socket_set.get_mut::<TcpSocket>(tcp_handle).state();
        if state == tcp::State::Established {
            break;
        }
        if state == tcp::State::Closed || state == tcp::State::LastAck {
            return Err(());
        }
        timestamp += Duration::from_millis(1);
        if timestamp > deadline {
            return Err(());
        }
    }

    // Build and send the GET request.
    let mut req = String::new();
    req.push_str("GET ");
    req.push_str(path);
    req.push_str(" HTTP/1.1\r\nHost: ");
    req.push_str(host);
    if server_port != 80 {
        req.push_str(&format!(":{}", server_port));
    }
    req.push_str("\r\nConnection: close\r\n");
    if let Some(r) = range {
        req.push_str("Range: ");
        req.push_str(r);
        req.push_str("\r\n");
    }
    req.push_str("\r\n");

    socket_set
        .get_mut::<TcpSocket>(tcp_handle)
        .send_slice(req.as_bytes())
        .map_err(|_| ())?;

    // Read the response.  Headers are accumulated until the blank line, then
    // the body is appended.  The request used Connection: close, so EOF marks
    // the end of the body.
    let mut headers = Vec::new();
    let mut headers_done = false;
    let mut body = Vec::new();
    loop {
        let _ = iface.poll(timestamp, device, socket_set);
        let sock = socket_set.get_mut::<TcpSocket>(tcp_handle);

        let mut chunk = [0u8; 512];
        loop {
            match sock.recv_slice(&mut chunk) {
                Ok(0) => break,
                Ok(n) => {
                    if headers_done {
                        body.extend_from_slice(&chunk[..n]);
                    } else {
                        headers.extend_from_slice(&chunk[..n]);
                        if let Some(pos) = find_header_end(&headers) {
                            body.extend_from_slice(&headers[pos + 4..]);
                            headers.clear();
                            headers_done = true;
                        }
                    }
                }
                Err(_) => break,
            }
        }

        let state = sock.state();
        if state == tcp::State::Closed
            || state == tcp::State::LastAck
            || state == tcp::State::CloseWait
        {
            break;
        }

        timestamp += Duration::from_millis(1);
        if timestamp > deadline {
            return Err(());
        }
    }

    socket_set.get_mut::<TcpSocket>(tcp_handle).close();
    Ok(FetchResult { body, server_ip })
}

/// Find `\r\n\r\n` in a byte slice.
fn find_header_end(buf: &[u8]) -> Option<usize> {
    let mut i = 0;
    while i + 3 < buf.len() {
        if buf[i] == b'\r' && buf[i + 1] == b'\n' && buf[i + 2] == b'\r' && buf[i + 3] == b'\n' {
            return Some(i);
        }
        i += 1;
    }
    None
}

#[cfg(test)]
mod tests {
    #[test]
    fn http_get_module_compiles() {
        // Just verify the module compiles; HTTP needs live network.
    }
}
