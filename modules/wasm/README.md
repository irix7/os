# wasm module

A first on-target `wasmi` probe. It embeds a 364-byte Wasm module exporting
`run() -> i32`, compiles and instantiates it with the no_std interpreter, runs
it and prints `wasm: host=1` followed by `wasm: run=42`.

The guest imports the `i7host` surface: `host_version`, `host_now`,
`host_submit`, `host_poll`, `host_cancel`, `host_cap_check`,
`host_console_write` and `host_yield`. The module uses the no_std
`wasm-host` crate, not a private copy of the ABI, and supplies its native
`HostHooks` for the clock and console. The guest also checks that its
submission produces a completion before returning `42`. This proves the call
surface and completion path, not the policy implementation.

Regenerate the guest after editing `src/guest.wat`:

```bash
wat2wasm src/guest.wat -o src/guest.wasm
```

It is linked for module slot B (`0x8848_0000`) and is loaded with
`module b ...`, so slot A's MCP module remains live. It does not yet run the
IRIX 7 policy core or provide a general relocatable module multiplexer.
