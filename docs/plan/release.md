# IRIX 7 — Release Engineering

**Status:** living document
**Scope:** versioning, channels, artefacts, image pipeline, signing, A/B updates, rollback,
support and release checklists.
**Related:** [`milestones.md`](milestones.md), [`../spec/system.md`](../spec/system.md), [`github.md`](github.md) §13, [`testing.md`](testing.md) §8, [`security.md`](security.md).

## 1. Versioning

- **Semantic versioning** for the OS: `MAJOR.MINOR.PATCH`.
- `v0.x` is pre-1.0: interfaces may change, but each release still gets a changelog and a
  migration note.
- `v1.0` is the real-Indy daily driver; after that, breaking changes require a major version.
- Git tags: `v0.1.0`, `v0.2.0`, …; releases are immutable.
- The kernel, boot bundle and root image share the OS version; the native ABI has its own
  version.

### 1.1 Wasm/mesh release line

The native **v0.1.3** image remains the last shipped release. The Wasm/mesh line is
not releasable until the M1 (single node) and M2 (two node) gates pass.

Wasm/mesh artefacts are added alongside, not instead of, the native artefacts:

| Artefact | Contents |
|---|---|
| `irix7-core.wasm` | The architecture-independent core; one BLAKE3 OID |
| `irix7-shim-<target>` | Indy, x86_64 PC and cloud shim image |
| `irix7-boot.manifest` | Core OID, shim version, membership/authority root, ABIs |
| `irix7-modules-abi10` | Existing native module set, unchanged contract |
| `irix7-user-sdk` | User SDK image/template artefacts |
| `irix7-mesh-config` | Join token/authority root material, never the private key |

The native kernel, boot bundle and EROFS root stay in the release until Phase 9.
A release manifest must state whether it is native-only, single-node or
mesh-capable. A mesh release must name the supported node matrix; “any
computer” is not a release claim.

## 2. Channels

| Channel | Cadence | Audience | Stability |
|---|---|---|---|
| `nightly` | Every night | Maintainers | May not boot |
| `alpha` | Milestone demo | Testers | Known gaps |
| `beta` | Release candidate | Early users | Feature-complete for the milestone |
| `stable` | Tagged release | Everyone | Release-gated |

The stable channel is the only one that promises rollback and support.

## 3. Artefacts

Every release attaches:

| Artefact | Contents |
|---|---|
| `irix7-kernel` | n32/MIPS III kernel, stripped, with symbols separately |
| `irix7-boot.bundle` | Network, block, FS, init, MCP, console, RTC cells |
| `irix7-root.erofs` | Read-only EROFS root image |
| `irix7-mcp` | Host-side MCP client/CLI (optional) |
| `SHA256SUMS` | Checksums of every artefact |
| `irix7.spdx.json` | SPDX SBOM |
| `irix7.cdx.json` | CycloneDX SBOM |
| `irix7.sig` / `.att` | cosign signature/attestation |
| `release-notes.md` | Generated release notes |
| `install.md` | Install, upgrade and rollback instructions |

## 4. Image pipeline

```text
source + lockfiles
      │
      ├─ cargo build --release --target mips3-sgi-irix7-n32   (kernel + ROM)
      ├─ build boot bundle (cells + init)
      ├─ mkfs.erofs -T -U clear root/ root.erofs
      ├─ manifest.json (versions, digests, sizes, ABI)
      ├─ SHA-256 + cosign sign
      ├─ upload to R2 (content-addressed)
      └─ GitHub Release (notes, SBOM, checksums, signature)
```

Requirements:

- Hermetic build: pinned rustc, GCC, binutils, `mkfs.erofs`, Nix devshell.
- Reproducible: same inputs → same hashes; record the toolchain versions.
- Content-addressed: the R2 object name is the SHA-256.
- Immutable: never overwrite a released object; publish a new digest.
- Manifest: `manifest.json` lists the kernel, bundle, root and ABI versions.

## 5. A/B updates and rollback

- Two root slots, A and B, plus a small boot configuration in NVRAM.
- The ROM/kernel selects the slot with the highest committed generation that verifies.
- An update writes the inactive slot, verifies the digest, marks it committed, and reboots.
- If the new slot fails to reach the MCP/shell within a timeout, the watchdog or the
  bootloader falls back to the previous slot.
- User data lives outside the slots (Git/R2/overlay), so a rollback does not lose it.
- v1 may ship A/B as a future feature; the release notes must state whether it is present.

## 6. Reproducible builds

- Nix pins the toolchain; `Cargo.lock` pins the crates.
- `SOURCE_DATE_EPOCH` is fixed.
- `mkfs.erofs -T -U clear` produces deterministic images.
- The build records the git commit, the toolchain versions and the image hashes.
- A rebuild check runs in CI for release tags.

## 7. Mirrors and availability

- R2 is the primary distribution channel (free egress).
- GitHub Releases is the secondary/archival copy (2 GB per file).
- The lab server keeps an offline mirror of the current stable and the previous stable.
- The ROM can use a pre-signed R2 URL or the lab mirror if the bucket is private.

## 8. Support policy

- The latest stable release is supported.
- The previous stable is supported for one release cycle for rollback.
- Security fixes are backported to the supported releases.
- Pre-1.0 releases are supported on a best-effort basis.
- The project is a hobby OS; there is no commercial SLA.

## 9. Deprecation

- Deprecations are announced in the release notes and the changelog.
- A deprecated interface remains for at least one minor release.
- Removal is a major-version change (or a pre-1.0 breaking change with a migration note).

## 10. Release checklist

- [ ] Milestone acceptance criteria met.
- [ ] CI green: build, test, licence, security, docs.
- [ ] Boot on iris and a real Indy.
- [ ] Compatibility corpus passes.
- [ ] MCP loop reproduces the demo.
- [ ] SBOM, checksums and signature generated.
- [ ] Reproducible-build check passes.
- [ ] Upgrade and rollback tested.
- [ ] Release notes and changelog written.
- [ ] Merge the release workflow's boot-configuration pull request so new machines
      fetch the published module.
- [ ] Install/upgrade docs updated.
- [ ] Announcement in Discussions.
- [ ] Post-release: archive the previous stable; update the mirror.

## 11. Rollback and incident

- If a release is broken: mark the GitHub Release as a pre-release, publish a fixed release,
  and keep the broken tag for the record (do not delete it).
- If a release is compromised: revoke the signing identity, publish an advisory, rotate keys,
  and rebuild from a known-good commit.
- The lab mirror keeps the last known-good image for offline recovery.

## 12. Open questions

- The exact A/B mechanism (NVRAM boot config vs a boot partition).
- Whether to sign with cosign keyless only or also a long-lived key.
- The support window for pre-1.0 releases.
- The mirror hosting and bandwidth budget.
