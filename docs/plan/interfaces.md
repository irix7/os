# IRIX 7 — Interface Specifications

**Status:** living index
**Rule:** a cross-boundary interface is not implemented until its spec is at least **draft**
and reviewed by the owners on both sides. The spec is versioned; a breaking change is a new
version, not a silent edit.

## 1. Register

| Interface | File | Status | Owner | Version | Tests |
|---|---|---|---|---|---|
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

The first three specs are the ones needed to build M1–M4:

1. **Boot protocol** — the ROM cannot hand off to the kernel without it.
2. **MCP** — the development harness depends on it.
3. **Driver interface** — every driver cell depends on it.

The next three are needed for M5–M9:

1. Native capability ABI.
2. Native libc ABI.
3. Image format / manifest.

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
