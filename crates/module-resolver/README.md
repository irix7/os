# module-resolver

Host-testable first slice of the runtime module manifest and dependency work
tracked in [#217](https://github.com/irix7/os/issues/217).

This crate does **not** load or execute module images. It defines:

- `Manifest` / `Record`: name, exact version, ABI, exact dependencies with
  SHA-256 digests and export declarations;
- `Registry`: a set of verified records;
- `resolve`: deterministic dependency order, with fail-closed handling for
  missing records, digest mismatches, ABI mismatches and cycles.

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
