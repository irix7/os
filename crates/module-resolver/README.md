# module-resolver

Host-testable first slice of the runtime module manifest and dependency work
tracked in [#217](https://github.com/irix7/os/issues/217).

This crate does **not** load or execute module images. It defines:

- `Manifest` / `Record`: name, exact version, ABI, exact dependencies with
  SHA-256 digests and export declarations;
- `Registry`: a set of verified records;
- `resolve`: deterministic dependency order, with fail-closed handling for
  missing records, digest mismatches, ABI mismatches and cycles;
- `Manifest::encode` / `Manifest::decode`: the version 1 little-endian manifest
  record (`I7MM`), with a 64 KiB bound and strict truncation/UTF-8 checks;
- `footer::wrap` / `footer::parse`: an optional `I7MF` footer that locates the
  manifest record after a flat image, while leaving standalone images valid;
- `manager::ModuleManager`: host model of atomic load, SHA-256 image
  verification, reference counting and unload. It does not execute an image.

Version 1 resolves exact versions only. Version ranges, a package-index parser,
signature validation, relocation, slot allocation and a module manager are
later slices.

```rust
let order = module_resolver::resolve(&root, host_abi, &registry)?;
for id in order.order {
    // load and verify the exact image, then call it in this order
}
```

The root module is last in `Resolution::order`, so each dependency is loaded
before its dependent.
