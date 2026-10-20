# Third-Party Notices

IRIX 7 is licensed under GPL-3.0. It depends on third-party crates and tools, each of which
retains its own copyright and licence.

This file is generated from `Cargo.lock` by `cargo-about` and `cargo-deny` in CI. Until the
Rust workspace exists, it records the known candidates and their licences.

## Runtime and build dependencies (candidates)

| Component | Licence | Use |
|---|---|---|
| `abi_stable` | MIT OR Apache-2.0 | Stable Rust FFI (candidate) |
| `smoltcp` | 0BSD | TCP/IP stack |
| `rustls` | Apache-2.0 OR MIT OR ISC | TLS |
| `gix` | MIT OR Apache-2.0 | Git object store and smart HTTP |
| `rmcp` | Apache-2.0 | MCP server (if used on the host) |
| `erofs-rs` | MIT OR Apache-2.0 | EROFS reference (not a port) |
| `xfs-core` | Apache-2.0 | XFS reference |
| `lamxfs` | Apache-2.0 | XFS reference |
| `backhand` | MIT OR Apache-2.0 | SquashFS fallback |
| `crc32c` | Apache-2.0 OR MIT | I7FS checksums |

## Build tools

| Tool | Licence | Use |
|---|---|---|
| rustc / Cargo | MIT OR Apache-2.0 | Rust toolchain |
| GCC / libgccjit | GPL-3.0-or-later | `rustc_codegen_gcc` |
| binutils | GPL-3.0-or-later | Linker and assembler |
| Nix | LGPL-2.1 | Build environment |
| mdBook | MPL-2.0 | Documentation site |

## Notes

- GPL-2.0-only code is not incorporated; it is reference-only.
- The EROFS on-disk header (`erofs_fs.h`) is MIT; the rest of the Linux EROFS driver is
  GPL-2.0-only and is not copied.
- Xorg/kdrive are not incorporated; the X11 server is clean-room Rust.
- The Contributor Covenant is CC BY 4.0; see `CODE_OF_CONDUCT.md`.

The authoritative list is generated at release time and attached to the GitHub Release.
