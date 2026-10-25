# M2-ADR-0001 — Two independent iris instances and a Rust host relay

**Status:** accepted for M2
**Date:** 2026-10-26
**Deciders:** M2-0, for review at the M2 gate
**Related:** [M2-ADR-0002](m2-0002-transport.md), [M2-ADR-0003](m2-0003-authority.md),
[`../plan/m2-rig.md`](../plan/m2-rig.md), [`tools/m2-rig`](../../tools/m2-rig/README.md)

## Context

M2 is the first gate that needs **two independent kernels**. The M1 gate is a single
logical computer: one kernel, one membership replica, every home local, and an asserted
zero mesh traffic. M2 must join a second replica, agree a membership epoch, route a
capability to a remote home, migrate an L0 actor in both directions, and define the
outcome of one-node failure and partition.

The emulator is the primary development machine. Its instance mode is explicitly **one
`Machine` per process**: idle parking, CPU affinity, the dev log and the CI socket path
are process-global, so two `Machine`s in one process would fight. The supported shape is
therefore N processes, each started with `--instance N`, which derives:

- a private state directory (`iris-instance-N` by default), NVRAM/EEPROM, snapshot store,
  COW/CHD directories, JIT cache, serial log, crash log and CI socket;
- a disjoint monitor/serial/CI port block (`9000 + N*10` by default);
- a stable SGI-OUI MAC from FNV-1a over the instance bytes;
- a per-instance NAT subnet `192.168.{N % 256}.0/24`, with the guest at `.2` and the
  gateway at `.1`.

The last point is the hard one. Each iris process runs its own **software NAT gateway**.
There is no shared L2 segment, no broadcast or ARP between the two guests, and no shared
memory between the two processes. A guest can address its own gateway, and the NAT maps a
connection to that gateway onto host loopback, but it cannot address the other guest's
`192.168.1.2` directly. Any claim of two-node connectivity has to demonstrate the actual
host path, not a logical one.

The project also has a policy simulator, `mesh-sim`. It is valuable for membership and
rendezvous policy, but it is explicitly **not** evidence of two kernels talking. M2-0
must not use it as a substitute.

## Decision

1. **Two `iris` processes, one per node.** The rig starts `iris --instance 0` and
   `iris --instance 1` with separate state directories, NVRAM images, MACs, NAT subnets,
   CI sockets, monitor ports and serial logs. It never starts one kernel and presents it
   as two.

2. **A Rust orchestrator, `tools/m2-rig`.** It derives the two instances through the
   emulator's own `--print-instance` surface, writes each DS1386 image, starts the two
   processes, waits for their CI sockets, starts the CPUs, waits for `BOOT-READY` in each
   serial transcript, checks the identity markers, probes both directions, stops the
   nodes and writes a self-contained evidence bundle.

3. **Explicit host forwards.** Each node's generated `iris.toml` adds three
   `[[port_forward]]` rules: guest 23 (kernel telnet console), guest 80 (MCP status page)
   and guest 7000 (the future mesh listener). The host-side ports are derived from the
   rig's base port:

   | Node | Monitor | Serial A/B | Telnet | MCP HTTP | Mesh |
   |---|---|---|---|---|---|
   | `n0` | 9100 | 9101/9102 | 9700 | 9701 | 9702 |
   | `n1` | 9200 | 9201/9202 | 9710 | 9711 | 9712 |

   The CI sockets are `<state-dir>/iris.sock`; they are the control plane, not the data
   plane.

4. **A local module fixture.** The kernel autoloads MCP from `mcp.url` in NVRAM. The rig
   serves a **freshly built** `mcp.module` from a loopback HTTP fixture and points each
   guest at its own gateway, e.g. `http://192.168.0.1:<fixture>/mcp.module` for `n0` and
   `http://192.168.1.1:<fixture>/mcp.module` for `n1`. Each NAT maps the gateway address
   to host loopback, so both guests fetch the same fixture without a shared subnet. The
   module's SHA-256 is checked by the kernel as usual.

   The checked-in `boot/modules/v0.1.3/mcp.module` is **not** used for the probe. It
   loads and prints `mcp: listening on port 80`, but its HTTP listener did not answer a
   `GET /` in this environment. A module built from the current tree does. The rig
   therefore treats the module as a build artefact of the current branch, not as a
   released constant.

5. **A Rust TCP relay is the chosen data path.** The probe path is:

   ```text
   guest n0 HTTP client
     -> n0 NAT (gateway 192.168.0.1:<relay port>)
     -> host 127.0.0.1:<relay port>
     -> Rust relay
     -> host 127.0.0.1:9711 (n1's port forward)
     -> n1 guest:80 (MCP)
   ```

   and the reverse through `192.168.1.1:<relay port>` to `127.0.0.1:9701`. The relay is a
   transparent byte pump. It is **not** a directory, membership service, protocol
   translator or message broker; it has no knowledge of node identity or mesh state. It
   exists because the two NATs are isolated and because the direct NAT-to-forward path
   resets or loses early data (below).

6. **Node identity is the MAC.** `kernel/src/mesh.rs` builds the 16-byte `NodeId` from
   the first six bytes of the DS1386 station address and zero padding. The rig derives
   the MAC through `--print-instance`, writes the same MAC into the NVRAM image, and
   asserts that the guest's `mesh: single node` line matches. This is the identity M2-1
   will use in `MeshHeader.src`/`dst`. It is not a random key and not a secret.

