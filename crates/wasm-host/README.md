# wasm-host

Host-side `wasmi` lifecycle for the IRIX 7 Wasm core: engine configuration,
fuel metering, the `i7host` import surface, the submission/completion queue and
instance startup.

The library is `no_std + alloc`; the tests use a small `wat` dev-dependency
and Rust's test harness. `HostHooks` is the seam to the native shim. It carries the clock, the console and a
`PolicyHooks` table for the M1 local policy:

- `host_node_id`, `host_policy_begin`/`commit`/`abort` and `host_policy_stats`;
- `host_ns_mkdir`/`put`/`lookup`/`get`/`root` for the staged namespace and CAS;
- `host_home` for the rendezvous home of an object;
- `host_actor_spawn`/`place`/`queue` for the actor directory and run queue;
- `host_cap_issue`/`check` for the generation-checked capability table.

The guest owns the policy choice and validation. The host only supplies bulk
state and the operations that are not safe or sensible inside Wasm. The
workstation tests use the inert defaults; `modules/wasm` installs the native
implementation and the kernel service table bridge. This is still a host
prototype, not the final PlatformShim.
