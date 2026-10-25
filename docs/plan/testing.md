# IRIX 7 — Testing Strategy

**Status:** living document
**Principle:** tests are the specification's executable half. Benchmarks inform; they do not gate.
**Related:** [`milestones.md`](milestones.md), [`../spec/system.md`](../spec/system.md), [`hardware-lab.md`](hardware-lab.md), [`release.md`](release.md).

## 0. Mesh gates (current)

The architecture adds a second test axis: the same logical computer at **1, 2, 8 and
128 nodes**. These are hard gates, not benchmark notes.

| Gate | What is tested |
|---|---|
| M1 | Single-node boot with no network/quorum/seed; zero mesh frames; local namespace/CAS/capabilities/actors/devices; POSIX/IRIX smoke |
| M2 | Join, membership epoch, rendezvous home, remote capability route, L0 migration both ways, one-node failure and partition |
| M3 | Eight-node sharded homes, CAS fetch-by-OID, placement, two-node failure, churn, GPU object path |
| M4 | 128-node simulator/QEMU: ≤2 normal hops, no global queue/lock/all-to-all, eight-node failure, churn, 1M OIDs, 10k actors |

The policy-level gates already run in `crates/mesh-sim` against the real
`mesh-service` membership table and `mesh-core` rendezvous. The kernel now
prints its single-node identity and initialises the same membership policy at
boot; this is M1 boot evidence, not a mesh. A host integration test,
`crates/mesh-sim/tests/single_node.rs`, joins the real membership table,
namespace/CAS, actor directory/run queue and capability engine on one node and
constructs no fabric frame. The loadable `logical` module runs the same policy
on iris outside the compressed PROM; it prints its local root, actor and
verified 1-of-1 capability, then ticks. The kernel now starts a local
namespace/CAS, actor and capability policy at boot as well; the module remains
the signed 1-of-1 probe. The slot-B Wasm module now runs the local policy from
a Rust guest through the staged `i7host` surface and prints evidence matching
the native `m1:` line; the general module multiplexer is still missing. Mesh
traffic is now instrumented and asserted zero on the M1 line. The fabric,
capability, CAS and Wasm host crates have host tests;
the 128-node execution gate requires the native shim and transport work.

Architecture CI additions:

| Job | What | Required |
|---|---|---|
| `architecture-host` | `sys-primitives`, `mesh-core`, `mesh-service`, `fabric`, `cas-core`, `cas-store`, `cap-engine`, `agnostic-core`, `mesh-sim` tests | Yes |
| `architecture-wasm32` | `sys-primitives`, `mesh-core`, `fabric`, `cas-core`, `cap-engine` check for wasm32 | Yes |
| `wasm-host` | wasmi host imports, fuel exhaustion and typed calls | Yes |
| `mesh-gates` | 1/2/8/128 policy gates and failure injection | Yes at Phase 3+ |
| `mesh-real` | 2/8-node iris or QEMU transport/migration gate | Yes at Phase 3+ |

The first `mesh-real` rung is the [M2 two-node rig](m2-rig.md) in `tools/m2-rig`. It
starts **two independent iris processes**, asserts `BOOT-READY`, distinct MACs, NodeIds
and guest addresses, and runs a two-way TCP probe through the host relay. It is evidence
for the M2-0 foundation only: it deliberately leaves both nodes at `mesh=single` and does
not implement join, membership, homes, capability routes or migration. Those are
M2-1..M2-5. The rig is a manual command until the M2-8 CI job wraps it; the evidence
bundle under `target/m2-rig/evidence/` is the review artefact.

The independent endian and migration tests are listed in [`milestones.md`](milestones.md)
and must include a big-endian MIPS → little-endian x86_64/AArch64 state hash.

## 1. Test pyramid

| Layer | What | Where | Speed |
|---|---|---|---|
| Unit | Pure logic, parsers, allocators, capability rights, state machines | Host, `cargo test` | Seconds |
| Integration | Cells + framework + mock devices | Host/iris | Minutes |
| Boot | ROM → kernel → init → shell | iris | Minutes |
| Conformance | IRIX ABI, X11, XFS, EROFS, 9P, MCP | iris + host oracles | Minutes–hours |
| Hardware | Real Indy: serial, SCSI, network, display, X11, XFS | Lab runner | Hours |
| Fuzz/verify | Parsers, unsafe, invariants | Host, nightly | Hours |
| Performance | Boot time, block throughput, X11 latency, audio glitches | iris + Indy | Tracked, not gated |

