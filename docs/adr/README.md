# Architecture decision records — M2 working series

**Status:** living index
**Scope:** the decisions M2 needs before the join, transport, route-cache, migration and
failure work can start.

The project's main ADR series is numbered from **ADR-0021** onward for the architecture
decisions D1–D31 in [`../spec/architecture.md`](../spec/architecture.md) §2.2; see
[`../plan/risks.md`](../plan/risks.md) §4. The M2 decisions below are deliberately
prefixed **M2-ADR** so they can be reviewed as a group without claiming a permanent
number. When a decision is promoted to the main series it keeps its title and gains the
next free global number.

| ADR | Decision | Status |
|---|---|---|
| [M2-ADR-0001](m2-0001-two-node-rig.md) | Two independent iris instances, a Rust host relay, and a repeatable two-way probe | Accepted for M2 |
| [M2-ADR-0002](m2-0002-transport.md) | TCP with length-prefixed `MeshEnvelope` framing; TLS deferred | Accepted for M2 |
| [M2-ADR-0003](m2-0003-authority.md) | A 1-of-1 authority root for the M2 membership epoch | Accepted for M2, limited |
| [M2-ADR-0004](m2-0004-actor-runtime.md) | Native stackless actors as the M2 runtime substrate; Wasm deferred to Phase 4 | Accepted for M2 |

The practical runbook is [`../plan/m2-rig.md`](../plan/m2-rig.md). The harness is [`tools/m2-rig`](../../tools/m2-rig/README.md).

## Template

Use the template in [`../plan/risks.md`](../plan/risks.md) §5. Every ADR records context,
decision, consequences, alternatives and evidence. A decision without evidence is a
proposal, not an ADR.
