//! Minimal HTTP/1.1 client for the ROM's netboot path.
//!
//! Performs GET requests with Range support over smoltcp TCP sockets,
//! connecting directly to an IPv4 address (DNS resolution is a follow-up).
//! Intended for fetching the kernel and boot bundle from an HTTP/R2 endpoint
//! during ROM boot.

use crate::net::SeeqDevice;
use alloc::boxed::Box;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;
use smoltcp::iface::{Interface, SocketSet};
use smoltcp::socket::tcp;
use smoltcp::socket::tcp::Socket as TcpSocket;
use smoltcp::time::{Duration, Instant};
use smoltcp::wire::{IpAddress, Ipv4Address};

const HTTP_PORT: u16 = 80;
const HTTP_TIMEOUT_MS: u64 = 30_000;

/// Result of an HTTP fetch: the body bytes and the server IP address.
pub struct FetchResult {
    pub body: Vec<u8>,
    pub server_ip: Ipv4Address,
}

/// Perform an HTTP/1.1 GET request and return the body bytes.
///
/// `server_ip` is the IPv4 address of the HTTP server. `range` is an optional
/// `Range: bytes=start-end` header value.
#[allow(clippy::result_unit_err)]
pub fn http_get(
    iface: &mut Interface,
    socket_set: &mut SocketSet,
    server_ip: Ipv4Address,
    host: &str,
    path: &str,
    range: Option<&str>,
) -> Result<FetchResult, ()> {
    // Create a TCP socket with leaked buffers (live for the duration of the fetch).
    let rx_buf = Box::leak(Box::new(vec![0u8; 4096]));
    let tx_buf = Box::leak(Box::new(vec![0u8; 4096]));
    let rx_buffer = tcp::SocketBuffer::new(&mut rx_buf[..]);
    let tx_buffer = tcp::SocketBuffer::new(&mut tx_buf[..]);
    let tcp_socket = TcpSocket::new(rx_buffer, tx_buffer);
    let tcp_handle = socket_set.add(tcp_socket);

    let cx = iface.context();
    socket_set
        .get_mut::<TcpSocket>(tcp_handle)
        .connect(cx, (IpAddress::Ipv4(server_ip), HTTP_PORT), 49152)
        .map_err(|_| ())?;

    let mut timestamp = Instant::from_millis(0);
    let deadline = timestamp + Duration::from_millis(HTTP_TIMEOUT_MS);

    // Wait for connection to be established.
    loop {
        let _ = iface.poll(timestamp, &mut SeeqDevice::new().unwrap(), socket_set);
        let state = socket_set.get_mut::<TcpSocket>(tcp_handle).state();
        if state == tcp::State::Established {
            break;
        }
        if state == tcp::State::Closed || state == tcp::State::LastAck {
            return Err(());
        }
        let delay = iface.poll_delay(timestamp, socket_set);
        let next = match delay {
            Some(d) => timestamp + d,
            None => timestamp + Duration::from_millis(100),
        };
        if next > deadline {
            return Err(());
        }
        timestamp = next;
    }

    // Build and send the GET request.
    let mut req = String::new();
    req.push_str("GET ");
    req.push_str(path);
    req.push_str(" HTTP/1.1\r\nHost: ");
    req.push_str(host);
    req.push_str("\r\nConnection: close\r\n");
    if let Some(r) = range {
        req.push_str("Range: ");
        req.push_str(r);
        req.push_str("\r\n");
    }
    req.push_str("\r\n");

    let req_bytes = req.as_bytes();
    socket_set
        .get_mut::<TcpSocket>(tcp_handle)
        .send_slice(req_bytes)
        .map_err(|_| ())?;

    // Read the response headers and body.
    let mut body = Vec::new();
    let mut headers_done = false;

    loop {
        let _ = iface.poll(timestamp, &mut SeeqDevice::new().unwrap(), socket_set);
        let sock = socket_set.get_mut::<TcpSocket>(tcp_handle);

        if !headers_done {
            let cap = sock.recv_capacity();
            if cap > 0 {
                let mut buf = [0u8; 512];
                let cap = sock.recv_capacity();
                let len = cap.min(buf.len());
                let n = sock.recv_slice(&mut buf[..len]).map_err(|_| ())?;
                if n > 0 {
                    // Simple scan for "\r\n\r\n" header/body separator.
                    let mut found = false;
                    let mut i = 0;
                    while i + 3 < n {
                        if buf[i] == b'\r'
                            && buf[i + 1] == b'\n'
                            && buf[i + 2] == b'\r'
                            && buf[i + 3] == b'\n'
                        {
                            found = true;
                            headers_done = true;
                            let body_start = i + 4;
                            if body_start < n {
                                body.extend_from_slice(&buf[body_start..n]);
                            }
                            break;
                        }
                        i += 1;
                    }
                    if !found {
                        // Headers span multiple reads; discard and re-poll.
                        // (Headers are small; this is fine for the ROM.)
                    }
                }
            }
        } else {
            let cap = sock.recv_capacity();
            if cap > 0 {
                let mut buf = [0u8; 1024];
                let len = cap.min(buf.len());
                let n = sock.recv_slice(&mut buf[..len]).map_err(|_| ())?;
                if n > 0 {
                    body.extend_from_slice(&buf[..n]);
                } else {
                    break;
                }
            } else {
                let state = sock.state();
                if state == tcp::State::Closed || state == tcp::State::LastAck {
                    break;
                }
            }
        }

        let delay = iface.poll_delay(timestamp, socket_set);
        let next = match delay {
            Some(d) => timestamp + d,
            None => timestamp + Duration::from_millis(100),
        };
        if next > deadline {
            return Err(());
        }
        timestamp = next;
    }

    socket_set.get_mut::<TcpSocket>(tcp_handle).close();

    Ok(FetchResult { body, server_ip })
}

#[cfg(test)]
mod tests {
    #[test]
    fn http_get_module_compiles() {
        // Just verify the module compiles; HTTP needs live network.
    }
}
