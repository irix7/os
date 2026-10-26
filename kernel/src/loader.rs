// SPDX-License-Identifier: GPL-3.0-only
//! Kernel loader/arena service for the downloadable runtime module manager.
//!
//! The manager module is linked in slot B after its 256 KiB image. It reads the
//! fixed [`loader_abi::TABLE_ADDR`] table and asks the kernel to fetch and
//! verify a dependency image into the arena, or to enter a loaded image with
//! cache maintenance. Resolution, signatures, package metadata and policy stay
//! in the manager module.

use core::ptr;
use core::slice;

use crate::http::Summary;
use crate::sha256::Sha256;
use loader_abi::{in_arena, Entry, LoaderTable, ARENA_BASE, ARENA_LEN, MAGIC, VERSION};

const URL_MAX: usize = 512;

/// Publish the loader table at its fixed kseg0 address.
pub fn init() {
    let table = LoaderTable {
        magic: MAGIC,
        version: VERSION,
        reserved: 0,
        arena_base: ARENA_BASE,
        arena_len: ARENA_LEN,
        load: loader_load,
        fetch: loader_fetch,
        enter: loader_enter,
    };
    // SAFETY: the page is in the reserved low window, below the module area,
    // and is initialised once on the boot path before any manager runs.
    unsafe {
        ptr::write_volatile(loader_abi::TABLE_ADDR as *mut LoaderTable, table);
    }
}

unsafe extern "C" fn loader_load(
    url: *const u8,
    url_len: usize,
    digest: *const u8,
    dest: usize,
    dest_len: usize,
) -> i32 {
    if url.is_null() || digest.is_null() || url_len == 0 || url_len > URL_MAX {
        return -1;
    }
    if !in_arena(dest, dest_len) {
        return -1;
    }
    // SAFETY: the caller supplied a readable URL/digest for this synchronous
    // call and an arena destination validated above.
    let url = unsafe { slice::from_raw_parts(url, url_len) };
    let Ok(url) = core::str::from_utf8(url) else {
        return -1;
    };
    // SAFETY: the digest is exactly 32 bytes by the loader contract.
    let digest = unsafe { slice::from_raw_parts(digest, 32) };
    let mut expected = [0u8; 32];
    expected.copy_from_slice(digest);
    let destination = unsafe { slice::from_raw_parts_mut(dest as *mut u8, dest_len) };
    let summary = match fetch_url(url, destination) {
        Ok(summary) => summary,
        Err(_) => return -1,
    };
    if !summary.sha256.eq_digest(&Sha256(expected)) {
        return -1;
    }
    0
}

unsafe extern "C" fn loader_fetch(
    url: *const u8,
    url_len: usize,
    out: *mut u8,
    out_len: usize,
) -> isize {
    if url.is_null() || out.is_null() || url_len == 0 || url_len > URL_MAX || out_len == 0 {
        return -1;
    }
    // SAFETY: the caller supplied readable URL bytes and a writable buffer for
    // this synchronous call.
    let url = unsafe { slice::from_raw_parts(url, url_len) };
    let Ok(url) = core::str::from_utf8(url) else {
        return -1;
    };
    let out = unsafe { slice::from_raw_parts_mut(out, out_len) };
    match fetch_url(url, out) {
        Ok(summary) => {
            if summary.body_len > out.len() {
                -1
            } else {
                summary.body_len as isize
            }
        }
        Err(_) => -1,
    }
}

unsafe extern "C" fn loader_enter(entry: usize, len: usize, context: *const u8) -> i32 {
    if !in_arena(entry, len) {
        return -1;
    }
    crate::cache::dcache_writeback_invalidate(entry, len);
    crate::cache::icache_invalidate(entry, len);
    crate::cache::sync();
    // SAFETY: the entry was written inside the reserved arena by the loader and
    // its SHA-256 was verified. The caller owns the context pointer for the
    // duration of the call.
    let entry: Entry = unsafe { core::mem::transmute(entry) };
    unsafe { entry(loader_abi::TABLE_ADDR as *const LoaderTable, context) }
}

fn fetch_url(url: &str, out: &mut [u8]) -> Result<Summary, &'static str> {
    let (secure, rest) = if let Some(rest) = url.strip_prefix("https://") {
        (true, rest)
    } else if let Some(rest) = url.strip_prefix("http://") {
        (false, rest)
    } else {
        (true, url)
    };
    let (host_port, path) = match rest.find('/') {
        Some(index) => (&rest[..index], &rest[index..]),
        None => (rest, "/"),
    };
    if host_port.is_empty() {
        return Err("loader: empty host");
    }
    let (host, port) = match host_port.rsplit_once(':') {
        Some((host, port)) => (host, port.parse::<u16>().map_err(|_| "loader: bad port")?),
        None => (host_port, if secure { 443 } else { 80 }),
    };
    let ip = crate::telnet::resolve_host(host.as_bytes()).ok_or("loader: unknown host")?;
    let (summary, captured) = if secure {
        crate::tls::get_to(ip, port, host, path, out)?
    } else {
        crate::http::get_to(ip, port, host, path, out)?
    };
    if summary.status != 200 {
        return Err("loader: HTTP status");
    }
    if captured != summary.body_len {
        return Err("loader: body too large");
    }
    Ok(summary)
}
