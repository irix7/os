# IRIX 7

A Rust-native operating system for the SGI Indy (IP22, MIPS III). The design
target is to reimplement everything on the IRIX discs clean-room in Rust: run
existing IRIX 6.5 application binaries unchanged and offer new Rust programs a
native capability/object interface.

> **[The Tree of IRIX 7](https://irix7.github.io/os/tree/)** — the living roadmap,
> from the toolchain roots to the v1.0 canopy.

Design targets:

- Monolithic Rust kernel, n32/MIPS III, accepting both o32 and n32 user binaries.
- Legacy ABI layer: ELF, the o32 and n32 ABIs (MIPS II/III), and the SGI libc
  and syscall ABI, built as a compat container over the kernel's object model.
- Native ABI: a capability/object model (handles + rights, jobs, channels,
  VMAR/VMO, ports), distinct from the C ABI.
- IRIX's identity carried forward as first-class services: `sproc`, XFS,
  real-time scheduling, graphics and digital media, hardware inventory, topology
  awareness.
- Backward-only compatibility; forward compatibility is out.

The advanced architecture is in [`docs/spec/architecture.md`](docs/spec/architecture.md), with
the evidence base in [`docs/research/`](docs/research/). It frames IRIX 7 as the successor SGI
might have built: a framekernel with hybrid SAS/MMU isolation, de-privileged cells, userspace
drivers, a network-first boot (with an R2/EROFS root still planned), an in-OS MCP control plane, and a new IRIX-inspired desktop.

The plan is in [`docs/plan/`](docs/plan/): milestones, GitHub, risks, interfaces, hardware lab,
testing, security, release, legal, community and cost. Live tracking is the
[IRIX 7 — Roadmap](https://github.com/orgs/irix7/projects/2); the documentation site is
[irix7.github.io/os](https://irix7.github.io/os/), and the roadmap grows as
[The Tree of IRIX 7](https://irix7.github.io/os/tree/).

Design decisions live in `irix7/project` (ADR-0001–0020); the advanced architecture decisions
D1–D31 are in `docs/spec/architecture.md` §2.2.

## Status

Bootable on iris. A MIPS III assembly PROM replacement sizes and tests DRAM, serial-boots,
decompresses the n32 kernel into RAM and jumps to it. The kernel owns the machine: Seeq 8003
networking, DHCP/DNS, NTP, ICMP, HTTP and HTTPS (TLS 1.3 with certificate verification), a
telnet console on port 23, a wall clock seeded from the DS1386 RTC, a CP0-timer idle path, and a loader for modules fetched over
HTTPS into RAM. The REX3 paints the boot log full-screen, then the kernel enters the native n32 user image on SCSI target 2; diagnostics are available from the telnet console. A CPU shadow of the REX3 output is streamed as a BMP and as an inline PNG thumbnail by
the MCP module. A loadable
module serves an MCP control plane (JSON-RPC `initialize`, `ping`, `tools/list`,
`tools/call`) at `POST /mcp`, with `status`, `echo`, `console`, `serial.read`, `system.reset` and `system.poweroff`
tools.

A native n32 ELF loader (`userelf`) maps a user image from SCSI target 2 into kuseg
and enters it with an argc/argv/envp stack.  A static Rust `std` image now runs
there: it reads arguments, time, a small ramfs (`read`, `write`, `stat`,
`read_dir`), `chdir`/relative paths and sleeps; the kernel has a provisional fd
table and the matching syscalls.  The image is single-threaded and its filesystem
is only the tiny ramfs: there is no process table, threads/TLS, termios, signals or
sockets yet.  `user/libc` and `crates/irix7-libc` are bring-up C ABI shims.

The production POSIX layer will be a fork of
[`relibc`](https://github.com/irix7/relibc); its n32 big-endian Linux/QEMU gate has
passed, including TLS, `setjmp` and pthreads.  A provisional IRIX PAL scaffold
(`irix7-pal`) now builds and links C probes, but it has not been booted on IRIX 7
and the kernel's error channel, TLS and `stat` ABI still need work.  This is a
bring-up path for the native process model, not an IRIX 6.5 compatibility
container.

The next milestones are in [`docs/plan/milestones.md`](docs/plan/milestones.md); the live
tree is at [irix7.github.io/os/tree](https://irix7.github.io/os/tree/).

Module developers can use the separate
[IRIX 7 module SDK](https://github.com/irix7/os-sdk): the ABI context and entry macros,
a copyable template, the n32 target and linker, and a build tool that produces a flat,
hashed module image. The SDK owns the `module-abi` crate; this repository consumes it at tag `v0.3.0`.

## Licence

GPL-3.0.
