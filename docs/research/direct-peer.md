# Direct peer transport for the mesh (#245)

**Status:** implemented; two-kernel direct and direct-fail evidence recorded.
**Date:** 2026-10-27
**Issue:** <https://github.com/irix7/os/issues/245>
**ADR:** [M2-ADR-0010](../adr/m2-0010-direct-peer-transport.md)

## 1. What changed

| Layer | Change |
|---|---|
| ABI (`irix7/os-sdk` v0.8.0) | `tcp_connect(slot, address, port)` appended at 0x70; `ABI_VERSION` 13; `Services` 0x74 bytes on n32. |
| Kernel | One `tcp_connect` service; a shared module socket factory for listen/connect; slot 1 keeps Nagle off and 1 ms ACK, slot 0 (HTTP) keeps the default; `embedded-tls` P-384 removed to fit the PROM. |
| Module | `mesh.direct`/`mesh.peer_id`; lower-NodeId initiates; 3 s listener settle, 5 × 500 ms attempts, then `direct fallback listen`; reconnect with bounded backoff. |
| Rig | `--transport relay\|direct\|direct-fail`; direct evidence from serial; `bulk` command; latency probe (`/mesh/latency`) recorded in `run.json`. |
| Docs | M2-ADR-0010; `docs/spec/modules.md` ABI v13; this note. |

The wire format did not change.  The direct path uses the same `MeshEnvelope`,
`mesh-transport` framing and `mesh-join` handshake as the relay path.

## 2. Real two-kernel evidence

`tools/m2-rig` with `--transport direct` on a shared host (two `iris` instances,
no host relay):

| Check | Result |
|---|---|
| Direct roles | n1 `08006969…` initiator, n0 `08006995…` listener |
| Connect latency | 21 ms (`connect_ms=21`), one connection |
| Join | epoch 2, two members, digest `2EDA13C5AC6F726132E0479DFE6045B7` |
| Homes | same 8-object table before and after the member rejoin; stale epoch refused |
| Route | one wire `MOVED` B→A with local retry; one local `MOVED` A→B with wire retry |
| Migration | n0→n1→n0, hashes `246A7909…` then `C0C3BD12…` |
| Mesh latency | 50 samples: p50 30 ms, p95 31–32 ms (relay: p50 31 ms, p95 52 ms) |
| Bulk | 64 KiB in 8 × 8 KiB chunks, 0.404 MiB/s mean (5.21× the 1 KiB baseline); a later run 3.72× with one 663 ms tail |
| Root restart | after the rebase onto `206d19a`, the signed epoch store restored epoch 2 on both nodes and the root restarted at the same epoch |
| HTTP probe | pass (both directions) on the rebased run |

The `direct_fail` mode points the initiator at `gateway:1`; it logs
`mesh-transport: direct fallback listen` and no join occurs.

## 3. What is not claimed

- The guest two-way HTTP probe passed on the rebased `206d19a` run; the earlier
  HTTP/1.0 close-race failure is fixed by the M2 remediation changes, not by this slice.
- Plain TCP, no TLS, no peer authentication, no replay window.  The join is signed; the
  data path is not.
- One listening port and one connection per peer.  A direct full mesh above two nodes
  needs the stream layer (#246) or a hub.
- The kernel TLS client now supports RSA and P-256 chains, not P-384.  The lab module
  fetch is HTTP; production HTTPS still needs a curve-coverage decision.

## 4. PROM and memory

| Budget | Value |
|---|---:|
| PROM (ABI 13, P-384 retained) | 523,556 / 524,288 |
| Module | 379,912 bytes image |
| Module `.bss` | 102,257 bytes (plus the image) |

Removing P-384 freed about 127 KiB but broke HTTPS module fetches (the Boot runtime-manager
step), so it was restored.  The slice fixes the ephemeral local port instead; the first
full build was 525,076 bytes, 788 over the window, and the final build is 523,556.
