## Destination

IRIX 7, a GPL-3.0 Rust-native operating system for the SGI Indy (IP22): a monolithic Rust kernel (n32/MIPS III) reimplementing everything on the IRIX discs clean-room, running existing IRIX 6.5 binaries unchanged through a legacy ABI layer, and offering new Rust programs a native capability/object interface. It carries IRIX's identity — `sproc`, XFS, real-time scheduling, graphics and digital media, hardware inventory, topology awareness — as first-class services. Backward-only, no-regressions; forward compatibility is out.

## Notes

- This is the final phase of the project. Its work starts only after the earlier phases clear, in order:
  1. Collate a complete IRIX source repo: the leaked sources plus decompilation of the 6.5.7m binaries to fill gaps (Ghidra; map #1's recovery work).
  2. Stock rebuild (map #1): 6.5.7m built with GCC 16.2, cross-compiled and booted/run on the guest.
  3. Upgrade the source repo to 6.5.22: decompile the 6.5.22 overlay contents and apply the findings to the 6.5.7m base source manually (optionally port back 6.5.30 updates).
  4. SOURCE to AST to RUST: this map.
- Governing decisions: ADR-0008 (compatibility mandate, backward-only), ADR-0009 (clean-room publication), ADR-0010 (Rust monolith, n32/MIPS III), ADR-0011 (native capability substrate + compat container), ADR-0012 (GPL-3.0), ADR-0013 (full reimplementation, factory PROM), ADR-0014 (harness-enforced clean-room, hybrid transcription), ADR-0015 (layered spec, similarity-screen verification), ADR-0016 (o32 ECOFF bootstrap into the n32 kernel), ADR-0017 (modern equivalents as references), ADR-0018 (GCC codegen backend primary, LLVM parallel).
- The legacy layer order is syscall ABI -> loader -> libc; the tracer bullet is a freestanding Rust kernel reached through an o32 ECOFF bootstrap.
- Transcription is orchestrated by `irix7/harness`; only clean-room Rust reaches the public `irix7/os` repo.
- Skills: grilling + domain-modeling for decision tickets; research for fact-finding tickets.

## Decisions so far

- [IRIX 7: Rust target for MIPS III big-endian ELF32](https://github.com/irix7/project/issues/41): stock rustc + custom target + `-Z build-std` is feasible; n32/MIPS III and o32/MIPS II build, "MIPS III + o32" does not.
- [IRIX 7: kernel architecture](https://github.com/irix7/project/issues/42): monolithic Rust kernel, n32/MIPS III, deep crates with language-level isolation.
- [IRIX 7: tracer bullet](https://github.com/irix7/project/issues/43): a freestanding `no_std` Rust kernel that boots via the PROM and prints a marker.
- [IRIX 7: SOURCE to AST to RUST pipeline and the clean-room spec](https://github.com/irix7/project/issues/44): hybrid transcription, clean-room enforced by the harness (structural split + verification).

## Not yet specified

- Native syscall dispatch encoding (how native calls differ from C syscalls).
- The reimplemented libc's ABI surfaces (o32 + n32 C ABIs) and their layout.
- The catalogue of IRIX components with a modern open-source equivalent to use as a reference (ADR-0017).

## Out of scope

- Forward compatibility: native IRIX 7 binaries running on IRIX 6.5 or 5.3 (ADR-0008).
- Publishing any SGI-derived expression to `irix7/os` (ADR-0009, ADR-0014).
- Reproducing the stock IRIX 6.5.7m build — that is map #1.
- The 6.5.22 decompilation and upgrade — a precursor effort, not this map.
- STREAMS as a first-class native interface (legacy-compat only).
