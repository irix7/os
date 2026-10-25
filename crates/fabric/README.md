# fabric

Host-testable state for the IRIX 7 asynchronous fabric: bounded tag
allocation, stream lifecycle and a completion buffer.

This crate is policy, not transport. It has no hardware dependency and uses
`alloc` so it can be tested on the host and compiled for `wasm32`. The kernel
and mesh service provide the ring, network and scheduling implementation.

The final hot-path completion queue is an SPSC ring; `CompletionBuffer` is a
bounded, safe reference model for tests and bring-up.
