// SPDX-License-Identifier: GPL-3.0-only
//! Boot-time module autoload from the PROM environment.
//!
//! The MCP control plane is a module, so a machine that reboots must be able
//! to bring it back without a human at the telnet console.  The DS1386
//! environment supplies `mcp.url` and `mcp.sha256`; the loader fetches the
//! image (HTTPS by default), checks the digest, and runs it in place.

use crate::nvram;
use crate::sha256::Sha256;
use crate::{console_println, http, module, tls};

/// Load and run the configured module, if any.
pub fn boot() {
    let mut url = [0u8; 192];
    let Some(n) = nvram::get("mcp.url", &mut url) else {
        return;
    };
    let Ok(url) = core::str::from_utf8(&url[..n]) else {
        console_println("autoload: mcp.url is not UTF-8");
        return;
    };

    let mut digest_buf = [0u8; 80];
    let Some(dn) = nvram::get("mcp.sha256", &mut digest_buf) else {
        console_println("autoload: mcp.sha256 missing");
        return;
    };
    let Some(digest) = parse_digest(&digest_buf[..dn]) else {
        console_println("autoload: mcp.sha256 is not 64 hex digits");
        return;
    };

    match run(url, &digest) {
        Ok(code) => {
            console_println("autoload: module returned");
            if code != 0 {
                console_println("autoload: non-zero status");
            }
        }
        Err(err) => {
            console_println("autoload: failed");
            console_println(err);
        }
    }
}

fn run(url: &str, digest: &Sha256) -> Result<i32, &'static str> {
    let (secure, rest) = if let Some(rest) = url.strip_prefix("https://") {
        (true, rest)
    } else if let Some(rest) = url.strip_prefix("http://") {
        (false, rest)
    } else {
        (true, url)
    };
    let (host_port, path) = match rest.find('/') {
        Some(i) => (&rest[..i], &rest[i..]),
        None => (rest, "/"),
    };
    if host_port.is_empty() {
        return Err("autoload: empty host");
    }
    let (host, port) = match host_port.rsplit_once(':') {
        Some((host, port)) => (host, port.parse::<u16>().map_err(|_| "autoload: bad port")?),
        None => (host_port, if secure { 443 } else { 80 }),
    };
    let ip = crate::telnet::resolve_host(host.as_bytes()).ok_or("autoload: unknown host")?;

    console_println("autoload: fetching module");
    module::clear();
    let area = module::area();
    let (summary, captured) = if secure {
        tls::get_to(ip, port, host, path, area)?
    } else {
        http::get_to(ip, port, host, path, area)?
    };
    if captured != summary.body_len {
        return Err("autoload: module did not fit");
    }
    if !summary.sha256.eq_digest(digest) {
        return Err("autoload: module hash mismatch");
    }
    // SAFETY: the image fits and its hash matched.
    unsafe { Ok(module::call_entry(captured)) }
}

fn parse_digest(hex: &[u8]) -> Option<Sha256> {
    if hex.len() != 64 {
        return None;
    }
    let mut digest = [0u8; 32];
    for (i, byte) in digest.iter_mut().enumerate() {
        let hi = hex_value(hex[i * 2])?;
        let lo = hex_value(hex[i * 2 + 1])?;
        *byte = (hi << 4) | lo;
    }
    Some(Sha256(digest))
}

fn hex_value(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}
