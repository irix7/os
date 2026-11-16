// SPDX-License-Identifier: GPL-3.0-only
//! Netboot an IRIX 7 kernel from the SGI factory PROM.
//!
//! The Indy PROM's `bootp()` device sends a BOOTP request, then fetches the
//! named file over TFTP and jumps to it.  This tool does the two host jobs:
//!
//! * `sgi-netboot image <KERNEL-ELF> <OUT>` converts the kernel ELF to the
//!   big-endian MIPS ECOFF file every Indy PROM revision loads.
//! * `sgi-netboot serve --iface IF --server IP --mac MAC --ip IP --dir DIR`
//!   answers BOOTP for one MAC address and serves files from `DIR` over TFTP.
//!
//! The server needs root for ports 67 and 69.  It answers plain BOOTP only:
//! a request carrying a DHCP message type is left to the LAN's DHCP server,
//! so the IRIX 7 kernel's own DHCP is not answered from here.

mod ecoff;
mod serve;

use std::env;
use std::process::ExitCode;

fn usage() -> ExitCode {
    eprintln!(
        "usage:\n  sgi-netboot image <KERNEL-ELF> <OUT>\n  sgi-netboot serve --iface IF --server IP --mac MAC --ip IP --dir DIR"
    );
    ExitCode::from(2)
}

fn main() -> ExitCode {
    let args: Vec<String> = env::args().skip(1).collect();
    let result = match args.first().map(String::as_str) {
        Some("image") if args.len() == 3 => ecoff::convert(&args[1], &args[2]),
        Some("serve") => match serve::Config::parse(&args[1..]) {
            Ok(config) => serve::run(&config),
            Err(error) => {
                eprintln!("sgi-netboot: {error}");
                return usage();
            }
        },
        _ => return usage(),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("sgi-netboot: {error}");
            ExitCode::FAILURE
        }
    }
}