## 2. Test categories

### Kernel

- Trap/exception vectors, TLB refill, cache maintenance, timer calibration.
- Frame/slab allocator invariants; huge-page promotion; OOM killer.
- Scheduler: EDF deadlines, fair share, priority inheritance, `is_running`.
- Capability rights: derivation, delegation, revocation, stale handles.
- IPC: synchronous channels, shared memory, cancellation, timeouts.
- Panic path: registers, stack, serial output.

### Drivers

- `ctl` parser fuzzing, `data` boundaries, `status` stability.
- Lifecycle: start/stop/reset/remove ×100.
- IRQ event delivery and coalescing.
- DMA cache sync on iris and real hardware.
- Mock device for SCSI, Seeq, HAL2, Newport, Vino.
- Error injection: timeout, parity, disconnect, FIFO overrun.

### Compatibility

- o32/n32 ELF loading, syscall decode, `errno`, signals, `fork`/`exec`/`sproc`.
- libc subset: `write`, `read`, `open`, `malloc`, `printf`, `pthread` (later).
- IRIX binary corpus: shell utilities, X11 clients, GL/dmedia samples.
- ABI conformance: register conventions, stack alignment, big-endian.
- Xlib/Xt/Motif subset; X Test Suite.
- XFS read-only: clean images, large files, sparse files, symlinks, corrupted images.

### Native

- Cell loader: relocation, symbols, ABI version.
- Capability ABI: rights, revocation, delegation, fuzz.
- Native libc: `open`/`read`/`write`/`mmap`/`fork`/`exec`.
- Display protocol: golden frames, damage, cursor.
- 9P: interoperability with a Linux/Plan 9 client.

### Network and MCP

- smoltcp: DHCP, DNS, TCP, IPv6, HTTP Range, Git smart HTTP.
- `/net` tree semantics.
- MCP: `initialize`, `tools/list`, `tools/call`, token rejection, rate limit, cancellation.
- Prompt-injection resilience is a client concern; test that tool results are data, not
  instructions.

### Security

- Capability fuzzing; revocation races; stale handles.
- Fuzz: ELF, EROFS, XFS, 9P, HTTP, Git, MCP JSON-RPC.
- Secret scanning and licence checks in CI.
- No `unsafe` outside the framework/drivers without a written invariant.

## 3. Test corpus

