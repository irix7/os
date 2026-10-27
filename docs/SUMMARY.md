# Summary

- [Introduction](README.md)
- [Install guide](install.md)
- [Project map](map.md)

## Architecture

- [System architecture](spec/system.md)
- [Hardware and compatibility architecture](spec/architecture.md)
- [Boot protocol](spec/boot-protocol.md)
- [IRIX ELF loader contract](spec/elf.md)
- [Loadable kernel modules](spec/modules.md)
- [Package index](spec/packages.md)
- [MCP control plane](spec/mcp.md)
- [Native POSIX API baseline](spec/posix.md)
- [Native capability ABI](spec/native-abi.md)
- [Native libc ABI](spec/native-libc.md)
- [Driver interface](spec/driver-interface.md)
- [Pre-release compatibility policy](spec/pre-release.md)

## Architecture decisions

- [ADR index](adr/README.md)
- [M2-ADR-0001 — two-node rig](adr/m2-0001-two-node-rig.md)
- [M2-ADR-0002 — transport](adr/m2-0002-transport.md)
- [M2-ADR-0003 — authority](adr/m2-0003-authority.md)
- [M2-ADR-0004 — actor runtime](adr/m2-0004-actor-runtime.md)
- [M2-ADR-0005 — join and epoch](adr/m2-0005-join-epoch.md)
- [M2-ADR-0006 — rendezvous homes](adr/m2-0006-homes.md)
- [M2-ADR-0007 — remote capability route](adr/m2-0007-route.md)
- [M2-ADR-0008 — failure, fencing and partition](adr/m2-0008-failure-fence.md)
- [M2-ADR-0009 — L0 actor migration](adr/m2-0009-actor-migration.md)
- [M3-ADR-0001 — first shard slice](adr/m3-0001-shard-slice.md)
- [M3-ADR-0002 — multi-member hub](adr/m3-0002-multi-node-hub.md)

## Plan

- [Plan index](plan/README.md)
- [Milestones](plan/milestones.md)
- [M2 two-node rig](plan/m2-rig.md)
- [M3 first shard slice](plan/m3-rig.md)
- [Roadmap tree (interactive)](roadmap-tree.md)
- [GitHub operating model](plan/github.md)
- [Risk register](plan/risks.md)
- [Interface specifications](plan/interfaces.md)
- [Hardware lab](plan/hardware-lab.md)
- [Testing](plan/testing.md)
- [Security](plan/security.md)
- [Release engineering](plan/release.md)
- [Legal and provenance](plan/legal.md)
- [Community and governance](plan/community.md)
- [Cost model](plan/cost.md)

## Research

- [Research index](research/README.md)
- [Indy hardware](research/indy-hardware.md)
- [Rust X11](research/rust-x11-research.md)
- [Native window systems](research/native-window-system-research.md)
- [Rust filesystems](research/rust-fs-research.md)
- [Plan 9 in Rust](research/plan9-rust-research.md)
- [Big-endian EROFS](research/erofs-big-endian-research.md)
- [Rust OS design survey](research/rust-os-design-research.md)
- [First Rust std image](research/std-hello-bringup.md)
- [relibc n32 bring-up](research/relibc-n32-bringup.md)
