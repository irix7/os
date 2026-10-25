# wasm module

A first on-target `wasmi` probe. It embeds a 36-byte Wasm module exporting
`run() -> i32`, compiles and instantiates it with the no_std interpreter, runs
it and prints `wasm: run=42`.

It is linked for module slot B (`0x8848_0000`) and is loaded with
`module b ...`, so slot A's MCP module remains live. It does not yet run the
IRIX 7 policy core, use the PlatformShim host functions, or provide a general
relocatable module multiplexer.
