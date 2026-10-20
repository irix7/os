# IRIX 7 — Community and Governance

**Status:** living document
**Scope:** code of conduct, roles, decision process, contribution workflow, review,
communication, recognition and the bus-factor plan.
**Related:** [`github.md`](github.md), [`legal.md`](legal.md), [`risks.md`](risks.md).

## 1. Code of conduct

The project adopts the **Contributor Covenant 2.1** (or the current version) as
`CODE_OF_CONDUCT.md`. Enforcement:

- report to the maintainers via the contact in `CODE_OF_CONDUCT.md`;
- a maintainer acknowledges within 72 hours;
- sanctions range from a warning to a permanent ban;
- reports are confidential; the reporter is protected from retaliation.

## 2. Roles

| Role | Responsibilities | Rights |
|---|---|---|
| **Maintainer** | Direction, releases, final decisions, security | Admin on the repos |
| **Reviewer** | Review in an area, triage | Write/triage in the area |
| **Clean-room reviewer** | Similarity screen, provenance | Read `irix6`, review kernel PRs |
| **Hardware maintainer** | Lab, machines, rig, spares | Access to the lab runner |
| **Contributor** | Issues, PRs, docs, tests | Read/write via fork |
| **User** | Bug reports, feedback | Issues/Discussions |

Start with a small set; add roles as the project grows. The current maintainer is the project
owner.

## 3. Decision process

1. **Lazy consensus** for small changes: a PR with the required approvals and no objection
   within 72 hours is accepted.
2. **ADR** for architecture, cross-boundary interfaces, security or licence changes: a
   `type/adr` issue, a draft ADR, review, then accepted/rejected.
3. **Maintainer decision** for direction, releases, code of conduct and security, after
   hearing the community.
4. **No silent decisions.** A decision that is not in an issue or an ADR does not exist.

## 4. Communication

- **GitHub Issues** for tracked work.
- **GitHub Discussions** for Q&A, ideas, show-and-tell and hardware.
- **Monthly update** in Discussions: what shipped, what is next, what is blocked.
- **No private DMs for project decisions**; keep the record public unless it is a security or
  conduct matter.
- **Security** reports go through `SECURITY.md`, not public issues.

## 5. Contribution workflow

1. Pick an issue (or open one). Good first issues are labelled.
2. Discuss the approach if it is large or crosses a boundary.
3. Write an ADR if the decision is architectural.
4. Fork, branch, implement, test, document.
5. `git commit -s` (DCO).
6. Open a PR with the template.
7. CI runs; a reviewer approves; the clean-room screen runs if needed.
8. Squash merge; the issue closes; the Project updates.

## 6. Review expectations

| Change | Approvals |
|---|---|
| Docs, typos, small tests | 1 |
| Feature/bug in one area | 1 area reviewer |
| Kernel, unsafe, capability, security | 2, one maintainer |
| Clean-room-sensitive | 1 clean-room reviewer |
| Release | 1 maintainer + release checklist |

Reviewers aim to respond within 5 working days. A PR without a response after 10 days is
escalated in the monthly update.

## 7. Good first issues

- Docs, tests, small drivers, tooling, fuzzing, benchmarks.
- Each has a clear file, a definition of done and a mentor.
- The `good first issue` label is kept accurate.

## 8. Recognition

- Contributors are credited in the release notes.
- Significant contributions are recognised in the monthly update.
- No unpaid labour is assumed; the project thanks people explicitly.

## 9. Release cadence

- Milestone releases (v0.1–v1.0) when the demo and gates pass.
- Nightly builds for maintainers.
- Security releases as needed.
- No fixed calendar until v1.0; the milestone plan is the cadence.

## 10. Bus factor

The project currently has one maintainer. Mitigations:

- every subsystem has a spec and a "start here" issue;
- every non-obvious decision is an ADR;
- the build is reproducible;
- the MCP harness lets a new contributor inspect a running system;
- the hardware lab has a checklist;
- the private `irix6` repo is not on the critical path for day-to-day work;
- a second reviewer is recruited before v0.4.

## 11. Website and demos

- GitHub Pages hosts the docs (mdBook).
- A short demo video/GIF per milestone in Discussions.
- Screenshots of the native desktop and X11 on real hardware.
- No tracking, no ads.

## 12. Open questions

- The final code of conduct version and enforcement contact.
- Whether to create a Discord/Matrix channel (Discussions is the default).
- The maintainer succession plan.
- The first second maintainer.
