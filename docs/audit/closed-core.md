# Closed-issue audit — boot, kernel and core

Tracking issue: [irix7/os#330](https://github.com/irix7/os/issues/330)
Audit base: `origin/main` at `cac1a5a`
Worktree: `/home/matt/.local/share/opencode/worktree/bafe0d/audit-core`

## 1. Scope and method

### 1.1 Issue set

The first query returned the 38 issues in the brief. A later query returned 39 because `#321` (PowerPC boot monitor) closed during the audit.

The final partition of the 39 closed issues is:

- **Core (this report, 16):** `1, 2, 33, 43, 115, 116, 117, 118, 119, 204, 205, 206, 213, 214, 217, 271`.
- **Mesh (6):** `3, 245, 246, 247, 248, 249`.
- **Platform (17):** `266, 287, 288, 291, 295, 296, 301, 302, 305, 306, 309, 310, 313, 314, 317, 318, 321`.

`#43` (install guide) and `#217` (module system) are core in the final partition because both affect the boot path and the kernel's fixed memory map. The seven userland issues `#288, #296, #302, #306, #310, #314, #318` are platform-owned and are audited in the platform report, not here.

### 1.2 Commands used

```sh
gh issue list --repo irix7/os --state closed --limit 1000 --json number,title,closedAt
gh issue view <n> --repo irix7/os --json number,title,body,comments,closedAt
gh pr view <n> --repo irix7/os --json title,body,files,mergedAt
gh pr checks <n> --repo irix7/os
gh run view <id> --repo irix7/os --json status,conclusion,headSha,jobs
git log --all --oneline --grep '#<n>'
git show --stat --oneline <commit>
rg -n <pattern> kernel rom crates modules tools docs .github
cat -n <path>
nix develop --command cargo test -p boot-protocol-tests -p module-resolver -p wasm-host
nix develop --command cargo test -p wasm-policy-host -p m1-policy-bridge
nix develop --command cargo build -Z build-std=core,alloc -Zjson-target-spec \
  --target targets/mips3-sgi-irix7-n32.json -p kernel
ls scripts/rig
```

I did not run `iris` or QEMU for this audit. Real-kernel evidence is the CI `Boot (iris)` job or a PR transcript from an iris boot; that is stated per issue.

### 1.3 Verdict counts

| Verdict | Count | Issues |
|---|---:|---|
| implemented | 12 | 2, 115, 116, 117, 118, 119, 204, 205, 206, 214, 217, 271 |
| partial | 4 | 1, 33, 43, 213 |
| divergent | 0 | — |
| contract-only | 0 | — |
| superseded | 0 | — |

## 2. Summary

| Issue | Title | Verdict | Foundation risk | Follow-up |
|---:|---|---|---|---|
| 1 | M0 — Toolchain unblocked | partial | medium | Reopen #1: fix default Nix shell or document rustup path; record LLVM-only decision against ADR-0018 |
| 2 | M1 — Single-node logical computer | implemented | high | #2/#16: real-Indy evidence; separate the M1 bridge claim from resident policy |
| 33 | M1: boot-protocol conformance tests | partial | high | Reopen #33: wire tests to the kernel parser; fix BootInfo endianness and map walking |
| 43 | docs: add install/getting-started guide | partial | low | Update `docs/install.md` run and status sections |
| 115 | console: unify kernel and user output | implemented | low | Add automated telnet/MCP assertion for user stdout |
| 116 | boot: emit a machine-readable READY marker | implemented | low | Make `boot=` monotonic or document RTC-second reuse |
| 117 | kernel: fix TLB teardown and refill after user mode | implemented | low | Add the `user` then `tlb` regression to CI |
| 118 | rex3: verify rgbmode and FIFO drain | implemented | medium | Resolved (2026-11-03): engine register readback, FIFO burst, host framebuffer gate |
| 119 | ci: replace sleep-based guest startup | implemented | low | Add explicit failed-boot cleanup assertion |
| 204 | M1-T1: complete the Wasm host ABI probe | implemented | medium | Record submit/poll/cancel as synthetic probe semantics |
| 205 | M1-T2: run the local policy from Wasm | implemented | medium | Keep the guest-only import test; add a negative trap boot |
| 206 | M1-T3: final M1 gate and hardware evidence | implemented | high | Re-state the gate as iris-only; #16 remains the hardware gate |
| 213 | M1-T4: make the Wasm policy the live kernel policy | partial | high | Reopen #213: adopt or explicitly exclude root/cap table; failed reload retention |
| 214 | M1-T5: record optional Wasm load time | implemented | low | None |
| 217 | Module system: runtime dependencies and manifests | implemented | medium | #217/#213: reserve or move the M1 bridge out of arena region 4 |
| 271 | m1: Wasm-autoload boot traps when service table grows | implemented | medium | #272 remains open; update stale stack-top docs |

## 3. Per-issue evidence

### #1 — M0 — Toolchain unblocked

**Verdict: partial. Foundation risk: medium.**

What is implemented:

- The root workspace and M0 cross-build gate exist: `Cargo.toml:1-3`, `.github/workflows/ci.yml:142-214`.
- The n32 kernel and ROM build with `-Z build-std=core,alloc -Zjson-target-spec` in CI: `ci.yml:163-180`.
- `cargo test` for the host crates includes the boot-protocol crate: `ci.yml:89-100`.
- CI run `40244974653` is green, including `Cross-build (M0 gate)`.

What is missing or divergent:

1. **The GCC backend was never delivered.** The issue body asks to "Make build-std and the GCC backend work reliably". The closure comment records the backend as blocked, and the tree still says:
   - `docs/spec/architecture.md:274`: "`rustc_codegen_gcc` (libgccjit from §3.1) — **primary**".
   - `docs/toolchain.md:166`: "`rustc_codegen_gcc` is not available; the LLVM backend is the only code generator."

   There is no `rustc_codegen_gcc`, libgccjit or GCC-backend build path in the tree. The spec and the toolchain document contradict each other.

2. **The o32 target no longer exists.** `targets/` contains only `mips3-sgi-irix7-n32.json`, `mips3-sgi-irix7-native-n32.json`, `powerpc-unknown-irix7-ppc.json`. `tools/build-o32-hello.sh` builds the n32 target and rewrites `e_flags` to `EF_MIPS_ABI_O32` (`tools/build-o32-hello.sh:15-38`); it is a personality probe, not an o32 build-std target. The M0 demo asked for a no_std binary for n32 **and o32**.

3. **The default Nix devshell cannot perform the M0 build.** `flake.nix:26-31` supplies nixpkgs `pkgs.rustc` / `pkgs.cargo`, and the comment claims this is "pinned by rust-toolchain.toml". It is not: `nix develop` reports `rustc 1.99.0 (2026-09-28)` and no `rust-src`, while `rust-toolchain.toml:1-3` pins `nightly-2026-10-03`. Running the actual M0 command in that shell fails:

   ```text
   error: the option `Z` is only accepted on the nightly compiler
   ```

   The command was:
   `nix develop --command cargo build -Z build-std=core,alloc -Zjson-target-spec --target targets/mips3-sgi-irix7-n32.json -p kernel`.

   CI uses rustup (`ci.yml:149-155`), which is why the gate is green while the documented devshell is not.

Evidence grade: CI cross-build (green) + host unit tests; no boot and no GCC-backend evidence.

### #2 — M1 — Single-node logical computer

**Verdict: implemented. Foundation risk: high.**

What is implemented:

- The native local policy starts before readiness and prints the `m1:` line: `kernel/src/logical.rs:35-60`, `kernel/src/main.rs:620-650`, `kernel/src/main.rs:757-800`.
- It builds a local namespace/CAS with `/etc/motd` and `/dev/null`, a single-node membership view, an actor table/run queue and a native capability: `kernel/src/logical.rs:271-331`.
- The native actor is taken off the run queue and reaped around userland: `kernel/src/logical.rs:62-82`, `kernel/src/main.rs:675-690`.
- The optional slot-B Wasm policy is adopted after readiness: `kernel/src/main.rs:655-656`, `kernel/src/logical.rs:127-149`.
- CI asserts the M1 marker, zero mesh counters and the post-smoke marker in every core boot:
  - `ci.yml:442-451` (diskless)
  - `ci.yml:533-545` (slot-B Wasm)
  - `ci.yml:1003-1007` (no module)
  - `ci.yml:1214-1221` (local module)
  - `ci.yml:1146-1221` (native hello and target-2 present)
- CI run `40244974653` is green, with `Boot (iris)` passing.

What limits the claim:

- **The gate is iris-only.** `docs/plan/milestones.md:35` says "M1 and M2 must run on real kernels." The issue records the hardware limitation instead: `docs/plan/milestones.md:150-160`, `CHANGELOG.md:236-240`. `#16` (hardware lab) is still open. No real SCC, Seeq DMA, SCSI, REX3 or DS1386 timing/cache behaviour has been validated.
- **`mesh=0/0/0` is not a packet measurement.** `kernel/src/mesh.rs:41-74` says no transport calls `note_frame`; the boot self-test increments the counters and resets them. The marker therefore proves the kernel's own counter baseline, not that a module did not emit a mesh frame. After the M2 transport moved into `modules/mcp`, the kernel counters no longer cover that module.
- **The "complete logical computer" is a bring-up policy, not a resident core.** The Wasm policy is a slot-B module behind a fixed bridge; the kernel remains the live authority for its own root and capability table (see #213).

Evidence grade: real-kernel CI (iris) + host tests.

### #33 — M1: boot-protocol conformance tests (BootInfo layout + CRC32C)

**Verdict: partial. Foundation risk: high.**

What is implemented:

- A 256-byte `#[repr(C, align(8))]` `BootInfo`, full offset assertions, CRC32C with the `123456789` known answer and a validator: `crates/boot-protocol-tests/src/lib.rs:1-232`.
- Nine tests pass: `cargo test -p boot-protocol-tests` → `9 passed; 0 failed`.
- The crate is a workspace member (`Cargo.toml:3`) and is in the CI test job (`ci.yml:92`).

Why this is not conformance:

1. **Neither side uses the crate.** `kernel/Cargo.toml` and `rom/Cargo.toml` do not depend on `boot-protocol-tests`; the ROM emits no `BootInfo` at all (`docs/spec/boot-protocol.md:69-71`, `rom/src/boot/reset.S`), and the kernel has a private parser.
2. **The private kernel parser reads little-endian.** `kernel/src/main.rs:330` uses `u32::from_le_bytes` for the magic and `:336` for `size`; the spec says the magic is "`0x49374249` (`"I7BI"` big-endian)" (`docs/spec/boot-protocol.md:75`). A spec-conformant image would be rejected.
3. **The map walk indexes the 256-byte struct with an absolute kseg0 address.** `kernel/src/main.rs:370-404` computes `kseg0_map = map_phys + 0x8000_0000` and then indexes `info[kseg0_map as usize + off]`. `info` is `&[u8; 256]`, so any non-zero map address is out of bounds and panics. The path is dead today only because the ROM passes `$a0 = 0`.
4. **The kernel validates only magic, size and checksum.** `kernel/src/main.rs:329-349` checks those three; it does not check `version_major`, `mem_map_entry_size`, `mem_map_count`, overlap, `cmdline`/`r2_url` termination or the image digests. The spec requires all eight checks in order (`docs/spec/boot-protocol.md:183-198`), and says failure halts rather than silently falling back.
5. **The test validator does not test every acceptance case.** It rejects bad magic, version, checksum, map count and kernel-outside-RAM (`crates/boot-protocol-tests/src/lib.rs:234-330`), but there is no bad-size test, no bad `mem_map_entry_size` test and no overlapping-region test. The `Overlap` error is used for containment, not overlap.

Evidence grade: host unit tests only; the live consumer is untested and non-conformant.

### #43 — docs: add install/getting-started guide

**Verdict: partial. Foundation risk: low.**

What is implemented:

- `docs/install.md` exists, is linked from `docs/README.md:22` and `docs/SUMMARY.md:4`, and the Docs workflow builds the book (`.github/workflows/docs.yml`).
- The build and test commands match the CI commands.

What is broken or stale:

- The guide's only emulator instruction is `scripts/rig/task-rig.sh start <item>` (`docs/install.md:69-73`). `scripts/rig` does not exist; `ls scripts/rig` returns "No such file or directory". The same non-existent path is repeated at `docs/spec/architecture.md:310` and `:1872`.
- The guide says `nix develop` is the supported environment (`docs/install.md:27-33`), but that shell is a stable nixpkgs Rust with no `rust-src` and cannot run the documented `-Z build-std` command (see #1).
- The status table still says `M2 — Kernel core | In progress` (`docs/install.md:108-110`), although M2 closed on 2026-10-27 and the tree has M2/M3 evidence.

Evidence grade: docs only; Docs CI green, but the run path is not executable as written.

### #115 — console: unify kernel and user output across serial, telnet and MCP telemetry

**Verdict: implemented. Foundation risk: low.**

What is implemented:

- One `write_bytes` path feeds the graphics console, the 8 KiB serial ring and the serial port, then queues the same bytes for telnet: `kernel/src/main.rs:183-194`, `:138-153`.
- Native user `write(fd=1, ...)` reaches that path: `kernel/src/user.rs:587-607`.
- MCP `serial.read` reads the same ring through the service table: `kernel/src/module.rs:969-975`, `modules/mcp/src/main.rs:1695-1701`.
- Telnet output is queued outside the socket borrow and CRLF-expanded once: `kernel/src/telnet.rs:371-403`.
- The routing rules are documented: `docs/spec/console.md:18-48`.

What is not proven in CI:

- There is no CI assertion that a user image's stdout appears on telnet and in MCP `serial.read`. The boot job only asserts serial text such as `hello from user` (`ci.yml:1214-1219`). PR #120 records the manual telnet/MCP observation, but no test enforces it.

Evidence grade: code + manual real-kernel evidence; no automated cross-surface test.

Reconciliation: the mesh report marks #115 partial because the telnet and MCP observations are ungated. I keep **implemented**. The issue's three acceptance bullets are behavioural: the user write is observable on the documented surfaces, output does not interleave mid-line, and the serial ring remains available. The code above satisfies all three, and PR #120 records the telnet/MCP observation. "Add a test" is in the Proposal, not in the acceptance list. The missing automated assertion is a real test gap and is carried as a follow-up, but it does not make the shipped behaviour partial.

### #116 — boot: emit a machine-readable READY marker

**Verdict: implemented. Foundation risk: low.**

What is implemented:

- `BOOT-READY v=1 boot=<rtc> kernel=up network=<up|off|down> mcp=<listening|local|disabled|failed> mesh=<single|not-ready>` and a distinct `BOOT-NOT-READY ... reason=<token>`: `kernel/src/main.rs:757-805`.
- Emitted once from the boot path: `kernel/src/main.rs:650`.
- Ready/not-ready logic covers loaded, disabled (network-off M1) and failed autoload: `kernel/src/main.rs:761-766`.
- CI waits for the marker rather than incidental strings: `ci.yml:443-446`, `:533-545`, `:1003-1007`.

Caveat:

- The `boot=` field is the RTC Unix second (`kernel/src/main.rs:758`), and the function comment admits it can repeat within the same second (`:752-756`). It is not a monotonic boot counter. This does not break the issue's acceptance, but it weakens "monotonically increasing boot id" in the proposal.

Evidence grade: real-kernel CI.

### #117 — kernel: fix TLB teardown and refill after user mode

**Verdict: implemented. Foundation risk: low.**

What is implemented:

- The self-test uses a VPN disjoint from the user image, anonymous heap and user stack (`0x3000_0000`): `kernel/src/traps.rs:1162-1166`.
- It writes through the uncached kseg1 alias, invalidates all non-wired JTLB entries before and after, and clears the test `EntryHi`/`EntryLo`: `kernel/src/traps.rs:1168-1198`.
- The refill vector only handles the test VPN and otherwise jumps to the fatal reporter: `kernel/src/traps.rs:430-457`.
- `invalidate_all` skips the wired entries: `kernel/src/tlb.rs:154-160`.

What is missing:

- The proposed guest-level regression (`user` then `tlb`, plus a fresh-boot control) is not in CI. `grep` finds no `tlb: self-test` assertion in `ci.yml`; the only caller is the telnet `tlb` command (`kernel/src/telnet.rs:704-706`). The evidence is the PR #120 transcript.

Evidence grade: manual real-kernel evidence; no automated regression.

### #118 — rex3: verify driver rgbmode and FIFO drain semantics

**Verdict: implemented (resolved 2026-11-03). Foundation risk: medium.**

The finding below is the audit of `origin/main` at `cac1a5a`. The resolution
that follows it replaces the shadow check with engine readback plus a host
framebuffer gate; the original text is kept so the divergence it records stays
visible.

What is implemented:

- The driver sets `DRAWMODE1` with the rgbmode bit (`DM1_RGB24 = 0x3000_F319`): `kernel/src/gfx.rs:160-168`.
- `wait_idle` polls `STATUS.GFXBUSY` and the FIFO level and increments wait counters: `kernel/src/gfx.rs:184-210`.
- A `gfx` diagnostic exists and is reachable from the telnet console: `kernel/src/gfx.rs:455-504`, `kernel/src/telnet.rs:707-709`.

Why the verdict is divergent:

- The self-test's "all three colour channels" check reads the **CPU shadow**, not the REX3 framebuffer. `fill_span` writes the shadow through `shadow_put` (`kernel/src/gfx.rs:429-445`), and the test reads it back with `shadow_read` (`kernel/src/gfx.rs:475-479`). The shadow is a CPU-side buffer at kseg0 `0x8870_0000` (`kernel/src/gfx.rs:35-41,58-66`). If the REX3 ignored rgbmode and only wrote red, the shadow would still read `[3, 2, 1]` and the test would still pass.
- The FIFO check is `calls >= 2` (`kernel/src/gfx.rs:470-474`). `fill_span` calls `wait_idle` once and the test calls it again, so two calls are guaranteed; the test does not fill the FIFO or show that a wait was needed. The issue asks that the wait path be exercised, not that a counter increments.
- The diagnostic is not run in CI, so even its shadow-only check is not a regression gate.

The issue's acceptance says: "The OS self-test fails if `rgbmode` is cleared or a fill only reaches the red channel" and "The driver's FIFO wait path is exercised, not bypassed". The code cannot satisfy the second half of the first sentence and does not fill the FIFO for the second.

Evidence grade: manual iris diagnostic; no hardware or CI evidence.

Resolution (2026-11-03):

- **Engine readback, not the shadow.** The self-test reads `DRAWMODE1`,
  `COLORRED`/`COLORGRN`/`COLORBLUE`, `WRMASK` and the staged
  `XYSTARTI`/`XYENDI` back from the REX3 register window
  (`kernel/src/gfx.rs`, `self_test`). `rgbmode`, the RGB planes, 24-bit depth
  and the SRC logicop are checked in the word the engine returns; a fill with
  only the red channel staged fails on the green/blue readbacks. The CPU
  shadow is no longer part of the check.
- **FIFO burst.** 1024 full-width span GOs are issued with no intervening
  wait, followed by a full-screen block fill, and only then does the test wait.
  `wait_idle` records polls, busy polls and the deepest level it saw
  (`WaitStats`). The test requires `busy_polls > 0` and a final
  `GFXBUSY=0`/level 0; that is evidence the wait was needed, not a call count.
- **Framebuffer gate.** Newport has no CPU aperture, so the pixel half is
  host-side: `kernel/src/graphics_console.rs` repaints a fixed probe rectangle
  after every console repaint, and `tools/rex3-gate.py` boots the ROM under
  iris, runs `gfx` over telnet, cross-checks the guest readback against iris's
  independent `rex status` dump, then reads the probe back from
  `rex fbdump`'s `rgb.bin`. All three channels must be present at five points
  in the rectangle.
- **Negative control.** `gfx fault` clears `rgbmode` in the engine and paints
  the probe in colour-index mode; the gate requires that same pixel check to
  observe a single-channel word (`rgb(42, 0, 0)`) and then requires a following
  `gfx` to restore the RGB probe. A build with the rgbmode bit cleared in
  `DM1_RGB24` fails the gate at the guest check (`rgbmode=CLEAR`, `FAILED`);
  a build that stages only `COLORRED` fails it at `colour=BAD`.
- **Evidence.** `tools/rex3-gate.py` passes on iris (three consecutive runs):
  probe `rgb(17,34,51)` at five points, fault pixel `rgb(42,0,0)`, `busy_polls`
  > 0, `GFIFO 0/65536`, `GO TOTAL +1026`. Remote-Indy evidence is still owed:
  a real REX3 has no readback path, so on hardware the probe has to be checked
  on the monitor and only the register half is automated. iris also queues
  64 K commands, so it cannot reproduce the 32-entry FIFO's dropped or stalled
  writes; see `docs/plan/testing.md` section 5.

### #119 — ci: replace sleep-based guest startup with readiness polling and cleanup

**Verdict: implemented. Foundation risk: low.**

What is implemented:

- The guest harness polls the READY marker and treats `BOOT-NOT-READY` as a diagnostic: `rom/tools/smoke.py:75-90`.
- `stop_iris` terminates and waits, escalating to kill: `rom/tools/smoke.py:93-108`.
- Per-run temporary directories and cleanup are used: `rom/tools/smoke.py:160-180`, `:218-235`.
- CI boots use per-run `$RUNNER_TEMP` directories and `trap cleanup EXIT`: `ci.yml:423-438` and repeated in each boot step.
- `tools/wait-serial-log.sh` polls a file with a bounded deadline instead of a fixed sleep: `tools/wait-serial-log.sh:18-28`.

Residual issues:

- The workflow still uses `sleep 2` for the MCP `/modules` HTTP retry (`ci.yml:721-726`, `:869-874`). That is a retry, not a startup sleep, but it is a fixed wait in a gate.
- There is no CI boot whose purpose is a failed boot followed by a process/listener assertion. The Python unit tests cover the not-ready parser, not an iris process.

Evidence grade: harness unit tests + CI usage.

### #204 — M1-T1: complete the Wasm host ABI probe

**Verdict: implemented. Foundation risk: medium.**

What is implemented:

- The probe imports only `i7host` and calls version, clock, submit, poll, cancel, cap-check, console write and yield: `guest/wasm-policy/src/lib.rs:32-60`, `:91-110`, `:220-265`.
- The host implements the surface: `crates/wasm-host/src/lib.rs:294-711`.
- The guest-import test rejects any non-`i7host` import: `crates/wasm-host/src/lib.rs:822-840`.
- The module is built under the 1.5 MiB slot-B limit and booted in CI with MCP in slot A: `ci.yml:273-289`, `:533-545`.
- Host tests pass: `cargo test -p wasm-host` → 5 passed.

Shortcut:

- `host_submit` is a synthetic completion queue: it allocates an event, pushes a `Status::Ok` completion and returns the event id (`crates/wasm-host/src/lib.rs:307-329`). `host_cancel` ignores the token and returns 0 (`:360-365`). The probe proves the call/return ABI, not a real kernel completion or cancellation path. That is acceptable for a probe, but it must not be read as a working async service.

Evidence grade: host tests + real-kernel CI.

### #205 — M1-T2: run the local policy from Wasm

**Verdict: implemented. Foundation risk: medium.**

What is implemented:

- The guest chooses the namespace layout, object bytes, actor, capability rights and home, and commits the stage as one transition: `guest/wasm-policy/src/lib.rs:121-265`.
- The host stages every mutation and publishes only on commit: `crates/wasm-policy-host/src/lib.rs:299-347`; `abort_leaves_no_partial_policy` passes.
- The guest imports only `i7host`; the import test passes (`crates/wasm-host/src/lib.rs:822-840`).
- CI checks the `m1-wasm:` evidence, distinct root, distinct actor, equal rights/home and `objects=9 actors=1 queued=1 caps=1`: `ci.yml:556-584`.

Residual risk:

- The module runs in kernel mode and holds the `Services` table; the guest is sandboxed by wasmi, but the module itself is a full kernel extension. "No ambient authority" is true for the guest, not for the containing module.
- The policy is module-local until the kernel adopts the bridge; it is not a resident policy service.

Evidence grade: host fixture + real-kernel CI.

### #206 — M1-T3: final M1 gate and hardware evidence

**Verdict: implemented. Foundation risk: high.**

What is implemented:

- The M1 gate is asserted in CI: `BOOT-READY network=off mcp=disabled`, `m1:`/`m1-post` zero counters, the Wasm policy evidence, target-2, POSIX/std, native hello and o32: `ci.yml:442-451`, `:533-545`, `:940-1007`, `:1011-1144`, `:1146-1303`.
- CI run `40244974653` is green, with `Boot (iris)` passing.
- The real-Indy limitation is recorded in `CHANGELOG.md:236-240` and the milestone plan (`docs/plan/milestones.md:153-158`).

Why the risk is high:

- The issue allows a recorded limitation, but the plan's hard gate says M1 and M2 must run on real kernels (`docs/plan/milestones.md:35`). The gate is a serial-log assertion against iris, not a hardware run.
- The gate depends on #213's live-policy claim. #213 is partial in this audit (see below). The final M1 gate therefore certifies a narrower bridge than the issue title suggests.

Evidence grade: real-kernel CI (iris) only.

### #213 — M1-T4: make the Wasm policy the live kernel policy

**Verdict: partial. Foundation risk: high.**

What is implemented:

- The kernel adopts the guest bridge for the userland `/etc/motd` and `/dev/null` read/device paths: `kernel/src/logical.rs:127-149`, `:151-216`, `:232-260`.
- The guest root is checked and reads go through the guest `lookup`/`get`/`authorize`: `kernel/src/logical.rs:151-180`.
- The guest actor is adopted into the kernel actor table/run queue before userland: `kernel/src/logical.rs:135-141`, `kernel/src/logical.rs:91-112`.
- CI asserts the guest and native roots/actors are distinct, rights match at 35 and home matches: `ci.yml:556-584`.
- The offline std boot asserts `m1-live: wasm-policy adopted`: `ci.yml:1070`.

What is missing or divergent:

1. **The kernel's own root and capability state are not the guest's.** `Local::start` keeps the native `root` and allocates a native capability over `(1, root_id)` (`kernel/src/logical.rs:304-318`). The `m1:` report still prints the native root (`:334-341`). CI explicitly requires the guest root to differ (`ci.yml:567-573`). Only the read/device path uses the guest root; the kernel capability table does not.
2. **The failed-guest AC is not met on reload.** The acceptance says "A failed/trapped guest leaves the previous live policy unchanged." `module::clear(Slot::B)` calls `logical::forget_wasm()` **before** the new image is fetched or verified (`kernel/src/module.rs:149-159`). If a reload fails, the previous bridge is already dropped and the native fallback takes over. The previous live policy is not unchanged.
3. The bridge is a read-only function table at a fixed address, not the kernel's resident namespace/actor/capability authority. The issue's own final comment narrows the claim to the M1 path; that is fair, but the title still overstates it.

Evidence grade: real-kernel CI + host tests.

Reconciliation: the mesh report marks #213 implemented because the guest policy path is live. I keep **partial**. The bridge is live for `/etc/motd`, `/dev/null` and the adopted actor, but the issue's first acceptance bullet asks for the live kernel policy root, actor, capability rights and home to be the guest's values. The kernel's own `Local.root` and native capability table are still native (`kernel/src/logical.rs:304-318`), the `m1:` line still prints the native root (`:334-341`), and CI explicitly requires the guest root to differ (`ci.yml:567-573`). The second bullet also fails on reload: `module::clear(Slot::B)` drops the previous bridge before the replacement is fetched (`kernel/src/module.rs:149-159`). A live guest read path is not the same as the kernel policy being adopted, so the narrower M1 bridge does not satisfy the ticket as written.

### #214 — M1-T5: record optional Wasm load time

**Verdict: implemented. Foundation risk: low.**

What is implemented:

- Both the local-SCSI and network paths time the slot-B load and print `autoload: wasm took <n> ms`: `kernel/src/autoload.rs:70-89`, `:112-130`.
- The module prints `wasm: runtime=<n> ms`: `modules/wasm/src/main.rs:492-497`.
- The numbers and the "observation, not a gate" decision are recorded in `docs/plan/testing.md:214-222`.

This is an observation, not a gate, exactly as the issue says. No code gap.

Evidence grade: manual measurement + docs.

### #217 — Module system: runtime dependencies and manifests

**Verdict: implemented. Foundation risk: medium.**

What is implemented:

- Exact-version manifests, the `I7MM` codec, the optional `I7MF` footer, a host resolver and a manager with atomic verification and reference counting: `crates/module-resolver/src/lib.rs`, `manager.rs`, `runtime.rs`.
- All-or-nothing loading and dependency-before-dependent execution: `crates/module-resolver/src/runtime.rs:1-12`, `:609`; tests `dependencies_load_and_execute_before_the_root` and `load_failure_unloads_everything_and_never_executes`.
- 62 module-resolver tests pass: `cargo test -p module-resolver` → 62 passed.
- The kernel loader and arena are in place: `kernel/src/module.rs:60-67`, `:375-410`; five runtime regions of 256 KiB.
- Real runtime graphs on iris: `demo-dep`→`demo-root` and `m2-mesh`→`m2-root`, asserted in `ci.yml:706-729`, `:858-878`.
- CI run `40323786494` / `40323786275` on `8203b3d` is green.

Residual risks:

1. **The M1 bridge sits inside runtime arena region 4.** Slot B is `0x8848_0000..0x8860_0000` (`crates/m1-policy-bridge/src/lib.rs:13-15`), and the bridge is at `SLOT_B_BASE + SLOT_B_SIZE - sizeof(Bridge)` = `0x885f_ffd8` on n32 (`crates/m1-policy-bridge/src/lib.rs:45-48`). The runtime arena is `0x884c_0000 + 5 * 0x4_0000` = `0x8860_0000` (`kernel/src/module.rs:61-65`), so the bridge is in the last 40 bytes of region 4. `module_load` accepts any region `0..5` (`kernel/src/module.rs:380-389`). A manager that assigns region 4 will overwrite the live M1 bridge. The current release graph uses regions 0–3 (`packages/runtime-modules.toml`), so this is latent, not currently failing.
2. **The M2 MCP listener still links `mesh-transport` at build time.** `modules/mcp/Cargo.toml:22` has a direct `mesh-transport` dependency; the issue comment records this as a follow-up under #3. The runtime graph infrastructure is real, but the M2 control plane is not yet a consumer of the exported table.
3. Nine earlier `gh` comments describe the host-only stages; the final runtime gates are merged, so this is history, not a gap.

Evidence grade: host tests + real-kernel CI.

### #271 — m1: the Wasm-autoload boot traps when the kernel's service table grows

**Verdict: implemented. Foundation risk: medium.**

What is implemented:

- The kernel stack top moved from `0x8830_0000` to `0x883f_0000`, and the ROM constant moved with it: `kernel/linker.ld:52-67`, `rom/src/boot/reset.S:31-33`.
- The linker asserts both `.bss < __stack_top` and `__stack_top <= 0x8840_0000`: `kernel/linker.ld:66-67`.
- The root cause and the five-run/five-pass verification are recorded in the issue and in `docs/adr/m3-0006-udp-transport.md`; CI run `41179573043` is green, including `Boot (iris)`.

Residual risks:

- `#272` is still open: nothing bounds native stack depth. The fix moves the floor; it does not detect a deeper future guest.
- Stale documentation still says the stack top is `0x8830_0000`: `docs/spec/modules.md:91`, `docs/spec/userland-services.md:98`. That is the value that caused the overflow.

Evidence grade: real-kernel CI.

### 3.17 Reconciliation with the mesh report

Eight core issues were reviewed in parallel by the mesh report: `#115, #204, #205, #206, #213, #214, #217, #271`. Two verdict disagreements were referred for reconciliation:

- **#115** — this report keeps **implemented**; the mesh report's **partial** is not adopted. The acceptance criteria are behavioural and the code plus PR #120 evidence meet them. The missing telnet/MCP assertion is a test gap, not a missing feature.
- **#213** — this report keeps **partial**; the mesh report's **implemented** is not adopted. The guest bridge is live, but the kernel's own root and capability table remain native, and a failed slot-B reload drops the previous policy before verification. Both contradict acceptance bullets.

The other six were read against the mesh report; this report's verdicts stand. The mesh report owns its remaining findings.

## 4. Cross-cutting findings

### 4.1 BootInfo is a paper contract

The boot protocol is the most serious foundation gap in this set.

- `docs/spec/boot-protocol.md:68-71` says the contract is checked by compile-time assertions "in both codebases". Neither the ROM nor the kernel links the assertion crate.
- The kernel's parser reads little-endian (`kernel/src/main.rs:330,336,342`), so it would reject a spec-conformant big-endian `BootInfo`.
- The map walk indexes a 256-byte array with an absolute kseg0 address (`kernel/src/main.rs:370-404`), which is an out-of-bounds panic on any non-empty map.
- The kernel checks 3 of the 8 required validation steps (`kernel/src/main.rs:329-349` versus `docs/spec/boot-protocol.md:183-198`).
- The nine tests in `crates/boot-protocol-tests` pass, but they test a third implementation, not the kernel's.

This is a latent failure: the ROM passes `$a0 = 0`, so the code path is dead. It becomes a boot failure as soon as a loader emits `BootInfo`, which the spec says is the intended next step.

### 4.2 The "live Wasm policy" is a bridge, not the kernel policy

\#205, #213 and #2 document a real slot-B policy with a committed root, actor, capability and home. The live kernel path for `/etc/motd` and `/dev/null` does use it. But:

- The kernel's own `Local.root` and native capability remain the native values (`kernel/src/logical.rs:304-331`); CI deliberately requires the guest root to differ (`ci.yml:567-573`).
- The kernel does not import the guest capability into its capability table; `authorize` on the bridge is the only check for the guest path.
- A failed reload drops the previous bridge before fetching the replacement (`kernel/src/module.rs:149-159`), contrary to #213's "failed guest leaves the previous live policy unchanged".

The result is a **live read path**, not a live kernel policy object graph. That is a valid M1 slice, but it must not be described as resident policy authority. The plan defers the resident core to Phase 5 (`docs/plan/milestones.md:150-153`), and #213 should say so in its own acceptance text.

### 4.3 The M1 bridge and the module arena overlap

This is a concrete address-space conflict introduced by combining #213 and #217.

- Bridge address: `0x8848_0000 + 0x18_0000 - sizeof(Bridge)` = `0x885f_ffd8` on n32 (`crates/m1-policy-bridge/src/lib.rs:13-15,45-48`).
- Runtime arena: `0x884c_0000 + 5 * 0x4_0000` = `0x8860_0000` (`kernel/src/module.rs:61-65`). Region 4 is `0x885c_0000..0x8860_0000`.
- `module_load` accepts region indices `0..5` and writes a full 256 KiB region (`kernel/src/module.rs:380-389`, `:402`).
- The checked-in release graph uses regions 0–3 (`packages/runtime-modules.toml`), so the collision is latent. Any future package that uses region 4 will overwrite the live bridge function pointers and `context`.

The fix is either to reserve region 4 for the bridge, move the bridge into its own page, or make the manager reject region 4. The current code has no reservation.

### 4.4 Real hardware is still absent from the M1/M2 foundation

`docs/plan/milestones.md:35` says "M1 and M2 must run on real kernels." The M1 issue permits a recorded limitation and records it in `CHANGELOG.md:236-240` and `#16`. `#16` is open; both Indys are uncharacterised. Every M1 core claim — SCC, Seeq DMA, SCSI, REX3, DS1386 timing/cache — is iris-only. This is the largest evidence gap in the closed set; it is recorded honestly, but it means M1 is not a hardware gate.

### 4.5 The documented Nix devshell cannot build the kernel

`flake.nix:26-31` uses nixpkgs `rustc`/`cargo`, not rustup. `rust-toolchain.toml` is ignored by that toolchain. `nix develop --command cargo build -Z build-std…` fails with "the option `Z` is only accepted on the nightly compiler", and the sysroot has no `rust-src`. The install guide tells a new user to use this shell (`docs/install.md:27-33`), so the documented path and the actual M0 gate diverge.

### 4.6 The M1 zero-mesh counter is now a constant

`kernel/src/mesh.rs:41-74` still has no transport caller. The `m1`/`m1-post` lines report `mesh=0/0/0` after a self-test that increments and resets the counters. At M1 closure there was no transport; today `modules/mcp` contains a mesh transport and `m2-mesh` exports one. The kernel counter is therefore not a whole-system "zero mesh RPC" measurement. The issue body says this honestly ("construction invariant, not packet-level instrumentation"), but the marker is still read as stronger evidence than it is.

### 4.7 The REX3 self-test does not test the REX3 (resolved)

At the audit base, `kernel/src/gfx.rs:475-479` read the CPU shadow. The shadow is written before the REX3 command (`:429-445`), so the test passed even if the hardware ignored rgbmode. The FIFO check counted `wait_idle` calls rather than filling the FIFO. This was the one closed issue whose evidence directly contradicted its acceptance text.

Resolved on 2026-11-03: the self-test now reads the engine's registers and `tools/rex3-gate.py` gates the engine's framebuffer; see the resolution under #118.

### 4.8 Documentation drift around the stack

- Stack top: `docs/spec/modules.md:91` and `docs/spec/userland-services.md:98` still say `0x8830_0000`; the fix is `0x883f_0000` (`kernel/linker.ld:63`).
- Install status: `docs/install.md:108-110` says M2 is in progress; M2 is closed.

These are low-risk but they mislead the next bring-up.

## 5. Recommended follow-ups

Ranked by foundation risk.

1. **Reopen #33 and fix the live BootInfo path.** Make the kernel read big-endian, walk the map through a real pointer rather than indexing `&[u8; 256]`, implement the eight validation steps or delete the dead path, and make the conformance crate the single source of the layout used by both ROM and kernel. Add a host test that feeds the kernel parser a synthetic valid/invalid `BootInfo`. Until this is done, the boot protocol is a draft, not a contract.

2. **Reopen #217 (with #213) and reserve the M1 bridge address.** Region 4 of the runtime arena overlaps the bridge at `0x885f_ffd8`. Reserve region 4, move the bridge, or reject region 4 in `module_load`; add a test that loads a module into every arena region while the bridge is live. Also decide whether a failed slot-B reload may drop the previous policy; #213's acceptance says it must not.

3. **Make #213's claim match the code, or finish the adoption.** Either import the guest root/capability into the kernel's live `Local` state and prove it in `m1:`, or amend #213 to say "the userland read path is guest-driven; the kernel capability table remains native". Do not close a future resident-policy ticket on the current bridge.

4. **Close the real-hardware gap in #16.** The plan's hard gate says M1 and M2 must run on real kernels. At minimum, record a dated M1 boot on an Indy with SCC/Seeq/SCSI/REX3 output; until then, keep every M1/M2 claim labelled iris-only. #2/#206 should carry the wording change.

5. **Reopen #1 and fix the default devshell or document the rustup path.** The Nix shell must either use rustup with `rust-toolchain.toml`/`rust-src` or stop claiming to be the canonical cross toolchain. Update `docs/install.md` to the working command. Record the LLVM-only decision against ADR-0018 and `docs/spec/architecture.md:274`, or revive the GCC backend.

6. ~~**Reopen #118 and test the framebuffer, not the shadow.**~~ Done: engine register readback, a 1024-command FIFO burst, and `tools/rex3-gate.py`, which reads the probe back from iris's REX3 framebuffer and requires the colour-index fault to fail the same check. The real-Indy readback limit is recorded in `docs/plan/testing.md` section 5.

7. **Update #43's install guide.** Replace `scripts/rig/task-rig.sh` with the actual `iris`/`iris-ci` commands, update the status table (M2 closed), and fix the `nix develop` instruction. Link the real CI boot commands from `.github/workflows/ci.yml`.

8. **Update #271's stale stack-top references.** `docs/spec/modules.md:91` and `docs/spec/userland-services.md:98` still document the value that caused the overflow. Add the `#272` stack-depth guard to the same follow-up.

9. **Make #116's `boot=` field monotonic.** Use a boot counter or combine the RTC second with a reset cause; the current field can repeat within a second. This is low risk but cheap to fix.

10. **Retire or instrument the M1 mesh counters (#2/#3).** Either move `note_frame`/`note_rpc` call sites into the module transport boundary and make `m1-post` a whole-system measurement, or label the marker explicitly as "kernel counters only" and stop citing it as a zero-mesh proof.

11. **Add the telnet and MCP `serial.read` assertion for `userelf` stdout (#115).** The unified path is implemented, but the acceptance observation is only manual. Boot `userelf`, read the same bytes from serial, telnet and the MCP ring, and check the CRLF framing at a line boundary.

## 6. Bottom line

The core M1 slice is real on iris: the assembly ROM hands over, the kernel owns the console, the local policy starts, the Wasm policy boots in slot B, and the module resolver loads a real dependency graph.

The weak foundations are:

- the boot contract is not consumed by either side and the consumer is non-conformant;
- the "live" Wasm policy is a bridge whose address overlaps the runtime arena and whose failed-reload behaviour contradicts its acceptance;
- M1/M2 are still iris-only, against the plan's real-kernel gate;
- the documented Nix toolchain cannot build the kernel;
- the REX3 self-test now reads the engine, but its pixel half runs under iris only: a real Indy has no framebuffer readback.

Those are the items that should carry the next round of work before the one-logical-computer plan treats M1 as a settled foundation.
