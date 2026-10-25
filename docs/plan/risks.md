# IRIX 7 — Risk Register and Decision Backlog

**Status:** living document
**Review cadence:** monthly, and at every milestone boundary.
**Scoring:** likelihood 1–5 × impact 1–5. **High** ≥ 15, **medium** 8–14, **low** ≤ 7.

## 1. Top risks

1. **Scope and bus factor.** The native plan is 76–118 person-weeks and the
   architecture adds 60–95, before the compatibility library surface is fully counted.
   A single maintainer is the largest existential risk.
2. **No Rust `no_std` runtime has full live-stack migration.** wasmi has no
   serialisable call stack; wasm3 is C. v1 is deliberately L0 only.
3. **The compatibility container is bigger than it looks.** libc, Motif, GL, dmedia, the
   IRIX desktop and the Xlib ABI are not yet estimated. This is the project's identity and
   its longest pole.
4. **`fork` in a single address space.** A plain copy is not viable for large cells on a
   32–256 MiB Indy. The native process model needs a decision before the native process phase.
5. **A driver bug is a kernel bug.** De-privileged cells with raw DMA and no full IOMMU mean
   a bad SCSI, Seeq or Vino driver can corrupt the kernel. v1 may panic on a driver fault.
6. **MCP is full root, no audit, HTTP and a bearer token.** A prompt injection or a LAN
   attacker can reset or overwrite the machine. Acceptable for a development harness, not
   for a product control plane.

## 1.1 Architecture risks

| ID | Risk | L | I | Score | Mitigation |
|---|---|---:|---:|---:|---|
| R21 | 128-node membership/failure detection produces gossip storms or false failures | 5 | 5 | 25 | SWIM-style O(log n), immutable epochs, bounded summaries, simulator/fault injection |
| R22 | A single hot home overloads; home movement loses data | 4 | 4 | 16 | Rendezvous homes, route caches, leases/epochs, forwarding; split objects in v2 |
| R23 | Threshold authority colludes or loses quorum | 3 | 5 | 15 | Independent operators, 3-of-5/5-of-7, audit, root rotation; document the trust anchor |
| R24 | Global namespace path sees mixed versions | 4 | 4 | 16 | Per-directory linearisability, epoch caches, `EXDEV` for cross-home rename |
| R25 | Stale route or moved object commits twice | 4 | 5 | 20 | TTL/hop bound, home epoch, lease fencing, one `MOVED` retry |
| R26 | Wasmi cannot build or perform on R4600 | 4 | 5 | 20 | Phase 4 build gate; native data plane; fallback to wasm3/Rust port/native core |
| R27 | 128-node test fidelity is weak | 5 | 4 | 20 | 8–16 QEMU + simulator, fault injection, real 2–8 nodes |
| R28 | Capability root custody without a TPM | 4 | 5 | 20 | Ephemeral/provisioned keys, short expiry, threshold root, physical security |
| R29 | L2 migration is assumed but unsolved | 4 | 5 | 20 | L0 v1; fund a separate runtime project; state the limitation |
| R30 | Native POSIX/IRIX regression during extraction | 4 | 4 | 16 | Native fast paths, compatibility suite at every phase, rollback image |

The architecture also inherits the native risks below.

## 2. Register

