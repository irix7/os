# logical module

The M1 single-node policy probe. It runs outside the compressed PROM and links
the real architecture crates: `mesh-service`/`mesh-core` for the local
membership view, `cas-store`/`cas-core` for the namespace, `agnostic-core` for
the actor directory and run queue, and `cap-engine` for the 1-of-1 local
capability.

On iris it prints:

```text
logical: root=<oid> motd=<oid> actor=<id> objects=5 actors=1 queued=1 cap=ok home=<node>
```

and then three `logical: tick` lines. It constructs no fabric frame and does
not touch the kernel's PEP, drivers or POSIX path.

## Build

```bash
export CARGO_TARGET_MIPS3_SGI_IRIX7_N32_RUSTFLAGS="-Clink-arg=-Tmodules/logical/linker.ld"
cargo build --release -Z build-std=core,alloc \
  -Z build-std-features=compiler-builtins-mem -Zjson-target-spec \
  --target targets/mips3-sgi-irix7-n32.json -p logical-module

$(rustc --print sysroot)/lib/rustlib/$(rustc --print host)/bin/llvm-objcopy \
  -O binary target/mips3-sgi-irix7-n32/release/logical-module logical.module
```

Load it through the telnet console with `module <url> <sha256>`, or point the
DS1386 `mcp.url`/`mcp.sha256` environment at it for a local autoload test. The
image is under 200 KiB; the module area limit is 2 MiB.