7. **Readiness is a serial marker plus the control socket.** `iris-ci status
   --wait-ready` proves the monitor is alive; `BOOT-READY ... network=up mcp=listening
   mesh=single` proves the kernel, DHCP, module load and mesh bootstrap are done. The rig
   does not infer readiness from a process being alive or a port being bound.

8. **Start/stop is explicit and repeatable.** `up` starts both and leaves them; `status`
   reports CI state and boot markers; `probe` runs the two-way check against running
   nodes; `down` sends `iris-ci quit` and waits for the child; `run` does all four and
   writes the evidence bundle.

## Consequences

- The rig is reproducible on a normal host account: no root, no `CAP_NET_RAW`, no pcap
  bridge, no host network reconfiguration. It needs one `iris` binary with instance
  support and one `iris-ci`.
- The two guests are genuinely separate processes with separate kernels, NVRAM, MACs,
  NATs and control sockets. A crash or reboot of one does not touch the other.
- The relay is a single host process and a single point of failure. It is acceptable for
  the M2 laboratory rig; it is not the production data path. A future real-LAN or
  two-machine Indy deployment uses the peer's real address and no relay.
- The relay's probe mode closes a target-to-client direction after a **5 s quiet
  period** because a guest MCP listener keeps its HTTP/1.0 side open after the response.
  That is a probe convenience, not a mesh transport contract. M2-1 must not reuse the
  probe relay for a persistent mesh connection without a longer idle timeout or a
  heartbeat.
- There is no broadcast, multicast or ARP between guests. Discovery and gossip must use
  the configured peer addresses (or a later rendezvous mechanism). `mesh-sim` remains a
  policy test only.
- The fixed host ports make the rig easy to inspect but can collide with another run.
  `--base-port` moves the block; the README documents the mapping.

## Direct-path observation

The first attempt used the guest's NAT gateway to connect directly to the peer's host
forward, without a relay:

```text
guest n0 -> 192.168.0.1:9711 -> host 127.0.0.1:9711 -> n1 guest:80
```

It is not reliable. The peer's forward accepts the host connection and opens a
guest-side connection, but data that arrives before that guest-side connection is ready
can be reset or dropped. The evidence is in the M2-0 report: a direct request produced
`http: receive timed out`, while the same request through the relay produced
`http: 200 38 bytes`. A host-side relay with a separate target connection avoids the
race. The narrowest iris fix would be to buffer host bytes in the forward until the
guest-side TCP connection is established; M2-0 does not change iris. Until that fix
lands, the relay is the supported path.

## Alternatives considered

| Alternative | Why it lost |
|---|---|
| One kernel pretending to be two nodes | Violates the M2 acceptance criterion "two independent kernels"; would not exercise join, failure or migration. |
| `mesh-sim` as the transport | Policy simulator only; the M2 gate explicitly requires a real-kernel run. |
| PCAP bridge onto a host interface | The available iris binary has no `pcap` feature and the host would need `CAP_NET_RAW`/root; the two guests would also need a shared L2 segment. Not reproducible on the target host. |
| Host port forwarding only, no relay | The direct path resets/loses early data; the peer forward does not buffer reliably. The relay is the narrow workaround. |
| Shared memory between the guests | The guests are separate processes with separate address spaces and emulated RAM; there is no shared-memory transport in iris or the kernel. |
| QEMU instead of iris | The project's primary machine is iris; QEMU would test a different device model and not the Indy Seeq path. |

## Evidence

The harness is `tools/m2-rig`; the runbook is [`../plan/m2-rig.md`](../plan/m2-rig.md).
A successful `run` produced:

```text
n0 -> n1: pass (HTTP 200 from n0's HTTP client, 38 bytes)
n1 -> n0: pass (HTTP 200 from n1's HTTP client, 38 bytes)
n0: BOOT-READY v=1 boot=... kernel=up network=up mcp=listening mesh=single
n1: BOOT-READY v=1 boot=... kernel=up network=up mcp=listening mesh=single
```

The serial transcripts contain the identities:

```text
network: mac 08:00:69:95:F5:00
network: dhcp...ok, ip 192.168.0.2
mesh: single node 08006995F50000000000000000000000
BOOT-READY v=1 boot=... kernel=up network=up mcp=listening mesh=single
```

```text
network: mac 08:00:69:69:B6:01
network: dhcp...ok, ip 192.168.1.2
mesh: single node 08006969B60100000000000000000000
BOOT-READY v=1 boot=... kernel=up network=up mcp=listening mesh=single
```

The probe transcripts contain the cross-node request and response:

```text
sash> http 192.168.0.1:<relay>/
http: fetching
sash> http: 200 38 bytes
sha256: ...
```

```text
sash> http 192.168.1.1:<relay>/
http: fetching
sash> http: 200 38 bytes
sha256: ...
```

The evidence bundle also records the module fixture requests (two `GET /mcp.module 200
89640`), the relay byte counts in both directions, the NVRAM environment, and the PROM
and module SHA-256 digests. See the M2-0 report at `/tmp/opencode/m2-0-report.md` and
`target/m2-rig/evidence/` in the M2-0 worktree.
