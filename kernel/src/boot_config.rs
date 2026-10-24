// SPDX-License-Identifier: GPL-3.0-only
//! Boot configuration fetched from the repository.
//!
//! The kernel's hard-coded bootstrap URL is
//! `https://raw.githubusercontent.com/irix7/os/main/boot/config.conf`.  The
//! release workflow updates that file after publishing a release, so the
//! running kernel can discover module URLs and digests without rebuilding the
//! PROM.  NVRAM `mcp.url`/`mcp.sha256` remain a local override for the test
//! rig.

use crate::sha256::Sha256;

const CONFIG_HOST: &str = "raw.githubusercontent.com";
const CONFIG_PATH: &str = "/irix7/os/main/boot/config.conf";
const CONFIG_MAX: usize = 4096;
const URL_MAX: usize = 192;

/// The module and release information from the remote boot configuration.
pub struct Config {
    pub url: [u8; URL_MAX],
    pub url_len: usize,
    pub digest: [u8; 32],
    pub release: [u8; 64],
    pub release_len: usize,
}

impl Config {
    fn new() -> Self {
        Self {
            url: [0; URL_MAX],
            url_len: 0,
            digest: [0; 32],
            release: [0; 64],
            release_len: 0,
        }
    }

    pub fn url_string(&self) -> Option<&str> {
        core::str::from_utf8(&self.url[..self.url_len]).ok()
    }

    pub fn digest(&self) -> Sha256 {
        Sha256(self.digest)
    }
}

/// Fetch and parse the repository boot configuration.
///
/// Returns `None` for a network/TLS/HTTP failure or a config without an
/// `mcp.url` and valid `mcp.sha256`.  Diagnostics are printed to the console.
pub fn fetch() -> Option<Config> {
    let ip = crate::telnet::resolve_host(CONFIG_HOST.as_bytes())?;
    let mut body = [0u8; CONFIG_MAX];
    let (summary, captured) = match crate::tls::get_to(ip, 443, CONFIG_HOST, CONFIG_PATH, &mut body)
    {
        Ok(value) => value,
        Err(error) => {
            crate::print_str("config: fetch failed: ");
            crate::println(error);
            return None;
        }
    };
    if summary.status != 200 {
        crate::print_str("config: HTTP status ");
        crate::print_hex32(summary.status as u32);
        crate::println("");
        return None;
    }
    let Some(config) = parse(&body[..captured]) else {
        crate::println("config: missing or invalid mcp.url/mcp.sha256");
        return None;
    };
    crate::print_str("config: release=");
    if let Ok(release) = core::str::from_utf8(&config.release[..config.release_len]) {
        crate::print_str(release);
    }
    crate::println("");
    Some(config)
}

fn parse(body: &[u8]) -> Option<Config> {
    let mut config = Config::new();
    for raw_line in body.split(|byte| *byte == b'\n') {
        let line = trim(raw_line);
        if line.is_empty() || line[0] == b'#' {
            continue;
        }
        let Some(equals) = line.iter().position(|byte| *byte == b'=') else {
            continue;
        };
        let key = trim(&line[..equals]);
        let value = trim(&line[equals + 1..]);
        match key {
            b"mcp.url" => {
                if value.is_empty() || value.len() > URL_MAX {
                    return None;
                }
                config.url[..value.len()].copy_from_slice(value);
                config.url_len = value.len();
            }
            b"mcp.sha256" => {
                config.digest = parse_hex_digest(value)?;
            }
            b"release" => {
                let len = value.len().min(config.release.len());
                config.release[..len].copy_from_slice(&value[..len]);
                config.release_len = len;
            }
            _ => {}
        }
    }
    if config.url_len == 0 || config.digest == [0; 32] {
        return None;
    }
    Some(config)
}

fn trim(mut value: &[u8]) -> &[u8] {
    while let Some((first, rest)) = value.split_first() {
        if first.is_ascii_whitespace() {
            value = rest;
        } else {
            break;
        }
    }
    while let Some((last, rest)) = value.split_last() {
        if last.is_ascii_whitespace() {
            value = rest;
        } else {
            break;
        }
    }
    value
}

fn parse_hex_digest(value: &[u8]) -> Option<[u8; 32]> {
    if value.len() != 64 {
        return None;
    }
    let mut digest = [0u8; 32];
    for (index, byte) in digest.iter_mut().enumerate() {
        let hi = hex_value(value[index * 2])?;
        let lo = hex_value(value[index * 2 + 1])?;
        *byte = (hi << 4) | lo;
    }
    Some(digest)
}

fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_release_config() {
        let body = b"# generated\nversion=1\nrelease=v0.1.0\nmcp.url=https://raw.githubusercontent.com/irix7/os/main/boot/modules/v0.1.0/mcp.module\nmcp.sha256=0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef\n";
        let config = parse(body).expect("valid config");
        assert_eq!(config.release_len, 6);
        assert_eq!(
            config.url_string(),
            Some("https://raw.githubusercontent.com/irix7/os/main/boot/modules/v0.1.0/mcp.module")
        );
        assert_eq!(config.digest[0], 0x01);
        assert_eq!(config.digest[31], 0xef);
    }

    #[test]
    fn rejects_missing_digest() {
        let body = b"mcp.url=https://example.invalid/mcp.module\n";
        assert!(parse(body).is_none());
    }

    #[test]
    fn ignores_comments_and_unknown_keys() {
        let body = b"# comment\nroot.url=https://example.invalid/root.erofs\nrelease=v1\n";
        assert!(parse(body).is_none());
    }
}
