# fabric

Host-testable state for the IRIX 7 asynchronous fabric: bounded tag
allocation, stream lifecycle and a completion buffer.

This crate is policy, not transport. It has no hardware dependency and uses
`alloc` so it can be tested on the host and compiled for `wasm32`. The kernel
and mesh service provide the ring, network and scheduling implementation.

The final hot-path completion queue is an SPSC ring; `CompletionBuffer` is a
bounded, safe reference model for tests and bring-up.

## Conformance tests

`cargo test -p fabric` runs the host conformance suite:

* `tests/conformance.rs` covers cancellation-versus-completion interleaving,
  completion backpressure, stale completions after tag reuse, and a 10,000-tag
  in-flight table.
* `tests/flow_control.rs` drives the real `mesh-transport` send queue and stream
  decoder against a slow in-process consumer, including send-queue exhaustion,
  stream-window exhaustion and recovery after a window update.

The 10,000-tag figure is the fabric model's capacity. The live MCP module has an
8-entry pending table (`PENDING_CAPACITY` in `modules/mcp/src/mesh.rs`) and is
not claimed to hold 10,000 outstanding requests.
