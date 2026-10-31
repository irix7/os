// SPDX-License-Identifier: GPL-3.0-only
//! TCP slots for loadable modules, over the x86 smoltcp stack.
//!
//! Semantics follow the Indy kernel's module TCP services: a fixed table of
//! slots, one socket per slot, level-triggered readiness, and the same return
//! codes.  Each slot owns a bounded receive and transmit ring, so a module can
//! never make the kernel allocate on its behalf.

use core::ptr::addr_of_mut;

use smoltcp::iface::SocketHandle;
use smoltcp::socket::tcp::{Socket as TcpSocket, SocketBuffer, State};
use smoltcp::time::Duration;
use smoltcp::wire::{IpAddress, Ipv4Address};

use crate::net;

/// Module TCP slots.  Matches the Indy kernel's policy.
pub const SLOTS: usize = 8;
/// Ring size per direction.  Sized for the module's HTTP responses.
const BUFFER: usize = 16384;

/// Readiness bits, as the Indy kernel defines them.
const EV_RX: u32 = 1 << 0;
const EV_TX: u32 = 1 << 1;
pub const EV_CLOSED: u32 = 1 << 2;
const EV_LISTEN: u32 = 1 << 3;
const EV_ESTABLISHED: u32 = 1 << 4;
const EV_NO_SOCKET: u32 = 1 << 31;

static mut HANDLES: [Option<SocketHandle>; SLOTS] = [None; SLOTS];
static mut TX_BLOCKED: [bool; SLOTS] = [false; SLOTS];
static mut RX: [[u8; BUFFER]; SLOTS] = [[0; BUFFER]; SLOTS];
static mut TX: [[u8; BUFFER]; SLOTS] = [[0; BUFFER]; SLOTS];

fn handle(slot: u32) -> Option<SocketHandle> {
    let slot = slot as usize;
    if slot >= SLOTS {
        return None;
    }
    // SAFETY: single-threaded; written only by the functions below.
    unsafe { HANDLES[slot] }
}

fn set_handle(slot: usize, value: Option<SocketHandle>) {
    // SAFETY: single-threaded; each slot has one owner at a time.
    unsafe {
        HANDLES[slot] = value;
        TX_BLOCKED[slot] = false;
    }
}

/// A new socket over the slot's own rings.
fn new_socket(slot: usize) -> TcpSocket<'static> {
    // SAFETY: each slot's rings belong to at most one socket at a time, and
    // `clear` removes the old socket before a slot is reused.
    let (rx, tx) = unsafe { (&mut *addr_of_mut!(RX[slot]), &mut *addr_of_mut!(TX[slot])) };
    let mut socket = TcpSocket::new(
        SocketBuffer::new(&mut rx[..]),
        SocketBuffer::new(&mut tx[..]),
    );
    socket.set_keep_alive(Some(Duration::from_secs(60)));
    socket
}

/// Remove whatever socket the slot holds.
fn clear(slot: usize) {
    if let Some(old) = handle(slot as u32) {
        let _ = net::with_sockets(|sockets| sockets.remove(old));
        set_handle(slot, None);
    }
}

pub extern "C" fn slots() -> u32 {
    SLOTS as u32
}

pub unsafe extern "C" fn listen(slot: u32, port: u16) -> i32 {
    let slot_index = slot as usize;
    if slot_index >= SLOTS || port == 0 {
        return -1;
    }
    clear(slot_index);
    let socket = new_socket(slot_index);
    let Some(added) = net::with_sockets(|sockets| sockets.add(socket)) else {
        return -1;
    };
    let listened =
        net::with_sockets(|sockets| sockets.get_mut::<TcpSocket>(added).listen(port).is_ok());
    if listened == Some(true) {
        set_handle(slot_index, Some(added));
        0
    } else {
        let _ = net::with_sockets(|sockets| sockets.remove(added));
        -1
    }
}

