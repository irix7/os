// SPDX-License-Identifier: GPL-3.0-only
//! Autoload: fetch a module image over HTTP and check it against its digest.
//!
//! This is the x86 side of the Indy's boot autoload.  The kernel reads the URL
//! from boot-disk sector 9 (`a.b.c.d:port/path`) and the expected digest from
//! the module header on sector 8.  It fetches with HTTP/1.0 over one module TCP
//! slot, so the fetch uses the same services a module does.  A fetched image
//! whose digest does not match is refused.

use alloc::vec::Vec;

use irix7_sha256::Sha256;
use smoltcp::wire::Ipv4Address;

use crate::memory;
use crate::net;
use crate::tcp;
use crate::timer;
use crate::virtio_blk::{Disk, SECTOR};

/// Disk sector holding the MCP module's URL.
pub const MCP_URL_SECTOR: u64 = 9;
/// Disk sector holding the wasm module's URL.
pub const WASM_URL_SECTOR: u64 = 11;
/// TCP slots the kernel's fetches use.  They are free at boot, before any
/// module.  Each image gets its own slot so a second fetch never races the
/// first fetch's close.
pub const SLOT_MCP: u32 = 7;
pub const SLOT_WASM: u32 = 6;
/// Largest image the fetch accepts: 2 MiB, one contiguous run of frames.
const CAPACITY: usize = 2 * 1024 * 1024;
/// Each network step gets this long before the fetch gives up: 30 s at 100 Hz.
const TIMEOUT_TICKS: u32 = 3000;

/// Fetch one module image named on the disk, and check it against the digest
/// in its header sector.  Returns the verified bytes.  Stops the machine on any
/// failure, like the module loader.
pub fn image(
    disk: &mut Disk,
    name: &str,
    url_sector: u64,
    header_sector: u64,
    slot: u32,
) -> &'static [u8] {
    let mut url_bytes = [0u8; SECTOR];
    if !disk.read(url_sector, &mut url_bytes) {
        fail("x86: autoload: url sector unreadable");
    }
    let text = &url_bytes[..url_bytes.iter().position(|&b| b == 0).unwrap_or(SECTOR)];
    let Some((address, port, path)) = parse_url(text) else {
        fail("x86: autoload: url not understood");
    };

    let mut sector = [0u8; SECTOR];
    if !disk.read(header_sector, &mut sector) {
        fail("x86: autoload: header unreadable");
    }
    let mut digest = [0u8; 32];
    digest.copy_from_slice(&sector[16..48]);

    let Some(base) = memory::alloc_frames(CAPACITY / 4096) else {
        fail("x86: autoload: no frames for the image");
    };
    // SAFETY: the frames were just allocated and are identity mapped, so the
    // physical address is a valid, unused buffer of CAPACITY bytes.
    let buffer = unsafe { core::slice::from_raw_parts_mut(base as usize as *mut u8, CAPACITY) };

    let length = match get(slot, address, port, path, buffer) {
        Ok(length) => length,
        Err(message) => fail(message),
    };
    crate::serial::write(b"x86: autoload ");
    crate::serial::write(name.as_bytes());
    crate::serial::write(b" fetched ");
    crate::print_dec(length as u64);
    crate::serial::write(b" bytes\n");

    let fetched = &buffer[..length];
    if Sha256::digest(fetched).0 != digest {
        fail("x86: autoload digest mismatch");
    }
    crate::serial::write(b"x86: autoload ");
    crate::serial::write(name.as_bytes());
    crate::serial::write(b" sha256 ok\n");
    fetched
}

fn fail(message: &str) -> ! {
    crate::print_line(&[message.as_bytes()]);
    crate::qemu_exit(1)
}

/// Parse `a.b.c.d:port/path`.  The path keeps its leading slash.
fn parse_url(text: &[u8]) -> Option<(Ipv4Address, u16, &str)> {
    let text = core::str::from_utf8(text).ok()?;
    let slash = text.find('/')?;
    let (authority, path) = text.split_at(slash);
    let (host, port) = authority.split_once(':')?;
    let mut octets = [0u8; 4];
    let mut parts = host.split('.');
    for octet in octets.iter_mut() {
        *octet = parts.next()?.parse().ok()?;
    }
    if parts.next().is_some() {
        return None;
    }
    let port: u16 = port.parse().ok()?;
    Some((Ipv4Address::from_octets(octets), port, path))
}

/// Wait until `done` holds, polling the stack, or give up after the timeout.
fn wait(mut done: impl FnMut() -> bool) -> bool {
    let start = timer::ticks();
    while !done() {
        if timer::ticks().wrapping_sub(start) >= TIMEOUT_TICKS {
            return false;
        }
        net::poll();
        core::hint::spin_loop();
    }
    true
}

/// HTTP/1.0 GET over the kernel's module slot.  The body is copied to the
/// start of `out`, and its length is returned.
fn get(
    slot: u32,
    address: Ipv4Address,
    port: u16,
    path: &str,
    out: &mut [u8],
) -> Result<usize, &'static str> {
    let octets = address.octets();
    let ip = u32::from_be_bytes(octets);
    // SAFETY: the slot is free at boot and the call matches the module ABI.
    if unsafe { tcp::connect(slot, ip, port) } != 0 {
        return Err("x86: autoload: connect refused");
    }
    if !wait(|| tcp::state(slot) == 2) {
        return Err("x86: autoload: connect timed out");
    }

    let mut request = Vec::new();
    request.extend_from_slice(b"GET ");
    request.extend_from_slice(path.as_bytes());
    request.extend_from_slice(b" HTTP/1.0\r\nHost: autoload\r\nConnection: close\r\n\r\n");
    let mut sent = 0;
    while sent < request.len() {
        // SAFETY: the request buffer lives for the call.
        let n = unsafe { tcp::send(slot, request[sent..].as_ptr(), request.len() - sent) };
        if n > 0 {
            sent += n as usize;
        }
        net::poll();
    }

    let mut received = 0usize;
    let start = timer::ticks();
    loop {
        if received == out.len() {
            return Err("x86: autoload: image larger than the buffer");
        }
        // SAFETY: the destination is inside `out`, with room for the call.
        let n = unsafe { tcp::recv(slot, out[received..].as_mut_ptr(), out.len() - received) };
        if n > 0 {
            received += n as usize;
            continue;
        }
        // A peer that has sent its whole response closes its side: smoltcp then
        // reports the receive as an error while the socket leaves Established.
        if tcp::events(slot) & tcp::EV_CLOSED != 0 || (n < 0 && tcp::state(slot) != 2) {
            break;
        }
        if timer::ticks().wrapping_sub(start) >= TIMEOUT_TICKS * 4 {
            return Err("x86: autoload: transfer timed out");
        }
        net::poll();
    }
    tcp::close(slot);

    let response = &out[..received];
    let headers_end = response
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .ok_or("x86: autoload: no HTTP header")?
        + 4;
    let status_line_end = response.iter().position(|&b| b == b'\r').unwrap_or(0);
    let status_line = &response[..status_line_end];
    if !status_line.starts_with(b"HTTP/1.") || !status_line.windows(5).any(|w| w == b" 200 ") {
        return Err("x86: autoload: HTTP status is not 200");
    }
    out.copy_within(headers_end..received, 0);
    Ok(received - headers_end)
}
