# IRIX 7 — Security Model

**Status:** living document
**Scope:** threat model, trust boundaries, MCP modes, legacy sandboxing, driver safety,
supply chain, secrets, disclosure and incident response.
**Related:** [`../spec/architecture.md`](../spec/architecture.md) §4, §5, §14, [`../SECURITY.md`](../SECURITY.md).

## 1. Assets and adversaries

| Asset | Why it matters |
|---|---|
| Kernel integrity | A corrupted kernel means a bricked machine or data loss |
| User data in Git/R2 | The user's documents and media |
| MCP token | Full root on the Indy |
| R2 credentials | Image integrity and cost |
| Clean-room boundary | The project's legal survival |
| Release artefacts | Supply-chain trust |

Adversaries:

- a **LAN attacker** who can reach the MCP port;
- a **malicious legacy IRIX binary** with raw device capabilities;
- a **malicious native cell** exploiting the shared address space;
- a **buggy driver** with raw DMA;
- a **prompt-injection payload** in a file, serial log or screen capture;
- a **supply-chain attacker** in a crate, action or release artefact;
- **physical access** to the Indy.

## 2. Trust boundaries

```text
+-------------------------------------------------------------+
| Privileged framework (unsafe TCB)                            |
+-------------------------------------------------------------+
| Core services (safe Rust)                                    |
+-------------------------------------------------------------+
| Drivers (trusted, documented unsafe)                         |
+-------------------------------------------------------------+
| Native cells (safe Rust, shared address space)               |
+-------------------------------------------------------------+
| Legacy IRIX processes (MMU-isolated, untrusted)              |
+-------------------------------------------------------------+
| MCP client (remote, untrusted input)                         |
+-------------------------------------------------------------+
```

The framework is the only place where `unsafe` is unavoidable. Drivers are trusted code with
documented `unsafe`; a driver bug is a kernel bug. Native cells are safe Rust but share the
address space; the capability model limits authority, not memory corruption. Legacy
processes are the only hardware-isolated code.

## 3. Principles

1. **Capabilities, not names.** A path, namespace or mount point is not authority.
2. **Least authority.** Every cell starts with only the capabilities it needs.
3. **Revocation is real.** A parent can revoke a child's device or file capability.
4. **No ambient authority.** No global root handle, no implicit device access.
5. **Memory safety by default.** `unsafe` is confined and documented.
6. **Defence in depth.** The MMU for legacy binaries, capabilities for native cells, the
   watchdog for hangs, digests for images.
7. **Assume the network is hostile.** The MCP server is a development harness, not an
   internet service.

## 4. MCP: development mode vs production mode

| Control | Development (v1) | Production (future) |
|---|---|---|
| Authority | Full root | Capability-scoped per tool |
| Auth | Bearer token | OAuth 2.1/mTLS + per-session token |
| Transport | HTTP on a trusted LAN | TLS 1.3 |
| Audit | None | Append-only audit log |
| Destructive tools | Direct | Human approval |
| Shell/GDB | Full | Disabled or gated |
| Exposure | LAN only | Explicit, authenticated, rate-limited |

The v1 choice is deliberate: the MCP server is how the OS gets built. It must not be exposed
to the public internet, and the token must be rotated if the lab network is shared.

## 5. Legacy binary sandbox

- Each legacy process gets an MMU address space and an ASID.
- The compat layer validates every user pointer with `copyin`/`copyout`.
- A syscall filter restricts the IRIX syscalls the container exposes.
- Device access is granted per library: GL/dmedia/audio may receive device capabilities;
  storage/network are mediated by the native servers.
- Raw DMA is the residual risk: a legacy binary with a DMA window can write physical memory.
  The MC's 4-entry DMA TLB confines it where the device path allows; otherwise the risk is
  accepted (D16).
- The container never runs with the kernel's page table; a trap switches to kseg0.

## 6. Driver safety

- Drivers are de-privileged cells but trusted code; `unsafe` is documented.
- The framework labels sensitive MMIO and only grants it to drivers that need it.
- DMA windows are physically contiguous and cache-managed; the framework owns the DMA TLB.
- IRQs are delivered as events; drivers cannot install raw handlers.
- A driver fault panics in v1; restartable drivers are a future goal.
- The watchdog is the recovery path for a hung driver or kernel.

## 7. Supply chain

- **Dependencies:** `cargo-deny` (licence, advisories, sources), pinned versions,
  `Cargo.lock` committed, `cargo-audit`.
- **Actions:** pin third-party actions to a commit SHA; prefer first-party actions; least
  privilege `GITHUB_TOKEN`.
- **CodeQL** for Rust and Actions; **Dependabot** for updates; **dependency review** on PRs.
- **SBOM** (SPDX and CycloneDX) for every release.
- **Signing:** cosign keyless (OIDC) for release artefacts; checksums in the release notes.
- **Reproducible builds:** the image and kernel must rebuild to the same hashes.
- **Provenance:** GitHub artifact attestations where available; clean-room attestations for
  kernel code.

## 8. Secrets

| Secret | Storage | Rotation |
|---|---|---|
| MCP token | GitHub environment `hardware`; boot config on the Indy | On compromise or quarterly |
| R2 keys | GitHub environment `release` | Quarterly |
| Lab SSH keys | Lab host, not the repo | Quarterly |
| Cosign | Keyless OIDC, no long-lived key | N/A |

Rules:

- No secrets in the repository, issues, logs or screen captures.
- Secret scanning and push protection are on.
- The MCP token is never in `BootInfo` or a world-readable file.
- CI logs are treated as public.

## 9. Vulnerability disclosure

- `SECURITY.md` explains private reporting.
- GitHub private vulnerability reporting is enabled.
- A report is acknowledged within 7 days; a fix or mitigation is targeted within 90 days.
- Security advisories are published after a fix, with credit unless the reporter prefers
  anonymity.
- There is no bug bounty.

## 10. Incident response

1. **Contain:** revoke the MCP token, power off the Indy, rotate R2 keys if exposed.
2. **Assess:** what was accessed, what was modified, which images are affected.
3. **Eradicate:** rebuild from a known-good image; verify digests.
4. **Recover:** restore user data from Git/R2; re-enable the watchdog.
5. **Learn:** write a post-mortem; add a test or a control; update the risk register.

## 11. Security review checklist

For a PR touching kernel, drivers, capabilities, MCP or release:

- [ ] `unsafe` blocks have a written invariant and a safety comment.
- [ ] Capabilities are least-authority and revocable.
- [ ] User pointers are validated with `copyin`/`copyout`.
- [ ] DMA buffers are cache-synced and owned.
- [ ] No secret in the diff or logs.
- [ ] Dependencies pass the licence and advisory gate.
- [ ] The change is covered by a test or a fuzz target.
- [ ] The threat model is updated if the trust boundary moved.
- [ ] The MCP tool, if new, has a schema and an error path.

## 12. Open questions

- The production-mode capability model and audit format.
- Whether to add a secure boot/measured boot chain (currently hobby-grade).
- The legacy syscall filter's exact allowlist.
- Whether native cells should get a debug-mode guard page or a separate address space.
- The incident-response owner and the disclosure timeline.
