// SPDX-License-Identifier: GPL-3.0-only
//! Boot-time module autoload from the repository boot configuration.
//!
//! The MCP control plane is a module, so a machine that reboots must be able
//! to bring it back without a human at the telnet console.  The kernel fetches
//! `boot/config.conf` from the repository, reads `mcp.url` and `mcp.sha256`,
//! then fetches the image over HTTPS, checks the digest, and runs it in place.
//! A local test rig can override the two keys in the DS1386 environment.

use crate::nvram;
use crate::sha256::Sha256;
use crate::{console_println, http, module, tls};

/// Outcome of the boot-time module autoload.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Status {
    /// No `mcp.url` is configured.
    Disabled,
    /// The configured image loaded and returned a zero status.
    Loaded,
    /// Configuration or fetch/verification failed.
    Failed,
}

/// Load and run the configured module, if any.
///
/// Local NVRAM `mcp.url`/`mcp.sha256` wins when present, because the test rig
/// uses it to point at a temporary HTTP fixture.  Otherwise the kernel reads
/// the repository boot configuration, which the release workflow updates with
/// raw GitHub URLs for the published module.  A normal Indy never needs the
/// NVRAM override; the chip is validated and repaired at startup.
pub fn boot() -> Status {
    let mut url = [0u8; 192];
    if let Some(n) = nvram::get("mcp.url", &mut url) {
        return boot_nvram(&url[..n]);
    }

    console_println("autoload: loading repository boot config");
    let Some(config) = crate::boot_config::fetch() else {
        return Status::Failed;
    };
    let Some(url) = config.url_string() else {
        return Status::Failed;
    };
    finish(run(url, &config.digest()))
}

fn boot_nvram(url: &[u8]) -> Status {
    let Ok(url) = core::str::from_utf8(url) else {
        console_println("autoload: mcp.url is not UTF-8");
        return Status::Failed;
    };

    let mut digest_buf = [0u8; 80];
    let Some(dn) = nvram::get("mcp.sha256", &mut digest_buf) else {
        console_println("autoload: mcp.sha256 missing");
        return Status::Failed;
    };
    let Some(digest) = parse_digest(&digest_buf[..dn]) else {
        console_println("autoload: mcp.sha256 is not 64 hex digits");
        return Status::Failed;
    };
    finish(run(url, &digest))
}

fn finish(result: Result<i32, &'static str>) -> Status {
    match result {
        Ok(0) => {
            console_println("autoload: module returned");
            Status::Loaded
        }
        Ok(_) => {
            console_println("autoload: module returned");
            console_println("autoload: non-zero status");
            Status::Failed
        }
        Err(err) => {
            console_println("autoload: failed");
            console_println(err);
            Status::Failed
        }
    }
}

const REDIRECT_LIMIT: usize = 4;
const REDIRECT_MAX: usize = 512;

enum FetchResult {
    Loaded(i32),
    Redirect(crate::http::Location),
}

fn run(url: &str, digest: &Sha256) -> Result<i32, &'static str> {
    if url.is_empty() || url.len() > REDIRECT_MAX {
        return Err("autoload: URL too long");
    }
    let mut current = [0u8; REDIRECT_MAX];
    current[..url.len()].copy_from_slice(url.as_bytes());
    let mut current_len = url.len();

    for _ in 0..=REDIRECT_LIMIT {
        let Ok(url) = core::str::from_utf8(&current[..current_len]) else {
            return Err("autoload: URL is not UTF-8");
        };
        match fetch_once(url, digest)? {
            FetchResult::Loaded(code) => return Ok(code),
            FetchResult::Redirect(location) => {
                let Some(target) = location.as_str() else {
                    return Err("autoload: redirect is not UTF-8");
                };
                if !(target.starts_with("https://") || target.starts_with("http://")) {
                    return Err("autoload: relative redirect");
                }
                if target.len() > REDIRECT_MAX {
                    return Err("autoload: redirect too long");
                }
                current[..target.len()].copy_from_slice(target.as_bytes());
                current_len = target.len();
            }
        }
    }
    Err("autoload: too many redirects")
}

fn fetch_once(url: &str, digest: &Sha256) -> Result<FetchResult, &'static str> {
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

    if matches!(summary.status, 301 | 302 | 303 | 307 | 308) {
        let Some(location) = summary.location else {
            return Err("autoload: redirect without Location");
        };
        return Ok(FetchResult::Redirect(location));
    }
    if summary.status != 200 {
        return Err("autoload: HTTP status was not 200");
    }
    if captured != summary.body_len {
        return Err("autoload: module did not fit");
    }
    if !summary.sha256.eq_digest(digest) {
        return Err("autoload: module hash mismatch");
    }
    // SAFETY: the image fits and its hash matched.
    unsafe { Ok(FetchResult::Loaded(module::call_entry(captured))) }
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
