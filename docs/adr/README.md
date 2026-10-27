# Architecture decision records — M2 and M3 working series

**Status:** living index
**Scope:** the decisions M2 needed for join, transport, route-cache, migration and failure,
and the first M3 shard and multi-member decisions.

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
| [M2-ADR-0005](m2-0005-join-epoch.md) | A signed join handshake, an authority-confirmed epoch and explicit restart rules | Accepted for M2, limited |
| [M2-ADR-0006](m2-0006-homes.md) | Rendezvous homes from the confirmed snapshot, with a bounded MCP evidence surface | Accepted for M2, limited |
| [M2-ADR-0007](m2-0007-route.md) | Remote capability routing, home leases and the one-`MOVED` retry | Accepted for M2, limited |
| [M2-ADR-0008](m2-0008-failure-fence.md) | Bounded failure detection, lease fencing and the no-second-home partition rule | Accepted for M2, limited |
| [M2-ADR-0009](m2-0009-actor-migration.md) | L0 stackless-actor state, root-signed move and the message-boundary lease protocol | Accepted for M2, limited |
| [M3-ADR-0001](m3-0001-shard-slice.md) | First sharded home slice: one rendezvous table, signed epoch refresh, two-kernel real limit | Accepted for this slice, limited |
| [M3-ADR-0002](m3-0002-multi-node-hub.md) | Bounded multi-member host hub, guest receive-path ordering and real multi-kernel convergence | Accepted for this slice, limited |

The practical M2 runbook is [`../plan/m2-rig.md`](../plan/m2-rig.md); the M3 first-slice
runbook is [`../plan/m3-rig.md`](../plan/m3-rig.md). The harness is
[`tools/m2-rig`](../../tools/m2-rig/README.md).

## Template

Use the template in [`../plan/risks.md`](../plan/risks.md) §5. Every ADR records context,
decision, consequences, alternatives and evidence. A decision without evidence is a
proposal, not an ADR.

- [M2-ADR-0010 — direct peer transport](m2-0010-direct-peer-transport.md)

- [M3-ADR-0003 — bounded mesh streams](m3-0003-multiplexed-streams.md)

- [M3-ADR-0004 — readiness-driven module I/O](m3-0004-event-driven-io.md)

- [M3-ADR-0005 — eight module TCP slots](m3-0005-multi-connection-transport.md)
