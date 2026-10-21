# IRIX 7

A Rust-native operating system for the SGI Indy (IP22, MIPS III), reimplementing
everything on the IRIX discs clean-room in Rust. It runs existing IRIX 6.5
application binaries unchanged and offers new Rust programs a native
capability/object interface.

> **[The Tree of IRIX 7](https://irix7.github.io/os/tree/)** — the living roadmap,
> from the toolchain roots to the v1.0 canopy.

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
drivers, an R2-first boot, an in-OS MCP control plane, and a new IRIX-inspired desktop.

The plan is in [`docs/plan/`](docs/plan/): milestones, GitHub, risks, interfaces, hardware lab,
testing, security, release, legal, community and cost. Live tracking is the
[IRIX 7 — Roadmap](https://github.com/orgs/irix7/projects/2); the documentation site is
[irix7.github.io/os](https://irix7.github.io/os/), and the roadmap grows as
[The Tree of IRIX 7](https://irix7.github.io/os/tree/).

Design decisions live in `irix7/project` (ADR-0001–0020); the advanced architecture decisions
D1–D31 are in `docs/spec/architecture.md` §2.2.

## Status

Empty. Transcribing IRIX into this Rust-native system is planned work,
orchestrated by `irix7/harness`, following the stock rebuild and the
modernised fork in `irix7/project`.

## Licence

GPL-3.0.
