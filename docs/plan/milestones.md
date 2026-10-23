# IRIX 7 — Milestone Plan

**Status:** living document
**Strategy:** walking skeleton. Every milestone is a vertical slice with a demo, not a horizontal platform layer.
**Companion:** [`../spec/architecture.md`](../spec/architecture.md), [`github.md`](github.md), [`risks.md`](risks.md).

> **See it grow:** [The Tree of IRIX 7](../roadmap-tree.md) is the interactive view of this plan.

## 1. Summary

| ID | Milestone | Demo | Estimate | Release |
|---|---|---|---|---|
| M0 | Toolchain unblocked | CI cross-compiles a `no_std` binary for n32 and o32; tracer bullet prints on iris | 2–4 weeks | — |
| M1 | ROM netboot | Assembly ROM tests DRAM, decompresses the n32 kernel and jumps; kernel does DHCP/DNS/NTP/HTTP/HTTPS, console and modules | 4–6 weeks | — |
| M2 | Kernel core | Timer ticks, page fault, panic screen, scheduler | 6–10 weeks | — |
| M3 | Network + MCP + shell | Agent connects over HTTP, runs a command, reads serial, captures screen | 4–6 weeks | v0.1 |
| M4 | SCSI + block + EROFS | `cat /etc/motd` from the R2 root image | 8–12 weeks | — |
| M5 | First IRIX binary | An o32 IRIX `hello` prints and exits | 8–12 weeks | v0.2 |
| M6 | Display + native desktop | A native window with text, cursor, damage repair | 8–12 weeks | v0.3 |
| M7 | X11 | `xterm` and `twm` under a display lease | 12–20 weeks | v0.4 |
| M8 | XFS read-only | `ls` an IRIX 6.5 disk | 8–12 weeks | v0.5 |
| M9 | Native cell + capability ABI | A Rust cell opens a file and prints | 8–12 weeks | v0.6 |
| M10 | Real-Indy release | A real Indy boots from R2, runs MCP/X11/XFS, updates | 8–12 weeks | v1.0 |

**Rough total:** 76–118 person-weeks (1.5–2.3 years for one full-time person, sequential). Parallel work on X11, XFS and the native ABI can compress the calendar; the critical path is M0 → M1 → M2 → M3 → M4 → M5 → M7 → M10.

The estimates are planning ranges, not commitments. They exclude holidays, hardware repairs and the inevitable "the emulator is wrong" detours.

## 2. Release map

| Release | Contents | Audience |
|---|---|---|
| v0.1 | Serial, network, MCP, shell, reset/watchdog | The developer/agent |
| v0.2 | First IRIX o32 binary, minimal libc | Compatibility testers |
| v0.3 | Newport display, native desktop shell | Early users |
| v0.4 | X11 lease, `xterm`/`twm`, Xlib subset | IRIX application testers |
| v0.5 | XFS read-only, IRIX disk access | Data-recovery users |
| v0.6 | Native cells, capability ABI, native libc | Rust developers |
| v1.0 | Real-Indy daily driver, updater, docs, signed release | The project |

## 3. Cross-cutting workstreams

These run in parallel with every milestone and are tracked as their own epics:

- **Clean-room and provenance** — harness, similarity screen, DCO, access control for `irix6`.
- **CI and automation** — cross-compile, emulator boot, smoke harness, hardware-in-the-loop, releases.
- **Documentation** — user, admin, install, troubleshooting; interface specs; ADRs.
- **Security** — threat model, MCP modes, supply chain, vulnerability disclosure.
- **Hardware lab** — the two Indys, serial, SCSI emulators, spares, backups.
- **Performance and benchmarks** — tracked, never gated.
- **Community** — contributing, governance, good-first issues, release notes.

---

## M0 — Toolchain unblocked

**Goal:** make the existing toolchain reliably build the kernel and ROM for n32/MIPS III
and produce a reproducible `no_std` artefact.

**Demo:** CI cross-compiles a `no_std` binary for `mips3-sgi-irix7-n32`, and the existing
tracer bullet boots on iris and prints its marker.

**Acceptance criteria:**

