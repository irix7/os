// SPDX-License-Identifier: GPL-3.0-only
//! Module TCP slots on the x86_64 machine.
//!
//! The slot table is the shared `module-tcp` crate, which the Indy kernel also
//! uses.  This file supplies the x86 stack and the machine's slot table: one
//! static table, and the extern entry points the module services call.

use smoltcp::iface::{Interface, SocketSet};

use crate::net;
use module_tcp::Slots;

/// Module TCP slots.  Matches the Indy kernel's policy.
pub const SLOTS: usize = 8;
/// Ring size per direction.  Sized for the module's HTTP responses.
const BUFFER: usize = 16384;

pub use module_tcp::EV_CLOSED;

/// The machine's network stack, for the shared slot table.
pub struct X86Stack;

impl module_tcp::Stack for X86Stack {
    fn with_sockets<R>(f: impl FnOnce(&mut SocketSet<'static>) -> R) -> Option<R> {
        net::with_sockets(f)
    }

    fn with_stack<R>(f: impl FnOnce(&mut Interface, &mut SocketSet<'static>) -> R) -> Option<R> {
        net::with_stack(f)
    }
}

static mut TABLE: Slots<SLOTS, BUFFER> = Slots::new();

/// The machine's slot table.  The machine is single-threaded.
pub fn table() -> &'static mut Slots<SLOTS, BUFFER> {
    // SAFETY: single-threaded; the table is only used from the service entry points.
    unsafe { &mut *core::ptr::addr_of_mut!(TABLE) }
}

pub unsafe extern "C" fn listen(slot: u32, port: u16) -> i32 {
    table().listen::<X86Stack>(slot, port)
}

pub unsafe extern "C" fn connect(slot: u32, address: u32, port: u16) -> i32 {
    table().connect::<X86Stack>(slot, address, port)
}

pub extern "C" fn events(slot: u32) -> u32 {
    table().events::<X86Stack>(slot)
}

pub extern "C" fn slots() -> u32 {
    table().slots()
}

pub extern "C" fn state(slot: u32) -> u32 {
    table().state::<X86Stack>(slot)
}

pub unsafe extern "C" fn recv(slot: u32, ptr: *mut u8, len: usize) -> isize {
    // SAFETY: the module owns the destination buffer for the call.
    unsafe { table().recv::<X86Stack>(slot, ptr, len) }
}

pub unsafe extern "C" fn send(slot: u32, ptr: *const u8, len: usize) -> isize {
    // SAFETY: the module owns the source buffer for the call.
    unsafe { table().send::<X86Stack>(slot, ptr, len) }
}

pub extern "C" fn close(slot: u32) {
    table().close::<X86Stack>(slot);
}

pub extern "C" fn abort(slot: u32) {
    table().abort::<X86Stack>(slot);
}
