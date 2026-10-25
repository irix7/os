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
over the exact bytes.  The description is the remainder of the line.

Comments and blank lines are ignored.  Unknown fields are rejected by the
client; a future format version is not guessed.

## 4. Client rules

Sash fetches the index over the kernel TLS client, lists entries, and on
`packages install NAME` selects the newest exact version.  A module is verified
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

## 6. Non-goals

Dependency solving, binary deltas, per-package signing keys, central accounts
and automatic execution without digest verification are out of scope for v1.
