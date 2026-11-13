# Architecture decision records — working series

This index is a living document. It covers the toolchain decision that unblocked M0 (the cross-build and first-boot milestone), the decisions M2 (the two-node milestone gate) needed for join, transport, route caching, migration and failure, and the first M3 (the eight-node milestone gate) decisions on sharded homes and multi-member membership. Project terms such as home, epoch and iris are defined in the [glossary](../introduction/glossary.md).

The project's main ADR series starts at **ADR-0021** and covers the architecture decisions D1–D31 listed in [`docs/internals/architecture.md`](../internals/architecture.md) §2.2; [`docs/project/risks.md`](../project/risks.md) §4 explains the numbering. The records below carry a milestone prefix (**M0-ADR**, **M2-ADR**, **M3-ADR**) so they can be reviewed as a group before they claim a permanent number. When a record is promoted to the main series it keeps its title and takes the next free global number.

| ADR | Decision | Status |
|---|---|---|
| [M0-ADR-0001](m0-0001-llvm-only.md) | LLVM is the only Rust code generation backend; ADR-0018 is amended | Accepted |
| [M2-ADR-0001](m2-0001-two-node-rig.md) | Two independent iris instances, a Rust host relay, and a repeatable two-way probe | Accepted for M2 |
| [M2-ADR-0002](m2-0002-transport.md) | TCP with length-prefixed `MeshEnvelope` framing; TLS deferred | Accepted for M2 |
| [M2-ADR-0003](m2-0003-authority.md) | A 1-of-1 authority root for the M2 membership epoch | Accepted for M2, limited |
| [M2-ADR-0004](m2-0004-actor-runtime.md) | Native stackless actors as the M2 runtime substrate; Wasm deferred to Phase 4 | Accepted for M2 |
| [M2-ADR-0005](m2-0005-join-epoch.md) | A signed join handshake, an authority-confirmed epoch and explicit restart rules | Accepted for M2, limited |
| [M2-ADR-0006](m2-0006-homes.md) | Rendezvous homes from the confirmed snapshot, with a bounded MCP evidence surface | Accepted for M2, limited |
| [M2-ADR-0007](m2-0007-route.md) | Remote capability routing, home leases and the one-`MOVED` retry | Accepted for M2, limited |
| [M2-ADR-0008](m2-0008-failure-fence.md) | Bounded failure detection, lease fencing and the no-second-home partition rule | Accepted for M2, limited |
| [M2-ADR-0009](m2-0009-actor-migration.md) | L0 stackless-actor state, root-signed move and the message-boundary lease protocol | Accepted for M2, limited |
| [M2-ADR-0011](m2-0011-one-computer-console.md) | Named-object route operations and one prompt over the M2 object/actor surfaces | Accepted for M2, limited |
| [M3-ADR-0001](m3-0001-shard-slice.md) | First sharded home slice: one rendezvous table, signed epoch refresh, two-kernel real limit | Accepted for this slice, limited |
| [M1-ADR-0002](m1-0002-network-kernel-download.md) | ROM downloads the kernel from GitHub; kernel leaves the PROM; no integrity check (accepted risk) | Accepted, limited |
| [M3-ADR-0002](m3-0002-multi-node-hub.md) | Bounded multi-member host hub, guest receive-path ordering and real multi-kernel convergence | Accepted for this slice, limited |

The M2 runbook is [`docs/project/m2-rig.md`](../project/m2-rig.md) and the M3 first-slice runbook is [`docs/project/m3-rig.md`](../project/m3-rig.md). Both run on the [`tools/m2-rig`](../../tools/m2-rig/README.md) harness.

## Template

Use the template in [`docs/project/risks.md`](../project/risks.md) §5. Every ADR records context, decision, consequences, alternatives and evidence. A record with no evidence is filed as a proposal and is not accepted as an ADR.

Further records in this series:

- [M2-ADR-0010: direct peer transport](m2-0010-direct-peer-transport.md)
- [M2-ADR-0011: one prompt over the M2 object and actor surfaces](m2-0011-one-computer-console.md)
- [M3-ADR-0003: bounded mesh streams](m3-0003-multiplexed-streams.md)
- [M3-ADR-0004: readiness-driven module I/O](m3-0004-event-driven-io.md)
- [M3-ADR-0005: eight module TCP slots](m3-0005-multi-connection-transport.md)
- [M3-ADR-0006: module UDP services and the two-kernel datagram gate](m3-0006-udp-transport.md)
