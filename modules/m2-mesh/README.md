# m2-mesh runtime dependency

The first real M2 runtime dependency: a flat module linked at arena region 2
(`0x88540000`) that links `mesh-transport` and `sys-primitives` and exports
`irix7.mesh.transport@1` at the `MESH_API` symbol.

The table is deliberately small and deterministic:

| Field | Meaning |
|---|---|
| `abi` | table generation, 1 |
| `probe` | encode/decode one fixed M2 Ping envelope, then check the framing bounds |
| `record_len` | encoded probe length (340) |
| `max_record_len` | `mesh_transport::MAX_RECORD_LEN` (65536) |
| `check_bound` | 0 valid, -1 oversize, -2 too short |

`probe` returns `0x4d324d31` only if the record encodes to 340 bytes, decodes
with the same source/destination/body, and `decode_record` rejects both an
oversize and a too-short prefix. Any other value is a fail-closed code.

`tools/module-release` records the exact `MESH_API` image offset in the
manifest v2 footer and package-index export record. The manager loads this
image before `m2-root`; the root resolves the table through
`module_find_export`, not through a build-time link.
