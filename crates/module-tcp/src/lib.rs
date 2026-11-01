// SPDX-License-Identifier: GPL-3.0-only
//! TCP slots for loadable modules, over any kernel's smoltcp stack.
//!
//! A module sees a fixed table of TCP slots.  Each slot owns at most one socket
//! and one pair of bounded rings, so a module can never make the kernel
//! allocate on its behalf.  Readiness is level-triggered and recomputed from
//! the live socket, and the return codes are the module ABI's.
//!
//! The kernel supplies the stack through [`Stack`] and declares one [`Slots`]
//! as a static.  Both kernels use this table, so the slot policy is written once.

#![no_std]

use core::ptr::addr_of_mut;

use smoltcp::iface::{Interface, SocketHandle, SocketSet};
use smoltcp::socket::tcp::{Socket as TcpSocket, SocketBuffer, State};
use smoltcp::time::Duration;
use smoltcp::wire::{IpAddress, Ipv4Address};

/// Readiness: data can be received.
pub const EV_RX: u32 = 1 << 0;
/// Readiness: a blocked send can now make progress.
pub const EV_TX: u32 = 1 << 1;
/// Readiness: the peer has closed.
pub const EV_CLOSED: u32 = 1 << 2;
/// The socket is listening.
pub const EV_LISTEN: u32 = 1 << 3;
/// The socket is established.
pub const EV_ESTABLISHED: u32 = 1 << 4;
/// The slot holds no socket.
pub const EV_NO_SOCKET: u32 = 1 << 31;