| Corpus | Contents | Location |
|---|---|---|
| IRIX binaries | o32/n32 utilities, X11 clients, GL/dmedia samples | `test/corpus/irix/` (binaries not committed if large; manifest + hashes) |
| IRIX disks | XFS v4 images (clean, dirty, fragmented, large files) | `test/images/xfs/` (LFS or R2) |
| EROFS | Plain, LZ4, fragments, xattrs | `test/images/erofs/` |
| Boot | ROM stub, kernel stub, boot bundle | `test/boot/` |
| X11 | X Test Suite subset, `xterm`, `twm` | `test/x11/` |
| MCP | JSON-RPC frames, tool schemas | `test/mcp/` |
| Native std | Static `std` image, args/time/sleep/fs/chdir fixture | `user/std-hello/` |
| Mesh policy | Fabric frames, mesh homes, membership, CAS manifests, capabilities | `crates/` |
| Wasm host | WAT/ABI/fuel fixtures | `crates/wasm-host/` |
| relibc n32 | `hello`, TLS, `setjmp`, pthread smoke tests | [`irix7/relibc`](https://github.com/irix7/relibc) |
| Fuzz seeds | ELF, EROFS, XFS, 9P, HTTP, Git | `test/fuzz/` |

Large binaries and images live in R2 or Git LFS, with a manifest and SHA-256 in the repo.

The kernel `sash` is exercised manually on both transports: telnet `help`,
`pwd`, `ls` and `cat /hello.txt`, and the local keyboard/display after the
target-2 image returns.  The local path uses the same parser but writes through
the console byte stream, so it works with `net=off`.

The current bring-up gates are the static `std` image under `user/std-hello` (args,
time, sleep, `std::fs` on a tiny ramfs, `chdir`) and the external
[`irix7/relibc`](https://github.com/irix7/relibc) n32 gate (TLS, `setjmp`,
pthreads).  Neither is a complete POSIX system; threads, spawn, signals, termios
and sockets remain open.

The CI `boot` job builds the release kernel and ROM and installs the published
IRIS release through `irix7/iris/.github/actions/setup-iris`.  Its first boot
**attaches no SCSI disk at all**: DHCP and the GitHub autoload path bring up the
network, the target-2 loader fails cleanly, local and telnet sash come up, and
the test runs `packages` against the GitHub index and checks the v0.1.3 MCP
record. This is the primary diskless gate; a SCSI disk is not a boot, discovery
or module prerequisite. Package installation from GitHub is exercised
separately because its release/TLS path is not part of the M1 boot gate.

A slot-B boot serves a 1,345,976-byte wasm module over local HTTP, sets
`wasm.url` and `wasm.sha256` in NVRAM, and asserts `wasm: host=1`,
`wasm: run=42`, `m1-wasm: root=... actor=... cap=ok rights=35 home=...`,
`m1: ... mesh=0/0/0`, the post-smoke `m1-post: mesh=0/0/0` line and
`BOOT-READY` with MCP still listening. A second local boot uses `wasm.local`
on SCSI target 2 with `net=off`, so the same boot asserts
`BOOT-READY network=off mcp=disabled` and the Wasm policy evidence. The CI
compares the `m1-wasm` root, actor, rights and home against the native `m1:` line
and checks `objects=5 actors=1 queued=1 caps=1`. The guest drives the
namespace, actor and capability choice; the host supplies only staged
CAS/table primitives. A failed or trapped guest aborts the stage, so no
partial policy is published. The image must stay within the
1,572,864-byte slot-B limit.

The no-network and slot-B policy boots are both part of the same `boot` CI
job. The M1 software path is reproducible from a clean checkout on iris. **No
physical Indy was available in this workspace: the real-Indy execution gate
is not claimed, and is recorded as the hardware limitation in §5.**
The policy guest is a boot-time proof, not yet a resident or multiplexed
policy service.

**Readiness and optional Wasm are measured separately.** With the local MCP
fixture, MCP-only boots reached `BOOT-READY` in 7.3–7.7 s. The optional slot-B
Wasm probe starts after `BOOT-READY`; the instrumented run reports
`autoload: wasm took 2140 ms` over HTTP and 1600 ms from the local SCSI cache,
with `wasm: runtime=0 ms`, so fetch/copy (not the guest policy) dominate. The
full-path load time is recorded as an observation under #214; it is not a gate,
does not delay `BOOT-READY`, and the local cache is the preferred optional
source. The M1 readiness budget does not include that optional probe, so
readiness is not regressed by Wasm. The observation is not a claim that the
Wasm policy is cheap to load on the R4600.

A second boot uses `net=off`, attaches no SCSI disk and has no local module.
It asserts the kernel's `m1:` policy line including `mesh=0/0/0`, the
post-smoke `m1-post: mesh=0/0/0` line, `BOOT-READY` with `network=off` and
`mcp=disabled`, the expected target-2 load failure and the sash prompt. This is
the M1 no-network, no-module boot.

The same job then runs the offline recovery image.  It wraps `user/hello` as a
256 KiB SCSI target-2 image and appends the `logical` module to the same disk,
writes an `mcp.local` NVRAM record, sets `net=off`, and waits for the module's
`cap=ok` line, `BOOT-READY` with `network=off` and `mcp=local`, `hello from
user`, `pipe=ok`, `winsize=100x40`, the exit marker and `logical: tick 3`,
then uploads the serial transcript.  The action's release build needs
`IRIS_NO_IDLE=1`; `docs/ci.md` in the IRIS repository documents that limitation.
The serial waits follow log order because the default `iris-ci` consumer cursor
is forward-only ([irix7/iris#110](https://github.com/irix7/iris/issues/110)).

The `boot` job also builds the patched Rust `std` rlib and the static
`user/std-hello` image, strips and pads it to target 2, and boots it with
`net=off`. It asserts `hello from std`, the ramfs/stat/read_dir/chdir checks,
exit 0 and the post-smoke `m1-post: mesh=0/0/0` line. This is now a CI
artifact rather than a local-only run.

The CI boot gate also builds `user/o32-hello`, rewrites its `e_flags` to
`EF_MIPS_ABI_O32`, and boots it with the same local logical module and
`net=off`.  It checks `hello from o32`, a read of `/hello.txt` through o32
`open`/`read`/`write`/`close`, and exit 42.  This is a synthetic loader and
syscall-personality probe, not an IRIX 6.5 binary; the IRIX libc, `rld`,
`sproc`, signals and full decoder remain future work.

## 4. CI

| Job | What | Required |
|---|---|---|
| `fmt` | `cargo fmt --check` | Yes |
| `clippy` | `cargo clippy -- -D warnings` | Yes |
| `test` | host unit/integration | Yes |
| `cross` | n32/o32 `-Z build-std` (toolchain gate) | Yes |
| `std` | static `std` image + native syscall fixture | Yes once std bring-up is in CI |
| `relibc-n32` | external n32/QEMU relibc gate | Manual/nightly until linked |
| `boot` | Release PROM + user hello under the published IRIS action | Yes |
| `mcp` | MCP conformance against a running iris instance | Yes once P3 is green |
| `licence` | `cargo-deny`, REUSE, SBOM | Yes |
| `security` | CodeQL, dependency review, `cargo-audit` | Yes |
| `docs` | mdBook build, link check | Yes |
| `hardware` | self-hosted `indy` runner | Manual/nightly |
| `nightly` | full build, fuzz corpus, long boot | Nightly |

The CI must be reproducible from a clean checkout; no manual steps.

## 5. Emulator and hardware

- **iris** is the first test target; every demo must run there.
- **Real Indy** is the second target; the milestone is not done until it runs there (or the
  limitation is recorded). The M1 gate currently has no physical Indy IP24 or lab runner, so
  real SCC, Seeq DMA, SCSI, REX3 and DS1386 timing/cache behaviour is unvalidated; see #16.
- **Differential testing**: run the same test on iris and the real Indy; compare serial,
  screen and block hashes.
- **Snapshots**: iris snapshots make long tests resumable; record the snapshot hash.
- **Hardware-in-the-loop**: the `indy` self-hosted runner is described in
  [`hardware-lab.md`](hardware-lab.md).

## 6. Tools

| Tool | Use |
|---|---|
| `cargo-nextest` | Fast test runner |
| `proptest` | Property tests for parsers and allocators |
| `cargo-fuzz`/AFL++ | ELF, EROFS, XFS, 9P, HTTP, Git, MCP |
| Miri | Undefined behaviour in safe code |
| Kani | Selected invariants (capability table, allocator) |
| `cargo-call-stack` | Stack usage |
| `cargo-llvm-cov` | Coverage (informational) |
| `criterion` | Host microbenchmarks |
| `perf`/`RPSS` | Indy profiling |
| `lychee` | Link checking |
| `markdownlint` | Docs style |
| `typos` | Spelling (Australian English) |

## 7. Benchmarks

Tracked, never gated:

- cold and warm boot time on iris and Indy;
- block throughput (SCSI, R2 Range);
- X11 `xterm` start and keystroke latency;
- audio buffer underruns per minute;
- display damage repair time;
- MCP round-trip latency;
- build time for the kernel and the image.

Store results in `docs/bench/YYYY-MM-DD.md` or a Project field; compare over time.

## 8. Release gates

A release requires:

- [ ] all required CI jobs green;
- [ ] the milestone demo on iris;
- [ ] the milestone demo on a real Indy (or a recorded limitation);
- [ ] the compatibility corpus passes;
- [ ] the security and licence checks pass;
- [ ] the docs build and the install/upgrade path is tested;
- [ ] the SBOM and signatures are attached;
- [ ] the release notes are complete.

## 9. Test data management

- Small fixtures in the repo; large images in R2/LFS with a manifest.
- Every image has a SHA-256 and a provenance note.
- Do not commit IRIX binaries that cannot be redistributed; keep them in the private corpus
  and record hashes.
- Fuzz corpora are minimised before commit.

## 10. Ownership

| Area | Owner |
|---|---|
| Kernel | kernel |
| Compat | compat |
| Drivers | drivers |
| Display/X11 | display |
| Storage | storage |
| Network/MCP | infra |
| Security | security |
| Hardware | hardware |

An area without a test owner is a risk (see [`risks.md`](risks.md)).
