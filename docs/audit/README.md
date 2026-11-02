# Closed-issue audit

Tracking issue: [#330](https://github.com/irix7/os/issues/330)
Date: 2026-11-02
Base: `origin/main` at `cac1a5a`, plus #321 at `5aff96e`

The audit checks every closed issue in `irix7/os` against the code, tests, gates and
contracts. It is not a reread of issue titles: each verdict cites the implementation, the
evidence grade and the shortcut that remains.

The 39 closed issues are split across three read-only reports:

- [`closed-core.md`](closed-core.md) — boot, ROM, kernel, TLB/MMU, traps, console, REX3,
  CI, Wasm/M1 and the module system (16 issues).
- [`closed-mesh.md`](closed-mesh.md) — M2/M3, join, homes, routing, membership, streams,
  batching, transport and event-driven I/O (6 issues).
- [`closed-platform.md`](closed-platform.md) — x86_64, PowerPC hardware and userland/PAL,
  threads, futex, fd, ramfs and pthreads (17 issues).

## Method

For each issue:

1. Read the body and acceptance criteria, the linked PRs and the merge commits.
2. Read the live code paths, tests, workflows and gate output.
3. Run a targeted gate or test where it directly settles a claim; otherwise cite the
   recorded CI run and say so.
4. Classify as **implemented**, **partial**, **divergent**, **contract-only** or
   **superseded**, with a foundation risk and a follow-up.
5. Record shortcuts: `TODO`/`unimplemented`/`ENOSYS`/no-op paths, log-only assertions,
   simulation passed off as real-kernel evidence, single-node assumptions, missing failure
   paths, stale docs and compatibility shims.

A closed issue with a real QEMU gate is `implemented` for a QEMU-scoped acceptance. A
hardware claim that is only QEMU-tested is recorded as a limitation, and the verdict drops
when the hardware is part of the acceptance. A gap that has a live follow-up issue is
`partial`, not `implemented`.

## Result

**25 implemented, 11 partial, 3 divergent.** No issue was `contract-only` or `superseded`.

The audit covered the 39 issues that were closed when it began. Five were reopened on the
evidence above — #1, #33, #118, #213 and #266 — so the current closed set is 34. The
table keeps the audited 39 so the verdicts remain traceable.

| Issue | Title | Area | Verdict | Risk | Follow-up |
|---:|---|---|---|---|---|
| 1 | M0 — Toolchain unblocked | core | partial | medium | Reopened #1: devshell cannot build the kernel; resolve the LLVM/GCC record |
| 2 | M1 — Single-node logical computer | core | implemented | high | Real-Indy evidence under #16; do not call the bridge resident policy |
| 3 | M2 — Two-node join and migration | mesh | partial | high | Root-failure recovery and epoch-store provisioning remain |
| 33 | M1: boot-protocol conformance tests (BootInfo layout + CRC32C) | core | partial | high | Reopened #33: tests are not linked to the live parser; endianness and map walk are wrong |
| 43 | docs: add install/getting-started guide | core | partial | low | Update `docs/install.md` run/status sections |
| 115 | console: unify kernel and user output across serial, telnet and MCP telemetry | core | implemented | low | Add the automated telnet/MCP assertion for user stdout |
| 116 | boot: emit a machine-readable READY marker | core | implemented | low | Make `boot=` monotonic or document RTC-second reuse |
| 117 | kernel: fix TLB teardown and refill after user mode | core | implemented | low | Add the user-then-TLB regression to CI |
| 118 | rex3: verify driver rgbmode and FIFO drain semantics | core | divergent | medium | Reopened #118: the test reads the CPU shadow, not the REX3; the FIFO is not filled |
| 119 | ci: replace sleep-based guest startup with readiness polling and cleanup | core | implemented | low | Add an explicit failed-boot cleanup assertion |
| 204 | M1-T1: complete the Wasm host ABI probe | core | implemented | medium | Record submit/poll/cancel as synthetic probe semantics |
| 205 | M1-T2: run the local policy from Wasm | core | implemented | medium | Keep the guest-only import test; add a negative trap boot |
| 206 | M1-T3: final M1 gate and hardware evidence | core | implemented | high | Re-state the gate as iris-only; #16 remains the hardware gate |
| 213 | M1-T4: make the Wasm policy the live kernel policy | core | partial | high | Reopened #213: kernel root/capability table remain native; failed reload drops policy |
| 214 | M1-T5: record optional Wasm load time | core | implemented | low | None |
| 217 | Module system: runtime dependencies and manifests | core | implemented | medium | Bridge overlaps runtime arena region 4; reserve it with #213 |
| 245 | net: direct peer transport for the mesh | mesh | partial | medium | Not in CI; `learn_peer` trusts the header source |
| 246 | net: multiplexed mesh streams | mesh | partial | high | #337: live module does not use the scheduler; cancel bundle is ungated |
| 247 | net: larger mesh buffers and batching | mesh | partial | medium | Merged 16 KiB, not 32 KiB; record both benchmark results |
| 248 | kernel: event-driven mesh I/O | mesh | divergent | high | #336: readiness polling, not the RX interrupt the goal requires |
| 249 | net: aggregated and gossip membership | mesh | partial | high | #264 carries the remainder; four real kernels converge via a host hub |
| 266 | arch: x86_64 as the second supported machine type | platform | divergent | high | Reopened #266: separate kernel, no shared core or x86 userland |
| 271 | m1: the Wasm-autoload boot traps when the kernel's service table grows | core | implemented | medium | #272 bounds the stack; update stale stack-top docs |
| 287 | arch: boot a PowerPC G3 kernel to serial under qemu-system-ppc | platform | implemented | low | Real G3/PowerBook run under #16 |
| 288 | userland: boot a real C probe through the IRIX PAL | platform | implemented | high | CI relibc lane (#13); process/capability gate (#326/#10) |
| 291 | ppc: install the kernel's own BATs and report a deliberate DSI | platform | implemented | low | Hardware BAT/TLB check under #16; delay-slot handling |
| 295 | ppc: complete the exception and system-call layer | platform | implemented | low | Hardware exception check under #16; real user ABI later |
| 296 | userland: broaden the fd surface and give directories a cookie | platform | implemented | high | Per-process fd table (#326); `*at` surface |
| 301 | ppc: take external timer interrupts through the Heathrow PIC | platform | partial | high | #335: the M4753 PMU path skips the proof |
| 302 | userland: poll and non-blocking descriptors | platform | partial | medium | Busy-waits; parked waits under #322/#326 |
| 305 | ppc: read the IDE disk through the Heathrow ATA controller | platform | implemented | medium | Real disk timing under #16; DMA/interrupt slice |
| 306 | userland: make the in-memory filesystem writable | platform | implemented | medium | Block-backed FS and per-cell namespace (#10/#326) |
| 309 | ppc: load a verified flat module from the ATA disk | platform | implemented | medium | Signed boot configuration; #338 fixes the slot assertion |
| 310 | userland: cooperative user threads and per-thread TCBs | platform | implemented | high | Preemption and blocking waits (#326) |
| 313 | ppc: make the boot console work on either ESCC channel | platform | implemented | medium | Real Geoport wiring under #16 |
| 314 | userland: futex wait/wake for cooperative threads | platform | implemented | medium | Timer-driven wakeups (#322) |
| 317 | ppc: select the Mac I/O node with the ESCC and ATA children | platform | implemented | medium | Real two-controller tree under #16 |
| 318 | userland: pthread_create/join and a mutex on the thread gate | platform | implemented | high | Condvars, cancellation and preemption (#326/#322) |
| 321 | ppc: serial input and a minimal boot monitor | platform | implemented | medium | No bounded RX queue; real Geoport test under #16 |

## Highest-risk findings

1. **The process and address-space foundation is still one image, not cells.** `USER_PAGES`,
   `USER_RANGES`, `HEAP_NEXT`, the fd table and the thread table are statics
   (`kernel/src/user.rs:22-38`, `kernel/src/fd.rs:19-20`); `getpid()` is always 1 and there
   is no `fork`, `execve`, `waitpid` or per-process page list. This blocks the D3
   process = cell model. #326 is the direct fix.
2. **The BootInfo contract is not the contract the two sides use.** The kernel reads the
   magic and size little-endian (`kernel/src/main.rs:330`), walks the map with an absolute
   kseg0 index (`:370-404`) and implements 3 of 8 validation steps. The nine conformance
   tests pass against a separate crate that neither the ROM nor the kernel links. #33 is
   reopened.
3. **x86_64 is a separate kernel, not the shared core.** `kernel-x86` does not depend on
   `kernel` and carries its own module loader, services, net and TCP code
   (`kernel-x86/Cargo.toml:8-17`, `docs/plan/x86.md:4-5`). The machine layer and M2 gate
   are real. #266 is reopened.
4. **The REX3 self-test reads the CPU shadow.** `gfx.rs:475-479` reads `0x8870_0000`, not
   the engine, so a fill that only reached the red channel would pass. The FIFO check is a
   counter that `fill_span` guarantees. #118 is reopened.
5. **The M1 policy is a read path, not the live kernel policy.** `Local.root` and the
   native capability table remain native (`kernel/src/logical.rs:304-318`), and a failed
   slot-B reload drops the previous policy before verification
   (`kernel/src/module.rs:149-159`). The bridge at `0x885f_ffd8` also sits inside runtime
   arena region 4. #213 is reopened.
6. **The mesh is not yet the 8-node computer the membership issue describes.** The live
   root has one decoder/queue/link, gossip is a host hub, the stream scheduler is not used
   by the live module and the event path is readiness polling. #264, #336 and #337 carry
   the missing work; #249's closure is a transfer, not a completion.
7. **PowerPC hardware claims are QEMU-only.** The M4753 exposes `via-pmu`, not `via-cuda`,
   so the real target prints `IRQ-SKIP reason=no-cuda`; ATA is polled PIO and no real
   PowerBook has run the gate. The M1/M2 assertion that they run on real kernels is also
   not met (#16). The PMU timer gap is #335.
8. **The userland evidence is transcript-only.** CI builds `std-hello` and the native hello
   probe, not the four relibc probes (`grep -c relibc .github/workflows/ci.yml` → 0). The
   relibc, thread, futex and pthread results are real iris runs, but a regression would not
   be caught. Add the lane under #13.
9. **The default Nix devshell cannot build the kernel.** It supplies stable rustc without
   `rust-src`; `-Z build-std` needs nightly. CI uses the right toolchain, but the documented
   path does not. #1 is reopened.
10. **The module slot had no link-time guard.** The linker allowed the image to grow to
    `0x0200_0000` while the module slot starts at `0x0180_0000`. #338 fixes the assertion.

## Follow-ups

Reopened on audit evidence:

- #1 — toolchain/devshell.
- #33 — live BootInfo parser and conformance.
- #118 — REX3 engine readback and FIFO drain.
- #213 — live policy adoption and failed-reload retention.
- #266 — shared core and x86 userland, or an explicit re-scope.

Created:

- #335 — PMU timer source for the M4753 `0x500` proof.
- #336 — Seeq RX interrupt and INT2 dispatch.
- #337 — stream scheduler and control-during-bulk/cancel gate.

Existing work that now carries the remainder: #16 (hardware), #264 (membership), #322
(blocking waits), #326/#327 (process cells), #13 (CI and relibc), #272 (stack budget),
#328 (Rage LT), #329 (one-computer demo), #331 (Indy browser scenario).

## Evidence and limits

- Real gates run during the audit: PowerPC QEMU at 64 MiB; x86_64 QEMU at 256 MiB; the #321
  monitor gate at 64 MiB; `cargo test -p native-abi` (12 pass); the mesh, stream, transport,
  resolver, route, service, core, migrate and rig suites (all pass, 140 + 80 tests).
- No real Indy, PowerBook or x86_64 hardware was available. #16 remains the hardware gate.
- The relibc probes were not rebuilt for the audit; their evidence is the in-tree sources,
  the pinned fork commit and the recorded iris transcripts. The missing CI lane is a
  finding, not a pass.
- The audit is read-only against code. The reports describe the tree at the base above;
  later PRs (#332, #333, #334, #338) are not part of the verdicts.
