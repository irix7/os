# IRIX 7 — Legal, Licensing and Provenance

**Status:** living document
**Licence:** GPL-3.0 (ADR-0012)
**Related:** [`../LICENCE`](../LICENCE), [`github.md`](github.md) §3, [`security.md`](security.md).

## 1. Licence

- The project is **GPL-3.0**; the full text is in [`../LICENCE`](../LICENCE).
- Every source file carries an SPDX header:
  `SPDX-License-Identifier: GPL-3.0-only` (or the appropriate licence for third-party code).
- Third-party code retains its own licence and notices; the combined work is conveyed under
  GPL-3.0.
- The licence gate (ADR-0012) blocks any dependency incompatible with GPL-3.0.

## 2. Clean-room policy

- The private `irix7/irix6` repository holds the leaked IRIX 6.5.7m source, decompilation and
  dirty notes. It is never public, never forked and has no Actions.
- Dirty readers produce a **behavioural specification**, not code.
- Clean writers see only the specification and the public research; their Rust is the only
  code that reaches `irix7/os`.
- The `irix7/harness` enforces the structural split and runs the similarity screen
  (ADR-0014/0015).
- Every kernel PR is screened; a similarity hit blocks the merge until resolved.
- The clean-room reviewers are listed in `github.md` §3.

## 3. Provenance

- **DCO:** every commit is signed off (`git commit -s`). The project uses the Developer
  Certificate of Origin 1.1; no CLA.
- **Access control:** only clean-room reviewers can read `irix6`; access is reviewed
  quarterly.
- **Record keeping:** the harness stores the specification version, the screen result and the
  reviewer for each contribution.
- **Attestations:** release artefacts carry build provenance (GitHub attestations or cosign)
  where available.
- **SBOM:** every release has SPDX and CycloneDX SBOMs.

## 4. Third-party notices

- `THIRD-PARTY-NOTICES.md` lists every dependency, its licence and its copyright.
- Generated from `Cargo.lock` by `cargo-about`/`cargo-deny` in CI.
- The file is attached to every release.
- If a dependency changes licence, CI fails until the gate is updated.

## 5. Trademarks and naming

- **IRIX**, **SGI**, **Indy**, **Indigo**, **O2**, **Octane** and related marks are the
  property of their respective owners.
- IRIX 7 is an independent, clean-room project; it is **not affiliated with, endorsed by or
  sponsored by SGI or any successor**.
- The README and the website carry a disclaimer.
- The name "IRIX 7" should be reviewed by a lawyer before any commercial distribution. A
  rename is possible without changing the code.
- Do not use SGI logos or artwork in the project or the website.
- "Indy" is used descriptively for the hardware; the project does not claim the mark.

## 6. Privacy

- No telemetry by default.
- The MCP server is a local control plane; it does not phone home.
- User data in GitHub/R2 is governed by the user's account and the provider's terms.
- The AI/MCP client may send prompts to a remote model; that is the client's choice, not the
  OS's.
- Logs may contain file paths and commands; treat them as sensitive.
- The lab network is isolated; do not expose the MCP port to the internet.

## 7. Export and cryptography

- The project uses TLS (`rustls`), SHA-256, Ed25519 and cosign. These are standard
  cryptographic tools; check local export rules if distributing in a restricted jurisdiction.
- This document is not legal advice; consult a lawyer for distribution.

## 8. Contributions

- By contributing, you agree to the DCO and the GPL-3.0 licence.
- Do not contribute code you do not have the right to license.
- Do not contribute SGI-derived expression; the clean-room policy applies.
- Report provenance concerns through the `clean_room.yml` issue form or privately to the
  maintainers.

## 9. Record keeping

| Record | Where | Retention |
|---|---|---|
| DCO sign-offs | Git history | Forever |
| Clean-room screen results | Harness/private | Project lifetime |
| SBOMs | Releases | Forever |
| Licence checks | CI logs/artefacts | 1 year |
| Security advisories | GitHub Security | Forever |
| Trademark review | `docs/legal/` (private if needed) | Project lifetime |

## 10. Open questions

- The final name and trademark position.
- Whether a CLA is ever needed (currently no).
- The privacy policy for the website and the MCP client.
- The jurisdiction for any future legal entity.
