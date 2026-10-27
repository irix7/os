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
| [`rust-std-irix7.md`](rust-std-irix7.md) | What `std` for IRIX 7 means, the native-vs-compat split, the POSIX.1-2024 native libc ABI it requires, and the implementation and PR plan. |
| [`rust-std-build-spike.md`](rust-std-build-spike.md) | The measured spike: build the `std` rlib for the native n32 target from a patched `rust-src` and libc, the exact method and patch set, the result, and the remaining runtime gaps. |
| [`std-hello-bringup.md`](std-hello-bringup.md) | The first linked and running static `std` image: the 42-symbol shim, kernel syscall additions, loader fixes, output and remaining POSIX gaps. |
| [`relibc-n32-bringup.md`](relibc-n32-bringup.md) | The relibc n32/QEMU gate: patched relibc, TLS, setjmp, pthreads, and the provisional (linked, not booted) IRIX PAL scaffold. |
| [`m2-completion-audit.md`](m2-completion-audit.md) | The adversarial audit of M2 closure against issue #3, with the three remaining gaps. |
| [`m2-remediation.md`](m2-remediation.md) | The remediation gate: signed persistent epoch, two-node CI, provenance, engineering review and the remaining limits. |
| [`quic-primary.md`](quic-primary.md) | Whether QUIC should replace TCP as the native transport, the Indy's crypto/code/memory budget, the recommended QUIC profile and the measured spike that decides it. |

The reports cite primary sources; they are kept verbatim (including their own spelling) for
traceability.

- [Bulk buffers and batching](bulk-batching.md) — #247.

- [Direct peer transport](direct-peer.md) — #245.

- [Bounded mesh streams](mesh-streams.md) — #246.