- [x] `-Z build-std` works for `core`/`alloc` on the custom target.
- [ ] `rustc_codegen_gcc` builds and produces working code for n32.
- [x] The LLVM path works for the custom target.
- [x] The Nix devshell pins rustc, GCC, binutils and the target spec.
- [x] A clean checkout can build the tracer bullet without manual steps.
- [x] CI caches the toolchain and the target artefacts.
- [x] The disk-space blocker is resolved or worked around.
- [ ] The build is reproducible (same inputs → same hashes).

**Dependencies:** access to `irix7/project`, the Nix flake, the captured sysroot, iris.

**Estimate:** 2–4 weeks.

**Risks:** host-toolchain glue; GCC-backend disk space; nightly drift.

**Out of scope:** kernel features, drivers, userspace.

---

## M1 — ROM netboot

**Goal (as delivered):** a MIPS III assembly PROM replacement that sizes and tests DRAM,
brings up the serial console, LZ4-decompresses the n32 kernel to `0x8804_0000` and jumps to
it. Networking, the console and the clock are initialised once in the kernel.

**Demo:** in iris the raw PROM image prints the boot banner, tests 32 MiB of RAM, decompresses
the kernel and prints `kernel: up`; DHCP and DNS complete; the telnet console answers and
survives `reboot`. `https example.com:443/` verifies the certificate chain and reports a body
digest matching the host.

**Acceptance criteria:**

- [x] Serial console works from the first instruction.
- [x] RAM is sized and tested stacklessly before any RAM structures exist.
- [x] The kernel is decompressed from PROM and entered at `0x8804_0000`.
- [x] Seeq 8003 + HPC3 PDMA; DHCPv4 with renewals; DNS; NTP; ICMP.
- [x] HTTP/1.1 and HTTPS/TLS 1.3 with certificate-chain verification.
- [x] SHA-256 over fetched bodies, and a digest gate for modules.
- [x] Telnet console with its command set, surviving boot and reconnect.
- [x] Loadable modules fetched over HTTPS into RAM.
- [x] The image fits in the 512 KiB PROM window (about 476 KiB).
- [x] The handoff is documented in [`../spec/boot-protocol.md`](../spec/boot-protocol.md);
      `BootInfo` is optional and the current ROM passes registers.
- [ ] HTTP Range support.

**Note:** the design changed during M1. The ROM started as a Rust netboot client; it is now
an assembly stub that embeds the kernel, and the network stack and console live in the kernel.
Because the PROM window is the binding constraint, new features are modules downloaded over
HTTPS rather than growth of the compressed image.

**Dependencies:** M0, iris (or an Indy), a local HTTP server for modules.

**Estimate:** 4–6 weeks.

**Risks:** Seeq quirks; kernel load address and `BootInfo` layout; cache flush before the
jump; ROM size; 10 Mbit timeouts; TLS.

**Out of scope:** HTTP Range, the full boot bundle, storage.

---

## M2 — Kernel core

**Goal:** the privileged framework: traps, TLB, caches, CP0 timer, frame allocator, scheduler,
capability table, panic path and serial console.

**Status:** in progress — the kernel links at `0x88040000`, consumes `BootInfo` in `$a0`,
uses a 2-entry memory map, clears `.bss`, sets up the stack, and prints `kernel: up`.
The telnet ROM console remains live after the hand-off via the resident console poll.

**Demo:** the kernel prints the ARCS memory map, calibrates the timer, runs two kernel tasks
with EDF and fair classes, handles a deliberate page fault, and panics cleanly with a
register dump.

**Acceptance criteria:**

