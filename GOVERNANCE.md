# Governance

## Roles

| Role | Responsibilities |
|---|---|
| Maintainer | Direction, releases, security, final decisions |
| Reviewer | Review in an area, triage |
| Clean-room reviewer | Similarity screen and provenance |
| Hardware maintainer | Lab, machines, rig, spares |
| Contributor | Issues, PRs, docs, tests |
| User | Bug reports and feedback |

The current maintainers are listed in [MAINTAINERS.md](MAINTAINERS.md).

## Decision process

1. **Lazy consensus** for small changes: a PR with the required approvals and no objection
   within 72 hours is accepted.
2. **ADR** for architecture, cross-boundary interfaces, security or licence changes. The ADR
   lives in `irix7/project/docs/adr/`; the template is in
   [`docs/plan/risks.md`](docs/plan/risks.md).
3. **Maintainer decision** for direction, releases, conduct and security, after hearing the
   community.
4. **No silent decisions.** If it is not in an issue or an ADR, it did not happen.

## Maintainers

- A new maintainer is nominated by an existing maintainer and approved by lazy consensus of
  the current maintainers.
- A maintainer may step down at any time.
- Inactive maintainers (no project activity for 6 months) become emeritus.

## Releases

- Releases follow the checklist in [`docs/plan/release.md`](docs/plan/release.md).
- A release requires a maintainer approval and the required CI gates.
- Release tags are immutable.

## Code of conduct

Enforcement follows [CODE_OF_CONDUCT.md](CODE_OF_CONDUCT.md). A maintainer with a conflict of
interest recuses themselves.

## Amendments

This document changes by a PR approved by the maintainers, with a 7-day comment period.
