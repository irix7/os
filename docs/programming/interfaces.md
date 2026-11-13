# IRIX 7 — Interface Specifications

**Status:** living index
**Rule:** a cross-boundary interface is not implemented until its spec is at least **draft**
and reviewed by the owners on both sides. The spec is versioned; a breaking change is a new
version, not a silent edit.

## 1. Register

| Interface | File | Status | Owner | Version | Tests |
|---|---|---|---|---|---|
| Logical computer / mesh | [`../spec/system.md`](../spec/system.md) | draft | kernel / mesh | 1.0 | `mesh-core`, `mesh-service`, `mesh-sim`; M1–M4 gates |
| Wasm host-call ABI | [`irix7/os-sdk/wasm-abi`](https://github.com/irix7/os-sdk/tree/main/wasm-abi) | draft | ABI / SDK | 1.0 (SDK v0.5.0) | `guest-abi`, `wasm-host`, wasm32 check |
| Fabric `MessageFrame` / `MeshHeader` | [`../spec/system.md`](../spec/system.md) §3 | draft | kernel / mesh | 1.0 | `sys-primitives` wire tests, `fabric` tests |
| Capability v2 / root manifest | [`../spec/system.md`](../spec/system.md) §4 | draft | security / mesh | 2.0 | `sys-primitives`, `cap-engine` tests |
| CAS manifest | [`../spec/system.md`](../spec/system.md) §6 | draft | storage | 1.0 | `cas-core` manifest/verification tests |
| Native `PlatformShim` | [`../spec/system.md`](../spec/system.md) §3.1 | draft | kernel / targets | 1.0 | `platform-api`; mock + Indy/PC/cloud later |
| ROM ↔ kernel boot handoff | [`../spec/boot-protocol.md`](../spec/boot-protocol.md) | draft | boot | 1.0 | iris + Indy |
| MCP control plane | [`../spec/mcp.md`](../spec/mcp.md) | draft | infra | 1.0 | MCP conformance + agent loop |
| MCP application endpoints | [`../spec/mcp-apps.md`](../spec/mcp-apps.md) | draft | infra | 0.1 | broker + local MCP app harness |
| Driver `ctl`/`data`/`status` | [`../spec/driver-interface.md`](../spec/driver-interface.md) | draft | drivers | 1.0 | mock + iris + Indy |
| Native capability ABI | [`../spec/native-abi.md`](../spec/native-abi.md) | draft | kernel | 0.1 | unit + fuzz |
| Native libc ABI | [`../spec/native-libc.md`](../spec/native-libc.md) | draft | ABI / libc | 0.1 | `tools/std-abi-surface.py`, layout assertions, static `std` image, POSIX subset |
| Boot bundle / image manifest | `../spec/image-format.md` | planned | infra | 0.1 | digest + boot |
| 9P profile | `../spec/9p.md` | planned | network | 0.1 | interoperability |
| Display protocol | `../spec/display-protocol.md` | planned | display | 0.1 | golden frames |
| Audio protocol | `../spec/audio.md` | planned | display | 0.1 | latency/glitch |
| X11 wire profile | `../spec/x11.md` | planned | display | 0.1 | X Test Suite |
| EROFS on-disk profile | [`../spec/erofs.md`](../spec/erofs.md) | draft | storage | 0.1 | golden `mkfs.erofs` fixtures + `fsck.erofs` |
| XFS v4 read-only profile | `../spec/xfs.md` | planned | storage | 0.1 | tree diff |
| I7FS on-disk format | `../spec/i7fs.md` | planned | storage | 0.1 | fsck + crash tests |

## 2. Status values

| Status | Meaning |
|---|---|
| `planned` | Named, not written; implementation may start on prototypes only |
| `draft` | Written; open for review; may change |
| `review` | Frozen for review; changes need an ADR |
| `accepted` | Approved; implementation may depend on it |
| `implemented` | Code exists and passes the conformance tests |
| `stable` | Shipped; breaking changes need a new major version |
| `deprecated` | Superseded; kept for compatibility |

## 3. Priority

The system interfaces come first:

1. **Logical computer / mesh** — homes, routing, authority and 1/128 gates.
2. **Wasm host-call ABI** — the core cannot run without it.
3. **Fabric and capability tokens** — local and remote operations share them.
4. **PlatformShim** — the native side of the line.

The native compatibility interfaces remain:

1. **Boot protocol** — the Indy ROM handoff.
2. **Native capability ABI and native libc ABI** — POSIX/IRIX.
3. **Driver interface and MCP** — the development harness.
4. **Image format / manifest** — boot bundle and release.

## 4. Change process

1. Open a `type/adr` or `type/feature` issue with the interface name.
2. Update the spec in a PR; bump the version.
3. Add or update the conformance tests.
4. Get approval from the owners on both sides.
5. Update the implementation and the plan.

## 5. Conformance

Each interface has a test suite named after it:

- `boot-protocol`: ROM → kernel handoff on iris and a real Indy.
- `mcp`: `initialize`, `tools/list`, `tools/call`, error paths, token rejection.
- `driver-interface`: mock device, iris device, real device.
- `native-abi`: capability rights, revocation, delegation, fuzz.
- `native-libc`: the std ABI surface audit (`tools/std-abi-surface.py`), layout assertions,
  errno and the POSIX subset smoke tests.
- `image`: digest, manifest, boot, rollback.
- `9p`: interoperate with a Linux/Plan 9 client.
- `display`: golden-frame comparison.
- `x11`: X Test Suite subset.
- `erofs`/`xfs`/`i7fs`: oracle or fsck.