- [x] Kernel links at `0x88040000` and consumes `BootInfo` in `$a0`.
- [x] Kernel memory map reserves ROM live state; usable RAM starts at `0x08B00000`.
- [x] Kernel clears `.bss`, sets up stack, uses 2-entry memory map.
- [x] Kernel prints `kernel: up`; serial console works after hand-off.
- [x] Telnet ROM console remains live after kernel boots (resident console poll).
- [ ] Exception vectors, TLB refill and `eret` work on MIPS III.
- [ ] CP0 `Count`/`Compare` calibrated against the 8254 reference.
- [ ] Frame allocator from the ARCS memory descriptor list; no probing.
- [ ] 16 MiB kseg2 mapping works; kseg0 direct map is used for the kernel.
- [ ] Capability table with generation counters and rights.
- [ ] EDF and fair run queues; `is_running` debug assertion.
- [ ] Panic screen with registers, stack trace and a serial dump.
- [ ] The `ctl`/`data`/`status` driver interface is specified and unit-tested.
- [ ] Real kernel idle path / throttle so an idle guest does not peg host CPU
      (follow up on iris idle-CPU issue).

**Dependencies:** M1.

**Estimate:** 6–10 weeks.

**Risks:** TLB/cache corner cases; exception vector placement; no full IOMMU; stack usage;
idle guest pegging host CPU (iris idle-CPU issue).

**Out of scope:** drivers, userspace, filesystems.

---

## M3 — Network + MCP + shell

**Goal:** the development harness: a de-privileged network cell, a de-privileged MCP cell, and
a shell. This is a deliberate MCP-first deviation from the hardware bring-up order: the
control plane is how everything else gets built.

**Demo:** an agent connects to the Indy over Streamable HTTP with a bearer token, runs
`uname`, lists files, reads the serial log, captures the framebuffer, and reboots the machine.

**Prototype:** `modules/httpd` (loaded over HTTPS into RAM) serves JSON-RPC 2.0
`initialize`, `ping`, `tools/list` and `tools/call` at `POST /mcp`, with the `status`,
`echo`, `console` and `serial.read` tools. The iris rig forwards host 8088 to the module's guest port 80.
It has no bearer token, SSE, shell or screen/serial tools yet.

**Acceptance criteria:**

- [ ] Network cell owns the Seeq, exposes `/net`, and survives a reset.
- [ ] MCP `initialize`, `tools/list`, `tools/call`, `ping` and progress notifications.
- [ ] Tools: `shell.exec`, `fs.read`, `fs.write`, `fs.list`, `serial.read`, `serial.write`,
      `screen.capture`, `system.reset`, `system.poweroff`.
- [ ] Bearer token authentication; no TLS in v1; binds to the LAN interface.
- [ ] Shell: `help`, `ls`, `cat`, `echo`, `uname`, `ps`, `reboot`, `poweroff`.
- [ ] `system.reset` is guarded by the watchdog and returns after the reboot.
- [ ] The agent loop is documented and reproducible.

**Dependencies:** M2, R2/local HTTP, a host-side MCP client.

**Estimate:** 4–6 weeks.

**Risks:** MCP prompt injection; full root; no audit; network driver bugs.

**Out of scope:** SCSI, filesystems, display, X11.

---

## M4 — SCSI + block + EROFS

**Goal:** read the root image from R2 over HTTP Range through a userspace block/FS stack.

**Demo:** the shell runs `cat /etc/motd` and `ls /` from the EROFS root image; a write to
`/tmp` goes to the overlay; the machine boots with no SCSI disk.

**Acceptance criteria:**

- [ ] SCSI cell initialises the WD33C93 and reads blocks.
- [ ] Block cell exposes a block-device capability and an LRU cache.
- [ ] EROFS reader mounts the standard little-endian image; byte-order codec tested.
- [ ] HTTP Range client fetches 64–256 KiB blocks with read-ahead.
- [ ] SHA-256 verifies the image before mount.
- [ ] Overlay supports `/tmp`, `/etc`, `/home` (RAM; optional local disk).
- [ ] The boot path works with the disk absent and with the disk present.

**Dependencies:** M3 (network), M2 (framework).

**Estimate:** 8–12 weeks.

**Risks:** SCSI DMA/direction; EROFS corner cases; R2 latency; 10 Mbit throughput.

**Out of scope:** XFS, I7FS, write-back to R2.

---

## M5 — First IRIX binary

**Goal:** run one real IRIX 6.5 o32 binary unchanged.

**Demo:** an o32 `hello` binary from the IRIX 6.5.7m sysroot prints `hello, IRIX 7` and exits
with status 0; the shell reports the exit status.

