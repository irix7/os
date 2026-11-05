// SPDX-License-Identifier: GPL-3.0-only
//! Eight simultaneous TCP sockets, as a kernel self-test.
//!
//! This replaces the `ports` module's TCP-slot probe. It runs only when the
//! `ports.base` NVRAM key is set, so ordinary boots never open these sockets.
//! It connects `ports.count` sockets to the fixture's echo listeners at
//! `ports.base + i`, sends one line on each, and checks every echo. The m2-rig
//! harness reads `ports: pass sockets=N` or `ports: fail <reason>`.

use alloc::boxed::Box;
use alloc::format;
use smoltcp::iface::SocketHandle;
use smoltcp::socket::tcp::{Socket as TcpSocket, SocketBuffer, State};
use smoltcp::wire::{IpAddress, IpEndpoint, Ipv4Address};

/// Address of the fixture host as seen from the guest (the iris NAT gateway).
const FIXTURE_IP: Ipv4Address = Ipv4Address::new(192, 168, 0, 1);
/// First local port; socket `i` uses `LOCAL_BASE + i`.
const LOCAL_BASE: u16 = 49_200;
/// Per-socket buffer size for each direction.
const BUF: usize = 512;
/// Time allowed for connect and for every echo to arrive.
const TIMEOUT_MS: i64 = 20_000;

/// Run the self-test if NVRAM asks for it.  Returns without printing when the
/// key is absent.
pub fn run_if_configured() {
    let mut base_text = [0u8; 16];
    let Some(n) = crate::nvram::get("ports.base", &mut base_text) else {
        return;
    };
    let mut count_text = [0u8; 16];
    let count = crate::nvram::get("ports.count", &mut count_text)
        .and_then(|n| {
            core::str::from_utf8(&count_text[..n])
                .ok()?
                .parse::<usize>()
                .ok()
        })
        .unwrap_or(8);
    let Some(base) = core::str::from_utf8(&base_text[..n])
        .ok()
        .and_then(|s| s.parse::<u16>().ok())
    else {
        crate::console_println("ports: fail ports.base is not a port number");
        return;
    };
    match run(base, count) {
        Ok(()) => crate::console_println(&format!("ports: pass sockets={count}")),
        Err(reason) => crate::console_println(&format!("ports: fail {reason}")),
    }
}

fn now_ms() -> i64 {
    crate::net::clock_now().total_millis()
}

fn run(base: u16, count: usize) -> Result<(), &'static str> {
    let mut handles: alloc::vec::Vec<SocketHandle> = alloc::vec::Vec::with_capacity(count);
    for _ in 0..count {
        let rx = Box::leak(Box::new([0u8; BUF]));
        let tx = Box::leak(Box::new([0u8; BUF]));
        let handle = crate::net::sockets_with(|sockets| {
            sockets.add(TcpSocket::new(
                SocketBuffer::new(&mut rx[..]),
                SocketBuffer::new(&mut tx[..]),
            ))
        })
        .ok_or("network stack is not up")?;
        handles.push(handle);
    }

    for (index, &handle) in handles.iter().enumerate() {
        let remote = IpEndpoint::new(IpAddress::Ipv4(FIXTURE_IP), base + index as u16);
        let local = LOCAL_BASE + index as u16;
        let connected = crate::net::with_stack(|iface, sockets| {
            sockets
                .get_mut::<TcpSocket>(handle)
                .connect(iface.context(), remote, local)
                .is_ok()
        });
        if connected != Some(true) {
            return Err("connect refused");
        }
    }

    let start = now_ms();
    loop {
        crate::net::poll();
        let all = handles.iter().all(|&h| {
            crate::net::sockets_with(|s| s.get::<TcpSocket>(h).state()) == Some(State::Established)
        });
        if all {
            break;
        }
        if now_ms() - start > TIMEOUT_MS {
            return Err("connect timed out");
        }
    }

    for (index, &handle) in handles.iter().enumerate() {
        let line = format!("ports-{index}\n");
        let sent = crate::net::sockets_with(|s| {
            s.get_mut::<TcpSocket>(handle)
                .send_slice(line.as_bytes())
                .is_ok()
        });
        if sent != Some(true) {
            return Err("send failed");
        }
    }

    let mut got: alloc::vec::Vec<alloc::vec::Vec<u8>> =
        (0..count).map(|_| alloc::vec::Vec::new()).collect();
    let start = now_ms();
    loop {
        crate::net::poll();
        let mut done = true;
        for (index, &handle) in handles.iter().enumerate() {
            let want = format!("ports-{index}\n");
            let echoed = &mut got[index];
            crate::net::sockets_with(|s| {
                let socket = s.get_mut::<TcpSocket>(handle);
                while socket.can_recv() && echoed.len() < want.len() {
                    let mut chunk = [0u8; 64];
                    match socket.recv_slice(&mut chunk) {
                        Ok(n) if n > 0 => echoed.extend_from_slice(&chunk[..n]),
                        _ => break,
                    }
                }
            });
            if echoed.len() < want.len() {
                done = false;
            } else if echoed.as_slice() != want.as_bytes() {
                return Err("echo mismatch");
            }
        }
        if done {
            break;
        }
        if now_ms() - start > TIMEOUT_MS {
            return Err("echo timed out");
        }
    }

    for handle in handles {
        let _ = crate::net::sockets_with(|s| {
            s.get_mut::<TcpSocket>(handle).close();
            s.remove(handle)
        });
    }
    Ok(())
}
