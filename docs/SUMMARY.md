# Summary

[Introduction](README.md)

# Getting started

- [Project map](introduction/map.md)
- [Glossary](introduction/glossary.md)

# Installation

- [Toolchain setup](installation/toolchain.md)
- [Building and running](installation/install.md)

# Using the system

- [Console](using/console.md)
- [MCP control plane](using/mcp.md)
  - [MCP application endpoints](using/mcp-apps.md)
- [Package index](using/packages.md)

# System administration

- [Security](administration/security.md)
- [Release engineering](administration/release.md)
- [Pre-release compatibility policy](administration/pre-release.md)

# Programming

- [Software development kit](programming/sdk.md)
- [Native capability ABI](programming/native-abi.md)
- [Native libc ABI](programming/native-libc.md)
- [POSIX API baseline](programming/posix.md)
- [Userland service contract](programming/userland-services.md)
- [IRIX ELF loader contract](programming/elf.md)
- [Driver interface](programming/driver-interface.md)
- [Interface register](programming/interfaces.md)

# System internals

- [System architecture](internals/system.md)
- [Hardware and compatibility architecture](internals/architecture.md)
- [Boot protocol](internals/boot-protocol.md)
- [EROFS on-disk profile](internals/erofs.md)
- [Overlay contract](internals/overlay.md)

# Hardware

- [SGI Indy](hardware/indy.md)
- [x86_64 machine](hardware/x86.md)
  - [x86 module load slice](hardware/x86-module-slice.md)
- [PowerBook G3 development shell](hardware/powerbook-devshell.md)
  - [PowerPC module load](hardware/powerpc-module-slice.md)

# Project

- [Plan index](project/README.md)
- [Milestones](project/milestones.md)
- [Roadmap tree](project/roadmap-tree.md)
- [The Logical Computer (film)](project/documentary.md)
- [M2 two-node rig](project/m2-rig.md)
- [M3 first shard slice](project/m3-rig.md)
- [x86 rig backend](project/x86-rig.md)
- [Hardware lab](project/hardware-lab.md)
- [Testing](project/testing.md)
- [GitHub operating model](project/github.md)
- [Risk register](project/risks.md)
- [Legal and provenance](project/legal.md)
- [Community and governance](project/community.md)
- [Cost model](project/cost.md)

# Appendices

- [Architecture decision records](adr/README.md)
  - [M0: ADR-0001 — LLVM is the only Rust code generation backend](adr/m0-0001-llvm-only.md)
  - [M1: ADR-0002: The ROM downloads the kernel over the network](adr/m1-0002-network-kernel-download.md)
  - [M1: ADR-0003: One kernel image, and its memory map](adr/m1-0003-kernel-only-memory-map.md)
  - [M2: ADR-0001 — Two independent iris instances and a Rust host relay](adr/m2-0001-two-node-rig.md)
  - [M2: ADR-0002 — TCP transport and MeshEnvelope framing](adr/m2-0002-transport.md)
  - [M2: ADR-0003 — 1-of-1 authority root for the M2 membership epoch](adr/m2-0003-authority.md)
  - [M2: ADR-0004 — Native stackless actors for the M2 runtime](adr/m2-0004-actor-runtime.md)
  - [M2: ADR-0005 — Join handshake and authority-confirmed membership epoch](adr/m2-0005-join-epoch.md)
  - [M2: ADR-0006 — Rendezvous homes from the confirmed membership snapshot](adr/m2-0006-homes.md)
  - [M2: ADR-0007 — Remote capability route and the one-MOVED retry](adr/m2-0007-route.md)
  - [M2: ADR-0008 — Failure detection, lease fencing and the partition rule](adr/m2-0008-failure-fence.md)
  - [M2: ADR-0009 — L0 stackless-actor migration at a message boundary](adr/m2-0009-actor-migration.md)
  - [M2: ADR-0010 — direct peer TCP and the lower-NodeId connect rule](adr/m2-0010-direct-peer-transport.md)
  - [M2: ADR-0011 — one prompt over the M2 object and actor surfaces](adr/m2-0011-one-computer-console.md)
  - [M3: ADR-0001 — First sharded home slice, with a two-kernel real limit](adr/m3-0001-shard-slice.md)
  - [M3: ADR-0002 — A bounded multi-member hub and a real eight-kernel convergence path](adr/m3-0002-multi-node-hub.md)
  - [M3: ADR-0003 — Version 1 bounded mesh streams](adr/m3-0003-multiplexed-streams.md)
  - [M3: ADR-0004 — readiness-driven module I/O with a 10 ms fallback](adr/m3-0004-event-driven-io.md)
  - [M3: ADR-0005 — eight module TCP slots for a multi-peer transport](adr/m3-0005-multi-connection-transport.md)
  - [M3: ADR-0006 — module UDP services and the two-kernel datagram gate](adr/m3-0006-udp-transport.md)
