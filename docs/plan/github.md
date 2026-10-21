# GitHub Operating Model

**Status:** living document
**Scope:** the organisation, repositories, Project, labels, templates, automation, CI, security and every free GitHub feature that is useful to IRIX 7.
**Current state (2026-10-21):** `irix7/os` is public; Issues and Projects are enabled; Discussions and Wiki are enabled; the advanced labels and the `v0.1`–`v1.0` milestones exist; Project **#2, IRIX 7 — Roadmap**, is linked to the repository; the M0–M10 and cross-cutting epics exist and are attached to the Project with their fields set. An existing Project, **IRIX 6.5.7m rebuild**, covers the precursor work.

The goal is a project-management goliath that costs **$0/month** and still behaves like a
serious engineering organisation: every issue tracked, every release reproducible, every
security signal visible, every decision recorded.

## 1. Principles

1. **Public by default.** `irix7/os` is GPL-3.0 and public, which unlocks the free GitHub
   feature set. The only private repository is `irix7/irix6` (clean-room source).
2. **Everything is an issue.** A task that is not on the Project does not exist. A decision
   that is not an ADR is a rumour.
3. **Labels are machine-readable; fields are human-readable.** Labels drive automation and
   filtering. Project fields drive the roadmap and the board.
4. **Automate the boring parts.** Adding to the Project, status transitions, stale closure,
   release notes, size labels and security scans are automated.
5. **Least privilege.** `GITHUB_TOKEN` is read-only by default; workflows request only the
   permissions they need. Secrets live in environments, not in the repository.
6. **Free tier only.** No paid feature is required. If a feature is paid or unavailable, the
   plan says so and provides a fallback.

## 2. Organisation and repositories

| Repository | Visibility | Purpose | Licence |
|---|---|---|---|
| `irix7/os` | Public | Kernel, native OS, compatibility container, plan, docs, research | GPL-3.0 |
| `irix7/project` | Public | Toolchain hub, ADRs, Nix flake, rig and smoke scripts | GPL-3.0 |
| `irix7/iris` | Public | Indy/Indigo2 emulator fork | Upstream licence |
| `irix7/gcc` | Public | GCC 17 trunk + IRIX patches + libgccjit | GPL-3.0 |
| `irix7/binutils-gdb` | Public | binutils 2.47 + IRIX series | GPL-3.0 |
| `irix7/ghidra` | Public | Ghidra fork for the recovery work | Apache-2.0 |
| `irix7/harness` | Public (planned) | Clean-room transcription and similarity screen | GPL-3.0 |
| `irix7/irix6` | **Private** | Leaked IRIX 6.5.7m source, decompilation, dirty notes | Private |
| `irix7/docs` | Optional | Published documentation site, if it outgrows Pages from `os` | GPL-3.0 |

Repository settings for `irix7/os`:

- Description and topics: `irix`, `sgi`, `mips`, `rust`, `operating-system`, `clean-room`.
- Default branch `main`; squash merge; delete branch on merge; linear history.
- Issues, Projects, Discussions, Wiki and Pages enabled.
- Private vulnerability reporting enabled; security policy linked.
- Dependabot alerts and security updates enabled.
- Secret scanning and push protection enabled (free for public repositories).
- CodeQL default setup enabled for Rust and Actions.
- Actions permissions: read-only `GITHUB_TOKEN` by default; allow select actions.
- Forking allowed; workflows from forks require approval for first-time contributors.
- Release immutability enabled if available.

## 3. Access and clean-room separation

| Team | Access | Members |
|---|---|---|
| `@irix7/maintainers` | Admin on `os`, `project`, `harness` | Project owner + trusted maintainers |
| `@irix7/kernel` | Write on `os` | Kernel maintainers |
| `@irix7/compat` | Write on `os` | Compatibility maintainers |
| `@irix7/display` | Write on `os` | Display/X11 maintainers |
| `@irix7/infra` | Maintain on workflows and CI | Infra maintainers |
| `@irix7/clean-room` | Read on `irix6`; review on `os` | Clean-room reviewers |

Rules:

- 2FA is mandatory for every member.
- `irix7/irix6` has no Actions, no forks, no public access and no external collaborators.
- Only clean-room reviewers may read `irix6`; their work product is the behavioural
  specification, not code.
- Every contribution carries a DCO sign-off (`git commit -s`).
- The similarity screen runs on every PR that touches kernel code.
- Secrets are never committed; secret scanning and push protection are on.