**Acceptance criteria:**

- [ ] ELF loader maps the o32 image and its interpreter/libs.
- [ ] o32 syscall decode: `write`, `exit`, `open`, `close`, `read`, `brk`, `getpid`.
- [ ] Minimal IRIX libc: `write`, `exit`, `open`, `close`, `read`, `malloc`/`free`, `printf`.
- [ ] Argument and environment passing; stack layout correct.
- [ ] `errno` mapping; exit status propagation.
- [ ] The test binary and expected output are in the compatibility corpus.
- [ ] The clean-room provenance of the binary and the libc is recorded.

**Dependencies:** M2, M4.

**Estimate:** 8–12 weeks.

**Risks:** ABI details; libc surface; stack/register conventions; big-endian.

**Out of scope:** n32, `fork`/`exec`, signals, Motif, X11.

---

## M6 — Display + native desktop

**Goal:** a native display server and a minimal IRIX-inspired desktop: window, text, cursor,
damage repair, 8-bit indexed.

**Demo:** a native window shows `hello, IRIX 7`, moves, raises, and redraws with damage
tracking; the hardware cursor moves; the desktop has a launcher and a clock.

**Acceptance criteria:**

- [ ] Newport REX3 blit, fill, line and text paths work.
- [ ] VC2 mode programming (1024×768 or 1280×1024) and XMAP9 palette.
- [ ] Hardware cursor in the XMAP9 auxiliary plane.
- [ ] Retained window images and damage-driven repair.
- [ ] Display lease for a full-screen cell.
- [ ] Input: keyboard and mouse events.
- [ ] A minimal native desktop: launcher, window manager, clock, terminal.
- [ ] The display protocol is documented in [`../spec/display-protocol.md`](../spec/display-protocol.md).

**Dependencies:** M2, M4.

**Estimate:** 8–12 weeks.

**Risks:** undocumented VRAM size; REX3 FIFO depth; 24-bit planar mode; input.

**Out of scope:** X11, GL, video capture/playback, 3D.

---

## M7 — X11

**Goal:** run real IRIX X clients under a clean-room Rust X11 server in display-lease mode.

**Demo:** `xterm` and `twm` run under X11 on the Indy; the keyboard and mouse work; the
server survives an X client crash.

**Acceptance criteria:**

- [ ] X11 connection handshake, MSBFirst, setup reply.
- [ ] Core protocol: windows, GCs, pixmaps, fonts, events, properties, selections.
- [ ] 8-bit PseudoColor and 24-bit TrueColor visuals.
- [ ] Extensions required by the corpus: SHAPE, MIT-SHM (or a local equivalent), XTEST.
- [ ] Xlib ABI subset sufficient for `xterm`/`twm`.
- [ ] Display lease integration with the native display server.
- [ ] X Test Suite subset passes.
- [ ] The X11 server is a native cell; legacy clients are MMU processes.

**Dependencies:** M5 (libc), M6 (display).

**Estimate:** 12–20 weeks.

**Risks:** Xlib/Motif surface; protocol corner cases; performance; 4Dwm parity.

**Out of scope:** rootless embedding, GLX, Motif applications, the full IRIX desktop.

---

## M8 — XFS read-only

**Goal:** read an IRIX 6.5 XFS v4 disk image.

**Demo:** `ls /mnt/irix` lists an IRIX 6.5 disk image and `cat` reads a file.

**Acceptance criteria:**

- [ ] Superblock, AG headers, inode core, extents, bmap B+tree.
- [ ] Four directory forms, symlinks, clean-log check.
- [ ] Big-endian metadata; no byte swapping on the hot path.
- [ ] Refuses a dirty log with a clear message.
- [ ] Test corpus: clean IRIX 6.5 images, large files, sparse files, symlinks.
- [ ] `fsck`-style consistency checks for the reader.
- [ ] The XFS server is a userspace cell over a block-device capability.

**Dependencies:** M4.

**Estimate:** 8–12 weeks.

**Risks:** IRIX v4 specifics (v1 dirs, ACLs); dirty logs; corrupted images.

**Out of scope:** log replay, write, XFS v5.

