# protocol-bench — host benchmark for the IRIX 7 wire format

A standalone host tool (like `tools/m2-rig`, it opts out of the kernel workspace) that
measures the **shipped** record framing from `mesh-transport` and `sys-primitives` and
reproduces the transport paths around them.

It answers "how many bytes and how much latency does one exchange cost, and which layer
spends it?"  It does not run a guest kernel and it makes no production-format changes.

- Design note and results: [`../../docs/research/wire-efficiency.md`](../../docs/research/wire-efficiency.md)

## Commands

```sh
cd tools/protocol-bench
cargo run --release --offline -- frames     # exact wire bytes per message
cargo run --release --offline -- codec      # encode/decode cost and throughput
cargo run --release --offline -- net        # loopback latency: blocking, tick, relay, hub
cargo run --release --offline -- http       # MCP HTTP/1.0 close vs keep-alive
cargo run --release --offline -- calibrate  # raw memcpy cost, for sanity
cargo run --release --offline -- all
```

`cargo test --release --offline` runs `tests/wire_sizes.rs`, which pins every byte count
quoted in the design note. A failure there is a wire-format change and must update the
document (and its ADR) in the same commit.

## What each section measures

| Section | Source of truth | Notes |
|---|---|---|
| `frames` | `mesh-transport`, `mesh-join`, `mesh-route`, `mesh-migrate` encoders | Exact bytes, not estimates. |
| `codec` | `encode_record` / `decode_record` | Median of five timed batches; each batch cycles 64 payloads so the optimiser cannot elide a copy. |
| `net` | real TCP sockets on loopback | `tick10` reproduces `kernel/src/idle.rs` (10 ms) and `modules/mcp/src/mesh.rs` (`READ_CHUNK = 1024`, `READS_PER_TICK = 8`). `hub` reproduces `tools/m2-rig/src/hub.rs` (128-byte chunks, 2 ms sleeps). |
| `http` | a server shaped like `modules/mcp/src/main.rs` | `Connection: close`, one request per connection, one listening slot, and the `Arm -> WaitRequest -> WaitClose -> Closing -> Arm` phase machine at 10 ms. |

## Limits

- Host wall-clock numbers only. The guest is an emulated 50 MHz-class R4000 and is not
  measured here; the guest numbers are strictly larger.
- The `net` and `http` tick rows model the kernel cadence and the module's loop shape.
  They are not a guest trace.
- The benchmark is not a CI gate; `tests/wire_sizes.rs` is.
