# IRIX 7 documentation

IRIX 7 is a clean-room, GPL-3.0, Rust-native operating system for the SGI Indy. Its
design target is to run existing IRIX 6.5 application binaries unchanged and offer new
software a native capability-based ABI.  The current bring-up is much smaller: see
[Current status](#current-status).

> **Explore the roadmap:** [The Tree of IRIX 7](roadmap-tree.md) is an interactive,
> dependency-linked view of M0–M10, from the toolchain roots to the v1.0 canopy.

Start here:

- [Install guide](install.md) — build from source, run in the iris emulator, run tests
- [Project map](map.md) — the destination, decisions and scope.
- [Interactive roadmap tree](roadmap-tree.md) — the milestones as a living, dependency-linked tree.
- [Architecture](spec/architecture.md) — the advanced architecture and the D1–D31 decisions.
- [Plan](plan/README.md) — milestones, GitHub, risks, interfaces, hardware, testing, security,
  release, legal, community and cost.
- [Research](research/README.md) — the evidence base.

The docs are built with mdBook and published to GitHub Pages.

## Current status

The replacement ROM and n32 kernel boot on iris.  The kernel owns the machine,
brings up Seeq networking, DHCP/DNS/NTP, HTTP/HTTPS, the boot-log display, the telnet
console and HTTPS module autoload, then enters a static Rust `std` image from SCSI
target 2.  That image exercises args, time, sleep, a small read/write ramfs, `stat`,
`read_dir`, `chdir` and relative paths through a provisional syscall ABI and a
42-symbol C shim.

The relibc n32 big-endian Linux/QEMU gate has passed (TLS, `setjmp`, pthreads).
A provisional IRIX PAL scaffold builds and links C probes, but it has not been
booted on IRIX 7; the kernel error channel, TLS and `stat` ABI remain open.
There is no process table, real threads/TLS, `posix_spawn`, signal delivery, termios
or socket API yet.  See [`install.md`](install.md) and
[`research/std-hello-bringup.md`](research/std-hello-bringup.md).
