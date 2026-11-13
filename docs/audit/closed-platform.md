# Closed platform and userland issues — foundations audit

Tracking issue: [#330](https://github.com/irix7/os/issues/330)
Base: `origin/main` at `cac1a5a`, plus `5aff96e` (#325) for #321; branch
`audit-platform`
Date: 2026-11-02

## 1. Scope and method

This report audits the closed platform and userland issues. It does not audit
the M0/M1 boot, core, console, REX3 or CI issues, and it does not audit the
mesh, module-resolver or M2/M3 issues; those belong to the core and mesh
reports.

### 1.1 Issues audited here (17)

| Group | Issues |
|---|---|
| Machine type | #266 |
| PowerPC boot and hardware | #287, #291, #295, #301, #305, #309, #313, #317, #321 |
| Userland, PAL, threads, futex, ramfs, fd | #288, #296, #302, #306, #310, #314, #318 |

The other 22 closed issues in the repository are allocated to the core and
mesh reports: #1, #2, #3, #33, #43, #115, #116, #117, #118, #119, #204, #205,
\#206, #213, #214, #217, #245, #246, #247, #248, #249, #271. The boundary is
deliberate: #115 (console unify), #117 (TLB teardown), #118 (REX3) and #119
(CI readiness) are core/boot or CI work even though they touch platform code.
With #321 the closed set is 39 issues; 17 here plus 22 in the core and mesh
reports covers all 39.

### 1.2 Sources read

- Issue bodies and acceptance criteria for all 17 issues.
- Merged PRs: #278, #289, #293, #298, #303, #307, #311, #315, #320, #325,
  #290, #294, #299, #304, #308, #312, #316, #319.
- Merge commits and diffs, including `git show --stat` for the userland
  slices to confirm which repository each change touched.
- Code: `kernel-ppc/src/` (main, platform, serial, monitor, mmu, trap, irq,
  pic, via, ata, module, services, cache, probe), `kernel-x86/`,
  `kernel/src/user.rs`,
  `kernel/src/thread.rs`, `kernel/src/fd.rs`, `kernel/src/ramfs.rs`,
  `kernel/src/native_sys.rs`, `crates/native-abi`, `crates/irix7-libc`,
  `tools/powerpc-qemu.sh`, `tools/powerpc-floppy.sh`, `tools/ppc-xcoff`,
  `tools/relibc-pal/`, `user/relibc-*`.
- Contracts: `docs/programming/native-abi.md` §5.8, §5.9, §6.5, §6.6;
  `docs/programming/native-libc.md` §1, §3.1, §4.7; `docs/programming/posix.md`;
  `docs/internals/architecture.md` §4.3, §5.4, §8.1–8.3; `docs/internals/system.md`;
  `docs/project/milestones.md`; `docs/administration/pre-release.md`; `AGENTS.md`.

### 1.3 Commands and gates run

- `gh issue list --repo irix7/os --state closed --limit 1000` — 38 closed
  issues at the first pass; #321 closed after that pass, so the audited set
  is 39. `gh issue view 321` for the later issue.
- `git show 5aff96e` for the #321 merge, and `git show 5aff96e:<path>` for
  the monitor, serial, main and gate files, so the branch was not rebased.
- `gh pr view` for each linked PR; `git log --all --oneline --grep '#NNN'`.
- `tools/powerpc-qemu.sh /tmp/opencode/ppc-audit` under
  `nix develop .#powerbook`, at `cac1a5a`. Exit 0. The gate printed:
  `powerpc-qemu: ok (64 MiB RAM, 10 decrementer ticks, 10 external
  interrupts, ... macio /pci@80000000/mac-io@10 escc=1 ata=2,
  two-controller /pci@80000000/mac-io@10, ata 8192 sectors ..., module 708
  bytes 44d8a8c7..., escc ch-a=0x81093020 ch-b=0x81093000 selected=a
  mirror=both)`. This reproduces the QEMU claims for #287, #291, #295, #301,
  #305, #309, #313 and #317 at 64 MiB.
- `tools/powerpc-qemu.sh` at `5aff96e` (the #321 monitor gate), extracted to
  `/tmp/opencode/audit-321` and run under `nix develop .#powerbook` at 64 MiB.
  Exit 0; the gate printed `powerpc-qemu: ok (... monitor info/help/halt
  path=/pci@80000000/mac-io@10 console=a input=a timebase=16600000)`, with
  the `info`, `help` and `halt` transcript and the no-input
  `ppc: monitor timeout` marker.
- `tools/x86-qemu.sh /tmp/opencode/x86-audit` under
  `nix develop .#powerbook`, at `cac1a5a`. Exit 0; the gate printed
  `x86-qemu: ok (255 MiB usable, exit 1)`, with `wasm: run=42`,
  `x86: exception vector=3 (breakpoint)` and `x86: halted`. This reproduces
  the machine-layer, virtio, autoload, module and deliberate-fault claims for
  #266.
- `cargo test -p native-abi` under the devshell — 12 tests pass, including
  `stat_is_the_relibc_136_byte_layout`, `directory_records`,
  `pread_pwrite_block_is_16_bytes`, `v0_syscall_table_is_pinned_and_distinct`
  and `native_gate_and_status_mapping`.
- `git ls-remote https://github.com/irix7/relibc refs/heads/irix7-pal` and the
  GitHub commit API. The pinned `729bdc3b` exists; the branch head is
  `bf5fee5e` (one commit later, a nanosleep change for #322).
- `grep -c relibc .github/workflows/ci.yml` — 0. There is no CI job that
  builds or boots a relibc probe.

### 1.4 Classification rule

`implemented` means the issue's acceptance criteria are met by code that is
present, with evidence appropriate to what the issue asked for.
`partial` means a material part of the acceptance or goal is missing.
`divergent` means the delivered architecture contradicts a stated requirement.
`contract-only` means only a specification or interface exists.
`superseded` means a later change replaced it.

For an issue whose acceptance is itself a QEMU or iris gate, an emulator run is
the specified evidence. Where a claim about real hardware, POSIX breadth or a
shared core rests only on an emulator, the claim is marked as a limitation and
the verdict is downgraded if that claim is part of the acceptance.

## 2. Summary

| Issue | Title | Verdict | Foundation risk | Follow-up |
|---:|---|---|---|---|
| #266 | arch: x86_64 as the second supported machine type | divergent | high | Reopen or re-scope #266; shared core and x86 userland via #267/#326 |
| #287 | arch: boot a PowerPC G3 kernel to serial under qemu-system-ppc | implemented | low | Real G3/PowerBook run under #16 |
| #291 | ppc: install the kernel's own BAT registers and report a deliberate DSI | implemented | low | Hardware BAT/TLB check under #16; delay-slot handling |
| #295 | ppc: complete the exception and system-call layer | implemented | low | Hardware exception check under #16; real user syscall ABI later |
| #301 | ppc: take external timer interrupts through the Heathrow PIC | partial | high | PMU timer slice; real M4753 under #16/#324 |
| #305 | ppc: read the IDE disk through the Heathrow ATA controller | implemented | medium | Real disk timing under #16; DMA/interrupt slice |
| #309 | ppc: load a verified flat module from the ATA disk | implemented | medium | Signed boot configuration; slot/linker fix |
| #313 | ppc: make the boot console work on either ESCC channel | implemented | medium | Real Geoport wiring under #16 |
| #317 | ppc: select the Mac I/O node with the ESCC and ATA children | implemented | medium | Real two-controller tree under #16 |
| #321 | ppc: serial input and a minimal boot monitor | implemented | medium | Bounded RX queue and real Geoport test under #16/#324 |
| #288 | userland: boot a real C probe through the IRIX PAL | implemented | high | CI probe gate (#13); process/capability gate (#326/#10) |
| #296 | userland: broaden the fd surface and give directories a cookie | implemented | high | Per-process fd table (#326); `*at` surface |
| #302 | userland: poll and non-blocking descriptors | implemented | medium | Wait queue for cell-switching readiness (#343 follow-up, M9.5) |
| #306 | userland: make the in-memory filesystem writable | implemented | medium | Block-backed FS and per-cell namespace (#10/#326) |
| #310 | userland: cooperative user threads and per-thread TCBs | implemented | high | Preemption and blocking waits (#322/#326) |
| #314 | userland: futex wait/wake for cooperative threads | implemented | medium | Timer-driven wakeups (#322); real wait queues |
| #318 | userland: pthread_create/join and a mutex on the thread gate | implemented | high | Preemption, condvars and cancellation (#326/#322) |

Counts: **15 implemented, 1 partial, 1 divergent, 0 contract-only, 0
superseded**.  #302 moved from partial to implemented in the remediation
recorded in §7.1; #301 remains partial.

## 3. Per-issue evidence

### #266 — arch: x86_64 as the second supported machine type

**Verdict:** divergent. **Foundation risk:** high. **Evidence grade:** QEMU
x86_64 gate and CI job; code and documentation read.

The issue required an explicit architecture boundary around one shared kernel
core: "move MIPS-specific boot, traps, TLB, timer, serial and device code
behind it. The core, mesh, module loader and service ABI remain shared; no x86
fork of kernel logic", and the first acceptance item is "QEMU x86_64 boots the
shared kernel core".

The tree does not do that. `kernel-x86` is a separate kernel binary. Its
`Cargo.toml` depends on eight in-tree crates and `smoltcp`, but not on the
`kernel` crate (`kernel-x86/Cargo.toml:8-17`). Its own doc comment says: "The
shared kernel core is not linked in yet; the machine layer is being built
first" (`kernel-x86/src/main.rs:8-10`). It has its own `module.rs`,
`services.rs`, `net.rs`, `tcp.rs`, `fetch.rs`, `heap.rs` and `memory.rs`.
`docs/hardware/x86.md:4-5` repeats that the shared core is not linked, and
`docs/hardware/x86.md:180-193` says the service-table bodies and HTTP clients stay
in each kernel. This is the "x86 fork of kernel logic" the issue prohibited.

The machine work that was delivered is real. `kernel-x86` boots through PVH,
builds a 4-level identity map, reads the PVH memory map, installs the IDT,
runs the PIT, drives virtio-net and virtio-blk, loads MCP and wasm modules
with SHA-256 checks, and reports a deliberate `int3`
(`tools/x86-qemu.sh:163-195`). The local run at `cac1a5a` passed with
`x86-qemu: ok (255 MiB usable, exit 1)`, `wasm: run=42` and
`x86: exception vector=3 (breakpoint)`. The CI job runs that gate and a
two-guest M2 mesh (`ci.yml:1377-1422`). The module layout is checked on both
machines (`ci.yml:1408-1410`).

What is missing:

- No shared core. The acceptance item is unmet.
- No x86 userland, fd table, ramfs, thread table or native syscall gate.
  `kernel-x86/src/` has no `user.rs`, `fd.rs`, `thread.rs`, `ramfs.rs` or
  `native_sys.rs`. The issue's goal says "at the same shared-core, module-ABI
  and POSIX/IRIX contract level as the Indy/MIPS target"; only the module ABI
  and M2 mesh level were reached. The issue's non-goals exclude "POSIX/IRIX
  userland parity" for the first slice, so the closure is at machine/M2
  parity, not at the stated goal.
- The documentation is stale in the other direction: `docs/hardware/x86.md:195-205`
  still lists the module loader, M1 probe and two-guest M2 gate as "next
  slices", and `docs/project/milestones.md:296-300` says "the virtio devices and
  the shared core are still ahead", although the tree has virtio and the M2
  gate. The acceptance item "architecture boundary and remaining x86
  limitations are documented" is therefore only partly satisfied.
- No real x86 hardware, only QEMU. That is expected for this slice.

Follow-up: reopen #266 or split it into "machine layer" (done) and "shared
core and userland" (not done), with #267 carrying the scale harness and #326
carrying the process model. Do not describe the machine as a second supported
POSIX target until it has the native gate.

### #287 — arch: boot a PowerPC G3 kernel to serial under qemu-system-ppc

**Verdict:** implemented. **Foundation risk:** low. **Evidence grade:** QEMU
`g3beige` gate, run locally at 64 MiB.

The code is a new `kernel-ppc` crate with a PPC32 big-endian no_std image
(`kernel-ppc/Cargo.toml`, `targets/powerpc-unknown-irix7-ppc.json`). The entry
path in `boot.S` clears `.bss`, sets the stack, passes the Open Firmware client
handler in `r3` and calls `kernel_main` (`kernel-ppc/src/boot.S:18-41`). The
kernel reads `/memory`, the Mac I/O BAR, the ESCC registers and the timebase
from the device tree (`kernel-ppc/src/platform.rs:87-108`, `373-500`,
`611-615`), initialises the serial console (`kernel-ppc/src/serial.rs:70-100`),
programs a 10 ms decrementer (`kernel-ppc/src/timer.rs:38-50`) and prints the
banner, RAM size, ticks and `BOOT-OK`.

The gate asserts the banner, memory size, non-zero tick count, `BOOT-OK` and
an ordered marker list; it fails a hang or a missing marker
(`tools/powerpc-qemu.sh:169-220`), and it rejects an image containing an FP
instruction (`tools/powerpc-qemu.sh:76-86`). The local run passed at 64 MiB
with 10 decrementer ticks.

Limits within the issue's non-goals: no MMU page tables, interrupts beyond the
decrementer, devices beyond serial, mesh, modules or userland. Real hardware
was not tested; that is the subject of #16. The gate is QEMU-only and the issue
is explicitly a QEMU gate, so the evidence matches the acceptance.

Follow-up: a real G3/PowerBook boot under #16. No code change is needed for the
slice itself.

### #291 — ppc: install the kernel's own BAT registers and report a deliberate DSI

**Verdict:** implemented. **Foundation risk:** low. **Evidence grade:** QEMU
gate, run locally; DSI fields parsed by the gate.

`kernel-ppc/src/mmu.rs` installs IBAT0/DBAT0 for the image and boot stack,
IBAT1 for the low 128 KiB vector page, DBAT1 for the Mac I/O window with
`WIMG_IO = 0x28`, and IBAT2/DBAT2 for the module slot; the remaining BAT registers are
cleared (`mmu.rs:78-101`). `activate` points SDR1 at a zeroed, 64 KiB-aligned
hash table and invalidates the owned ranges page by page with `tlbie`, because
QEMU's 750 model lacks `tlbia` (`mmu.rs:103-132`). The image BAT is sized from
`__kernel_start`..`__kernel_end`; the linker asserts the image stays in one
BAT block (`kernel-ppc/linker.ld:45-48`).

`kernel-ppc/src/trap.rs` installs the `0x300` DSI vector and prints `SRR0`,
`SRR1`, `DSISR` and `DAR` (`trap.rs:32-50`, `99-140`). `kernel-ppc/src/probe.rs`
performs a deliberate load from `0x2000_0000`; the gate parses the DSI line,
checks that `DSISR` is a translation-not-found load, that `DAR` is
`0x20000000`, that `SRR0` is inside the image BAT, and that `FAULT-OK` is
present (`tools/powerpc-qemu.sh:365-420`). The local run passed with
`dsisr=0x40000000 dar=0x20000000`.

Limits within the issue's non-goals: no user-mode translation, no page tables
or demand paging; SDR1 points at an empty table and the kernel is
supervisor-only. The DSI handler reports and resumes by advancing `SRR0` by
four (`trap.rs:101-118`); a fault in a branch delay slot would be stepped
incorrectly. That is not exercised by the probe and is a future handler
concern, not a failure of this slice.

Follow-up: real 750 BAT/TLB behaviour under #16, and delay-slot handling when
the fault handler starts recovering contexts.

### #295 — ppc: complete the exception and system-call layer

**Verdict:** implemented. **Foundation risk:** low. **Evidence grade:** QEMU
gate, run locally; strict marker order and field checks.

The kernel owns `0x300` DSI, `0x400` ISI, `0x500` external, `0x600`
alignment, `0x700` program, `0x900` decrementer and `0xC00` system call
(`kernel-ppc/src/trap.rs:24-50`). A common `trap_entry` saves the volatile
GPRs, CR, LR, CTR and XER, calls `trap_dispatch` and returns through `rfi`
(`kernel-ppc/src/boot.S:68-145`). The dispatcher reports the vector, `SRR0` and
`SRR1`, plus `DSISR`/`DAR` for memory faults, advances `SRR0` for DSI,
alignment and program faults, resumes an ISI through LR after checking it is
inside the image, and turns a `sc` into `r3 + 1` (`trap.rs:99-131`).

`kernel-ppc/src/probe.rs` triggers each path: an unmapped load, an unmapped
fetch through `bctrl`, a misaligned `lwarx`, an illegal `.long 0` and one
`sc` with call number `0x2a`. The gate asserts `FAULT-OK`, `ISI-OK`,
`ALIGN-OK`, `PROGRAM-OK`, `SYSCALL-OK` and `TRAPS-OK` in order, parses each
report and checks the field values, including that the alignment `DAR` is not
word-aligned and the program `SRR1` has the illegal-instruction cause
(`tools/powerpc-qemu.sh:350-455`). The local run passed.

Limits within the issue's non-goals: this is a kernel-mode probe, not a user
syscall ABI. The "system call" reads `r3` and returns `r3 + 1`; it is not the
native gate and has no user context. The issue's non-goals explicitly exclude
user-mode page tables and devices, so the slice is complete, but it does not
provide a PPC userland entry.

Follow-up: a real PPC user syscall ABI and user-mode translation if PPC is to
reach the Indy's POSIX level; otherwise record PPC as a kernel-only machine.

### #301 — ppc: take external timer interrupts through the Heathrow PIC

**Verdict:** partial. **Foundation risk:** high. **Evidence grade:** QEMU
`g3beige` gate (10 interrupts); code path for the real PowerBook examined.

The PIC work is real. `kernel-ppc/src/pic.rs` implements the two Heathrow
banks, event/mask/acknowledge/level registers and EOI, with little-endian
Mac I/O accesses. `kernel-ppc/src/via.rs` programs the CUDA MOS 6522 timer 1
in continuous mode, and `kernel-ppc/src/irq.rs` acknowledges the source and
controller and counts ten interrupts. The platform lookup accepts only
`heathrow` or `gatwick` and refuses another controller
(`kernel-ppc/src/platform.rs:558-567`). The local gate run printed
`IRQ-OK count=10` and `pending=0x0 ifr=0x0`; the gate checks the controller
line, contiguous counts, saved `EE`, `SRR0` inside the image BAT, the final
count and the final pending/IFR state (`tools/powerpc-qemu.sh:505-565`).

The gap is the target machine. The M4753 exposes `via-pmu`, not `via-cuda`.
`platform::cuda` searches only for a `via-cuda` child
(`kernel-ppc/src/platform.rs:589-590`), so on a real PowerBook the lookup
returns `None` and `kernel_main` prints `IRQ-SKIP reason=no-cuda` and
continues to ATA and module loading (`kernel-ppc/src/main.rs:284-309`). The
boot still succeeds, but the issue's acceptance ("ten external timer
interrupts arrive through the Heathrow PIC") is not exercised on the hardware
the issue names. The QEMU gate cannot see this fallback because `g3beige`
provides a CUDA.

There is also no other interrupt source on the real path: ATA is polled PIO
with `nIEN` set (`kernel-ppc/src/ata.rs:161-180`), the ESCC is polled, and
there is no preemption. The PIC layer exists but is not used by any driver on
the PowerBook.

Follow-up: add a PMU timer source and make the `0x500` proof run on the M4753
under #16; #324 covers ADB/PMU input and is the natural place for PMU device
work. Until then, #301's evidence is QEMU-only and the real target prints
`IRQ-SKIP`.

### #305 — ppc: read the IDE disk through the Heathrow ATA controller

**Verdict:** implemented. **Foundation risk:** medium. **Evidence grade:**
QEMU `g3beige` gate with a synthetic 4 MiB disk.

`kernel-ppc/src/ata.rs` implements ATA `IDENTIFY DEVICE` and one LBA28
`READ SECTORS` command with polled PIO. Every wait checks `BSY`, `DRQ`,
`DRDY`, `ERR` and `DF`, bounded by two seconds of timebase
(`ata.rs:140-159`). The driver sets `nIEN`, uses the Mac I/O 16-byte register
stride and the `ata-3` node's `reg` window, and reads the model, capacity and
signature sector (`ata.rs:161-239`). `platform::ata` scans the selected Mac
I/O node's children and prefers a channel with a `disk` child
(`platform.rs:617-697`).

The gate creates an 8192-sector disk, writes `IRIX7-ATA-SIG001` at LBA 4,
attaches it to `if=ide,index=0`, and asserts the ATA window, model, sector
count, signature bytes and `ATA-OK` (`tools/powerpc-qemu.sh:130-220`,
`590-635`). The local run passed with `ata 8192 sectors sector4=...`.

Limits within the issue's non-goals: no writes, DMA, ATAPI, interrupts,
partition parsing or filesystem. The evidence is QEMU only; real PowerBook
timing and the Mac I/O ATA register behaviour are unverified. The issue's
acceptance is the QEMU gate, so this is implemented at the stated level.

Follow-up: real disk timing under #16; a DMA or interrupt-driven block path
belongs to a later block-device slice.

### #309 — ppc: load a verified flat module from the ATA disk

**Verdict:** implemented. **Foundation risk:** medium. **Evidence grade:**
QEMU gate, including a corrupted-image negative test.

`kernel-ppc/src/module.rs` reads a header at LBA 6 (`IRIX7MOD`, big-endian
length, 32-byte SHA-256), zeroes the 2 MiB slot at `0x0180_0000`, copies the
image from LBA 16 while hashing it, refuses a bad magic, length or digest,
flushes the data cache and invalidates the instruction cache, and calls the
entry with the shared `module_abi::Services` table (`module.rs:101-160`;
`cache.rs:20-44`). The vtable is checked to point inside the verified image
(`module.rs:169-180`). `services.rs` implements `write`, `clock_ms` and
`info`; allocation, network, block, screen, config, console-read and module
loader entries refuse with -1/`REFUSED` (`services.rs:37-150`).

The gate builds `modules/hello` at the PPC slot, writes it to the disk,
asserts the digest, `PPC-MODULE-RUN`, `module info: ppc: g3beige`, the tick,
status and `MODULE-OK`, then boots a copy with one image byte flipped and
requires `MODULE-DIGEST-BAD` with no module output
(`tools/powerpc-qemu.sh:110-160`, `640-705`). The local run passed with a
708-byte module.

Two limits are worth recording. First, the digest comes from the same disk
header as the image; it verifies integrity, not authenticity. A writer who can
change the disk can replace both the image and the digest. The kernel's own
comment says a real boot would take the digest from the signed boot
configuration, which the PowerPC machine does not have (`module.rs:4-10`;
`services.rs:6-12`). Second, the slot is at 24 MiB
(`module.rs:24-30`), only 8 MiB above the image base at 16 MiB, while the
linker asserts only `__kernel_end <= 0x02000000` (`kernel-ppc/linker.ld:48`).
That allows a 16 MiB image and therefore an overlap with the slot for any
image larger than 8 MiB. The current image is about 134 KiB, so the gate is
safe, but the assertion should be `__kernel_end <= SLOT_BASE` (24 MiB), not
0x02000000.

Follow-up: signed boot configuration for the PPC module digest; a linker
assertion against `SLOT_BASE`, not 32 MiB; service coverage for network and
block when a PPC network driver exists.

### #313 — ppc: make the boot console work on either ESCC channel

**Verdict:** implemented. **Foundation risk:** medium. **Evidence grade:**
QEMU gate with two serial files and a byte-for-byte comparison.

`kernel-ppc/src/serial.rs` initialises every channel the device tree
describes, resets each with its own WR9 bit, programs 38400 8N1 from the
3.6864 MHz RTxC clock, and mirrors every byte to both channels
(`serial.rs:70-129`). `platform::escc` reads the `ch-a`/`ch-b` children,
resolves `AAPL,address` or `reg`, falls back to the fixed channel layout, and
reads `/options:output-device` and `/chosen:stdout-path`
(`platform.rs:373-500`). The boot log records the window, both control
addresses, the firmware properties and the selected channel
(`main.rs:212-235`).

The gate runs `-nographic` with two serial files, asserts the channel
addresses and the firmware selection, and requires channel B's kernel log to
match channel A byte for byte from the BAT switch onwards
(`tools/powerpc-qemu.sh:285-350`). The local run passed with
`ch-a=0x81093020 ch-b=0x81093000 selected=a mirror=both`.

Limits within the issue's non-goals: no real PowerBook wiring test, no input,
flow control, termios or baud negotiation; transmit-only at 38400 8N1. The
mirror design means a wiring difference does not hide the log, which is the
right response to the hardware uncertainty.

Follow-up: real Geoport and internal-IR wiring under #16. The mirror remains
the safe default until the wiring is measured.

### #317 — ppc: select the Mac I/O node with the ESCC and ATA children

**Verdict:** implemented. **Foundation risk:** medium. **Evidence grade:**
QEMU gate with a second `macio-oldworld` device.

`platform::macio` walks the `/pci` children and scores each Mac I/O candidate
by descendants: ESCC 100, ATA channel with a disk child 50, each ATA channel
10; ties stay with tree order; a candidate without an ESCC is refused
(`platform.rs:100-185`). The selected node supplies the Mac I/O BAR, and the
ESCC, ATA, VIA and PIC lookups start there (`main.rs:115-160`,
`platform.rs:373-500`, `617-697`). The boot log prints
`ppc: macio base=... path=... children escc=... ata=... disks=...`
(`main.rs:205-220`).

The gate adds a `macio-oldworld` device ahead of the built-in controller and
requires the kernel to keep selecting `/pci@80000000/mac-io@10`, with the full
console/ATA/IRQ/module path on both channels
(`tools/powerpc-qemu.sh:560-620`). The local run passed with the same path and
`MODULE-OK` on both channels.

Limits within the issue's non-goals: no real M4753 two-controller test; the
tie-break is tree order when two controllers have identical scores, which is
exactly the firmware-order risk the issue was written to remove. The scoring
makes the common case correct but not the general one. ADB, framebuffer,
PMU/NVRAM and a second Mac I/O driver remain out of scope.

Follow-up: real two-controller tree under #16. If both controllers can carry
an ESCC and a disk, the selection rule needs a boot-device or
`device_type`/`compatible` tie-break, not tree order.

### #321 — ppc: serial input and a minimal boot monitor

**Verdict:** implemented. **Foundation risk:** medium. **Evidence grade:**
QEMU `g3beige` monitor gate and PR #325 green checks; code read at `5aff96e`.

The receive path is in `kernel-ppc/src/serial.rs`. `read_status` selects RR0
and tests bit 0 for a waiting character (`serial.rs:39-41`, `129-134`);
`read_data` reads the data port; `drain` clears up to 16 stale bytes at
initialisation (`serial.rs:136-152`); `read_byte` polls the selected channel
and returns `None` when none is waiting (`serial.rs:182-190`). Output stays
mirrored to both channels, but input is deliberately one channel: the
firmware's selected channel, otherwise channel A, otherwise B
(`serial.rs:11-14`, `84-107`). `main.rs` passes `escc.selected` into
`serial::init` (`main.rs:130`) and uses the returned channel for the monitor
(`main.rs:373-398`).

`kernel-ppc/src/monitor.rs` implements the one-second first-key wait
(`monitor.rs:17-20`, `53-67`), the `ppc>` prompt, echo and backspace, a
32-character line buffer (`monitor.rs:22-25`, `76-105`), and the fixed
`help`, `info` and `halt` commands (`monitor.rs:116-131`). `info` reports the
selected Mac I/O base/size/path, console and input channel, memory, timebase,
ATA model and sectors, and module state (`monitor.rs:39-51`, `140-179`).
Silence prints `ppc: monitor timeout` and the unchanged `ppc: halted`
(`main.rs:387-401`), so a headless boot keeps the old markers.

The gate covers the acceptance. Its marker list adds
`ppc: monitor wait input=a` and `ppc: monitor timeout`
(`tools/powerpc-qemu.sh:229-231`). A second boot replaces channel A with a
pipe chardev, waits for `ppc: monitor wait`, sends `info`, `help` and `halt`
one byte at a time, and asserts at least three prompts, the echo lines, every
help line, the Mac I/O values against the boot log, `console=input=selected`,
the memory size, timebase, ATA sectors and module status
(`tools/powerpc-qemu.sh:631-791`). It also requires channel B to mirror the
whole session (`:793-810`) and requires the no-input boot never to print a
prompt (`:812-817`). The two-controller boot also requires
`ppc: monitor timeout` (`:860`). PR #325 reports the PowerPC, iris, x86 and
module gates green, including `Boot (PowerPC G3 QEMU)` and
`Boot (x86_64 QEMU)`.

The known limits are present and documented: three fixed commands, a
32-character line buffer, no line editor or history, serial-only input (no
ADB keyboard wiring), and input only on the selected ESCC channel
(`docs/research/powerbook-qemu-boot.md:415-438`). The new finding is that the
scope's "bounded console input queue" is not implemented. `read_byte` polls
the ESCC's single receive register and `monitor::read_blocking` spins on it
(`serial.rs:182-190`; `monitor.rs:107-114`); there is no ring buffer between
the receiver and the line buffer. The gate compensates by writing one byte
every 30 ms (`tools/powerpc-qemu.sh:687-695`) and its own comment explains
that a burst loses every byte after the first until the monitor drains the
register (`:631-635`). A human typing works; a paste or a fast host does not.
The gate also only exercises the selected-A path; a B-selected channel and
the fallback-A path are code-only. The acceptance criteria are met; the queue
is a scope gap and a robustness limitation, not an acceptance failure, which
is why the verdict stays implemented.

Limits: no real Geoport test, no keyboard, no line editing, no history, no
argument parsing, no module loading from the monitor. The monitor is polled,
not interrupt-driven; that is within the issue's "polled or interrupt-driven"
scope. `info` reports the loader's status, which is always zero because the
loader rejects a nonzero status (`module.rs:169-185`).

Follow-up: add a bounded RX ring and a burst/backspace/overlong/unknown-command
gate; test a B-selected channel; run the monitor on a real Geoport under #16.
The input path is the base for #323 and #324 but does not yet provide their
keyboard or framebuffer coverage.

### #288 — userland: boot a real C probe through the IRIX PAL

**Verdict:** implemented. **Foundation risk:** high. **Evidence grade:** iris
0.1.13 transcript in `docs/research/relibc-pal-bringup.md`; host layout tests
run locally; no CI gate.

The native gate is `syscall 0x1F7`, with a flat syscall number in `$v0`,
`CELL_E_*` status in `$v0` and the value in `$v1` on return
(`kernel/src/native_sys.rs:1-20`; `kernel/src/traps.rs:817-830`). The kernel
writes relibc's 136-byte n32 `stat` and the 280-byte `dirent`
(`crates/native-abi/src/types.rs`; tests in
`crates/native-abi/tests/layout.rs:49-76`). `user::initial_stack` builds the
System V stack with `argc`, `argv`, `envp` and an auxiliary vector, and sets
`$28` to the 16-byte TCB (`kernel/src/user.rs:313-390`;
`docs/research/c-probe-bringup.md:11-40`). The negative-errno channel is
proved by `fstat(999) -> EBADF` and `stat("/missing") -> ENOENT`
(`docs/research/c-probe-bringup.md:42-60`).

`tools/relibc-pal/build.sh` clones the `irix7-pal` branch at a pinned commit,
builds `librelibc.a`, compiles the C probe, the thread, futex and pthread
probes, strips and pads them (`build.sh:28-40`, `110-192`). The current pin is
`729bdc3b` (`build.sh:31`), which exists on the remote; the branch head has
since moved to `bf5fee5e` for a nanosleep change. The relibc-linked probe
transcript reports `IRIX7-RELIB-PROBE-4`, stat fields, `tp=0x...`, `checks=226
failures=0` and exit 0 (`docs/research/relibc-pal-bringup.md:13-27`). The
`native-abi` host tests pass 12/12, including the 136-byte `stat`, 280-byte
`dirent`, 16-byte `io_at`, 8-byte `pollfd` and pinned syscall table.

What is missing or weak:

- The probe is a real C/relibc binary, not a Rust shim, and it exercises a
  broad slice. It is still a bring-up gate, not a POSIX layer. The same issue
  explicitly lists no threads, signals, `posix_spawn`, sockets, termios or
  process table (`#288` non-goals), and the kernel has no process table.
- The evidence is an iris transcript, not a CI gate. `grep -c relibc
  .github/workflows/ci.yml` is 0; the only userland images CI builds and boots
  are `std-hello` and the Rust `user-hello` (`ci.yml:375-382`, `1029-1080`,
  `1146-1218`).
- `getentropy` succeeds by zero-filling the buffer
  (`kernel/src/user.rs:724-734`), and `AT_RANDOM` is an xorshift of CP0 Count
  and NTP time (`kernel/src/user.rs:332-342`). Neither is suitable for
  security-sensitive randomness; a program that trusts `getentropy` is misled.
- `signal` is an explicit no-op success (`kernel/src/native_sys.rs:115-118`),
  and `mmap` is a fixed 1 MiB bump allocator with `munmap`/`mprotect` as
  no-ops (`kernel/src/native_sys.rs:84`; `kernel/src/user.rs:235-310`).
- `getpid` always returns 1 (`kernel/src/native_sys.rs:98`); there is no
  process identity or parent.
- The flat v0 gate is deliberately transitional (`docs/programming/native-abi.md:1518-1522`);
  it is not the capability cell ABI and must be replaced, not extended.

Follow-up: add a CI job that builds the pinned relibc and boots the four
probes (see #13 and #330); do not describe the v0 gate as the native
capability ABI; #326 for the process model; #10 for the capability gate.

### #296 — userland: broaden the fd surface and give directories a cookie

**Verdict:** implemented. **Foundation risk:** high. **Evidence grade:** iris
transcript (110 then 226 checks) and host layout tests; no CI gate.

`kernel/src/fd.rs` adds `lseek` for all `SEEK_*`, `pread`/`pwrite` through a
16-byte `io_at`, `dup2`, `pipe2`, and `fcntl` `F_GETFD`/`F_SETFD` with real
per-descriptor `FD_CLOEXEC` state (`fd.rs:600-627`). Pipes are a bounded
4 KiB ring with `EAGAIN` on an empty read while a writer is live and `EPIPE`
when no reader remains (`fd.rs:42-43`, `749-823`). `getdents` fills whole
280-byte records and advances a cookie; `d_off` is the next cookie, and the
one-name `readdir` remains a wrapper (`docs/programming/native-abi.md:1620-1640`).
The relibc PAL maps the calls and passes 64-bit offsets across two registers
(`tools/relibc-pal/README.md:90-110`).

The probe reports `IRIX7-RELIB-PROBE-2` with `checks=110 failures=0` and the
later `PROBE-4` run reports 226 checks (`docs/research/relibc-pal-bringup.md:13-27`).
The checks cover seek modes, positional I/O position invariance, shared
offsets across `dup`/`dup2`, close-on-exec flags, pipe `EAGAIN`/`EPIPE`/EOF,
three-record `getdents`, cookie resume and `readdir`/`seekdir`/`telldir`
(`user/relibc-probe/README.md:20-40`).

What is missing or weak:

- The fd table is one global static table: `MAX_FDS = 16`,
  `MAX_OPEN_FILES = 16`, `MAX_PIPES = 8`, with `FD_OPEN`, `FD_FLAGS` and
  `OPEN_STATUS` as statics (`fd.rs:19-20`, `42-43`, `84-87`). The file's
  heading says "per-process descriptor table", but there is no process. Every
  thread and every image shares it until `fd::reset()`.
- `FD_CLOEXEC` is recorded, but there is no `execve`; the probe can only check
  `F_GETFD`/`F_SETFD`, not an actual close-on-exec. The issue's "real
  close-on-exec state" is true as state, not as behaviour.
- The kernel has no `*at` syscalls; the PAL's `openat` only accepts
  `AT_FDCWD` and otherwise returns `ENOSYS` (`tools/relibc-pal/README.md:125-135`).
- Pipes are always non-blocking because the kernel has no scheduler; a
  blocking pipe read gives up after 100 ms with `EAGAIN`
  (`fd.rs:45-55`, `749-780`).

Follow-up: per-process fd tables and a real `exec` under #326; add the `*at`
surface and `F_DUPFD_CLOEXEC` when the fd table moves. The global table is a
direct blocker for `fork`.

### #302 — userland: poll and non-blocking descriptors

**Verdict:** implemented, after remediation on 2026-11-03 (§7.1). **Foundation
risk:** medium. **Evidence grade:** iris transcript (142 checks), code read,
and the parked-wait change reviewed against the descriptor probe.

The functional surface is there. `poll` reports `POLLIN`, `POLLOUT`,
`POLLERR`, `POLLHUP` and `POLLNVAL` per record, ignores negative descriptors,
returns 0 for a zero count and honours a timeout
(`kernel/src/user.rs:640-684`; `fd.rs:847-898`). `fcntl(F_GETFL/F_SETFL)`
carries the access mode and `O_NONBLOCK` on the open file description, and
`dup` shares it (`fd.rs:600-640`). The probe reports
`IRIX7-RELIB-PROBE-3` with 142 checks, covering pipe data/space/HUP/ERR, a
closed descriptor, negative descriptors, per-record `revents`, zero count,
the bounded timeout, `O_NONBLOCK` get/set across `dup`, empty-pipe `EAGAIN`
and console `POLLOUT` (`docs/research/relibc-pal-bringup.md:35-37`).

At the audit this was partial: the acceptance item "`poll` returns the
expected `revents` and a bounded timeout; no busy loop" was not met.
`poll_user` spun on `core::hint::spin_loop()` until readiness or the timeout,
and `fd::wait_for` did the same for a blocking read or write.  The remediation
removes both spins:

- `poll_user` parks the CPU on CP0 Compare for `WAIT_SLICE_MS` (1 ms) slices
  and rechecks readiness between slices (`kernel/src/user.rs`).  A device or
  console interrupt ends a slice early, so the console stays responsive.
- `fd` no longer waits.  `fd::read` reports `EAGAIN` for an empty pipe or idle
  console, and `user::read_fd` owns the blocking rule: it parks in the same
  timer slices until `WAIT_LIMIT_MS`, returns `EAGAIN` at the bound, and
  returns a short count once it has already copied bytes.  `O_NONBLOCK` never
  enters the wait and reports `EAGAIN` immediately.
- A full-pipe write still takes the bounded path, now parked on the timer
  rather than spun (`fd::wait_for`).

The wait remains **syscall-local** and bounded at 100 ms.  The flat gate has
no continuation, so a `poll` or read cannot install another cell in the middle
of the gate and resume its scan; that needs the M9.5 wait queue/wakeup this
report already listed as a follow-up.  `nanosleep`, which is a complete park
rather than a continuation, is the call that yields a cell (§8.2).  A negative
`poll` timeout is still clamped to 100 ms, as the issue permits ("the
single-threaded kernel must bound the wait; document the chosen behaviour").
The probe's `O_NONBLOCK`, pipe and console readiness checks are unchanged.

### #306 — userland: make the in-memory filesystem writable

**Verdict:** implemented. **Foundation risk:** medium. **Evidence grade:**
iris transcript (226 checks); code read.

`kernel/src/ramfs.rs` is a fixed table of 32 inodes with parent, name, mode,
size and a 4 KiB content buffer (`ramfs.rs:22-24`). Open honours `O_CREAT`,
`O_EXCL`, `O_TRUNC` and `O_APPEND`; reads and writes check the access mode;
`unlink`, `rename`, `mkdir`, `rmdir`, `ftruncate` and `truncate` implement the
POSIX error cases (`ramfs.rs:177-470`; `fd.rs:398-550`). An unlinked inode
stays readable through the last descriptor (`ramfs.rs:426-444`). The tree is
reseeded on every image start (`ramfs.rs:120-160`).

The probe reports `IRIX7-RELIB-PROBE-4` with 226 checks, covering create,
truncate, write/read, `O_TRUNC`, `O_APPEND`, rename, `mkdir`/`rmdir`,
`ENOTEMPTY`, live `getdents`, `chdir`/`getcwd`, 4 KiB fill and `ENOSPC`, and
an unlinked inode staying readable (`docs/research/relibc-pal-bringup.md:13-27`;
`user/relibc-probe/README.md:45-70`).

Limits within the issue's non-goals: no persistence, block-backed storage,
permissions, ownership, timestamps, hard links or symlinks. The 32-inode and
4 KiB limits are bring-up limits. Directory cookies are positional, so
mutation between `getdents` calls can repeat or skip an entry
(`docs/programming/native-abi.md:1640-1648`).

Follow-up: a block-backed root with EROFS/overlay and a per-cell namespace
belongs to #10/#326. The current ramfs is a test fixture, not a root
filesystem.

### #310 — userland: cooperative user threads and per-thread TCBs

**Verdict:** implemented. **Foundation risk:** high. **Evidence grade:** iris
transcript (20 checks); code read.

`kernel/src/thread.rs` adds an eight-slot thread table and
`SYS_THREAD_CREATE`, `SYS_YIELD` and `SYS_THREAD_EXIT`
(`thread.rs:34`, `196-250`, `260-270`). Each thread saves and restores a user
`TrapFrame`; a switch happens only at a syscall boundary, so one kernel stack
suffices. The initial frame is `EPC=entry`, `SP=stack_top`, `$a0=arg`,
`$28=tcb`, other GPRs zero (`thread.rs:221-226`). `$28` is the 16-byte TCB
header; the kernel preserves it and does not interpret the tail
(`thread.rs:19-22`).

The probe reports `THREADS-OK` with 20 checks: a rejected out-of-range stack,
a distinct worker TCB and argument, both threads making progress across seven
yields, a distinct worker errno, and main's TCB/errno unchanged after the
worker exits (`user/relibc-thread-probe/README.md:8-22`).

The issue's non-goals are explicit: no timer preemption, no blocking
futexes, no per-thread kernel stack, no signals, no processes. Within that
scope the slice is complete. The foundation risk is what the scope leaves
out: a CPU-bound thread that never yields starves every peer; there is no
join primitive in the kernel (the libc implements one with an exit word);
thread ids are reused after a slot dies; the table is eight slots; and the
address space is shared with no per-thread kernel stack
(`thread.rs:2-27`, `456-468`). This is not the preemptive EDF/fair scheduler
in `docs/internals/architecture.md:893-935`.

Follow-up: preemption, per-thread kernel stacks and a real join under #326;
\#322 for parking sleeps. Keep the cooperative gate until the scheduler
replaces it.

### #314 — userland: futex wait/wake for cooperative threads

**Verdict:** implemented. **Foundation risk:** medium. **Evidence grade:**
iris transcript (32 checks); code read.

`SYS_FUTEX_WAIT`/`SYS_FUTEX_WAKE` are 40/41
(`crates/native-abi/src/syscall.rs:76-81`). `thread::futex_wait` range-checks
the word, compares before parking (`CELL_E_AGAIN` on mismatch), parks a
`Blocked` thread, yields, and returns `CELL_OK` only when woken; `futex_wake`
marks up to `count` waiters runnable and returns the count
(`kernel/src/thread.rs:279-405`). A zero timeout probes without parking; a
negative timeout is bounded by `fd::WAIT_LIMIT_MS` (100 ms) and a positive one
is clamped; expiry re-reads the word and returns `CELL_E_AGAIN` or
`CELL_E_TIMEDOUT` (`thread.rs:340-363`, `456-468`).

The probe reports `FUTEX-OK` with 32 checks: park/wake, wake count limits,
compare-before-park, `EINVAL`/`EFAULT`, zero timeout, 20 ms expiry, a changed
word at expiry and a no-peer negative timeout bounded at about 100 ms
(`user/relibc-futex-probe/README.md:8-29`). The probe drives the raw gate;
relibc's `futex_wait` mapping is exercised later by #318, which is why the
PAL mapping is not proved by this issue alone.

Limits within the issue's non-goals: no preemption, no timer wakeup, no
condition variables, no signals, no processes. The bounded wait is a
deliberate substitute for a scheduler; a waiter that is never woken returns
`CELL_E_TIMEDOUT` after 100 ms. The kernel does not re-check the word on wake
by design (`thread.rs:365-369`), which matches futex semantics but makes the
libc responsible for the predicate loop.

Follow-up: timer-driven wakeups and a real wait queue under #322/#326. The
`thread::exit_current` path already waits out parked peers rather than
abandoning them, which is the right failure mode for the current model.

### #318 — userland: pthread_create/join and a mutex on the thread gate

**Verdict:** implemented. **Foundation risk:** high. **Evidence grade:** iris
transcript (29 checks); relibc commit verified through GitHub; no CI gate.

The kernel is unchanged. The work is in relibc's `irix7-pal` branch and the
in-tree review mirror (`tools/relibc-pal/patches/0007-irix7-run-pthreads-on-the-cooperative-thread-gate.patch`).
`pthread_create` allocates the stack and TCB, calls `SYS_THREAD_CREATE` through
`rlct_clone` and records the kernel tid in `Pthread.os_tid`; `pthread_join`
waits on a libc-owned exit word with `futex_wait`; `pthread_self` and
`__errno_location` read `$28`; the mutex fast path uses `futex_wait`/`futex_wake`
(`tools/relibc-pal/README.md:140-165`; patch 0007).

The probe reports `PTHREADS-OK` with 29 checks: two create/join rounds, 16
lock/increment/yield rounds per thread with a static
`PTHREAD_MUTEX_INITIALIZER` (counter 32 then 64), a worker-specific
`pthread_setspecific` value and destructor, worker errno 77 across 16 yields,
main errno 55, stable and distinct `pthread_self`, and the create handle
matching the worker's self (`docs/research/relibc-pthread-bringup.md:8-28`).
The pinned relibc commit `729bdc3b` exists on the remote; the branch head is
`bf5fee5e` (a nanosleep change for #322), so the pin is valid but one commit
behind the branch.

The foundation limits are the same as #310/#314, now applied to a POSIX API:
the kernel is cooperative, so a thread that never yields starves its peers; a
`pthread_mutex` held by a thread that never yields cannot be released by
another thread; cancellation with the target enabled returns `ENOSYS`; a
detached thread's stack and TCB are not reclaimed because `munmap` is a
no-op; and the eight-slot table limits the program (`tools/relibc-pal/README.md:169-185`).
The probe yields explicitly between rounds, so it proves the mapping, not
preemptive POSIX behaviour.

Follow-up: preemption and real blocking under #326/#322; condition variables
and cancellation later; add the pthread probe to CI (#13/#330). Do not
present the cooperative gate as the POSIX threads implementation in the
native-libc contract.

## 4. Cross-cutting findings

### 4.1 Process, address space and fd foundations versus the cell model

The userland slices are threads in one image, not processes in cells. The
address space is a set of statics: `STACK_BASE`/`STACK_TOP`, a fixed
`HEAP_BASE`..`HEAP_LIMIT` 1 MiB window, `USER_PAGES`, `USER_RANGES` and
`HEAP_NEXT` (`kernel/src/user.rs:22-38`, `235-310`). `range_ok` is a coarse
range check over eight recorded ranges, not a page-table walk
(`kernel/src/user.rs:36-38`, `440-455`). A new image is loaded into the same
statics and resets them (`kernel/src/user.rs:117-160`); `run_user` resets the
thread table to tid 1 (`kernel/src/traps.rs:1279-1280`).

The fd table is also global: 16 descriptors, 16 open descriptions and 8
pipes as statics (`kernel/src/fd.rs:19-20`, `42-43`, `84-87`). `fd::reset()`
is called per image start, not per process. The file heading calls it
"per-process", but there is no process object to own it (`fd.rs:1-10`).

The image entry is also hard-coded. `start_userland` always loads SCSI target
2 (`kernel/src/main.rs:670-676`), and `initial_stack` always builds
`argc = 1`, `argv[0] = "hello"`, `HOME=/`, `PATH=/` and `AT_PLATFORM=mips`
(`kernel/src/user.rs:322-346`). There is no path-based exec and no user argv
or environment.

There is no `SYS_FORK`, `SYS_EXECVE`, `SYS_POSIX_SPAWN` or `SYS_WAITPID` in
the native gate. `SYS_GETPID` returns 1 unconditionally
(`kernel/src/native_sys.rs:98`), and unknown calls return `CELL_E_NOSYS`
(`native_sys.rs:121`). The specs are explicit that general `fork` is
deferred in the kseg0 SAS model and `posix_spawn`/`exec` are the process
creation path (`docs/programming/native-abi.md:1912-1916`; §6.6). The tree has
neither. The cell model requires a process to be one cell with a `Job`
capability, a capability table and a per-cell fd table
(`docs/internals/architecture.md:393-400`, `536-558`); the current kernel has none
of those objects.

Consequences:

- `fork` is blocked not only by the single address space but by the absence
  of per-process page lists, TLB sets, fd tables, capability tables and a
  process table. A copy of the statics would alias the parent's mappings and
  descriptors. This is exactly the scope of open issue #326.
- `exec` is not a syscall. The loader exists as a boot-time path from SCSI
  target 2 (`kernel/src/main.rs:670-687`; `kernel/src/user.rs:117-160`), but
  there is no path-based image load, no argv/envp from user memory, and no
  `FD_CLOEXEC` close on exec. The probe's close-on-exec checks are state
  checks only.
- `FD_CLOEXEC` is recorded (`fd.rs:600-627`) but unobservable. The same
  applies to the fd table's "per-process" description and to `getpid`.
- The open #326 issue is the direct follow-up: process table, per-process
  address space, TLB switch, `SYS_FORK`, `SYS_EXECVE`, `SYS_WAITPID`, and an
  fd table copied on fork with close-on-exec at exec. Until #326 lands, the
  userland slices should be described as an image with cooperative threads.

### 4.2 PowerPC hardware coverage versus the claims

All eight PowerPC issues are proved on QEMU `g3beige`, not on a PowerBook. The
gate is not a marker-only smoke test: it parses the DSI/ISI/alignment/program
fields, checks marker order, verifies two-controller Mac I/O selection,
compares both ESCC logs byte for byte, and boots a corrupted module image to
prove the digest refusal. We reran it at 64 MiB and it passed. That is strong
evidence for the QEMU machine.

It is not evidence for the M4753. The differences that matter:

- **Interrupt source.** `#301` programs the CUDA VIA. The M4753 exposes
  `via-pmu`, and `platform::cuda` only finds `via-cuda`
  (`platform.rs:589-590`), so the real target prints
  `IRQ-SKIP reason=no-cuda` and continues (`main.rs:284-309`). The PIC driver
  is real, but the acceptance is not exercised on the named hardware.
- **ATA.** `#305` is polled PIO with `nIEN` set (`ata.rs:161-180`); real
  Mac I/O ATA timing and error behaviour are untested. The gate's disk is
  synthetic.
- **Console.** `#313` mirrors both channels, which is the right hedge for
  unknown Geoport wiring, but the actual channel, baud and cable are
  unmeasured.
- **Serial input.** `#321` reads only the selected channel; output stays
  mirrored. There is no receive queue, so the monitor depends on draining the
  one-byte ESCC register before the next byte arrives; the gate sends one
  byte every 30 ms (`tools/powerpc-qemu.sh:687-695`). Only the selected-A
  path is tested; a B-selected channel and the real Geoport are code-only.
- **Mac I/O selection.** `#317` selects by child score; the gate's second
  controller is a QEMU device, not the M4753's `gatwick` tree. If both
  controllers have an ESCC and a disk, tree order is still the tie-break
  (`platform.rs:100-185`).
- **Module load.** On a real disk without `IRIX7-ATA-SIG001`, the kernel
  skips the module (`main.rs:340-360`), so `MODULE-OK` is not exercised on a
  PowerBook. That is the safe behaviour, but it means the module path is
  QEMU-only.
- **No PPC user mode.** `BAT_VS` is cleared and the comment says "there is no
  user mode" (`mmu.rs:25`). There is no PPC ELF loader, syscall gate or
  userland. The PPC issues' non-goals allow this, but the machine is not at
  the Indy's POSIX level and should not be described as such.

The highest-risk hardware claim is #301: the issue's title and goal name the
PowerBook, and the real target path skips the proof. #305, #313, #317 and #321
are also QEMU-only but their acceptance is explicitly a QEMU gate or a build
check, so they are implemented with a hardware caveat. #321's missing receive
queue is the one functional gap in that group: the gate sends bytes slowly
enough to hide it.

### 4.3 Userland POSIX coverage versus the native-libc contract

The v0 gate has 41 flat calls (`crates/native-abi/src/syscall.rs:20-81`;
`docs/programming/native-abi.md:1537-1610`). The native-libc profile requires the
process/signal set (`fork`, `posix_spawn`, `execve`, `waitpid`, `kill`,
`sigaction`, signal masks), sockets, termios, the `*at` family,
`readv`/`writev`, `link`/`symlink`, `chmod`/`chown`, `statvfs`, `realpath`,
`getaddrinfo` and dynamic loading (`docs/programming/native-libc.md:108-118`). The
tree has none of those. It has a narrow file/console/ramfs surface, a
cooperative thread table and a futex.

The specific no-op and partial calls matter more than the count:

- `munmap` and `mprotect` return success without changing mappings
  (`kernel/src/native_sys.rs:84`). A program that relies on `mprotect` for a
  guard page or W^X gets neither.
- `signal` returns success but installs nothing (`native_sys.rs:115-118`).
  `SIGPIPE` is not raised on `EPIPE`; the comment says std ignores `SIGPIPE`,
  which is true for the probe, not for POSIX.
- `getentropy` zero-fills (`kernel/src/user.rs:724-734`). `AT_RANDOM` is a
  weak xorshift (`user.rs:332-342`). Any ASLR or key material derived from
  these is predictable.
- `mmap` ignores address, protection and flags; it is a bump allocator in a
  fixed 1 MiB window (`user.rs:286-310`). `pthread` stacks are never
  reclaimed; a program that creates and joins many threads exhausts the
  window.
- `getpid` is 1 and there is no parent (`native_sys.rs:98`).
- The ramfs is 32 inodes and 4 KiB per file (`ramfs.rs:22-24`), reset per
  image. It is a fixture, not a root filesystem.
- `poll`, blocking read/write and futex waits are bounded at 100 ms.  They no
  longer spin: `poll`/read park on CP0 Compare in 1 ms slices and the futex
  wait parks on a deadline or a cell switch (§7.1–7.2).  The descriptor calls
  remain syscall-local because the gate has no continuation.  `nanosleep`
  parks the thread and, with no runnable thread left, the cell.
- There are no sockets, termios, signals, process groups, job control or
  dynamic linking. The probes do not test them.
- The static Rust `std` shim remains the abicalls exception: it uses `$gp`
  small data and has no TLS, while the relibc probes use `$28`
  (`tools/relibc-pal/README.md:76-80`). Two different user ABIs coexist in
  the same image slot.

The probes are stronger than "one syscall": the relibc probe checks 226
conditions, the thread probe 20, the futex probe 32 and the pthread probe 29
(`docs/research/relibc-pal-bringup.md:13-27`;
`docs/research/relibc-pthread-bringup.md:8-28`;
`user/relibc-thread-probe/README.md:8-22`;
`user/relibc-futex-probe/README.md:8-29`). They are real C/relibc binaries,
not marker stubs. But they are a narrow, hand-written conformance suite for
the implemented calls, and they do not make the layer POSIX. The issue text
is mostly careful about this; the risk is in the cumulative reading of seven
closed "userland/POSIX" issues as a POSIX layer.

### 4.4 Threads, scheduling and blocking

The thread model is cooperative by construction (`kernel/src/thread.rs:2-10`).
A thread runs until it yields, exits, parks on a futex or the image exits.
There is no timer preemption, no per-thread kernel stack, no join primitive,
eight slots and a 100 ms bound on every wait (`thread.rs:34`, `456-468`). The
scheduler in `docs/internals/architecture.md:893-935` is preemptive EDF/fair with
one run queue per CPU, a saved register context and a kernel stack per task.
The tree is not that scheduler.

This is acceptable for #310/#314/#318 as written, but it changes the meaning
of the POSIX API. A `pthread_mutex` is only a lock if the holder yields or
blocks; a CPU-bound holder starves the waiter. `pthread_join` waits on a
libc exit word with a 100 ms bounded futex; the libc re-parks, so a long join
is a sequence of 100 ms parks, not a blocked wait. `pthread_cancel` with
cancellation enabled is `ENOSYS`; a detached thread's stack is not reclaimed
(`tools/relibc-pal/README.md:169-185`).

`poll` and blocking reads are bounded, but no longer busy-waits: they park on
CP0 Compare in 1 ms slices and recheck (§7.1).  `nanosleep` parks the calling
thread and, when the cell has no runnable thread left, the cell itself
(`process.rs` `Sleeping`/`sleep_deadline`; §7.2).  A sleeping parent now yields
to a runnable child cell and resumes at its deadline.  The remaining
scheduling limits are the cooperative model (no preemption) and the absent
readiness wait queue.  The userland is a cooperative runtime, not a POSIX
process environment.

### 4.5 Evidence, CI and documentation

The PowerPC and x86 gates are in CI and are strong
(`ci.yml:1332-1375`, `1377-1432`). The userland probes are not. The workflow
has zero `relibc` references; the only userland images it builds and boots are
the static `std-hello` (`ci.yml:375-382`, `1029-1080`) and the Rust
`user-hello` (`ci.yml:1146-1218`), not `user/relibc-probe`,
`user/relibc-thread-probe`, `user/relibc-futex-probe` or
`user/relibc-pthread-probe`. The transcripts in
`docs/research/` are from iris 0.1.13 and are not reproducible from CI. This
is the weakest evidence in the platform/userland set and the reason the
audit's "simulator or log only" rule bites: the code is real, but the
acceptance run is not gate-backed, and iris is an emulator rather than a real
Indy (#16 stands). The PPC gate is marker-based but parses the DSI, ISI,
alignment, program, IRQ and ESCC fields and now the monitor prompt, echo and
`info` values, so it is not a log-only assertion; the userland evidence is a
transcript with no gate.

The documentation has drifted in both directions:

- `docs/hardware/x86.md:4-5` says the shared core is not linked; that is accurate,
  but the same file's "Next slices" (`:195-205`) still lists the module
  loader, M1 probe and two-guest M2 gate, which the tree now has.
- `docs/project/milestones.md:296-300` says virtio and the shared core are
  "still ahead", although virtio and the x86 M2 gate exist.
- `docs/programming/native-libc.md:48-50` says there is "still no pthread/TLS,
  process table, signals, termios or capability boundary"; cooperative
  pthreads now exist (though no TLS block, process table, signals or
  capability boundary). The sentence is stale in the pthread half.
- `docs/programming/native-abi.md` is accurate for the v0 gate, including the
  cooperative thread model and the deferred `fork`.

These are low-risk fixes, but #266's acceptance includes documented limits,
so the stale x86 status is a real gap, not a cosmetic one.

## 5. Recommended follow-ups, ranked

1. **#326 — fork, execve and waitpid with per-process address spaces.**
   This is the highest-risk foundation gap. Move `USER_PAGES`,
   `USER_RANGES`, `HEAP_NEXT`, the fd table and the TCB/thread table into a
   process object; add a process table, `SYS_FORK`, `SYS_EXECVE`,
   `SYS_WAITPID`, per-process TLB sets and a per-process fd table with
   close-on-exec. This closes the single-global-address-space and
   single-fd-table findings in #296 and #310/#318, and is the M9 contract in
   `docs/programming/native-abi.md` §6.5–6.6.

2. **#266 — shared core and x86 userland, or a formal re-scope.**
   Reopen #266 or split it: the machine layer is done and the M2 gate is real,
   but the "shared kernel core" and "same POSIX/IRIX contract level" claims
   are not. Either link the actual kernel core behind an architecture
   boundary, or re-scope #266 to "second machine type, separate kernel" and
   record that x86 has no userland. #267 carries the scale harness; #326
   carries the process model on both machines.

3. **CI gate for the relibc probes (#13, #330).**
   Add a job that checks out `irix7-pal` at the pinned commit, builds
   `librelibc.a` and the four probes with the pinned Bootlin toolchain, and
   boots each on iris with the marker and `failures=0` assertions. Without
   this, the userland closures remain transcript-only and a regression in
   `fork`/`exec` work will not be caught. Keep `std-hello` as the static
   fallback.

4. **PPC real hardware and the PMU interrupt source (#16, #324).**
   Run the PowerPC gate on a real G3/PowerBook. For #301, add a PMU timer
   path (the M4753 has `via-pmu`, not `via-cuda`) or explicitly record the
   issue as QEMU-CUDA-only and move the real acceptance to a PMU slice. #324
   is the natural home for PMU/ADB device work. For #321, add the bounded RX
   queue the scope asked for (the current path polls the one-byte ESCC
   register), test a B-selected channel and a burst, and run the monitor on a
   real Geoport.

5. **Preemptive scheduling and blocking waits (#322, #326).**
   Replace the 100 ms bounded spin in `poll`, blocking read/write, futex and
   `nanosleep` with parked waits and timer wakeups. This makes
   `pthread_mutex`, `pthread_join` and `pthread_cond_timedwait` behave like
   POSIX rather than a cooperative yield loop. #322 already covers
   `nanosleep`; extend it to `poll`/read and the futex expiry path.

6. **Per-cell fd and capability tables, and the capability gate (#10).**
   The flat `syscall 0x1F7` gate is a bring-up path, not the capability cell
   ABI. Replace it per `docs/programming/native-abi.md` §5.8; move fds to capability
   handles; make `signal` either real or `ENOSYS`, not a no-op success; make
   `getentropy` a real entropy source or fail. This is the contract-level
   follow-up to #288/#296.

7. **Reconcile the docs (#14).**
   Update `docs/hardware/x86.md` to the post-#278 state, update the Phase 6
   status in `docs/project/milestones.md`, and correct the native-libc status
   sentence for cooperative pthreads. Add the PPC hardware caveats from
   #301/#305/#313/#317/#321 to the PowerBook documents.

8. **PPC module trust and layout (#309 follow-up).**
   Take the module digest from the signed boot configuration rather than the
   disk header, and change the linker assertion to protect `SLOT_BASE`
   (24 MiB), not `0x02000000`. The current image is far below the slot, but
   the assertion does not express the invariant.

## 6. Limits of this audit

- No PowerPC or x86 real hardware was available. The PPC gate was run under
  QEMU at 64 MiB, and the #321 monitor gate at `5aff96e` was run from an
  extracted tree at 64 MiB; the 32 and 128 MiB runs were not repeated. The
  x86 gate was run from the `cac1a5a` tree; its CI job is the stronger
  evidence.
- The relibc probes were not rebuilt. The audits of #288/#296/#302/#306/#310/
  #314/#318 rely on the in-tree probe sources, the pinned fork commit and
  the transcripts in `docs/research/`; the host `native-abi` layout tests
  were run. The missing CI gate is recorded as a finding, not treated as a
  pass.
- The report base is `cac1a5a`; #321 is audited at the later merged commit
  `5aff96e` without rebasing the branch. The 22 core/mesh issues are outside
  this report. The split is declared in §1.1 so the three reports together
  cover the 39 closed issues.

## 7. Status update — #266 shared-core extraction (2026-11-03)

Branch `audit-x86` lands the first shared-core slice against #266/#330. It does
not change the verdicts above, which describe `cac1a5a`; this section records
what has moved since.

`crates/kernel-core` now owns the module image protocol (canonical `IRIX7MOD`
header, size and SHA-256 checks, vtable bounds check, entry call) and the one
`module_abi::Services` table with a `Platform` boundary. The Indy, x86_64 and
PowerPC kernels implement `Platform` and no longer carry their own service
tables. The x86 loader gains the vtable bounds check it lacked, and the x86
disk header becomes the canonical big-endian format used by the PowerPC
machine. `tools/x86-qemu.sh` and `tools/powerpc-qemu.sh` pass; the MIPS release
image links; `cargo test -p kernel-core -p tcp-slots -p block-store -p kernel`
passes. See `docs/research/x86-shared-core.md` for the evidence and the exact
remaining split.

Still unmet from the original finding: there is no shared userland (fd table,
ramfs, thread table, native syscalls) and no x86 userland/native gate, because
both depend on the process model in #326. The module fetch transports, slot
layouts, runtime arena registry, heaps and all machine drivers remain
per-machine. The x86 target must still not be described as a second POSIX/IRIX
target. #266 should stay open for the userland half, or be split into
"machine and shared module core" (now done) and "x86 userland and native gate"
(blocked on #326).

## 8. Follow-up remediation (2026-11-03)

Two findings from this report were fixed on branch `audit-process`, based on
`8c7a270` (tree-identical to `aaf0d4e`, the #342 process branch merged as
\#326).  The work is kernel-side; the relibc pin and the C probes are
unchanged apart from the new per-cell probe.  All seven userland probes were
rebuilt at the pinned relibc `8a7e044f` and rerun on iris 0.1.13 with the
merged kernel: the descriptor probe (226 checks), fork/exec, sleep (48),
thread (20), futex (32), pthread (29) and the new per-cell sleep probe (11)
all pass.

### 8.1 #302 — parked poll and blocking-read waits

The two busy loops are gone:

- `user::poll_user` parks the CPU on CP0 Compare for `WAIT_SLICE_MS` (1 ms)
  slices and rechecks every record between slices.  A timer or device
  interrupt ends a slice, so console input is still observed promptly.
- `fd::read` no longer waits.  `fd::read_pipe`/`fd::read_console` report
  `EAGAIN` for an empty pipe or idle console, and `user::read_fd` owns the
  blocking rule: it parks in the same 1 ms slices until `WAIT_LIMIT_MS`
  (100 ms), returns `EAGAIN` at the bound, or returns a short count once it
  has copied bytes.  A non-blocking descriptor reports `EAGAIN` on the first
  check and never parks.
- A full-pipe write still uses the bounded path, now parked on CP0 Compare
  rather than spun (`fd::wait_for`).

The wait remains **syscall-local and bounded at 100 ms**.  The flat gate has
no continuation, so a `poll` or read cannot install another cell in the middle
of the gate and resume its scan; that is the M9.5 readiness wait queue, still
a follow-up.  `nanosleep` is a complete park, so it is the call that yields a
cell (§8.2).  A negative `poll` timeout is still clamped to 100 ms, which the
issue permits and `docs/programming/native-abi.md` §5.8 documents.

### 8.2 #343 — per-cell sleep deadlines

The process record now carries a sleep deadline and a `Sleeping` state:

- `Sleeping` is set when the cell has no runnable thread and another local
  cell can run.  `sleep_deadline` is the earliest deadline of the cell's
  parked threads (the thread wait fields are `wait_start`/`wait_limit` in
  CP0 Count ticks).
- `process::next_runnable` runs `expire_sleepers` first and skips a sleeping
  cell until its deadline.  `process::nearest_deadline` returns the earliest
  cell deadline; when nothing is runnable, `next_runnable_or_expired` and the
  thread wait paths arm CP0 Compare for it and park the CPU in `wait`.
- `process::park_current` calls its prepare closure only after it has found a
  runnable target, so a cell with no peer falls through to the existing timer
  wait instead of being parked in a table nothing selects.  The closure marks
  the calling thread `Sleeping` or `Blocked`, saves its frame with `EPC`
  advanced and writes its result word.
- `switch_to` applies the target's thread deadlines with
  `thread::resume_after_wake` after its address space is active, so a cell
  woken by a sibling thread's deadline resumes that thread, and a futex
  waiter still re-reads its word before returning.
- `thread::sleep_current` parks the cell when no thread peer exists;
  `thread::bounded_wait` does the same for a futex wait with no thread peer
  and a runnable sibling cell, preserving the word contract.

Preserved unchanged: the wrap-safe Count comparison (every deadline is
shorter than half the 32-bit range), the 60 s `SLEEP_LIMIT_MS` cap and libc
re-arm, `sched_yield`, `waitpid` and `exit`.  A child that exits while its
parent sleeps is retained as a zombie because `index_of_pid` now counts a
sleeping parent as alive; the parent resumes at its deadline and reaps it.

### 8.3 Per-cell sleep probe

`user/relibc-sleep-cell-probe` creates a non-blocking pipe, forks, and sleeps
20 ms.  The child increments a counter 2,000 steps, writes the final value to
the pipe and exits 42.  The non-blocking read is deliberate: a
blocking read would wait for the child and hide the scheduling failure.  The
parent measures `nanosleep` alone, then checks the pipe value, the elapsed
deadline, `waitpid(WNOHANG)` reaping an already-exited child, and status 42.

```text
fork: pid=1 child=2 actor=...
sleep: wait tid=1 limit=20ms
process: sleep pid=1 limit=19ms
process: exit pid=2 status=0x0000002A
process: sleep expiry pid=1 overrun=0ms
sleep: expiry tid=1 limit=19ms elapsed=33ms
SLEEP-CELL-PROBE: counter=2000 expected=2000 elapsed=63ms checks=11 failures=0
SLEEP-CELL-OK
process: exit pid=1 status=0x00000000
user: exit syscall code=0x00000000
kernel: userland returned to kernel console
```

The child's exit line appears while the parent is still asleep, which is the
scheduling evidence.  The parent's `elapsed` is measured around `nanosleep`,
so an early wake fails the lower bound; the 60 s upper bound is a no-hang
check for a loaded JIT host.  The host was heavily loaded during the run, so
the millisecond values are bounds rather than fixed numbers.

### 8.4 Remaining blocking paths

No user-visible call now busy-loops for the length of its wait.  (The timer
park itself counts down the final 1,000 Count ticks, about 30 µs, because a
Compare value already behind Count would wrap.)  The paths that remain
bounded are:

| Path | Bound | Why it cannot switch cells |
|---|---|---|
| `poll` | 100 ms | the gate cannot resume its record scan after a switch |
| blocking `read` | 100 ms | partial data would be re-read if the gate restarted |
| blocking `write` to a full pipe | 100 ms | producer/consumer wakeup needs the wait queue |
| `futex_wait` | 100 ms | parks the cell only when another cell can run; no cross-cell wake |
| `nanosleep` | 60 s per park | yields the cell; libc re-arms for longer intervals |

None of them spins: the descriptor calls and a no-peer futex wait park the
CPU on the timer between checks, and a futex wait with a runnable sibling cell
parks the cell.  A readiness wait queue that wakes the reader or poller from
the producer, and a true infinite wait, remain the M9.5 work; the cooperative
model still has no preemption, so a thread or cell that never yields also
never observes its own deadline.

### 8.5 Test conditions

- Worktree branch `audit-process`; kernel built with `tools/m2-rig/build.sh`
  at the pinned nightly, ROM 503,712 bytes.  Probe images built with
  `tools/relibc-pal/build.sh` at relibc `8a7e044f584945dbf5fdbad11fd76e16daa420b9`.
- Each image was booted on iris 0.1.13 as SCSI target 2 (Indy profile
  `indy_ip24`, 64 MiB, `IRIS_NO_IDLE=1`); the fork/exec probe also attached
  the child image at target 3.
- The host was shared with other emulator runs, so short sleeps show host
  dilation.  The probes assert lower bounds and use generous upper bounds.