/// The stack operations the slot table needs.  A kernel implements this over
/// its own global stack; the functions return `None` before the stack exists.
pub trait Stack {
    /// Run `f` with the socket set.
    fn with_sockets<R>(f: impl FnOnce(&mut SocketSet<'static>) -> R) -> Option<R>;

    /// Run `f` with the interface and the socket set together.
    fn with_stack<R>(f: impl FnOnce(&mut Interface, &mut SocketSet<'static>) -> R) -> Option<R>;
}

/// The slot table: handles, send state, and the bounded rings of every slot.
pub struct Slots<const SLOTS: usize, const BUFFER: usize> {
    handles: [Option<SocketHandle>; SLOTS],
    tx_blocked: [bool; SLOTS],
    rx: [[u8; BUFFER]; SLOTS],
    tx: [[u8; BUFFER]; SLOTS],
}

impl<const SLOTS: usize, const BUFFER: usize> Default for Slots<SLOTS, BUFFER> {
    fn default() -> Self {
        Self::new()
    }
}

impl<const SLOTS: usize, const BUFFER: usize> Slots<SLOTS, BUFFER> {
    /// An empty table.  Declare it as a `static` in the kernel.
    pub const fn new() -> Self {
        Self {
            handles: [None; SLOTS],
            tx_blocked: [false; SLOTS],
            rx: [[0; BUFFER]; SLOTS],
            tx: [[0; BUFFER]; SLOTS],
        }
    }

    /// The number of slots the table provides.
    pub fn slots(&self) -> u32 {
        SLOTS as u32
    }

    fn handle(&self, slot: u32) -> Option<SocketHandle> {
        let index = slot as usize;
        if index >= SLOTS {
            return None;
        }
        self.handles[index]
    }

    fn set_handle(&mut self, index: usize, value: Option<SocketHandle>) {
        self.handles[index] = value;
        self.tx_blocked[index] = false;
    }

    /// A new socket over the slot's own rings, with the module keepalive.
    fn socket(&mut self, index: usize) -> TcpSocket<'static> {
        // SAFETY: the table is a static in the kernel, so the rings live for the
        // whole run.  A slot's rings belong to at most one socket at a time:
        // `clear` removes the old socket before the slot is reused.
        let rx: &'static mut [u8; BUFFER] = unsafe { &mut *addr_of_mut!(self.rx[index]) };
        let tx: &'static mut [u8; BUFFER] = unsafe { &mut *addr_of_mut!(self.tx[index]) };
        let mut socket = TcpSocket::new(
            SocketBuffer::new(&mut rx[..]),
            SocketBuffer::new(&mut tx[..]),
        );
        socket.set_keep_alive(Some(Duration::from_secs(60)));
        socket
    }

    /// Remove whatever socket the slot holds.
    fn clear<S: Stack>(&mut self, index: usize) {
        if let Some(old) = self.handle(index as u32) {
            let _ = S::with_sockets(|sockets| sockets.remove(old));
            self.set_handle(index, None);
        }
    }

    /// Listen on `port` in `slot`.  Returns 0, or -1 on failure.
    pub fn listen<S: Stack>(&mut self, slot: u32, port: u16) -> i32 {
        let index = slot as usize;
        if index >= SLOTS || port == 0 {
            return -1;
        }
        self.clear::<S>(index);
        let socket = self.socket(index);
        let Some(added) = S::with_sockets(|sockets| sockets.add(socket)) else {
            return -1;
        };
        let listened =
            S::with_sockets(|sockets| sockets.get_mut::<TcpSocket>(added).listen(port).is_ok());
        if listened == Some(true) {
            self.set_handle(index, Some(added));
            0
        } else {
            let _ = S::with_sockets(|sockets| sockets.remove(added));
            -1
        }
    }

    /// Connect `slot` to IPv4 `address` (network byte order) and `port`.  Each
    /// slot binds its own ephemeral local port, so several connects can be live.
    pub fn connect<S: Stack>(&mut self, slot: u32, address: u32, port: u16) -> i32 {
        let index = slot as usize;
        if index >= SLOTS || address == 0 || port == 0 {
            return -1;
        }
        self.clear::<S>(index);
        let socket = self.socket(index);
        let octets = address.to_be_bytes();
        let remote = Ipv4Address::new(octets[0], octets[1], octets[2], octets[3]);
        let local_port = 49152 + index as u16;
        let result = S::with_stack(|iface, sockets| {
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
                self.set_handle(index, Some(added));
                0
            }
            Some((added, Err(_))) => {
                let _ = S::with_sockets(|sockets| sockets.remove(added));
                -1
            }
            None => -1,
        }
    }

    /// Readiness bits for `slot`, recomputed from the live socket.
    pub fn events<S: Stack>(&self, slot: u32) -> u32 {
        let Some(added) = self.handle(slot) else {
            return EV_NO_SOCKET;
        };
        let tx_blocked = self.tx_blocked[slot as usize];
        S::with_sockets(|sockets| {
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

    /// The module ABI's coarse state: 0 none, 1 opening or listening, 2 open,
    /// 3 closing.
    pub fn state<S: Stack>(&self, slot: u32) -> u32 {
        let Some(added) = self.handle(slot) else {
            return 0;
        };
        match S::with_sockets(|sockets| sockets.get::<TcpSocket>(added).state()) {
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

    /// Receive into `buffer`.  Returns the byte count, or -1.
    ///
    /// # Safety
    ///
    /// `ptr` must be valid for `len` writes for the duration of the call.
    pub unsafe fn recv<S: Stack>(&self, slot: u32, ptr: *mut u8, len: usize) -> isize {
        if ptr.is_null() || len == 0 {
            return -1;
        }
        let Some(added) = self.handle(slot) else {
            return -1;
        };
        // SAFETY: the caller guarantees the destination for the call.
        let buffer = unsafe { core::slice::from_raw_parts_mut(ptr, len) };
        match S::with_sockets(|sockets| sockets.get_mut::<TcpSocket>(added).recv_slice(buffer)) {
            Some(Ok(count)) => count as isize,
            _ => -1,
        }
    }

    /// Send from `buffer`.  Returns the byte count accepted, or -1.  A short
    /// send marks the slot blocked, so its events report `EV_TX` once room opens.
    ///
    /// # Safety
    ///
    /// `ptr` must be valid for `len` reads for the duration of the call.
    pub unsafe fn send<S: Stack>(&mut self, slot: u32, ptr: *const u8, len: usize) -> isize {
        if ptr.is_null() || len == 0 {
            return -1;
        }
        let Some(added) = self.handle(slot) else {
            return -1;
        };
        // SAFETY: the caller guarantees the source for the call.
        let buffer = unsafe { core::slice::from_raw_parts(ptr, len) };
        match S::with_sockets(|sockets| sockets.get_mut::<TcpSocket>(added).send_slice(buffer)) {
            Some(Ok(count)) => {
                self.tx_blocked[slot as usize] = count < len;
                count as isize
            }
            _ => -1,
        }
    }

    /// Close the slot's socket gracefully.
    pub fn close<S: Stack>(&mut self, slot: u32) {
        if let Some(added) = self.handle(slot) {
            let _ = S::with_sockets(|sockets| sockets.get_mut::<TcpSocket>(added).close());
        }
    }

    /// Abort the slot's socket.
    pub fn abort<S: Stack>(&mut self, slot: u32) {
        if let Some(added) = self.handle(slot) {
            let _ = S::with_sockets(|sockets| sockets.get_mut::<TcpSocket>(added).abort());
        }
    }
}
