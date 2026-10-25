# wasm-host

Host-side `wasmi` lifecycle for the IRIX 7 Wasm core: engine configuration,
fuel metering, the `i7host` import surface and instance startup.

This is a host prototype, not the kernel host. It proves the runtime and ABI
shape on the workstation before the same surface is moved behind the native
shim on the Indy. It is not a kernel module and does not claim `no_std` yet.
