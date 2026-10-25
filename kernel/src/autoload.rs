// SPDX-License-Identifier: GPL-3.0-only
//! Boot-time module autoload from the repository boot configuration.
//!
//! The MCP control plane is a module, so a machine that reboots must be able
//! to bring it back without a human at the telnet console.  The kernel fetches
//! `boot/config.conf` from the repository, reads `mcp.url` and `mcp.sha256`,
//! then fetches the image over HTTPS, checks the digest, and runs it in place.
//! A local test rig can override the two keys in the DS1386 environment.  A
//! recovery or offline image can instead use `mcp.local`, a target:LBA:sectors
//! record with the same SHA-256 digest; it is read from the local SCSI bus
//! before the network path.

use crate::nvram;
use crate::sha256::Sha256;
use crate::{console_println, console_write, http, module, scsi, tls};

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
/// Local NVRAM `mcp.local` wins when present: it names a SCSI target, LBA and
/// sector count, and is used by the offline/recovery boot.  Otherwise local
/// NVRAM `mcp.url`/`mcp.sha256` wins, because the test rig uses it to point at
/// a temporary HTTP fixture.  Otherwise the kernel reads the repository boot
/// configuration, which the release workflow updates with raw GitHub URLs for
/// the published module.  A normal Indy never needs the NVRAM override; the
/// chip is validated and repaired at startup.
pub fn boot() -> Status {
    let mut local = [0u8; 160];
    if let Some(n) = nvram::get("mcp.local", &mut local) {
        return finish(load_local(&local[..n]));
    }

    // A network-off M1 boot does not have a repository to consult and does
    // not need MCP.  With no local module, report Disabled rather than a
    // failed autoload; the console and local policy remain usable.
    if crate::net::disabled() {
        console_println("autoload: network off; no local module");
        return Status::Disabled;
    }

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

/// Load the optional slot-B wasm probe from `wasm.url` NVRAM.
///
/// M1 uses this to prove the runtime starts during boot without replacing the
/// slot-A MCP module.  It is optional; a missing key is silent.
pub fn boot_wasm() {
    let mut url = [0u8; 192];
    let Some(n) = nvram::get("wasm.url", &mut url) else {
        return;
    };
    if crate::net::disabled() {
        console_println("autoload: network off; wasm module skipped");
        return;
    }
    let Ok(url) = core::str::from_utf8(&url[..n]) else {
        console_println("autoload: wasm.url is not UTF-8");
        return;
    };
    let mut digest_buf = [0u8; 80];
    let Some(dn) = nvram::get("wasm.sha256", &mut digest_buf) else {
        console_println("autoload: wasm.sha256 missing");
        return;
    };
    let Some(digest) = parse_digest(&digest_buf[..dn]) else {
        console_println("autoload: wasm.sha256 is not 64 hex digits");
        return;
    };
    console_println("autoload: loading wasm module");
    let started = crate::idle::uptime_ms();
    let result = run_slot(module::Slot::B, url, &digest);
    let elapsed = crate::idle::uptime_ms().saturating_sub(started);
    console_write("autoload: wasm took ");
    write_dec(elapsed);
    console_println(" ms");
    match result {
        Ok(0) => console_println("autoload: wasm returned"),
        Ok(_) => console_println("autoload: wasm returned non-zero"),
        Err(error) => {
            console_println("autoload: wasm failed");
            console_println(error);
        }
    }
}

fn write_dec(mut value: u64) {
    let mut buf = [0u8; 20];
    let mut len = 0;
    if value == 0 {
        console_write("0");
        return;
    }
    while value > 0 && len < buf.len() {
        buf[len] = b'0' + (value % 10) as u8;
        value /= 10;
        len += 1;
    }
    while len > 0 {
        len -= 1;
        console_write(core::str::from_utf8(&buf[len..len + 1]).unwrap_or("?"));
    }
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

/// Load a module from a local SCSI target and run it in place.
///
/// The NVRAM record is `<target>:<lba>:<sectors>:<sha256>`.  The image is read
/// in 255-sector chunks because the bring-up READ(6) path cannot cross that
/// boundary, then verified before the entry call.
fn load_local(record: &[u8]) -> Result<i32, &'static str> {
    let text = core::str::from_utf8(record).map_err(|_| "autoload: mcp.local is not UTF-8")?;
    let mut fields = text.split(':');
    let target = parse_u32(fields.next().ok_or("autoload: mcp.local target missing")?)?;
    let lba = parse_u32(fields.next().ok_or("autoload: mcp.local LBA missing")?)?;
    let sectors = parse_u32(fields.next().ok_or("autoload: mcp.local sectors missing")?)?;
    let digest = parse_digest(
        fields
            .next()
            .ok_or("autoload: mcp.local digest missing")?
            .as_bytes(),
    )
    .ok_or("autoload: mcp.local digest is not 64 hex digits")?;
    if fields.next().is_some() {
        return Err("autoload: mcp.local has extra fields");
    }
    if target >= 7 {
        return Err("autoload: mcp.local target must be 0..6");
    }
    let len = (sectors as usize)
        .checked_mul(512)
        .ok_or("autoload: mcp.local length overflow")?;
    if len == 0 || len > module::slot_size(module::Slot::A) {
        return Err("autoload: mcp.local does not fit module slot A");
    }
    if lba.checked_add(sectors).is_none() {
        return Err("autoload: mcp.local LBA overflow");
    }

    console_println("autoload: loading local module");
    module::clear(module::Slot::A);
    let area = module::area(module::Slot::A);
    let mut done = 0usize;
    while done < len {
        let count = ((len - done) / 512).min(255);
        let start_lba = lba + (done / 512) as u32;
        scsi::read_blocks(target as u8, start_lba, &mut area[done..done + count * 512])?;
        done += count * 512;
    }
    if !Sha256::digest(&area[..len]).eq_digest(&digest) {
        return Err("autoload: local module hash mismatch");
    }
    // SAFETY: the image fits and its hash matched.
    unsafe { Ok(module::call_entry(module::Slot::A, len)) }
}

fn parse_u32(text: &str) -> Result<u32, &'static str> {
    text.parse::<u32>()
        .map_err(|_| "autoload: mcp.local field is not a decimal integer")
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

/// Load and run a module from an explicit URL and SHA-256 digest.
///
/// Used by the package index; the normal boot path uses [`boot`].
pub fn load_url(url: &str, digest: &Sha256) -> Result<i32, &'static str> {
    run_slot(module::Slot::A, url, digest)
}

fn run(url: &str, digest: &Sha256) -> Result<i32, &'static str> {
    run_slot(module::Slot::A, url, digest)
}

fn run_slot(slot: module::Slot, url: &str, digest: &Sha256) -> Result<i32, &'static str> {
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
        match fetch_once(slot, url, digest)? {
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

fn fetch_once(slot: module::Slot, url: &str, digest: &Sha256) -> Result<FetchResult, &'static str> {
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
    module::clear(slot);
    let area = module::area(slot);
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
    unsafe { Ok(FetchResult::Loaded(module::call_entry(slot, captured))) }
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