pub unsafe extern "C" fn connect(slot: u32, address: u32, port: u16) -> i32 {
    let slot_index = slot as usize;
    if slot_index >= SLOTS || address == 0 || port == 0 {
        return -1;
    }
    clear(slot_index);
    let socket = new_socket(slot_index);
    let octets = address.to_be_bytes();
    let remote = Ipv4Address::new(octets[0], octets[1], octets[2], octets[3]);
    // Each slot takes its own ephemeral local port so several connects can be live.
    let local_port = 49152 + slot_index as u16;
    let result = net::with_stack(|iface, sockets| {
        let added = sockets.add(socket);
        let connected = sockets.get_mut::<TcpSocket>(added).connect(
            iface.context(),
            (IpAddress::Ipv4(remote), port),
            local_port,
        );
        (added, connected)
    });
    match result {
        Some((added, Ok(()))) => {
            set_handle(slot_index, Some(added));
            0
        }
        Some((added, Err(_))) => {
            let _ = net::with_sockets(|sockets| sockets.remove(added));
            -1
        }
        None => -1,
    }
}

pub extern "C" fn events(slot: u32) -> u32 {
    let Some(added) = handle(slot) else {
        return EV_NO_SOCKET;
    };
    let tx_blocked = unsafe { TX_BLOCKED[slot as usize] };
    net::with_sockets(|sockets| {
        let socket = sockets.get_mut::<TcpSocket>(added);
        let mut mask = 0;
        if socket.can_recv() {
            mask |= EV_RX;
        }
        if tx_blocked && socket.can_send() {
            mask |= EV_TX;
        }
        match socket.state() {
            State::Listen => mask |= EV_LISTEN,
            State::Established => mask |= EV_ESTABLISHED,
            State::Closed | State::TimeWait => mask |= EV_CLOSED,
            _ => {}
        }
        mask
    })
    .unwrap_or(EV_NO_SOCKET)
}

/// The module ABI's coarse state codes: 0 none, 1 opening or listening, 2 open,
/// 3 closing.
pub extern "C" fn state(slot: u32) -> u32 {
    let Some(added) = handle(slot) else {
        return 0;
    };
    match net::with_sockets(|sockets| sockets.get::<TcpSocket>(added).state()) {
        Some(State::Listen) | Some(State::SynSent) | Some(State::SynReceived) => 1,
        Some(State::Established) => 2,
        Some(State::FinWait1)
        | Some(State::FinWait2)
        | Some(State::CloseWait)
        | Some(State::Closing)
        | Some(State::LastAck)
        | Some(State::TimeWait) => 3,
        Some(State::Closed) | None => 0,
    }
}

pub unsafe extern "C" fn recv(slot: u32, ptr: *mut u8, len: usize) -> isize {
    if ptr.is_null() || len == 0 {
        return -1;
    }
    let Some(added) = handle(slot) else {
        return -1;
    };
    // SAFETY: the module owns the destination buffer for the call.
    let buffer = unsafe { core::slice::from_raw_parts_mut(ptr, len) };
    match net::with_sockets(|sockets| sockets.get_mut::<TcpSocket>(added).recv_slice(buffer)) {
        Some(Ok(count)) => count as isize,
        _ => -1,
    }
}

pub unsafe extern "C" fn send(slot: u32, ptr: *const u8, len: usize) -> isize {
    if ptr.is_null() || len == 0 {
        return -1;
    }
    let Some(added) = handle(slot) else {
        return -1;
    };
    // SAFETY: the module owns the source buffer for the call.
    let buffer = unsafe { core::slice::from_raw_parts(ptr, len) };
    match net::with_sockets(|sockets| sockets.get_mut::<TcpSocket>(added).send_slice(buffer)) {
        Some(Ok(count)) => {
            // SAFETY: single-threaded.
            unsafe { TX_BLOCKED[slot as usize] = count < len };
            count as isize
        }
        _ => -1,
    }
}

pub extern "C" fn close(slot: u32) {
    if let Some(added) = handle(slot) {
        let _ = net::with_sockets(|sockets| sockets.get_mut::<TcpSocket>(added).close());
    }
}

pub extern "C" fn abort(slot: u32) {
    if let Some(added) = handle(slot) {
        let _ = net::with_sockets(|sockets| sockets.get_mut::<TcpSocket>(added).abort());
    }
}
