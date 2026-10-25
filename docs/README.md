# IRIX 7 documentation

IRIX 7 is a clean-room, GPL-3.0, Rust-native operating system that runs
existing IRIX 6.5 application binaries through a legacy ABI layer. It inherits
the IRIX 6.5 compatibility mandate without forward compatibility. The first
hardware target is the SGI Indy; x86_64 PCs and KVM/VirtIO cloud machines
follow. The architecture target is **one logical computer built from 1–128
kernel replicas**: a native shim per node and the same Wasm core on every node,
with a global object/capability namespace and an asynchronous fabric.

The current bring-up is smaller: the native Indy ROM/kernel and a static Rust
`std` image run, while the Wasm/mesh crates are host-tested and the kernel has
not yet joined a mesh. See [Current status](#current-status).

> **Explore the roadmap:** [The Tree of IRIX 7](roadmap-tree.md) is an
> interactive view of the native track. The active sequencing is in
> [`plan/milestones.md`](plan/milestones.md).

Start here:

- [Install guide](install.md) — build from source, run in the iris emulator,
  run tests.
- [System architecture](spec/system.md) — the 1–128-node logical computer.
- [Milestone plan](plan/milestones.md) — phases, M1–M4 gates, compatibility and rollback.
- [Project map](map.md) — the destination, decisions and scope.
- [Architecture](spec/architecture.md) — the native and compatibility layers.
- [Plan](plan/README.md) — milestones, GitHub, risks, interfaces, hardware,
  testing, security, release, legal, community and cost.
- [Research](research/README.md) — the evidence base.

The docs are built with mdBook and published to GitHub Pages.

## Current status

The replacement ROM and n32 kernel boot on iris. The kernel owns the machine,
brings up Seeq networking, DHCP/DNS/NTP, HTTP/HTTPS, the boot-log display, the
telnet console and HTTPS module autoload, then enters a static Rust `std` image
from SCSI target 2. That image exercises args, time, sleep, a small read/write
ramfs, `stat`, `read_dir`, `chdir` and relative paths through a provisional
syscall ABI and a 42-symbol C shim.

The architecture foundation is merged: `sys-primitives`, `mesh-core`, `mesh-service`,
`fabric`, `guest-abi`, `cas-core`, `cas-store`, `cap-engine`, `agnostic-core`,
`mesh-sim` and a host-only `wasm-host` prototype. The kernel now initialises
the one-node membership view; it does not yet run the Wasm core or join a mesh.

The relibc n32 big-endian Linux/QEMU gate has passed (TLS, `setjmp`, pthreads).
A provisional IRIX PAL scaffold builds and links C probes but has not been
booted on IRIX 7; the kernel error channel, TLS and `stat` ABI remain open.
There is no process table, real threads/TLS, `posix_spawn`, signal delivery,
termios or socket API yet. See [`install.md`](install.md),
[`plan/milestones.md`](plan/milestones.md) and
[`research/std-hello-bringup.md`](research/std-hello-bringup.md).
