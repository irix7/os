# IRIX 7 — Cost Model

**Status:** living document
**Target:** **$0/month** in recurring service costs; a small one-off hardware budget.
**Related:** [`github.md`](github.md) §17, [`release.md`](release.md).

## 1. Free tiers

| Service | Free allowance (verify current terms) | IRIX 7 use |
|---|---|---|
| GitHub public repos | Unlimited repos, unlimited collaborators | Everything except `irix6` |
| GitHub Actions (public) | Unlimited standard-runner minutes | CI, security, docs, release |
| GitHub Actions (private) | 2,000 minutes/month | `irix6` has no Actions |
| GitHub Actions cache | 10 GB/repo | Cargo, Nix, iris |
| GitHub Actions artefacts | Free-tier storage, 90-day retention | Build outputs, boot logs |
| GitHub Releases | 2 GB per file | Kernel, bundle, image, SBOM |
| GitHub Packages | Free for public packages; 500 MB private | Optional MCP client |
| GitHub LFS | 10 GiB storage + 10 GiB/month bandwidth | Small binaries only |
| GitHub Pages | Free for public repos; ~1 GB site, ~100 GB/month soft bandwidth | Docs site |
| GitHub Codespaces | 120 core-hours + 15 GB-month (personal) | Docs/onboarding |
| GitHub Discussions/Wiki/Projects | Free | Project management |
| Cloudflare R2 | 10 GB-month storage, 1M Class A + 10M Class B ops/month, free egress | OS image, bundle, large media |
| Oracle/other free VM | Varies | Optional lab mirror |

## 2. Cost drivers

| Driver | Recurring? | Estimate | Mitigation |
|---|---|---|---|
| R2 storage above 10 GB | Yes | $0.015/GB-month | Content-addressed images; prune old releases; 10 GB is plenty for v1 |
| R2 Class A/B ops above free | Yes | $4.50/M writes, $0.36/M reads | Cache; read-ahead; immutable objects |
| GitHub LFS above 10 GiB | Yes | Pay-as-you-go | Keep the OS image on R2; LFS for small binaries only |
| GitHub Actions private minutes | Yes | $0.008/min (Linux) | Keep actions on public repos; self-hosted for heavy jobs |
| Domain name | Yes | ~$10–15/year | Use `github.io` until a domain is needed |
| Electricity (Indy + lab host) | Yes | ~$100–300/year | Power off when idle; smart plugs |
| Hardware spares | One-off | ~$300–800 | Buy slowly; use BlueSCSI/ZuluSCSI |
| Smart plugs/UPS/cables | One-off | ~$150–300 | Essential for remote reset |
| Backup storage | Yes | ~$5–10/month | Local disk + GitHub/R2; no cloud backup tier |
| Legal/trademark review | One-off | Variable | Defer until distribution; use a disclaimer now |

## 3. Budget

| Period | Recurring | One-off |
|---|---|---|
| Year 1 | $0–120 (electricity, domain) | $450–1,100 (spares, plugs, cables) |
| Year 2+ | $0–120 | $0–200 (replacements) |

The project does not need a paid GitHub plan, a paid CI service, or a paid CDN for v1. If R2
outgrows the free tier, the next cheapest option is a self-hosted mirror plus GitHub Releases.

## 4. Quotas and monitoring

- **GitHub:** billing page → Actions/Packages/LFS usage; set a $0 budget for LFS to block
  overages.
- **R2:** dashboard → storage, Class A/B ops; set a budget alert.
- **Pages:** bandwidth is soft-limited; a popular release could hit it. Mirror the image on R2.
- **LFS:** 10 GiB/month; never put the OS image there.
- **Actions:** public repos are unlimited; keep private work in `irix6` without Actions.

Review monthly in the project update.

## 5. Scaling scenarios

| Scenario | Impact | Response |
|---|---|---|
| 10 users | Free tier fine | Nothing |
| 100 users | R2 reads may approach 10M/month | Add a caching mirror; reduce block size; read-ahead |
| 1,000 users | R2 storage/ops likely paid | Self-hosted mirror; sponsor; GitHub Releases as fallback |
| Many contributors | Public Actions fine | Add reviewers; split repos; don't add private CI |
| Large media | R2 storage grows | User pays for their own R2/Git LFS; the OS image stays small |

The OS image should stay under ~256 MB; the boot bundle under ~16 MB. A smaller image keeps
the free tier comfortable and the 10 Mbit boot tolerable.

## 6. Sustainability

- No paid tier is required to use or contribute.
- Donations/sponsors are optional; the project does not depend on them.
- The hardware lab is the main real cost; spares are bought second-hand.
- If the project grows, the first paid item is a domain and a backup disk, not CI.

## 7. Open questions

- Whether to accept donations or sponsors.
- Whether a paid GitHub plan is ever needed for private CI.
- The long-term R2/GitHub free-tier terms.
- The electricity cost of leaving the lab on.
