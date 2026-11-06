// SPDX-License-Identifier: GPL-3.0-only
//! Module TCP slots on the x86_64 machine.
//!
//! The slot table, its bounds and its event policy are the shared
//! `tcp-slots` crate, which the Indy kernel also uses.  This file supplies the
//! machine's stack: one static table and the two `Stack` accessors.

use smoltcp::iface::{Interface, SocketSet};

use crate::net;
use tcp_slots::Slots;

/// Module TCP slots.  Matches the Indy kernel's policy.
pub const SLOTS: usize = 8;
/// Ring size per direction.  Sized for the module's HTTP responses.
const BUFFER: usize = 16384;

/// The machine's network stack, for the shared slot table.
pub struct X86Stack;

impl tcp_slots::Stack for X86Stack {
    fn with_sockets<R>(f: impl FnOnce(&mut SocketSet<'static>) -> R) -> Option<R> {
        net::with_sockets(f)
    }

    fn with_stack<R>(f: impl FnOnce(&mut Interface, &mut SocketSet<'static>) -> R) -> Option<R> {
        net::with_stack(f)
    }
}

static mut TABLE: Slots<SLOTS, BUFFER> = Slots::new();

/// The machine's slot table.  The machine is single-threaded.
fn table() -> &'static mut Slots<SLOTS, BUFFER> {
    // SAFETY: single-threaded; the table is only used from these entry points.
    unsafe { &mut *core::ptr::addr_of_mut!(TABLE) }
}

pub fn listen(slot: u32, port: u16) -> i32 {
    table().listen::<X86Stack>(slot, port)
}

pub fn connect(slot: u32, address: u32, port: u16) -> i32 {
    table().connect::<X86Stack>(slot, address, port)
}

pub fn events(slot: u32) -> u32 {
    table().events::<X86Stack>(slot)
}

pub fn slots() -> u32 {
    table().slots()
}

pub fn state(slot: u32) -> u32 {
    table().state::<X86Stack>(slot)
}

pub fn recv(slot: u32, out: &mut [u8]) -> isize {
    table().recv_into::<X86Stack>(slot, out)
}

pub fn send(slot: u32, bytes: &[u8]) -> isize {
    table().send_from::<X86Stack>(slot, bytes)
}

pub fn close(slot: u32) {
    table().close::<X86Stack>(slot);
}

pub fn abort(slot: u32) {
    table().abort::<X86Stack>(slot);
}
