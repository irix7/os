# IRIX 7 packages

A small, HACS-style community index for loadable modules and userland images.
The index is deliberately plain text so the kernel `sash` can fetch and parse it
without a JSON stack, and so a contribution is just a pull request.

`index.conf` is the source of truth.  Each record is:

```text
kind name version url sha256 [depends=...] [abi=...] [region=N] \
  [exports=...] [sig=epoch:128-hex] description...
```

- `kind` is `module` or `userland`.
- `url` is an immutable HTTPS URL (prefer a release asset or a raw commit URL).
- `sha256` covers the exact bytes served at `url`.
- `depends=` is an exact `name@version:sha256` list for the runtime manager.
- `abi=`, `region=`, `exports=` and `sig=` are the v2 runtime fields described
  in `docs/spec/packages.md`.
- `description` is free text to the end of the line.
- Records with the same name are newest first; `sash` loads the first match.

`packages/runtime-modules.toml` is the release graph spec. The release build
runs `tools/module-release` to wrap each image with its manifest v2 footer, sign
it, and emit `dist/runtime/package-index.conf`, its detached signature and
`release-trust.conf`. Those generated records carry the exact `depends=`,
`abi=`, `region=`, `exports=` and `sig=` fields. The spec contains the demo
pair (`demo-dep`, `demo-root`, regions 0 and 1) and the M2 runtime pair
(`m2-mesh`, `m2-root`, regions 2 and 3). Both graphs share the signed index;
`manager.root` selects which one runs. `tools/module-release --root
m2-root@1.0.0` self-checks the M2 plan from the same spec. The checked-in
`packages/release-trust.conf` is the manager's embedded release policy; the
private signing key is not in the repository. An unsigned release is possible
only with an explicit `IRIX7_RELEASE_UNSIGNED=1` build and a policy that says
`require-signatures=0`.

The catalog includes module binaries under `modules/` and userland images under
`userland/`; both are served as immutable raw files for the version recorded in
the index.  A separate `irix7/packages` repository can host the same tree.

The kernel's `sash` fetches this file from:

```text
https://raw.githubusercontent.com/irix7/os/main/packages/index.conf
```

`packages` lists the index.  `packages install NAME` loads a module record, or
writes a verified userland image to SCSI target 2 when that disk is present;
`reboot` then runs it.  `packages run NAME` runs a userland image directly
from RAM without a SCSI disk.  A SCSI disk is never required: if one is present it is an
opportunistic cache, not a dependency.

The index can move to a separate `irix7/packages` repository without changing
the format; only the default URL changes.