- [Research reports](research/README.md)
  - [ATI Rage LT Pro (M4753) graphics driver](research/ati-rage-lt.md)
  - [Larger buffers, batching and a bounded bulk path (#247)](research/bulk-batching.md)
  - [First C probe on the native syscall gate](research/c-probe-bringup.md)
  - [Compressed RAM (CRAM) and IRIX 7](research/compressed-ram.md)
  - [Direct peer transport for the mesh (#245)](research/direct-peer.md)
  - [Forking EROFS to big-endian for IRIX 7 (SGI Indy, IP24)](research/erofs-big-endian-research.md)
  - [SGI Indy (IP24 / “Guinness”) hardware reference](research/indy-hardware.md)
  - [M2 completion audit — issue #3](research/m2-completion-audit.md)
  - [M2 remediation gate — audit #242 and issue #3](research/m2-remediation.md)
  - [Bounded mesh streams (#246, #337)](research/mesh-streams.md)
  - [Native window system for IRIX 7 (SGI Indy, IP24)](research/native-window-system-research.md)
  - [Plan 9, Rust, and IRIX 7](research/plan9-rust-research.md)
  - [PowerBook G3 (M4753) as the third machine type](research/powerbook-g3.md)
  - [Booting a PowerPC kernel on QEMU's g3beige](research/powerbook-qemu-boot.md)
  - [QUIC as the primary IRIX 7 transport](research/quic-primary.md)
  - [relibc n32 bring-up spike](research/relibc-n32-bringup.md)
  - [relibc PAL on the native n32 kernel](research/relibc-pal-bringup.md)
  - [pthreads on the cooperative thread gate](research/relibc-pthread-bringup.md)
  - [Parked sleeps on the cooperative thread gate](research/relibc-sleep-bringup.md)
  - [IRIX 7 — Storage and Filesystem Architecture](research/rust-fs-research.md)
  - [IRIX 7 — Architectural Design Decision Research Report](research/rust-os-design-research.md)
  - [Rust repository and crate sourcing for IRIX 7](research/rust-repository-sourcing.md)
  - [Rust std build spike for native IRIX 7](research/rust-std-build-spike.md)
  - [Rust std for IRIX 7](research/rust-std-irix7.md)
  - [X11 in Rust for IRIX 7 (IP24 Indy)](research/rust-x11-research.md)
  - [Seeq receive interrupt and IP2 dispatch](research/rx-interrupt.md)
  - [First Rust std image on IRIX 7](research/std-hello-bringup.md)
  - [Wire efficiency: IRIX 7 custom protocols, transport paths and a v2 design space](research/wire-efficiency.md)
  - [Shared kernel core: module loader and service ABI](research/x86-shared-core.md)
- [Closed-issue audit](audit/README.md)
  - [Closed-issue audit — boot, kernel and core](audit/closed-core.md)
  - [Closed-issue audit: mesh, network and modules](audit/closed-mesh.md)
  - [Closed platform and userland issues — foundations audit](audit/closed-platform.md)
- [Engineering log](log/2026-10-21.md)
