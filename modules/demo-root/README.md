# demo root

The root of the CI runtime graph, linked at arena region 1 (`0x88500000`). Its
manifest declares an exact dependency on `demo-dep@1.0.0` and its index record
carries the same SHA-256.

The manager enters `demo-dep` first, then this image. It resolves
`irix7.demo.service@1` with `module_find_export`, checks the owner handle,
checks the interface ABI and calls the exported function. It prints
`demo-root: dep=0x217 owner=0x...`, which proves both dependency order and the
verified export table.

The ABI v12 `module_find` service is not used: the current kernel loader does
not store `LoadedModule::version_len`, so exact name/version lookup cannot
match. That is recorded in `docs/spec/modules.md`; `module_find_export` is not
affected.
