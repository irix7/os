# platform-api

The safe native contract between an IRIX 7 target shim and the
architecture-independent core. It covers physical pages, DMA, MMU domains,
timers, MMIO, interrupts, NIC/block submission, shared rings, console,
entropy, time and reset.

The Wasm core does not see this Rust trait. A shim exposes a small integer
`host_submit` ABI through `wasm-host`; this trait is the native side of that
boundary. Implementations contain the architecture-specific `unsafe` code and
the trait remains the audit seam.
