# wasm module

A first on-target `wasmi` probe. It embeds a 36-byte Wasm module exporting
`run() -> i32`, compiles and instantiates it with the no_std interpreter, runs
it and prints `wasm: run=42`.

This proves the interpreter can run on n32/iris from a loadable module. It does
not yet run the IRIX 7 policy core, use the PlatformShim host functions, or
solve the one-module-slot problem.
