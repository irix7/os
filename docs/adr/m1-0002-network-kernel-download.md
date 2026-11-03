# M1-ADR-0002: The ROM downloads the kernel over the network

**Status:** Accepted, with the risk recorded below
**Supersedes:** the M1 "no network at boot" gate and the AGENTS.md ROM rules
**Date:** 2026-11-03

## Decision

1. The ROM becomes a **Rust downloader**. It brings up the Indy network (Seeq, DHCP, DNS,
   TLS, HTTPS), fetches the kernel image from the release URL `kernel-indy-latest` on
   GitHub, and runs it.
2. The kernel is **not embedded in the PROM**. `rom/build.rs` no longer compresses the
   kernel into `payload.S`. The PROM size budget applies to the downloader only.
3. **No integrity check.** The downloaded kernel is run without a signature or a pinned
   digest. This is the owner's explicit choice for this lab hardware.
4. **Network is required to boot.** Boot needs a working network and GitHub reachable over
   TLS. There is no offline or local-copy fallback.

## Consequences

- **M1 gate rewritten.** "Boot with no network" is replaced by "boot with network; zero
  mesh RPCs at boot". The offline boot evidence in `docs/plan/milestones.md` is withdrawn.
- **AGENTS.md ROM rules replaced.** The ROM is no longer a pure assembly stub. The rule that
  the kernel lives in the PROM is removed. The "one program" description of ROM and kernel
  is replaced by a two-stage boot: a downloader, then a separately loaded kernel.
- **PROM budget superseded.** The scheduler and kernel growth no longer compete for the
  524,288-byte PROM window, so the "shrink the kernel first" plan is cancelled.
- **Failure mode.** If GitHub, DNS or the network is unavailable, the Indy does not boot.
  Recovery needs the network, not a local disk.
- **Trust boundary moves.** Anyone who can publish the `kernel-indy-latest` release, or
  intercept the TLS connection to GitHub, can run code on the Indy at boot.

## Accepted risk

Running an unverified kernel from a moving URL means the boot chain trusts GitHub's release
store and the TLS path to it, with no further check. The owner has accepted this for lab
hardware. It must not be presented as secure boot, and it must not be used on a network the
owner does not control. Revisit this ADR before the Indy is used on any other network, or
before a release is offered to other people.

## Not decided here

- The network stack in the ROM: the implementation language split between assembly and
  Rust for the Seeq driver.

## Size measurement (spike, 2026-11-04)

The network stack that the ROM would carry was measured with a probe that links the
kernel's TLS 1.3 client (`embedded-tls` with `rustpki`, P-384 and RSA), its certificate
verifier and embedded roots, its HTTP and range code, and smoltcp (TCP, UDP, DHCP and DNS).
The probe stubs the kernel services those modules call (clock, socket set, phy device) with
code that runs without a frame, so the protocol code stays live and linked.

| Measure | Value |
|---|---|
| Linked text | 438 KiB |
| Linked read-only data | 9 KiB |
| Flat image (text, rodata, data) | 455 KB |
| LZ4 frame of the flat image (`lz4 -9`) | 266 KB |

The PROM budget is 512 KiB. The stack fits uncompressed with about 60 KiB to spare, and
with room to spare if compressed. The Seeq driver, the ROM stub and the boot code add to
this and are not included.

Not measured: the RAM the stack needs at run time (TLS record buffers, the certificate
buffer, the socket buffers and the stack), which the ROM must provide from its own RAM
before any kernel structures exist. The first real ROM build will measure that.

## Not decided here (continued)

- Whether the kernel download replaces or sits beside the current module autoload.
- Rollback: a bad `latest` kernel has no automatic recovery.

## Validation required before code lands

- A boot trace from iris where the ROM fetches `kernel-indy-latest` over TLS and jumps to it.
- A negative test: the GitHub URL unreachable halts with a clear marker, not a hang.
- The revised M1 gate, with the offline boot removed and the network boot added.
