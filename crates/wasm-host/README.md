# wasm-host

Host-side `wasmi` lifecycle for the IRIX 7 Wasm core: engine configuration,
fuel metering, the `i7host` import surface, the submission/completion queue and
instance startup.

The library is `no_std + alloc` unless the `std` feature is enabled for tests.
`HostHooks` is the seam to the native shim: the kernel module supplies clock,
console and capability functions through it, while the workstation tests use
the inert defaults. It remains a host prototype, not the full policy core.
