// SPDX-License-Identifier: GPL-3.0-only
//! Kernel URL fetch used by the runtime module loader.
//!
//! The ABI v12 `module_load` service performs SHA-256 verification and copies
//! into the arena; this module owns the HTTP/TLS fetch and bounded capture.

use crate::http::Summary;

const URL_MAX: usize = module_abi::MODULE_URL_MAX as usize;

pub(crate) fn fetch_url(url: &str, out: &mut [u8]) -> Result<Summary, &'static str> {
    if url.is_empty() || url.len() > URL_MAX {
        return Err("loader: URL too long");
    }
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
