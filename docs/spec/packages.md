# IRIX 7 package index

**Status:** draft
**Version:** 0.2
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

Version 2 adds metadata fields before the description:

```text
version=2
kind name version url sha256 [depends=name@version:sha256,...] \
  [abi=major.minor] [region=N] \
  [exports=interface@version:offset,...] \
  [sig=epoch:128-hex-digits] description...
```

`kind` is `module` or `userland`.  Names are ASCII, contain no whitespace, and
are stable across versions.  `version` is a release tag or SemVer.  `url` must
be HTTPS and immutable for that version.  `sha256` is 64 lowercase hex digits
over the exact bytes.  The description is the remainder of the line.  Records
with the same name are newest first; the client loads the first match.  The
current client holds a 16 KiB index; a larger store needs shards, which are a
later format change.  `tools/module-release` emits v2 records and the
`module-resolver` index parser bounds an index at 64 KiB and 256 records.

Comments and blank lines are ignored.  Unknown fields are rejected by the
client; a future format version is not guessed.

### 3.1 Runtime records (v2)

A runtime module record carries the metadata the downloadable manager needs:

- `depends=` is a comma-separated list of `name@version:sha256` entries with
  exact versions and 64 lowercase hex SHA-256 digests, matching the module
  manifest.  Duplicate names in one record are rejected.
- `abi=major.minor` is the module ABI the image was built for.  A record
  without it is not runtime-loadable.
- `region=N` is the fixed arena region the image was linked for.  Images are
  not relocatable; the release tool records the link address as this region.
- `exports=interface@version:offset,...` copies the manifest export table
  offsets so `module_load` can register them in the same call.  Offset zero is
  a declaration with no table.
- `sig=epoch:128-hex-digits` is the detached Ed25519 release signature over
  the record digest.  The epoch selects a key from the release trust policy.
  The signature is over `IRIX7-MODULE-IMAGE-v1 || sha256`, matching
  `crates/module-trust`.

The index parser is in `crates/module-resolver/src/index.rs`; the release
generator is `tools/module-release`.  The manager verifies the detached index
signature (from `manager.index.sig`) and every record signature before it
loads an image.  A record without `sig=` is rejected unless the embedded trust
policy explicitly sets `require-signatures=0`.

Version 2 does not add ranges, conflict solving or automatic upgrades; a later
format change can.  Version 1 records remain standalone and are not
runtime-loadable.

## 4. Client rules

Sash fetches the index over the kernel TLS client, lists entries, and on
`packages install NAME` selects the first (newest) exact version.  A module is
verified and loaded through the existing module path.  A userland record is
fetched into the 256 KiB loader buffer, verified, and written to SCSI target 2;
`reboot` then starts it.  `packages run NAME` instead fetches and verifies the
image in RAM and enters it without a disk.  No entry is executed if the digest
does not match.

## 5. SCSI cache policy

A functional SCSI disk is **optional**.  The normal path is network plus RAM.
If a writable SCSI target is present, verified downloads may be cached there by
digest and fetched again without network; absence or failure of that disk never
blocks the network path.  The cache is an optimisation, never a source of
authority: bytes are re-verified on every install.

The current bring-up uses the existing 256 KiB RAM read-ahead window for
network reads.  ABI v11 adds `block_write` and a probe module can persist one
sector to a cache-owned target during its idle tick.  A bounded download queue
and automatic lookup are still the next storage slice; `sash` and `mount` must
not assume a disk is present.

## 6. Optional SCSI cache module

The cache is not part of the compressed PROM and is not a boot prerequisite.
It is a loadable kernel module (or a component of the resident policy module
while there is only one module slot) with a tick callback.  The verified
download path places each blob in a bounded RAM queue keyed by its SHA-256;
the module drains that queue only from its idle tick, writes the bytes to a
**cache-owned** SCSI range through the ABI v11 `block_write` service, and
records the digest.  A cache hit is re-verified against the digest before use.
A power loss may leave a partial record; the next boot ignores it and
re-fetches.

The module never writes a data disk.  A target/range is used only when it
carries an `I7CACHE` reservation or is explicitly configured as cache.  A
missing, read-only or failed disk leaves the network/RAM path untouched.

The current implementation has the RAM window and a one-sector probe:
`modules/cache` reads target 1 LBA 0, hashes it with BLAKE3, and writes an
`I7CACHE1` header plus the data to target 3 during two idle ticks, then
verifies the readback.  The bounded download queue and automatic cache lookup
are still to come.  The one-slot module area also means the cache cannot
coexist with the MCP module under the fixed-region kernel module multiplexer.

## 7. Non-goals

Version ranges/SAT solving, binary deltas, per-package signing keys, central
accounts and automatic execution without digest verification are out of scope.
The v1 `sash` client remains standalone and ignores the v2 metadata fields.  A
record is still only loaded after its digest matches; the runtime manager adds
the exact graph, manifest footer and release-signature checks on top.

The DS1386 environment holds `manager.index`, `manager.index.sig` and
`manager.root` for the runtime manager.  The MAC address occupies byte 314 of
the chip, so the complete environment must stay below 298 bytes: a test rig
uses a short index path (`/i`) and relies on the manager's embedded trust
policy.  `manager.trust=<url>,<sha256>` is available when a longer-lived rig
needs a separate trust source.

The release workflow passes the Ed25519 signing seed in the
`MODULE_SIGNING_KEY` secret (and an optional `MODULE_SIGNING_EPOCH` variable).
A missing key fails the release unless `IRIX7_RELEASE_UNSIGNED=1` is set
explicitly.  The matching public key belongs in `packages/release-trust.conf`;
a key rotation updates that file and lists the revoked epoch.
