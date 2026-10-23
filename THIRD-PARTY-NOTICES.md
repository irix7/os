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

## Kernel runtime (current)

| Component | Licence | Use |
|---|---|---|
| `smoltcp` | 0BSD | TCP/IP stack, DHCP, DNS, ICMP |
| `embedded-tls` | Apache-2.0 | TLS 1.3 client |
| `embedded-io` | MIT OR Apache-2.0 | Blocking I/O traits for the TLS adapter |
| `rand_core` | MIT OR Apache-2.0 | RNG traits for the TLS provider |
| `p384` | Apache-2.0 OR MIT | ECDSA P-384 verification |
| `x509-cert` | Apache-2.0 OR MIT | Certificate subject/issuer parsing for chain trimming |
| `der` | Apache-2.0 OR MIT | DER decoding |
| `heapless` | MIT OR Apache-2.0 | Fixed-capacity buffers in the sockets and TLS |

The embedded CA root (`kernel/src/ca/sslcom-ecc-root.der`) is public certificate data from
the Mozilla CA bundle, not a separately licensed work; see `kernel/src/ca/README.md`.

## Build tools

| Tool | Licence | Use |
|---|---|---|
| rustc / Cargo | MIT OR Apache-2.0 | Rust toolchain |
| GCC / libgccjit | GPL-3.0-or-later | `rustc_codegen_gcc` |
| binutils | GPL-3.0-or-later | Linker and assembler |
| Nix | LGPL-2.1 | Build environment |
| mdBook | MPL-2.0 | Documentation site |

## Website fonts

The generated roadmap site bundles three typefaces under the SIL Open Font License 1.1.
Their licence texts are in `tree/assets/fonts/`.

| Typeface | Licence | Use |
|---|---|---|
| Fraunces | SIL OFL 1.1 | Display and panel headings |
| Space Grotesk | SIL OFL 1.1 | Interface and node labels |
| IBM Plex Mono | SIL OFL 1.1 | Metadata and code |

## Notes

- GPL-2.0-only code is not incorporated; it is reference-only.
- The EROFS on-disk header (`erofs_fs.h`) is MIT; the rest of the Linux EROFS driver is
  GPL-2.0-only and is not copied.
- Xorg/kdrive are not incorporated; the X11 server is clean-room Rust.
- The Contributor Covenant is CC BY 4.0; see `CODE_OF_CONDUCT.md`.

The authoritative list is generated at release time and attached to the GitHub Release.
