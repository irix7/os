# IRIX 7 — Project Plan

**Status:** living document
**Purpose:** turn the system architecture in [`../spec/system.md`](../spec/system.md) and the hardware/compatibility layers in [`../spec/architecture.md`](../spec/architecture.md) into an executable plan.
**Scope:** the project operating system around the technical system: milestones, GitHub, risks, interfaces, hardware, testing, security, release, legal, community and cost.

The system architecture answers *what* IRIX 7 is; the hardware/compatibility spec covers the native layers. This directory answers *how the project gets built, tracked and shipped*.

## Documents

| Document | What it covers |
|---|---|
| [`milestones.md`](milestones.md) | **Active plan:** the 1–128-node logical computer, phases 0–9, M1–M4 gates, native compatibility and rollback. |
| [`m2-rig.md`](m2-rig.md) | **M2 runbook:** two independent iris kernels, the host relay path, readiness, the two-way probe and the M2-1 handoff. |
| [`github.md`](github.md) | Organisation, repositories, Projects v2, labels, templates, automation, Actions, security and every free GitHub feature we use. |
| [`risks.md`](risks.md) | The risk register, decision backlog and review process. |
| [`interfaces.md`](interfaces.md) | Index of interface specifications and their status. |
| [`hardware-lab.md`](hardware-lab.md) | The two real Indys, the rig, spares, safety and hardware-in-the-loop testing. |
| [`testing.md`](testing.md) | Test pyramid, corpus, CI, fuzzing, conformance and benchmarks. |
| [`security.md`](security.md) | Threat model, trust boundaries, MCP development/production modes, supply chain and disclosure. |
| [`release.md`](release.md) | Versioning, channels, artefacts, image pipeline, signing, A/B, rollback and support. |
| [`legal.md`](legal.md) | GPL-3.0, clean-room, provenance, DCO, trademarks and privacy. |
| [`community.md`](community.md) | Contributing, governance, maintainers, communication and recognition. |
| [`cost.md`](cost.md) | Free tiers, quotas, budget and scaling. |

## How the project works

The live Project is [IRIX 7 — Roadmap #2](https://github.com/orgs/irix7/projects/2). It is
created and refreshed by [`../../scripts/github/bootstrap.sh`](../../scripts/github/bootstrap.sh).

1. **Walking skeleton first.** Every milestone is a vertical slice with a demo. No horizontal platform work that cannot be demonstrated.
2. **The MCP loop is the development method.** Edit on the host, build, deploy to the Indy (or iris), boot, observe through MCP, repeat.
3. **Every issue is tracked.** An issue has a milestone, area, priority, size, risk and hardware field in the GitHub Project.
4. **Every decision becomes an ADR.** The architecture decisions D1–D31 in `../spec/architecture.md` §2.2 become ADR-0021 onward. Open questions become ADRs when settled.
5. **Clean-room is a gate, not a slogan.** The harness, the similarity screen and the provenance checklist apply to every contribution.
6. **Benchmarks inform; they do not gate.** Functional tests and hardware validation are the gates.
7. **One maintainer can stop the project.** The bus-factor plan is part of the risk register; documentation and reproducible builds are survival equipment.

## Definition of done

A milestone is done when:

- the demo runs on iris **and** the target real Indy (or the limitation is recorded);
- the acceptance criteria are met;
- the code has tests, documentation and a licence/provenance check;
- the relevant ADR or interface spec is updated;
- the MCP loop can reproduce the demo from a clean checkout;
- the next milestone's dependencies are unblocked.

## Cadence

- **Weekly:** review the Project board, unblock, update risks.
- **Per milestone:** demo, retrospective, update this plan.
- **Per release:** changelog, notes, artefacts, SBOM, signature, announcement.

## Revision

- 2026-10-20 — initial plan after the advanced-architecture grilling.
- 2026-10-25 — the plan now describes the 1–128-node Wasm/mesh logical computer; native POSIX/IRIX and ABI 10 compatibility remain workstreams; ABI 11 adds block_write.
- 2026-10-26 — M2 starts on two independent iris kernels: the [M2 rig runbook](m2-rig.md) and the [M2 ADR series](../adr/README.md) record the transport, 1-of-1 authority and native stackless actor decisions.