---

## M9 — Native cell + capability ABI

**Goal:** load and run a native Rust cell through the capability ABI and the Unix-shaped libc.

**Demo:** a Rust cell opens a file, maps a shared buffer, calls a server over a channel, and
prints `hello from a cell`; `fork`/`exec` on a small cell works.

**Acceptance criteria:**

- [ ] Cell loader: `.o` relocation, `CellNamespace`, symbol resolution.
- [ ] Capability table: rights, derivation, delegation, revocation.
- [ ] Synchronous IPC + shared memory; `Port` events.
- [ ] Native libc: `open`/`read`/`write`/`close`/`mmap`/`fork`/`exec`/`wait`.
- [ ] Process = one cell with a Unix personality; fd = capability.
- [ ] `fork` is limited to small cells; `posix_spawn`/`exec` is the preferred path.
- [ ] ABI version negotiation; incompatible cells are refused.
- [ ] The native ABI is documented in [`../spec/native-abi.md`](../spec/native-abi.md).

**Dependencies:** M2, M3, M4.

**Estimate:** 8–12 weeks.

**Risks:** `fork` in a SAS kernel; cell relocation; capability/namespace impedance.

**Out of scope:** full POSIX, `sproc` for native, hot-swap.

---

## M10 — Real-Indy release

**Goal:** boot, run and update IRIX 7 on a real Indy from R2.

**Demo:** a real Indy cold-boots from the ROM, fetches the kernel and root image from R2,
starts MCP, runs X11, reads an XFS disk, and updates to a new image with rollback.

**Acceptance criteria:**

- [ ] Cold boot on both real Indys (or the supported configuration).
- [ ] Graphics board detection (Newport 8-bit/24-bit; XZ/Extreme if present).
- [ ] SCSI, Seeq, HAL2, Vino detection and basic operation.
- [ ] MCP over the real network; watchdog recovery.
- [ ] X11 `xterm`/`twm` on the real hardware.
- [ ] XFS read-only on a real IRIX disk.
- [ ] Image update: fetch, verify, A/B switch, rollback.
- [ ] Installer/user/admin docs; troubleshooting guide.
- [ ] SBOM, checksums, signature, release notes.
- [ ] The release is reproducible from a clean checkout.

**Dependencies:** all milestones.

**Estimate:** 8–12 weeks.

**Risks:** hardware failures; real-Indy configuration; R2 quotas; last-mile bugs.

**Out of scope:** video output (Indy Video), 3D, ISDN, hard real-time, full IRIX desktop parity.

---

## 4. Critical path

```text
M0 → M1 → M2 → M3 → M4 → M5 → M7 → M10
                 │         │
                 ├→ M6 ────┘
                 ├→ M8
                 └→ M9
```

- **M7 (X11)** is the longest single item; start the X11 server design during M5.
- **M8 (XFS)** can run in parallel with M6/M7 once M4 is stable.
- **M9 (native ABI)** can run in parallel with M7/M8 once M3 is stable.
- **M10** is the integration and hardware-validation gate.

## 5. Scope cuts

The following are deliberately excluded from v1.0:

- ISDN.
- Video output (Indy Video board).
- 3D hardware acceleration.
- Hard real-time guarantees.
- X11 rootless embedding and GLX.
- XFS write and dirty-log replay.
- I7FS (fallback: RedoxFS or read-only images).
- The original Indigo Magic desktop as the native desktop.
- Full POSIX and full IRIX userland parity.
- Forward compatibility.

## 6. Milestone template

New milestones use this shape:

```markdown
### Mx — Title
**Goal:** one sentence.
**Demo:** what the user sees.
**Acceptance criteria:** testable checklist.
**Dependencies:** milestone IDs, ADRs, hardware.
**Estimate:** range in person-weeks.
**Risks:** top three.
**Out of scope:** what this milestone deliberately does not do.
```

## 7. Change control

- A milestone change requires an ADR or a maintainer decision recorded in the issue.
- Slipping a milestone is normal; silently dropping an acceptance criterion is not.
- The Project board is the source of truth for status; this document is the source of truth
  for scope.
