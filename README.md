# IRIX 7

A Rust-native operating system for the SGI Indy (IP22, MIPS III), reimplementing
everything on the IRIX discs clean-room in Rust. It runs existing IRIX 6.5
application binaries unchanged and offers new Rust programs a native
capability/object interface.

- Monolithic Rust kernel, n32/MIPS III, accepting both o32 and n32 user binaries.
- Legacy ABI layer: ELF, the o32 and n32 ABIs (MIPS II/III), and the SGI libc
  and syscall ABI, built as a compat container over the kernel's object model.
- Native ABI: a capability/object model (handles + rights, jobs, channels,
  VMAR/VMO, ports), distinct from the C ABI.
- IRIX's identity carried forward as first-class services: `sproc`, XFS,
  real-time scheduling, graphics and digital media, hardware inventory, topology
  awareness.
- Backward-only compatibility; forward compatibility is out.

Design decisions live in `irix7/project`: ADR-0008 (compatibility mandate),
ADR-0009/0014 (clean-room), ADR-0010 (kernel), ADR-0011 (native interface),
ADR-0012 (licence), ADR-0013 (scope).

## Status

Empty. Transcribing IRIX into this Rust-native system is planned work,
orchestrated by `irix7/harness`, following the stock rebuild and the
modernised fork in `irix7/project`.

## Licence

GPL-3.0.
