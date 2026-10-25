# wasm module

The slot-B Wasm core for M1. It embeds a small Rust guest exporting
`run() -> i32`, instantiates it with the no_std `wasmi` interpreter and runs
the M1 local policy from it.

The guest owns the policy: it chooses the `/etc` namespace layout, stores the
`/etc/motd` blob, resolves the root home, derives the local actor, chooses the
capability rights and validates every allow and deny case. It prints
`wasm: host=1` for the host-ABI probe, then reproducible evidence:

```text
wasm-policy: root=<64 hex> actor=<32 hex> cap=ok home=<32 hex> objects=5 actors=1 queued=1 caps=1
```

The root, actor, home and capability state match the native `m1:` line. The
module prints `wasm: run=42` only when the guest returns success and the policy
is committed.

The guest imports only the `i7host` surface; it has no WASI or native import
and never receives the kernel service table. The host side supplies the clock,
console and these staged policy primitives through `wasm-host::HostHooks`:

| Primitive | Purpose |
|---|---|
| `host_node_id` | This machine's 16-byte node identity |
| `host_policy_begin` / `commit` / `abort` | Staged policy lifecycle |
| `host_ns_mkdir` / `put` / `lookup` / `get` / `root` | Namespace and CAS, including fetch by OID |
| `host_home` | Rendezvous home for an object |
| `host_actor_spawn` / `place` / `queue` | Actor directory and run queue |
| `host_cap_issue` / `check` | Generation-checked capability table |
| `host_policy_stats` | Object/actor/queue/capability counters |

Every mutation is staged. `commit` publishes the stage as one value and
`abort` discards it; the module aborts on a guest trap or a non-42 result, so a
failed policy leaves no partial object, actor, queue entry or capability. The
kernel's native `m1:` policy remains the fallback.

Build the guest from `guest/` and then the flat slot-B image:

```bash
tools/build-wasm-guest.sh
HOST="$(rustc -vV | sed -n 's/^host: //p')"
LLVM_OBJCOPY="$(rustc --print sysroot)/lib/rustlib/$HOST/bin/llvm-objcopy"
CARGO_PROFILE_RELEASE_OPT_LEVEL=z \
CARGO_TARGET_MIPS3_SGI_IRIX7_N32_RUSTFLAGS="-Clink-arg=-Tmodules/wasm/linker.ld" \
  cargo build --release -Z build-std=core,alloc \
    -Z build-std-features=compiler-builtins-mem -Zjson-target-spec \
    --target targets/mips3-sgi-irix7-n32.json -p wasm-module
"$LLVM_OBJCOPY" -O binary \
  target/mips3-sgi-irix7-n32/release/wasm-module wasm.module
```

The image must stay within the 1,572,864-byte slot-B limit. It is linked at
`0x8848_0000` and loaded with `module b ...` or the NVRAM
`wasm.url`/`wasm.sha256` pair, so slot A's MCP module remains live. There is
still no general relocatable module multiplexer.
