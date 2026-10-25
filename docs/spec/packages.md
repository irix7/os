# IRIX 7 package index

**Status:** draft
**Version:** 0.1
**Owner:** release / SDK
**Related:** [`modules.md`](modules.md), [`../plan/milestones.md`](../plan/milestones.md).

## 1. Purpose

Modules and userland images are distributed through a community index rather
than built into the kernel or fetched one at a time from an ad-hoc URL.  The
index is a plain-text, git-hosted catalog; it is intentionally closer to HACS
than to a package manager with a solver.  There is no central service.

## 2. Repository layout

```text
packages/
  index.conf          canonical catalog
  modules/            module binaries
  userland/           userland images
  README.md
```

A future `irix7/packages` repository has the same `index.conf` at its root.
The default URL is the raw `main` file; immutable release URLs belong in the
records, not in the index URL itself.

## 3. Record format

```text
version=1
kind name version url sha256 description...
```

`kind` is `module` or `userland`.  Names are ASCII, contain no whitespace, and
are stable across versions.  `version` is a release tag or SemVer.  `url` must
be HTTPS and immutable for that version.  `sha256` is 64 lowercase hex digits
over the exact bytes.  The description is the remainder of the line.  Records
with the same name are newest first; the client loads the first match.  The
current client holds a 16 KiB index; a larger store needs shards, which are a
later format change.

Comments and blank lines are ignored.  Unknown fields are rejected by the
client; a future format version is not guessed.

## 4. Client rules

Sash fetches the index over the kernel TLS client, lists entries, and on
`packages install NAME` selects the first (newest) exact version.  A module is verified
and loaded through the existing module path.  Userland records are listed but
are not executed by `packages install` yet; the diskless run/cache layer is the
next storage slice.  No entry is executed if the digest does not match.

## 5. SCSI cache policy

A functional SCSI disk is **optional**.  The normal path is network plus RAM.
If a writable SCSI target is present, verified downloads may be cached there by
digest and fetched again without network; absence or failure of that disk never
blocks the network path.  The cache is an optimisation, never a source of
authority: bytes are re-verified on every install.

The current bring-up uses the existing 256 KiB RAM read-ahead window.  The
SCSI write-cache path is the next storage slice; `sash` and `mount` must not
assume a disk is present.

## 6. Optional SCSI cache module

The cache is not part of the compressed PROM and is not a boot prerequisite.
It is a loadable kernel module (or a component of the resident policy module
while there is only one module slot) with a tick callback.  The verified
download path places each blob in a bounded RAM queue keyed by its SHA-256;
the module drains that queue only from its idle tick, writes the bytes to a
**cache-owned** SCSI range through a future append-only `block_write` service,
and records the digest.  A cache hit is re-verified against the digest before
use.  A power loss may leave a partial record; the next boot ignores it and
re-fetches.

The module never writes a data disk.  A target/range is used only when it
carries an `I7CACHE` reservation or is explicitly configured as cache.  A
missing, read-only or failed disk leaves the network/RAM path untouched.

The current implementation has the RAM window only.  `block_write` and the
cache queue are not in ABI v10; the SDK will append them with a version bump.
The one-slot module area also means the cache cannot coexist with the MCP
module until a multiplexer or merged policy module lands.

## 7. Non-goals

Dependency solving, binary deltas, per-package signing keys, central accounts
and automatic execution without digest verification are out of scope for v1.
