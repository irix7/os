# sys-primitives

Shared `no_std` vocabulary for the IRIX 7 object fabric: `ObjectId`, `NodeId`,
capability tokens, `MessageFrame`, completion records and the mesh envelope.

This crate has no allocation, no hardware, no cryptography and no unsafe code.
It is the byte-level contract between the native shims, the Wasm core, the
migration codec and the 9P gateway. Every wire field is little-endian; native
protocols are converted at their gateway.

The crate is host-tested and builds for `wasm32-unknown-unknown`. It does not
depend on the existing kernel or module ABI.
