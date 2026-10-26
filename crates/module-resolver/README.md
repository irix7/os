# module-resolver

Host-testable manifest, package-index and runtime-transaction core for the
runtime module manager tracked in
[#217](https://github.com/irix7/os/issues/217).

The crate does not call the kernel loader itself. It defines:

- `Manifest` / `Record`: name, exact version, ABI, exact dependencies with
  SHA-256 digests and export declarations;
- `Registry`: a set of verified records;
- `resolve`: deterministic dependency order, with fail-closed handling for
  missing records, digest mismatches, ABI mismatches and cycles;
- `Manifest::encode` / `Manifest::decode`: the version 2 little-endian manifest
  record (`I7MM`), with entry/export offsets, a 64 KiB bound, v1 compatibility
  and strict truncation/UTF-8 checks;
- `footer::wrap` / `footer::parse`: an optional `I7MF` footer that locates the
  manifest record after a flat image, while leaving standalone images valid;
- `manager::ModuleManager`: host model of atomic load, SHA-256 image
  verification, reference counting and unload;
- `index::parse_record` and `index::PackageIndex::parse`: package-index v2
  records with exact `depends=`, `abi=`, `region=`, `exports=` and `sig=`
  fields, bounded to 64 KiB and 256 records;
- `runtime::Plan` / `runtime::run`: the manager transaction over a
  `RuntimeBackend`. It verifies every release signature, loads every image
  before entering any of them, parses each manifest footer, checks the entry
  offset and registered exports, executes dependencies before the root, and
  unloads the whole set on any failure. Stale handles, digest mismatches,
  oversized images, missing dependencies, cycles, ABI/region limits and export
  mismatches are host tests.

```rust
let index = module_resolver::index::PackageIndex::parse(bytes)?;
let plan = module_resolver::runtime::Plan::from_index(&index, "demo-root", "1.0.0", host)?;
let report = module_resolver::runtime::run(&plan, &policy, &mut backend)?;
```

The root module is last in `Plan::nodes`, so each dependency is loaded and
entered before its dependent. Modules are linked for one arena region; the
index carries that region because the loader does not relocate images.
