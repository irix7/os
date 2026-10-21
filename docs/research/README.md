# Research index

These reports are the evidence base for the advanced architecture. They are research input,
not ADRs; the decisions are in [`../spec/architecture.md`](../spec/architecture.md) §2.2.

| Report | What it answers |
|---|---|
| [`indy-hardware.md`](indy-hardware.md) | The Indy's CPU, caches, TLB, HPC3, SCSI, Seeq, HAL2, Newport, Vino, DS1386, GIO and PROM. |
| [`rust-x11-research.md`](rust-x11-research.md) | The Rust X11 ecosystem, what a minimal IRIX-capable server needs, and the build-vs-port verdict. |
| [`native-window-system-research.md`](native-window-system-research.md) | Every realistic native window-system option, with the bandwidth arithmetic. |
| [`rust-fs-research.md`](rust-fs-research.md) | Rust filesystems, the XFS v4 on-disk format, I7FS and the storage architecture. |
| [`plan9-rust-research.md`](plan9-rust-research.md) | Plan 9's ideas, Rust 9P, 9P over WAN, capabilities, MCP and content-addressed images. |
| [`erofs-big-endian-research.md`](erofs-big-endian-research.md) | Whether to fork EROFS to big-endian (verdict: do not). |
| [`compressed-ram.md`](compressed-ram.md) | What Meta's CRAM actually is and whether compressed RAM fits the Indy (verdict: no). |
| [`rust-os-design-research.md`](rust-os-design-research.md) | A broad survey of Rust OS design choices. |

The reports cite primary sources; they are kept verbatim (including their own spelling) for
traceability.