## 4. Labels

Create these labels in addition to GitHub's defaults. Colours are suggestions; the names
matter.

### Area

| Label | Colour | Meaning |
|---|---|---|
| `area/kernel` | `#1d76db` | Traps, TLB, scheduler, VM, IPC, capabilities |
| `area/compat` | `#5319e7` | IRIX syscall ABI, loader, libc, libraries |
| `area/drivers` | `#0e8a16` | SCSI, Seeq, HAL2, Newport, Vino, serial, RTC |
| `area/display` | `#d93f0b` | Display server, desktop, X11, graphics |
| `area/storage` | `#bfd4f2` | EROFS, XFS, I7FS, block cache, Git sync |
| `area/network` | `#006b75` | smoltcp, `/net`, HTTP Range, TLS |
| `area/toolchain` | `#fbca04` | rustc, GCC, LLVM, binutils, build-std |
| `area/infra` | `#c2e0c6` | CI, Actions, runners, releases, R2 |
| `area/docs` | `#0075ca` | Documentation, specs, ADRs, website |
| `area/security` | `#b60205` | Threat model, MCP security, supply chain |
| `area/legal` | `#e99695` | Licence, provenance, trademark |
| `area/clean-room` | `#f9d0c4` | Harness, similarity screen, provenance |

### Type

| Label | Colour | Meaning |
|---|---|---|
| `type/epic` | `#3e4b9e` | A milestone-sized body of work |
| `type/feature` | `#a2eeef` | New capability |
| `type/bug` | `#d73a4a` | Something is broken |
| `type/research` | `#d4c5f9` | Question to answer, not code to write |
| `type/adr` | `#fef2c0` | Architecture decision record |
| `type/hardware` | `#f0e68c` | Real-Indy work |
| `type/task` | `#ededed` | Small unit of work |
| `type/debt` | `#e6ccb3` | Refactor or cleanup |

### Priority, size and risk

| Label | Meaning |
|---|---|
| `priority/P0` | Blocks the milestone or the release |
| `priority/P1` | Must be in the milestone |
| `priority/P2` | Should be in the milestone |
| `priority/P3` | Nice to have |
| `size/XS` | < 1 hour |
| `size/S` | 1–4 hours |
| `size/M` | 1–3 days |
| `size/L` | 1–2 weeks |
| `size/XL` | > 2 weeks; split it |
| `risk/low` | Local, reversible |
| `risk/medium` | Crosses a subsystem or the toolchain |
| `risk/high` | Clean-room, security, data loss, hardware damage |

### Hardware and status

