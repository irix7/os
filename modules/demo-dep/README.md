# demo dependency

A small ABI v12 runtime module linked at arena region 0 (`0x884c0000`). It
exports `irix7.demo.service@1` at image offset 136 and returns vtable status 0.

`tools/module-release` records the `DEMO_API` symbol offset in the manifest v2
footer and package-index record. `modules/demo-root` resolves the export through
`module_find_export`, checks the owner handle and calls `DEMO_API.magic`, which
returns `0x217`.
