// SPDX-License-Identifier: GPL-3.0-only
//! Host benchmark for the IRIX 7 mesh wire format and transport paths.
//!
//! The benchmark is a *host* measurement of the real codec and framing code.
//! It does not run a guest kernel: the 10 ms poll cadence, the host relay and
//! the M3 hub writer are reproduced faithfully from their source, and each
//! result says which parts are measured on this host and which are modelled.
//!
//! Subcommands:
//!
//! * `frames` — exact wire bytes for every protocol message, from the real
//!   encoders.
//! * `codec` — encode/decode/queue cost and throughput for the real record
//!   framing.
//! * `net` — loopback TCP round-trip latency for the blocking (event-driven)
//!   path, the 10 ms module-tick path, the two-hop host relay and the M3 hub
//!   chunk writer.
//! * `http` — MCP HTTP/1.0 `Connection: close` versus keep-alive, with and
//!   without the 10 ms tick, for the real response sizes.
//! * `all` — every section in the order above.
//!
//! Every command prints a markdown table so the numbers can be pasted into
//! `docs/research/wire-efficiency.md` without transcription.

mod calibrate;
mod codec;
mod frames;
mod httpbench;
mod netbench;
mod proto;
mod stats;

use std::process::ExitCode;

fn main() -> ExitCode {
    let command = std::env::args().nth(1).unwrap_or_else(|| "all".into());
    match command.as_str() {
        "calibrate" => calibrate::run(),
        "frames" => frames::run(),
        "codec" => codec::run(),
        "net" => netbench::run(),
        "http" => httpbench::run(),
        "all" => {
            frames::run();
            codec::run();
            netbench::run();
            httpbench::run();
        }
        other => {
            eprintln!("unknown command: {other}");
            eprintln!("usage: protocol-bench [frames|codec|net|http|all]");
            return ExitCode::FAILURE;
        }
    }
    ExitCode::SUCCESS
}