| ID | Risk | Category | L | I | Score | Mitigation | Trigger | Owner | Status |
|---|---|---|---|---|---|---|---|---|---|
| R1 | Scope exceeds available time; the project stalls like Harvey/Akaros | Schedule | 5 | 5 | 25 | Walking skeleton; v1 scope cuts; milestone demos; bus-factor plan | Two milestones slip by > 50% | maintainer | open |
| R2 | Compat container (libc/Motif/GL/dmedia/Xlib) underestimated | Technical | 5 | 5 | 25 | Separate compat spec; estimate before the compatibility phase; phase X11 lease first; defer GL/dmedia | Compatibility phase slips or libc grows past its estimate | compat | open |
| R3 | `fork` in a SAS kernel is too expensive or unsafe | Technical | 4 | 4 | 16 | Native `fork` limited to small cells; `posix_spawn`/`exec` preferred; ADR before the native process phase | A cell > 8 MiB cannot fork | kernel | open |
| R4 | Driver bug corrupts the kernel (raw DMA, no full IOMMU) | Security | 4 | 5 | 20 | Documented `unsafe`; DMA TLB confinement where possible; watchdog; driver restart later | A driver fault panics the kernel | drivers | accepted |
| R5 | MCP prompt injection or LAN attacker controls the machine | Security | 5 | 4 | 20 | Bearer token; trusted LAN/SSH tunnel; watchdog; production-mode capability scoping later | An unexpected MCP reset or file write | security | accepted |
| R6 | `-Z build-std` glue or GCC backend never unblocks | Toolchain | 4 | 5 | 20 | The toolchain gate is first; fallback to LLVM path; dedicated toolchain owner | Toolchain gate slips > 4 weeks | toolchain | open |
| R7 | R2/GitHub outage or quota exhaustion blocks boot/update | Operations | 3 | 5 | 15 | Local cache; retries; mirror; digest verification; quota monitoring | Boot fails twice from R2 | infra | open |
| R8 | Real Indy hardware fails (PSU, SCSI, RTC, RAM) | Hardware | 4 | 4 | 16 | Two machines; spares; recapping; BlueSCSI/ZuluSCSI; backups; lab checklist | A machine stops booting | hardware | open |
| R9 | Clean-room contamination reaches `irix7/os` | Legal | 3 | 5 | 15 | Harness, similarity screen, DCO, access control, review gate | Similarity screen flags a PR | clean-room | open |
| R10 | "IRIX 7" name or SGI marks create a legal problem | Legal | 2 | 5 | 10 | Trademark review; disclaimer; no SGI endorsement; rename option | A takedown or objection | legal | open |
| R11 | Native desktop does not feel like IRIX | Product | 3 | 3 | 9 | UX brief; Indigo Magic study; prototype early; user testing | Users say it feels generic | display | open |
| R12 | Too little RAM for the full stack on 32–64 MiB Indys | Performance | 3 | 4 | 12 | Pick a 64 MiB floor; lite mode; lazy loading; no swap v1 | OOM at boot on a real Indy | kernel | open |
| R13 | XFS/EROFS/I7FS bug causes data loss | Data | 3 | 5 | 15 | Read-only first; fsck oracles; golden images; no write until trusted | A corrupt image or wrong file | storage | open |
| R14 | Supply-chain or licence violation in a dependency | Legal | 3 | 4 | 12 | `cargo-deny`, SBOM, licence gate, pinned versions, review | CI licence failure | infra | open |
| R15 | iris diverges from real hardware | Technical | 4 | 3 | 12 | Hardware-in-the-loop tests; differential tests; real-Indy milestones | A demo works on iris but not on Indy | hardware | open |
| R16 | GitHub/R2 free-tier limits change or are exceeded | Operations | 2 | 4 | 8 | Cache; mirrors; quota alerts; self-hosted where possible | A quota warning or block | infra | open |
| R17 | Nightly rustc drift breaks the build | Toolchain | 4 | 3 | 12 | Pin the nightly; upgrade deliberately; CI matrix | A nightly upgrade breaks the cross-build | toolchain | open |
| R18 | Documentation drifts from the code | Docs | 3 | 2 | 6 | Docs in the same PR; mdBook build; link check | A spec contradicts the code | docs | open |
| R19 | Real-Indy lab access is unavailable | Hardware | 2 | 4 | 8 | Emulator-first; remote serial/power; spares | Cannot run a hardware milestone | hardware | open |
| R20 | No second maintainer or reviewer | Community | 5 | 4 | 20 | Good-first issues; docs; ADRs; onboarding; pair reviews | PRs sit unreviewed for > 1 week | maintainer | open |

## 3. Risk process

- **Monthly review:** update L/I, close resolved risks, add new ones.
- **Milestone review:** check the top five and the triggers.
- **Escalation:** a high risk with a triggered condition becomes a P0 issue and is discussed
  in the next weekly review.
- **Acceptance:** a risk may be accepted explicitly (for example R4 and R5). Accepted risks
  are recorded in the relevant ADR and are not silently ignored.
- **Ownership:** every risk has an owner (a person or a team). An unowned risk is a gap.

## 4. Decision backlog

The architecture decisions D1–D31 in
[`../spec/architecture.md`](../spec/architecture.md) §2.2 become **ADR-0021 onward** in
`irix7/project/docs/adr/`.

Suggested grouping:

| ADR range | Topic |
|---|---|
| 0021–0023 | Identity, native ABI, process model |
| 0024–0027 | Framekernel, hybrid isolation, capabilities, IPC |
| 0028–0030 | Memory, DMA, scheduler |
| 0031–0033 | Drivers, boot, storage |
| 0034–0036 | Display, X11, audio/video |
| 0037–0038 | MCP, Plan 9 selectivity |
| 0039–0040 | Hardware target, compatibility scope |

The open questions in §17 of the spec become ADRs when settled; until then they are
`type/adr` + `status/needs-adr` issues on the Project.

## 5. ADR template

```markdown
# ADR-00xx — Title

**Status:** proposed | accepted | rejected | superseded
**Date:** YYYY-MM-DD
**Deciders:** ...
**Related:** ...

## Context
What problem are we solving? What constraints apply?

## Decision
What we will do.

## Consequences
What becomes easier, harder, more expensive or impossible.

## Alternatives considered
What else was on the table, and why it lost.

## Evidence
Links to research, prototypes, benchmarks or hardware tests.
```

## 6. Bus-factor plan

- Every subsystem has a written spec and a "start here" issue.
- Every non-obvious decision is an ADR.
- Every build is reproducible from a clean checkout.
- The MCP harness lets a new contributor inspect a running system without private context.
- The hardware lab has a checklist so a second person can reproduce a result.
- The private `irix6` repo is not on the critical path for day-to-day work.
