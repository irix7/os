// SPDX-License-Identifier: GPL-3.0-only
//! Host tests for the bootstrap configuration parser.
//!
//! `src/boot_config.rs` fetches over TLS at boot, but the parser and digest
//! handling are pure logic.  The network entry points are stubbed here so the
//! tests can run on the host without kernel hardware.

#![no_std]
#![allow(dead_code)]
#![allow(clippy::wrong_self_convention)]

#[path = "../src/sha256.rs"]
mod sha256;

mod telnet {
    pub fn resolve_host(_: &[u8]) -> Option<u32> {
        None
    }
}

mod tls {
    pub struct Summary {
        pub status: u16,
    }

    pub fn get_to(
        _: u32,
        _: u16,
        _: &str,
        _: &str,
        _: &mut [u8],
    ) -> Result<(Summary, usize), &'static str> {
        Err("host test")
    }
}

fn print_str(_: &str) {}
fn println(_: &str) {}
fn print_hex32(_: u32) {}

#[path = "../src/boot_config.rs"]
mod boot_config;