| Label | Meaning |
|---|---|
| `hardware/iris` | Reproducible on the emulator |
| `hardware/indy` | Requires a real Indy |
| `hardware/both` | Must be validated on both |
| `hardware/none` | Tooling/docs only |
| `status/blocked` | Waiting on a dependency |
| `status/needs-adr` | Decision required before code |
| `status/needs-design` | Interface or UX design required |
| `status/needs-hardware` | Blocked on a part or machine |
| `status/clean-room-review` | Needs the similarity screen |
| `status/security-review` | Needs a security review |
| `status/needs-triage` | Not yet triaged |
| `good first issue` | Newcomer-friendly (keep GitHub's label) |
| `help wanted` | Extra attention welcome (keep GitHub's label) |

The bootstrap also creates the operational labels `area/boot`, `area/hardware`,
`type/task`, `type/debt`, `stale`, `dependencies`, `skip-changelog`, and
`release/major`, `release/minor`, `release/patch`.

## 5. Milestones

Create release milestones only; the `Phase` Project field carries M0–M10.

| Milestone | Due | Contents |
|---|---|---|
| `v0.1` | M3 | Serial, network, MCP, shell, reset/watchdog |
| `v0.2` | M5 | First IRIX o32 binary, minimal libc |
| `v0.3` | M6 | Newport display, native desktop shell |
| `v0.4` | M7 | X11 lease, `xterm`/`twm`, Xlib subset |
| `v0.5` | M8 | XFS read-only, IRIX disk access |
| `v0.6` | M9 | Native cells, capability ABI, native libc |
| `v1.0` | M10 | Real-Indy daily driver, updater, docs, signed release |

Due dates are optional and should be set only when the milestone is active; a stale date is
worse than no date.

## 6. Issue types, sub-issues and epics

GitHub's Issue Types and sub-issues are used where the org has them enabled; otherwise the
`type/*` labels and task lists are the fallback.

- **Epic** = a milestone or a cross-cutting workstream. One epic per M0–M10 plus the seven
  cross-cutting workstreams.
- **Sub-issue** = a feature, research task, hardware task or bug inside an epic.
- **Task list** = the checklist inside an epic; every unchecked item links to an issue.
- **Draft issue** = a Parking-lot idea in the Project, not yet promoted to a repository issue.

Epic body template:

```markdown
## Goal
## Demo
## Acceptance criteria
- [ ] ...
## Dependencies
## Out of scope
## Links
- Spec:
- ADR:
- Research:
```

## 7. Issue forms

The `.github/ISSUE_TEMPLATE/` directory contains YAML issue forms:

| Form | Use |
|---|---|
| `bug_report.yml` | Reproducible defect, with build, hardware and logs |
| `feature_request.yml` | New capability, with the problem and the proposed shape |
| `adr.yml` | A decision that needs an ADR |
| `research.yml` | A question that needs evidence, not code |
| `hardware.yml` | Real-Indy, rig, SCSI, serial, PROM or GIO work |
| `compat_bug.yml` | An IRIX binary that does not run, with ABI and binary details |
| `clean_room.yml` | A provenance or similarity concern |
| `epic.yml` | A milestone-sized body of work |

`config.yml` disables blank issues and links to Discussions, the architecture spec and the
plan.

## 8. Pull requests and review

- **Trunk-based**: short-lived branches, squash merge to `main`, linear history.
- **Conventional commits**: `feat:`, `fix:`, `docs:`, `refactor:`, `test:`, `chore:`,
  `build:`, `ci:`, `perf:`, `spec:`, `adr:`.
- **PR template**: summary, linked issue, milestone, type, tests, hardware, clean-room
  checklist, licence check, documentation.
- **Review**:
  - docs and trivial changes: 1 approval;
  - kernel, unsafe, capability or security code: 2 approvals, one from a maintainer;
  - clean-room-sensitive changes: 1 clean-room reviewer.
- **Required checks**: `ci`, `licence`, `security`, `docs` (when they exist); the merge is
  blocked until green.
- **CODEOWNERS**: `* @belikh` for now; replace with teams as they form.
- **Merge queue**: use it if the free plan offers it; otherwise require up-to-date branches.

## 9. Projects v2

Create a project **IRIX 7 — Roadmap** owned by `irix7`, linked to `irix7/os`.

### 9.1 Fields

GitHub creates a default `Status` field (Todo, In Progress, Done). The Project uses a
separate **`Workflow`** field for the detailed state; the default `Status` is kept for the
built-in "item closed" automation.

| Field | Type | Options |
|---|---|---|
| `Workflow` | Single select | Backlog, Ready, In Progress, Blocked, In Review, Done |
| `Phase` | Single select | M0 Toolchain, M1 ROM, M2 Kernel, M3 Network+MCP, M4 Storage, M5 Compat, M6 Display, M7 X11, M8 XFS, M9 Native, M10 Release |
| `Area` | Single select | Kernel, Compat, Drivers, Display, Storage, Network, Toolchain, Infra, Docs, Security, Legal, Clean-room |
| `Priority` | Single select | P0, P1, P2, P3 |
| `Size` | Single select | XS, S, M, L, XL |
| `Risk` | Single select | Low, Medium, High |
| `Hardware` | Single select | iris, Indy, Both, None |
| `Target` | Single select | v0.1, v0.2, v0.3, v0.4, v0.5, v0.6, v1.0 |
| `Estimate` | Number | Person-days |
| `Clean-room` | Single select | Not needed, Review, Approved |
| `Start` | Date | Planned start |
| `Target date` | Date | Planned finish |

### 9.2 Views

| View | Layout | Filter / grouping |
|---|---|---|
| Roadmap | Roadmap | Group by `Phase`, date by `Target date` |
| Board | Board | Columns by `Workflow` |
| Table | Table | All items, all fields |
| Blocked | Table | `Workflow = Blocked` |
| My work | Table | Assignee = `@me` |
| By area | Board | Columns by `Area` |
| Hardware | Table | `Hardware != None` |
| Clean-room | Table | `Clean-room = Review` |
| Backlog | Table | `Workflow = Backlog`, sorted by `Priority` |
| Releases | Roadmap | Group by `Target` |
| Epics | Table | `type/epic` |

### 9.3 Workflows

Built-in Project workflows:

- **Auto-add to project**: issues and PRs in `irix7/os` with `type/epic`, `type/feature`,
  `type/bug`, `type/research`, `type/adr`, `type/hardware` are added to the project.
- **Item closed**: set `Workflow = Done` (and the built-in `Status = Done`).
- **Pull request linked**: set `Workflow = In Review`.
- **Item reopened**: set `Workflow = Ready`.
- **Auto-archive**: archive items done for more than 30 days.
- **Stale**: mark items with no activity for 60 days; close after 14 more.

GitHub Actions fills the gaps:

- `.github/workflows/project.yml` adds new issues/PRs and maps labels to fields.
- `.github/workflows/stale.yml` runs the stale policy.
- `.github/workflows/release-drafter.yml` drafts release notes.

### 9.4 Templates

- **Epic**: goal, demo, acceptance criteria, dependencies, out of scope.
- **Feature**: problem, proposal, alternatives, acceptance criteria.
- **Bug**: build, hardware, steps, expected/actual, logs.
- **ADR**: context, decision, consequences, alternatives.
- **Research**: question, sources, findings, recommendation.
- **Hardware**: machine, board, serial, steps, result.
- **Release checklist**: tests, hardware, docs, SBOM, signature, announcement.

## 10. Actions and CI

### 10.1 Workflow inventory

| Workflow | Trigger | What it does |
|---|---|---|
| `ci.yml` | PR, push to `main` | fmt, clippy, test, host build, cross-build (M0 gate), typos, markdownlint |
| `security.yml` | PR, push, schedule | `cargo-deny`, `cargo-audit`, dependency review, secret scan status |
| `scorecard.yml` | schedule, push to `main` | OpenSSF Scorecard and SARIF upload |
| `licence.yml` | PR, push | licence headers, third-party notices, SBOM |
| `docs.yml` | PR, push to `main`, schedule, dispatch | mdBook build, roadmap sync, and GitHub Pages deploy |
| `release.yml` | tag `v*` | build image, SBOM, checksums, cosign sign, R2 upload, GitHub Release |
| `project.yml` | issues, PRs | add to Project; detailed field mapping is manual or via the UI |
| `stale.yml` | schedule | mark and close stale items |
| `labeler.yml` | PR | path-based area labels |
| `dependency-review.yml` | PR | block vulnerable dependencies |
| `pr-size.yml` | PR | apply `size/*` labels |
| `release-drafter.yml` | push to `main` | draft release notes |
| `hardware.yml` | `workflow_dispatch` | run on the self-hosted `indy` runner |
| `nightly.yml` | schedule | nightly build and boot test |

CodeQL is enabled through the repository's **default setup** (Settings → Code security),
not a custom workflow. A custom `codeql.yml` cannot coexist with the default setup; the
default setup is the GitHub-native, zero-maintenance option.

### 10.2 Runners

- `ubuntu-latest` for host builds, linting, docs and security.
- `ubuntu-latest` with a QEMU/iris container for emulator boot tests (when the image is
  available).
- **Self-hosted `indy`** for real-hardware tests: a small machine connected to the Indy's
  serial console, network and power. Self-hosted runners are free; the hardware is the
  project's.

### 10.3 Caching

- `Swatinem/rust-cache` for Cargo.
- `actions/cache` for the Nix store and the iris binary.
- GitHub Actions cache is free up to 10 GB per repository; prune old caches.
- Cross-build artefacts are uploaded with `actions/upload-artifact` and downloaded by the
  boot-test job.

### 10.4 Secrets and environments

| Secret / variable | Environment | Use |
|---|---|---|
| `R2_ACCOUNT_ID`, `R2_ACCESS_KEY_ID`, `R2_SECRET_ACCESS_KEY`, `R2_BUCKET` | `release` | Image and bundle upload |
| `MCP_TOKEN` | `hardware` | Agent connects to the Indy |
| `INDY_HOST`, `INDY_SERIAL` | `hardware` | Self-hosted rig |
| `COSIGN_*` | `release` | Keyless signing via OIDC (no long-lived key) |
| `GITHUB_TOKEN` | all | Read-only by default; `release` gets `contents: write` |

Environment protection rules: `release` requires a maintainer approval; `hardware` is
limited to the `main` branch.

### 10.5 Permissions

Every workflow starts with:

```yaml
permissions:
  contents: read
```

and adds only what it needs (`contents: write` for releases, `security-events: write` for
CodeQL/Scorecard, `pages: write`/`id-token: write` for Pages, `issues: write`/`pull-requests:
write` for project automation).

## 11. Security features

| Feature | Free for public repos | Use |
|---|---|---|
| Dependabot alerts | Yes | Monitor vulnerable dependencies |
| Dependabot security updates | Yes | Automatic patch PRs |
| Dependency graph | Yes | Licence and dependency visibility |
| Dependency review | Yes | Block vulnerable PRs |
| CodeQL | Yes | Static analysis for Rust and Actions |
| Secret scanning | Yes | Detect committed secrets |
| Push protection | Yes | Block secrets before push |
| Private vulnerability reporting | Yes | Private security reports |
| Security advisories | Yes | Coordinated disclosure |
| OpenSSF Scorecard | Yes | Supply-chain hygiene score |
| SBOM export | Yes | SPDX/CycloneDX from the dependency graph |
| Artifact attestations | Public preview | Provenance for release artefacts |
| Rulesets | Yes | Branch protection |
| Signed commits | Yes (optional) | Verify authorship |

The security policy and threat model live in [`security.md`](security.md) and
[`../SECURITY.md`](../SECURITY.md).

## 12. Discussions, wiki and pages

- **Discussions**: enable with categories `Announcements`, `Roadmap`, `Q&A`, `Show and
  tell`, `Hardware`, `Research`. Use Discussions for open-ended questions; use Issues for
  tracked work.
- **Wiki**: enable for the **lab notebook** and **meeting notes** only; canonical docs stay
  in `docs/`. If the wiki duplicates the repo, close it.
- **Pages**: build `docs/` with mdBook and deploy to `https://irix7.github.io/os/`. Pages is
  free for public repositories.

## 13. Releases and packages

- **Releases**: every `v*` tag creates a GitHub Release with:
  - kernel, boot bundle and EROFS image;
  - SHA-256 checksums;
  - SBOM (SPDX and CycloneDX);
  - cosign signature and attestation;
  - release notes generated from PR labels;
  - upgrade and rollback instructions.
- **Packages**: use GitHub Packages only if we publish an MCP client or container image; the
  free tier is sufficient for public packages.
- **Container registry**: not required for v1.

## 14. Insights and metrics

Use the free Insights tabs and the Project charts:

- **Pulse**: weekly activity.
- **Contributors**: bus-factor awareness.
- **Dependency graph**: licence and vulnerability view.
- **Network**: branch and fork activity.
- **Project charts**: burn-up by milestone, throughput, cycle time, WIP.

Track monthly:

- open issues by area and priority;
- WIP (should stay small);
- cycle time from `Ready` to `Done`;
- bug rate (bugs found after `Done`);
- milestone burn-up;
- hardware-test pass rate.

## 15. Codespaces and devcontainer

Codespaces is useful for docs, CI debugging and contributor onboarding, not for the Indy
hardware.

- A `.devcontainer/devcontainer.json` with Nix, Rust, `gh`, `just`, `mdbook` and the
  cross-toolchain stubs.
- Free quota: 120 core-hours and 15 GB-month for a personal account; stop idle codespaces.
- Do not commit secrets; use Codespaces secrets for `MCP_TOKEN` if needed.

## 16. CLI bootstrap

`scripts/github/bootstrap.sh` is idempotent and performs the setup. It created Project
[#2](https://github.com/orgs/irix7/projects/2) and the M0–M10 epics on 2026-10-20.

1. Checks `gh auth status` and the required scopes (`repo`, `workflow`, `project`).
2. Enables Discussions and Wiki, disables blank issues.
3. Creates the labels in §4.
4. Creates the milestones in §5.
5. Creates the **IRIX 7 — Roadmap** project, links it to `irix7/os`.
6. Creates the fields in §9.1.
7. Creates the epics M0–M10 (and the cross-cutting epics) as issues, adds them to the
   project, and sets `Phase`, `Area`, `Priority`, `Size`, `Risk`, `Hardware`, `Target`.
8. Prints the project URL and the next manual steps (views, rulesets, CodeQL, Pages), because
   some features have no stable CLI/API.

Run it with:

```sh
gh auth refresh -s project,repo,workflow
scripts/github/bootstrap.sh
```

The script never deletes labels, issues, milestones or projects. Re-running it is safe.

### 16.1 Known limitations

- The `gh project` CLI reports `unknown owner type` for some organisation-owned projects.
  Use the GraphQL API or the Project UI for fields and items.
- The Projects API applies a cost-based secondary rate limit. A burst of item/field edits
  can fail even when the hourly quota looks healthy. The issues are created first, so no
  work is lost; re-run the bootstrap later or drag the issues into the Project in the UI.
  The initial bulk sync was completed on 2026-10-21; the caveat applies to future bulk
  changes.
- The views are created with the right layout. Filters, grouping and visible fields are
  configured in the UI (the API for views is limited).
- `PROJECT_URL` and `PROJECT_TOKEN` must be set manually for `project.yml` and the
  scheduled roadmap sync in `docs.yml`; until then `project.yml` skips and the docs build
  falls back to closed epics and release tags. `PROJECT_TOKEN` needs the `project` scope,
  plus repository write access if it is used to open the `roadmap/sync` pull request.
  The sync is described in
  [`tree/README.md`](https://github.com/irix7/os/blob/main/tree/README.md).
- The default CodeQL setup is used; do not add a custom `codeql.yml`.

## 17. Free-feature matrix

| Feature | Free tier | How IRIX 7 uses it |
|---|---|---|
| Public repositories | Unlimited | `os`, `project`, `iris`, `gcc`, `binutils-gdb`, `ghidra`, `harness` |
| Private repositories | Unlimited private repos, limited features | `irix6` only |
| Issues | Unlimited | Every task, bug, research question and ADR |
| Issue forms | Free | Eight forms in `.github/ISSUE_TEMPLATE/` |
| Sub-issues | Free (if enabled) | Epic breakdown |
| Labels | Unlimited | Area/type/priority/size/risk/hardware/status |
| Milestones | Unlimited | v0.1–v1.0 |
| Projects v2 | Free | Roadmap, board, table, custom fields, workflows |
| Discussions | Free | Q&A, announcements, hardware, research |
| Wiki | Free | Lab notebook and meeting notes |
| Pages | Free for public repos | mdBook documentation site |
| Actions (public) | Unlimited standard runners | CI, security, docs, release |
| Actions (private) | 2,000 minutes/month | `irix6` has no Actions |
| Self-hosted runners | Free | Real-Indy hardware tests |
| Actions cache | 10 GB/repo | Cargo, Nix, iris |
| Artifacts | Free tier storage | Cross-build outputs, boot logs |
| Releases | Free, 2 GB/file | Kernel, bundle, image, SBOM, signatures |
| Packages | Free for public | Optional MCP client/container |
| Dependabot | Free | Dependency updates |
| CodeQL | Free for public | Rust/Actions static analysis |
| Secret scanning | Free for public | Secrets and push protection |
| Dependency review | Free for public | Block vulnerable PRs |
| Scorecard | Free | Supply-chain score |
| SBOM | Free | Release artefacts |
| Artifact attestations | Public preview | Provenance |
| Private vulnerability reporting | Free | Security reports |
| Security advisories | Free | Coordinated disclosure |
| Rulesets | Free for public | Protect `main` |
| Codespaces | 120 core-hours/month | Docs and onboarding |
| GitHub CLI | Free | Bootstrap, triage, releases |
| GitHub Mobile | Free | Triage and notifications |
| Sponsors/FUNDING | Free | Optional sustainability |

Not used: Copilot paid tiers, Advanced Security for private repos, Enterprise, merge queue if
it is paid-only, GitHub Models for project management.

## 18. Setup order

1. `scripts/github/bootstrap.sh` — labels, milestones, project, fields, epics.
2. Enable Discussions and Wiki; create categories.
3. Create the Project views in the UI (the GraphQL view API is not exposed by `gh project`).
4. Create the `main` ruleset: require PR, 1–2 approvals, linear history, required checks,
   no force-push, no deletion.
5. Enable CodeQL default setup, Dependabot alerts, secret scanning, push protection.
6. Enable Pages with GitHub Actions as the source; run `docs.yml`.
7. Create the `release` and `hardware` environments with protection rules and secrets.
8. Register the self-hosted `indy` runner.
9. Create the first epic issues and add them to the project (the bootstrap does this).
10. Post an announcement in Discussions with the plan links.
