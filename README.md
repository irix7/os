# IRIX 7

A Rust-native operating system for the SGI Indy, reached by transcribing the
IRIX 6.5.7m source tree and reimplementing it in Rust.

IRIX 7 inherits the IRIX 6.5 application compatibility mandate (SGI document
007-4405-001) as its own contract, with forward compatibility dropped. See
ADR-0008 in `irix7/project`.

- A legacy ABI layer runs existing conforming IRIX 6.5 binaries unchanged:
  ELF, the o32 and n32 ABIs, the MIPS III ISA, and the SGI libc and syscall ABI.
- New native Rust interfaces carry the same no-regressions, backward-only
  discipline.
- Native IRIX 7 binaries are never expected to run on IRIX 6.5, 5.3, or any
  earlier release.

## Status

Empty. Transcribing IRIX into this Rust-native system is planned work,
orchestrated by `irix7/harness`, following the stock rebuild and the
modernised fork in `irix7/project`.
