# IRIX 7 packages

A small, HACS-style community index for loadable modules and userland images.
The index is deliberately plain text so the kernel `sash` can fetch and parse it
without a JSON stack, and so a contribution is just a pull request.

`index.conf` is the source of truth.  Each record is:

```text
kind name version url sha256 description...
```

- `kind` is `module` or `userland`.
- `url` is an immutable HTTPS URL (prefer a release asset or a raw commit URL).
- `sha256` covers the exact bytes served at `url`.
- `description` is free text to the end of the line.

The catalog includes module binaries under `modules/` and userland images under
`userland/`; both are served as immutable raw files for the version recorded in
the index.  A separate `irix7/packages` repository can host the same tree.

The kernel's `sash` fetches this file from:

```text
https://raw.githubusercontent.com/irix7/os/main/packages/index.conf
```

`packages` lists the index.  `packages install NAME` currently loads a module
record; userland records are listed but their install/run path is the next
layer.  A SCSI disk is never required: if one is present it is an
opportunistic cache, not a dependency.

The index can move to a separate `irix7/packages` repository without changing
the format; only the default URL changes.
